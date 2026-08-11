//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Physics tick orchestration and performance knobs.

use std::collections::HashSet;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::active::{clear_all_dirty, partition_checkerboard, plan_active};
use crate::grid::World;

use super::grain::{
    active_has_unsupported_grain, settle_loose_grains_regions, GRAIN_SETTLE_PASSES,
    GRAIN_SETTLE_PASSES_SHALLOW,
};
use super::gravity::apply_gravity_fall_regions;
use super::seepage::apply_seepage_regions;
use super::water_flow::apply_water_flow_regions;

/// Re-settle budget after a raft punch — enough to sink punched cargo
/// through a water seat without the full ×1024 freefall path.
const GRAIN_SETTLE_PASSES_PUNCH: u32 = 32;

/// Accumulated wall time for sub-passes inside [`tick_with_life`].
///
/// Used by `perf_profile` so the printed breakdown matches the real
/// physics tick (the old post-hoc mirror diverged on settle / punch).
#[derive(Debug, Default, Clone)]
pub struct PhysicsTimings {
    pub plan_clear: Duration,
    pub gravity: Duration,
    pub water_flow: Duration,
    pub seepage: Duration,
    pub wake_grains: Duration,
    pub settle: Duration,
    pub punch: Duration,
    pub rise_soak: Duration,
    pub failure: Duration,
    pub confined: Duration,
    pub mycelium: Duration,
    pub substeps_ran: u64,
    pub active_regions: u64,
    pub active_area: u64,
    /// Ticks that took the deep ([`GRAIN_SETTLE_PASSES`]) settle path.
    pub deep_settle_ticks: u64,
    /// Ticks where [`super::grain::punch_through_floating_rafts`] moved mass.
    pub punch_hits: u64,
}

impl PhysicsTimings {
    pub fn total(&self) -> Duration {
        self.plan_clear
            + self.gravity
            + self.water_flow
            + self.seepage
            + self.wake_grains
            + self.settle
            + self.punch
            + self.rise_soak
            + self.failure
            + self.confined
            + self.mycelium
    }

    fn merge_from(&mut self, other: &Self) {
        self.plan_clear += other.plan_clear;
        self.gravity += other.gravity;
        self.water_flow += other.water_flow;
        self.seepage += other.seepage;
        self.wake_grains += other.wake_grains;
        self.settle += other.settle;
        self.punch += other.punch;
        self.rise_soak += other.rise_soak;
        self.failure += other.failure;
        self.confined += other.confined;
        self.mycelium += other.mycelium;
        self.substeps_ran += other.substeps_ran;
        self.active_regions += other.active_regions;
        self.active_area += other.active_area;
        self.deep_settle_ticks += other.deep_settle_ticks;
        self.punch_hits += other.punch_hits;
    }
}

/// How many gravity→surface-flow cycles run inside one [`tick`].
///
/// Several substeps with re-planned dirty halos let ponds level and
/// hill water drain at a liquid pace. On flat shelves where cascade
/// edges are 5-10 cells away, half-gap propagates at ~1 cell/substep,
/// so we need enough substeps to keep up with steady rain.
pub const FLOW_SUBSTEPS: usize = 12;
/// Minimum flow substeps before a quiet dirty halo may early-out.
pub const FLOW_SUBSTEPS_MIN: usize = 6;
/// If the planned dirty area (cells) drops to this or below after
/// [`FLOW_SUBSTEPS_MIN`], stop the flow loop early — settled films
/// don't need the full ×12. Busy rain / cascades stay at max.
pub const FLOW_QUIET_AREA: usize = 512;

/// Live-tunable physics trade-offs (Tab → Performance).
///
/// Defaults favour interactive FPS: every-other surface flow + quiet
/// early-out, with **rayon off**. Demo dirty plans stay ~6–12 regions /
/// ~10–13k cells — too narrow for rayon to win (32-core Super-Server:
/// parallel ON was ~1.6× slower than OFF). Opt parallel back on in Tab
/// for wide dirty worlds once the active plan is fat.
///
/// **Do not pair away odd-step gravity.** Super-Server A/B showed that
/// skipping interstitial gravity fattens the dirty halo (~13k → ~17k
/// cells) and makes each flow call ~2× slower — net regression despite
/// fewer flow passes.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PerfConfig {
    /// Run surface water flow only on even substeps (gravity still every
    /// substep). Default **on** — ~half the surface-flow scan work.
    /// Odd-step gravity keeps shore columns seated so the next flow
    /// halo stays narrow.
    pub flow_every_other_substep: bool,
    /// After [`FLOW_SUBSTEPS_MIN`], stop when the dirty halo is tiny or
    /// has shrunk enough. Default **on** for settled films / quiet ponds.
    pub flow_quiet_early_out: bool,
    /// Rayon checkerboard parallelism for gravity / grain / flow scan.
    /// Default **off** — wins only when many chunks are dirty at once.
    pub parallel_physics: bool,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            flow_every_other_substep: true,
            flow_quiet_early_out: true,
            parallel_physics: false,
        }
    }
}

impl PerfConfig {
    /// Full ×12 surface-flow path with no early-out — scenario / unit
    /// tests and Tab A/B against the FPS-biased [`Default`].
    pub const fn full_feel() -> Self {
        Self {
            flow_every_other_substep: false,
            flow_quiet_early_out: false,
            parallel_physics: false,
        }
    }
}

fn active_cell_area(active: &[crate::active::ActiveChunk]) -> usize {
    active
        .iter()
        .map(|a| {
            let w = (a.rect.x1 as usize).saturating_sub(a.rect.x0 as usize) + 1;
            let h = (a.rect.y1 as usize).saturating_sub(a.rect.y0 as usize) + 1;
            w.saturating_mul(h)
        })
        .sum()
}

/// Keep only active regions whose chunk sticky-flag says loose material
/// may be present. Bootstrap (no flags set yet) keeps the full list.
fn filter_loose_regions(
    world: &crate::grid::World,
    active: &[crate::active::ActiveChunk],
) -> Vec<crate::active::ActiveChunk> {
    if active.is_empty() {
        return Vec::new();
    }
    let any_flag = world.chunks.values().any(|c| c.has_loose);
    if !any_flag {
        return active.to_vec();
    }
    active
        .iter()
        .copied()
        .filter(|ac| {
            world
                .chunks
                .get(&ac.coord)
                .map(|c| c.has_loose)
                .unwrap_or(false)
        })
        .collect()
}

/// Advance the sim by one tick.
///
/// Runs the sub-passes in a fixed order:
///
/// 1. **Flow substeps** (×[`FLOW_SUBSTEPS`]): gravity fall, then
///    Air–Air hydraulic-head surface flow (horizontal + diagonal).
///    Each substep re-plans from dirty so water can advance several
///    cells per tick and seek a flat free surface on slopes.
/// 2. Seepage — water soaks into / through porous solids by head,
///    rate-limited by permeability.
/// 3. Grain settle — multi-pass fall + repose ([`GRAIN_SETTLE_PASSES`])
///    so unsupported litter seats in one tick instead of one cell/frame.
///
/// Rain, evaporation, and [`apply_flow_erosion`] are **opt-in**: callers
/// wire them into their per-frame loop. Scenario tests pass `tick(world)`
/// alone and stay deterministic without weather / sediment.
///
/// **Dirty / active chunks.** Each flow substep [`plan_active`]s from
/// dirty rects (halo + neighbour wake), then [`clear_all_dirty`].
/// Writes rebuild dirty for the next substep / tick. Seepage + grain
/// settle prefer post-flow dirty; if water was quiet they reuse the last
/// non-empty flow halo so F3-painted Organic / sand mid-air still falls.
/// A fully settled world plans nothing and the physics passes early-out.
///
/// **Checkerboard.** Gravity and grain run four colour sub-passes
/// (EE → OE → EO → OO); within a colour, regions run on rayon when
/// enabled. Surface flow and seepage scan the same partition (also
/// parallel per colour) but apply from one snapshot so edges are not
/// re-solved mid-rule.
pub fn tick(world: &mut World) {
    // Full feel — scenario / unit water suites. Interactive demo uses
    // [`PerfConfig::default`] (FPS-biased) via `tick_with_perf`.
    tick_with_perf(world, &PerfConfig::full_feel());
}

/// [`tick`] with live [`PerfConfig`] knobs (demo Tab → Performance).
///
/// Uses default [`crate::failure::FailureConfig`] (roof collapse on).
pub fn tick_with_perf(world: &mut World, perf: &PerfConfig) {
    tick_with_configs(world, perf, &crate::failure::FailureConfig::default());
}

/// [`tick`] with performance + geotech failure knobs.
pub fn tick_with_configs(
    world: &mut World,
    perf: &PerfConfig,
    failure: &crate::failure::FailureConfig,
) {
    tick_with_configs_and_geotech(world, perf, failure, None);
}

/// [`tick_with_configs`] plus optional [`crate::geotech_map::GeotechMap`]
/// for map-gated F2b shear (S3).
pub fn tick_with_configs_and_geotech(
    world: &mut World,
    perf: &PerfConfig,
    failure: &crate::failure::FailureConfig,
    geotech: Option<&crate::geotech_map::GeotechMap>,
) {
    let _ = tick_with_life(world, perf, failure, geotech, None, None, None);
}

/// [`tick_with_configs_and_geotech`] plus optional living-root cells for
/// grain repose binding (Set D plants / legacy E15 intent).
///
/// `grain` supplies floating-Organic waterlog rate (Tab → Grain / sediment);
/// `None` uses [`super::grain::GrainConfig::default`].
/// `fungi` supplies mycelium compost knobs (Tab → Fungi / carbon);
/// `None` uses [`crate::fungi::FungiConfig::default`].
pub fn tick_with_life(
    world: &mut World,
    perf: &PerfConfig,
    failure: &crate::failure::FailureConfig,
    geotech: Option<&crate::geotech_map::GeotechMap>,
    rooted: Option<&HashSet<(i32, i32)>>,
    grain: Option<&super::grain::GrainConfig>,
    fungi: Option<&crate::fungi::FungiConfig>,
) -> crate::failure::FailureStats {
    tick_with_life_inner(world, perf, failure, geotech, rooted, grain, fungi, None)
}

/// [`tick_with_life`] while accumulating [`PhysicsTimings`].
pub fn tick_with_life_profiled(
    world: &mut World,
    perf: &PerfConfig,
    failure: &crate::failure::FailureConfig,
    geotech: Option<&crate::geotech_map::GeotechMap>,
    rooted: Option<&HashSet<(i32, i32)>>,
    grain: Option<&super::grain::GrainConfig>,
    fungi: Option<&crate::fungi::FungiConfig>,
    timings: &mut PhysicsTimings,
) -> crate::failure::FailureStats {
    tick_with_life_inner(
        world,
        perf,
        failure,
        geotech,
        rooted,
        grain,
        fungi,
        Some(timings),
    )
}

/// [`tick_with_perf`] while accumulating [`PhysicsTimings`].
pub fn tick_with_perf_profiled(
    world: &mut World,
    perf: &PerfConfig,
    timings: &mut PhysicsTimings,
) -> crate::failure::FailureStats {
    tick_with_life_profiled(
        world,
        perf,
        &crate::failure::FailureConfig::default(),
        None,
        None,
        None,
        None,
        timings,
    )
}

fn tick_with_life_inner(
    world: &mut World,
    perf: &PerfConfig,
    failure: &crate::failure::FailureConfig,
    geotech: Option<&crate::geotech_map::GeotechMap>,
    rooted: Option<&HashSet<(i32, i32)>>,
    grain: Option<&super::grain::GrainConfig>,
    fungi: Option<&crate::fungi::FungiConfig>,
    mut timings: Option<&mut PhysicsTimings>,
) -> crate::failure::FailureStats {
    // Opt-in cell-sat inventory (debug only). Atmosphere stores are
    // outside this tick — see `audit::tracked_totals`.
    #[cfg(debug_assertions)]
    let mass_before = if crate::audit::mass_audit_enabled() {
        Some(crate::audit::sat_totals(world))
    } else {
        None
    };

    let profile = timings.is_some();
    let mut local = PhysicsTimings::default();

    crate::parallel::set_parallel_enabled(perf.parallel_physics);
    // Last non-empty flow plan — grain/seepage fall back to this when
    // water writes nothing (painted solids mid-air, dry edits, …).
    let mut flow_halo: Vec<crate::active::ActiveChunk> = Vec::new();
    let mut start_area: usize = 0;
    // FPS knobs on → cap at 8 substeps. Gravity runs every step;
    // surface flow is every-other. full_feel keeps the full ×12 path.
    let max_steps = if perf.flow_every_other_substep && perf.flow_quiet_early_out {
        FLOW_SUBSTEPS_MIN + 2 // 8
    } else {
        FLOW_SUBSTEPS
    };
    for step in 0..max_steps {
        let t0 = profile.then(Instant::now);
        let active = plan_active(world);
        clear_all_dirty(world);
        if let (true, Some(t0)) = (profile, t0) {
            local.plan_clear += t0.elapsed();
        }
        if active.is_empty() {
            break;
        }
        if profile {
            local.substeps_ran += 1;
            local.active_regions += active.len() as u64;
            local.active_area += active_cell_area(&active) as u64;
        }
        flow_halo = active.clone();
        let this_area = active_cell_area(&active);
        if step == 0 {
            start_area = this_area;
        }
        let passes = partition_checkerboard(&active);
        let t0 = profile.then(Instant::now);
        for pass in &passes {
            apply_gravity_fall_regions(world, pass);
        }
        if let (true, Some(t0)) = (profile, t0) {
            local.gravity += t0.elapsed();
        }
        // Every-other flow: gravity still runs every pass; surface
        // leveling runs on even substeps when opted in. (Must include
        // step 0 — odd-only skipped flow after clear_all_dirty, then
        // step 1 saw an empty plan and broke before any leveling.)
        // Odd-step gravity is load-bearing: without it the dirty halo
        // fattens and each flow call costs more than the gravity saved.
        let run_flow = !perf.flow_every_other_substep || (step % 2 == 0);
        if run_flow {
            let t0 = profile.then(Instant::now);
            apply_water_flow_regions(world, &active);
            if let (true, Some(t0)) = (profile, t0) {
                local.water_flow += t0.elapsed();
            }
        }
        // Quiet early-out: after the minimum passes, peek at dirty
        // written by this substep — a tiny halo means water settled.
        // Absolute threshold catches truly settled worlds; a *shrink*
        // check catches busy shores that started large but have since
        // fallen off (adaptive substeps for the tuned feel path).
        if perf.flow_quiet_early_out && step + 1 >= FLOW_SUBSTEPS_MIN {
            let next = plan_active(world);
            let next_area = active_cell_area(&next);
            if next.is_empty() || next_area <= FLOW_QUIET_AREA {
                break;
            }
            // Adaptive: halo shrunk by ≥ 1/3 relative to start-of-tick —
            // remaining flow is polishing, not cascading.
            if start_area > 0 && next_area * 3 <= start_area * 2 {
                break;
            }
            // Steady busy halo (closed-loop rain): area barely moved —
            // further substeps are polish. Super-Server demo sat at
            // ~9–13k cells for all ×12; this exits after the minimum.
            if start_area > FLOW_QUIET_AREA && next_area * 10 >= start_area * 9 {
                break;
            }
        }
    }

    // Communicating vessels: a filled pipe can go locally quiet while the
    // reservoir head is still higher. Periodic full-chunk confined scan.
    {
        let t0 = profile.then(Instant::now);
        super::water_flow::wake_confined_head(world);
        if let (true, Some(t0)) = (profile, t0) {
            local.confined += t0.elapsed();
        }
    }

    // Seepage follows the water dirty / flow halo.
    let flow_active = {
        let t0 = profile.then(Instant::now);
        let dirty = plan_active(world);
        if let (true, Some(t0)) = (profile, t0) {
            local.plan_clear += t0.elapsed();
        }
        if dirty.is_empty() {
            flow_halo
        } else {
            dirty
        }
    };
    if !flow_active.is_empty() {
        let t0 = profile.then(Instant::now);
        apply_seepage_regions(world, &flow_active);
        if let (true, Some(t0)) = (profile, t0) {
            local.seepage += t0.elapsed();
        }
    }

    // Re-wake unsupported grains and steep cliff faces. Cadence-gated:
    // full sticky-loose scan every 16 ticks; dirty-halo wake every 4.
    // Tick 0 always full-scans so save-load / first frame catch orphans.
    const GRAIN_WAKE_EVERY: u64 = 4;
    const GRAIN_WAKE_FULL_EVERY: u64 = 16;
    let mut freefall_woken = 0u32;
    if world.tick % GRAIN_WAKE_EVERY == 0 {
        let t0 = profile.then(Instant::now);
        if world.tick % GRAIN_WAKE_FULL_EVERY == 0 {
            freefall_woken = super::grain::wake_grains_for_settle(world);
        } else {
            let halo = filter_loose_regions(world, &flow_active);
            let coords: Vec<_> = halo.iter().map(|ac| ac.coord).collect();
            freefall_woken = super::grain::wake_grains_for_settle_coords(world, &coords);
        }
        if let (true, Some(t0)) = (profile, t0) {
            local.wake_grains += t0.elapsed();
        }
    }
    let grain_active = {
        let t0 = profile.then(Instant::now);
        let dirty = plan_active(world);
        let src = if dirty.is_empty() {
            flow_active
        } else {
            dirty
        };
        // Water-dirty ocean/sky chunks have no sand/litter — settle was
        // still walking them every tick (~physics gap on Super-Server).
        let out = filter_loose_regions(world, &src);
        if let (true, Some(t0)) = (profile, t0) {
            local.plan_clear += t0.elapsed();
        }
        out
    };
    if !grain_active.is_empty() {
        // Deep settle for sky freefall / mid-air paint, and for full-feel
        // (unit tests / Tab A/B). FPS defaults stay shallow unless wake or
        // empty/haze Air under a grain — not wet shore seats (those were
        // forcing ×1024 every rainy tick).
        let fps_path = perf.flow_every_other_substep && perf.flow_quiet_early_out;
        let deep = !fps_path
            || freefall_woken > 0
            || active_has_unsupported_grain(world, &grain_active);
        let passes = if deep {
            GRAIN_SETTLE_PASSES
        } else {
            GRAIN_SETTLE_PASSES_SHALLOW
        };
        if profile && deep {
            local.deep_settle_ticks += 1;
        }
        let t0 = profile.then(Instant::now);
        settle_loose_grains_regions(world, &grain_active, rooted, passes);
        if let (true, Some(t0)) = (profile, t0) {
            local.settle += t0.elapsed();
        }
    }

    // Dense cargo cannot ride floating Organic/Snow/Ice. Full-grid punch
    // once per wake cadence, then a short re-settle so punched grains
    // sink through the water seat (capped — not the ×1024 freefall path).
    if world.tick % GRAIN_WAKE_EVERY == 0 {
        let t0 = profile.then(Instant::now);
        let punched = super::grain::punch_through_floating_rafts(world);
        if let (true, Some(t0)) = (profile, t0) {
            local.punch += t0.elapsed();
        }
        if punched > 0 {
            if profile {
                local.punch_hits += 1;
            }
            let t0 = profile.then(Instant::now);
            let _ = super::grain::wake_grains_for_settle(world);
            if let (true, Some(t0)) = (profile, t0) {
                local.wake_grains += t0.elapsed();
            }
            let sink = filter_loose_regions(world, &plan_active(world));
            if !sink.is_empty() {
                let t0 = profile.then(Instant::now);
                settle_loose_grains_regions(world, &sink, rooted, GRAIN_SETTLE_PASSES_PUNCH);
                if let (true, Some(t0)) = (profile, t0) {
                    local.settle += t0.elapsed();
                }
            }
        }
    }
    // Clear submerged litter lines, then let rafts drink — shared litter scan.
    {
        let t0 = profile.then(Instant::now);
        match grain {
            Some(g) => super::grain::rise_and_soak_buoyant_litter_cfg(world, g),
            None => super::grain::rise_and_soak_buoyant_litter(world),
        }
        if let (true, Some(t0)) = (profile, t0) {
            local.rise_soak += t0.elapsed();
        }
    }

    // Geotech: roof / overhang collapse after grain has seated.
    // Cadence-gated — full-grid ~1.3 ms/call on Super-Server; every 4
    // ticks keeps cliffs responding without owning the quiet-world budget.
    const FAILURE_EVERY: u64 = 4;
    let failure_stats = if world.tick % FAILURE_EVERY == 0 {
        let t0 = profile.then(Instant::now);
        let stats = crate::failure::apply_failure(world, failure, geotech);
        if let (true, Some(t0)) = (profile, t0) {
            local.failure += t0.elapsed();
        }
        stats
    } else {
        crate::failure::FailureStats::default()
    };

    // Reset network sym "last" before field + later organism plant trade
    // share one inspector window (organism step clears plant lasts only).
    crate::symbiosis::clear_sym_net_flow_lasts(world);

    // Mycelium field: lives in Organic independently of fruiting bodies.
    {
        let t0 = profile.then(Instant::now);
        match fungi {
            Some(f) => crate::fungi::step_mycelium_field_cfg(world, f),
            None => crate::fungi::step_mycelium_field(world),
        }
        if let (true, Some(t0)) = (profile, t0) {
            local.mycelium += t0.elapsed();
        }
    }

    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
    }

    if let Some(t) = timings.as_mut() {
        t.merge_from(&local);
    }

    #[cfg(debug_assertions)]
    if let Some(before) = mass_before {
        let after = crate::audit::sat_totals(world);
        crate::audit::assert_cell_sat_conserved(&before, &after, "tick_with_life");
    }

    failure_stats
}
