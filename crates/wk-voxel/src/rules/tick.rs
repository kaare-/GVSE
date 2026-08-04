//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Physics tick orchestration and performance knobs.

use std::collections::HashSet;

use crate::active::{clear_all_dirty, partition_checkerboard, plan_active};
use crate::grid::World;

use super::grain::{settle_loose_grains_regions, GRAIN_SETTLE_PASSES};
use super::gravity::apply_gravity_fall_regions;
use super::seepage::apply_seepage_regions;
use super::water_flow::apply_water_flow_regions;

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

/// Live-tunable physics trade-offs (Tab → Performance). Defaults keep
/// the full water-feel path; opt-ins trade some leveling speed for ms.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PerfConfig {
    /// Run surface water flow only on odd substeps (gravity still every
    /// substep). Default **off** — same feel as the tuned ×12 path.
    pub flow_every_other_substep: bool,
    /// After [`FLOW_SUBSTEPS_MIN`], stop when the dirty halo is tiny.
    pub flow_quiet_early_out: bool,
    /// Rayon checkerboard parallelism for gravity / grain / flow scan.
    pub parallel_physics: bool,
}

impl Default for PerfConfig {
    fn default() -> Self {
        Self {
            flow_every_other_substep: false,
            // Off by default — early-out can stall hill drains / shelf
            // cascades when the dirty halo shrinks mid-leveling. Opt in
            // via Tab → Performance after eyeballing water feel.
            flow_quiet_early_out: false,
            parallel_physics: true,
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
    tick_with_perf(world, &PerfConfig::default());
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
    tick_with_life(world, perf, failure, geotech, None);
}

/// [`tick_with_configs_and_geotech`] plus optional living-root cells for
/// grain repose binding (Set D plants / legacy E15 intent).
pub fn tick_with_life(
    world: &mut World,
    perf: &PerfConfig,
    failure: &crate::failure::FailureConfig,
    geotech: Option<&crate::geotech_map::GeotechMap>,
    rooted: Option<&HashSet<(i32, i32)>>,
) {
    // Opt-in cell-sat inventory (debug only). Atmosphere stores are
    // outside this tick — see `audit::tracked_totals`.
    #[cfg(debug_assertions)]
    let mass_before = if crate::audit::mass_audit_enabled() {
        Some(crate::audit::sat_totals(world))
    } else {
        None
    };

    crate::parallel::set_parallel_enabled(perf.parallel_physics);
    // Last non-empty flow plan — grain/seepage fall back to this when
    // water writes nothing (painted solids mid-air, dry edits, …).
    let mut flow_halo: Vec<crate::active::ActiveChunk> = Vec::new();
    let mut start_area: usize = 0;
    for step in 0..FLOW_SUBSTEPS {
        let active = plan_active(world);
        clear_all_dirty(world);
        if active.is_empty() {
            break;
        }
        flow_halo = active.clone();
        let this_area = active_cell_area(&active);
        if step == 0 {
            start_area = this_area;
        }
        let passes = partition_checkerboard(&active);
        for pass in &passes {
            apply_gravity_fall_regions(world, pass);
        }
        // Every-other flow: gravity still runs every pass; surface
        // leveling runs on even substeps when opted in. (Must include
        // step 0 — odd-only skipped flow after clear_all_dirty, then
        // step 1 saw an empty plan and broke before any leveling.)
        let run_flow = !perf.flow_every_other_substep || (step % 2 == 0);
        if run_flow {
            apply_water_flow_regions(world, &active);
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
        }
    }

    // Communicating vessels: a filled pipe can go locally quiet while the
    // reservoir head is still higher. Periodic full-chunk confined scan.
    super::water_flow::wake_confined_head(world);

    // Seepage follows the water dirty / flow halo.
    let flow_active = {
        let dirty = plan_active(world);
        if dirty.is_empty() {
            flow_halo
        } else {
            dirty
        }
    };
    if !flow_active.is_empty() {
        apply_seepage_regions(world, &flow_active);
    }

    // Re-wake unsupported grains and steep cliff faces — lakes often
    // leave a non-empty dirty plan far from F3 paint, and seated
    // Organic/sand walls have solid under them so fall-wake alone
    // never sees them. Cadence-gated: this is a full-grid safety scan
    // that only matters when a grain was orphaned mid-air; running
    // every tick was ~1.6 ms of pure insurance. Every 4 ticks trades
    // a ≤4-tick delay before a stranded grain drops for that budget.
    // Also runs on tick 0 (fresh world / after save-load) so painted
    // mid-air grains fall on the first tick.
    const GRAIN_WAKE_EVERY: u64 = 4;
    if world.tick % GRAIN_WAKE_EVERY == 0 {
        super::grain::wake_grains_for_settle(world);
    }
    let grain_active = {
        let dirty = plan_active(world);
        if dirty.is_empty() {
            flow_active
        } else {
            dirty
        }
    };
    if !grain_active.is_empty() {
        settle_loose_grains_regions(world, &grain_active, rooted, GRAIN_SETTLE_PASSES);
    }

    // Dense cargo cannot ride floating Organic/Snow/Ice. Full-grid punch
    // once per tick (not per settle pass — that re-scanned oceans to death),
    // then a short re-settle so punched grains sink through the water seat.
    // Cadence-gated with grain wake to share the "no fresh grain paint"
    // quiet path (~0.7 ms/tick).
    if world.tick % GRAIN_WAKE_EVERY == 0
        && super::grain::punch_through_floating_rafts(world) > 0
    {
        super::grain::wake_grains_for_settle(world);
        let sink = plan_active(world);
        if !sink.is_empty() {
            settle_loose_grains_regions(world, &sink, rooted, GRAIN_SETTLE_PASSES);
        }
    }
    // Clear submerged litter lines, then let rafts drink — shared litter scan.
    super::grain::rise_and_soak_buoyant_litter(world);

    // Geotech: roof / overhang collapse after grain has seated.
    crate::failure::apply_failure(world, failure, geotech);

    // Mycelium field: lives in Organic independently of fruiting bodies.
    crate::fungi::step_mycelium_field(world);

    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
    }

    #[cfg(debug_assertions)]
    if let Some(before) = mass_before {
        let after = crate::audit::sat_totals(world);
        crate::audit::assert_cell_sat_conserved(&before, &after, "tick_with_life");
    }
}
