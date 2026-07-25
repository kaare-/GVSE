//! Gated studio CA tick — same rule functions as the world, optional passes.
//!
//! Gates skip work for training benches; they must not invent alternate
//! gravity / flow maths (docs/organism/STUDIO.md).

use wk_voxel::{
    apply_failure, apply_grain_fall_regions, apply_grain_repose_regions, apply_gravity_fall_regions,
    apply_seepage_regions, apply_water_flow_regions, clear_all_dirty, partition_checkerboard,
    plan_active, set_parallel_enabled, FailureConfig, PerfConfig, World, FLOW_QUIET_AREA,
    FLOW_SUBSTEPS, FLOW_SUBSTEPS_MIN,
};

/// Which production CA passes the studio runs each tick.
#[derive(Debug, Clone, Copy)]
pub struct StudioPhysicsConfig {
    /// Master switch for voxel CA. Off → body-only ticks.
    pub ca_enabled: bool,
    pub gravity: bool,
    pub water_flow: bool,
    pub seepage: bool,
    pub grain: bool,
    pub failure: bool,
    /// Step the activated body graph after CA.
    pub body_enabled: bool,
    /// Open-loop sinusoidal muscle drive (S2); off when a net takes over.
    pub scripted_muscle: bool,
    pub perf: PerfConfig,
}

impl Default for StudioPhysicsConfig {
    fn default() -> Self {
        // General bench: solids settle, no ocean flow until opted in.
        Self::dry_walk()
    }
}

impl StudioPhysicsConfig {
    /// Morphology / controller debug without CA cost.
    pub fn body_only() -> Self {
        Self {
            ca_enabled: false,
            gravity: false,
            water_flow: false,
            seepage: false,
            grain: false,
            failure: false,
            body_enabled: true,
            scripted_muscle: true,
            perf: PerfConfig {
                parallel_physics: true,
                ..PerfConfig::default()
            },
        }
    }

    /// Walking / rough terrain — solids settle; skip ocean flow.
    pub fn dry_walk() -> Self {
        Self {
            ca_enabled: true,
            gravity: true,
            water_flow: false,
            seepage: false,
            grain: true,
            failure: false,
            body_enabled: true,
            scripted_muscle: true,
            perf: PerfConfig::default(),
        }
    }

    /// Flapping fin in water.
    pub fn hydro_fin() -> Self {
        Self {
            ca_enabled: true,
            gravity: true,
            water_flow: true,
            seepage: false,
            grain: false,
            failure: false,
            body_enabled: true,
            scripted_muscle: true,
            perf: PerfConfig::default(),
        }
    }

    /// Match world-demo CA (still no rain/clouds unless the app adds them).
    pub fn full() -> Self {
        Self {
            ca_enabled: true,
            gravity: true,
            water_flow: true,
            seepage: true,
            grain: true,
            failure: true,
            body_enabled: true,
            scripted_muscle: true,
            perf: PerfConfig::default(),
        }
    }
}

fn active_cell_area(active: &[wk_voxel::ActiveChunk]) -> usize {
    active
        .iter()
        .map(|a| {
            let w = (a.rect.x1 as usize).saturating_sub(a.rect.x0 as usize) + 1;
            let h = (a.rect.y1 as usize).saturating_sub(a.rect.y0 as usize) + 1;
            w.saturating_mul(h)
        })
        .sum()
}

/// Run gated production passes; always advances `world.tick` when called
/// so body scripting stays phase-locked to the clock.
pub fn tick_world_gated(world: &mut World, cfg: &StudioPhysicsConfig) {
    set_parallel_enabled(cfg.perf.parallel_physics);
    if cfg.ca_enabled && (cfg.gravity || cfg.water_flow || cfg.seepage || cfg.grain || cfg.failure)
    {
        for step in 0..FLOW_SUBSTEPS {
            let active = plan_active(world);
            clear_all_dirty(world);
            if active.is_empty() {
                break;
            }
            if cfg.gravity {
                let passes = partition_checkerboard(&active);
                for pass in &passes {
                    apply_gravity_fall_regions(world, pass);
                }
            }
            let run_flow = cfg.water_flow
                && (!cfg.perf.flow_every_other_substep || step % 2 == 0);
            if run_flow {
                apply_water_flow_regions(world, &active);
            }
            if cfg.perf.flow_quiet_early_out && step + 1 >= FLOW_SUBSTEPS_MIN {
                let next = plan_active(world);
                if next.is_empty() || active_cell_area(&next) <= FLOW_QUIET_AREA {
                    break;
                }
            }
        }

        let active = plan_active(world);
        if !active.is_empty() {
            if cfg.seepage {
                apply_seepage_regions(world, &active);
            }
            if cfg.grain {
                let passes = partition_checkerboard(&active);
                for pass in &passes {
                    apply_grain_fall_regions(world, pass);
                }
                let repose_active = plan_active(world);
                if !repose_active.is_empty() {
                    let repose_passes = partition_checkerboard(&repose_active);
                    for pass in &repose_passes {
                        apply_grain_repose_regions(world, pass);
                    }
                }
            }
        }

        if cfg.failure {
            apply_failure(world, &FailureConfig::default(), None);
        }
    }

    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;
    use wk_voxel::{Cell, ChunkCoord};

    #[test]
    fn body_only_still_advances_tick() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(0, 0, Cell::solid(MaterialId::Bedrock));
        let t0 = w.tick;
        tick_world_gated(&mut w, &StudioPhysicsConfig::body_only());
        assert_eq!(w.tick, t0 + 1);
    }
}
