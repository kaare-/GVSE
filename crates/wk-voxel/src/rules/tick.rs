//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Physics tick orchestration and performance knobs.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::active::{clear_all_dirty, partition_checkerboard, plan_active};
use crate::grid::World;

use super::grain::{
    settle_loose_grains_regions, GRAIN_SETTLE_PASSES, GRAIN_SETTLE_PASSES_SHALLOW,
};
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

/// Live-tunable physics trade-offs (Tab → Performance).
///
/// Defaults favour interactive FPS: every-other surface flow + quiet
/// early-out, with **rayon off**. Demo dirty plans stay ~6 regions /
/// ~9k cells — too narrow for rayon to win (32-core Super-Server:
/// parallel ON was ~1.6× slower than OFF). Opt parallel back on in Tab
/// for wide dirty worlds once the active plan is fat.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PerfConfig {
    /// Run surface water flow only on even substeps (gravity still every
    /// substep). Default **on** — ~half the surface-flow scan work.
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
    // FPS knobs on → cap at 8 substeps (gravity dominates the mirror at
    // ~0.6 ms × 12). full_feel keeps the full ×12 path.
    let max_steps = if perf.flow_every_other_substep && perf.flow_quiet_early_out {
        FLOW_SUBSTEPS_MIN + 2 // 8
    } else {
        FLOW_SUBSTEPS
    };
    for step in 0..max_steps {
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
            // Steady busy halo (closed-loop rain): area barely moved —
            // further substeps are polish. Super-Server demo sat at
            // ~9k cells for all ×12; this exits after the minimum.
            if start_area > FLOW_QUIET_AREA && next_area * 10 >= start_area * 9 {
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

    // Re-wake unsupported grains and steep cliff faces. Cadence-gated:
    // full sticky-loose scan every 16 ticks; dirty-halo wake every 4.
    // Tick 0 always full-scans so save-load / first frame catch orphans.
    const GRAIN_WAKE_EVERY: u64 = 4;
    const GRAIN_WAKE_FULL_EVERY: u64 = 16;
    let mut freefall_woken = 0u32;
    if world.tick % GRAIN_WAKE_EVERY == 0 {
        if world.tick % GRAIN_WAKE_FULL_EVERY == 0 {
            freefall_woken = super::grain::wake_grains_for_settle(world);
        } else {
            let halo = filter_loose_regions(world, &flow_active);
            let coords: Vec<_> = halo.iter().map(|ac| ac.coord).collect();
            freefall_woken = super::grain::wake_grains_for_settle_coords(world, &coords);
        }
    }
    let grain_active = {
        let dirty = plan_active(world);
        let src = if dirty.is_empty() {
            flow_active
        } else {
            dirty
        };
        // Water-dirty ocean/sky chunks have no sand/litter — settle was
        // still walking them every tick (~physics gap on Super-Server).
        filter_loose_regions(world, &src)
    };
    if !grain_active.is_empty() {
        // Deep settle only for freefall / small paint dirty; busy shores
        // get a shallow repose polish (was burning toward ×1024 passes).
        let area = active_cell_area(&grain_active);
        let passes = if freefall_woken > 0 || area <= 1024 {
            GRAIN_SETTLE_PASSES
        } else {
            GRAIN_SETTLE_PASSES_SHALLOW
        };
        settle_loose_grains_regions(world, &grain_active, rooted, passes);
    }

    // Dense cargo cannot ride floating Organic/Snow/Ice. Full-grid punch
    // once per wake cadence, then a short re-settle so punched grains
    // sink through the water seat.
    if world.tick % GRAIN_WAKE_EVERY == 0
        && super::grain::punch_through_floating_rafts(world) > 0
    {
        let _ = super::grain::wake_grains_for_settle(world);
        let sink = filter_loose_regions(world, &plan_active(world));
        if !sink.is_empty() {
            settle_loose_grains_regions(world, &sink, rooted, GRAIN_SETTLE_PASSES);
        }
    }
    // Clear submerged litter lines, then let rafts drink — shared litter scan.
    match grain {
        Some(g) => super::grain::rise_and_soak_buoyant_litter_cfg(world, g),
        None => super::grain::rise_and_soak_buoyant_litter(world),
    }

    // Geotech: roof / overhang collapse after grain has seated.
    // Cadence-gated — Super-Server tail profile ~1.3 ms/full-grid call;
    // every other tick keeps cliffs responding within 2 frames.
    const FAILURE_EVERY: u64 = 2;
    let failure_stats = if world.tick % FAILURE_EVERY == 0 {
        crate::failure::apply_failure(world, failure, geotech)
    } else {
        crate::failure::FailureStats::default()
    };

    // Reset network sym "last" before field + later organism plant trade
    // share one inspector window (organism step clears plant lasts only).
    crate::symbiosis::clear_sym_net_flow_lasts(world);

    // Mycelium field: lives in Organic independently of fruiting bodies.
    match fungi {
        Some(f) => crate::fungi::step_mycelium_field_cfg(world, f),
        None => crate::fungi::step_mycelium_field(world),
    }

    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
    }

    #[cfg(debug_assertions)]
    if let Some(before) = mass_before {
        let after = crate::audit::sat_totals(world);
        crate::audit::assert_cell_sat_conserved(&before, &after, "tick_with_life");
    }

    failure_stats
}
