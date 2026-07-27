//! Gated studio CA tick — same rule functions as the world, optional passes.
//!
//! Gates skip work for training benches; they must not invent alternate
//! gravity / flow maths (docs/organism/STUDIO.md).

use wk_voxel::{
    apply_failure, apply_grain_fall_regions, apply_grain_repose_regions, apply_gravity_fall_regions,
    apply_seepage_regions, apply_water_flow_regions, clear_all_dirty, partition_checkerboard,
    plan_active, set_parallel_enabled, FailureConfig, PerfConfig, Rect, World, FLOW_QUIET_AREA,
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
        // Paint-friendly sandbox: solids settle and water can spread.
        Self::sandbox()
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

    /// General studio bench — gravity, lateral water flow, and grain.
    pub fn sandbox() -> Self {
        Self {
            ca_enabled: true,
            gravity: true,
            water_flow: true,
            seepage: false,
            grain: true,
            failure: false,
            body_enabled: true,
            scripted_muscle: true,
            perf: PerfConfig::default(),
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

    /// Turn on the CA passes water needs (gravity + lateral flow).
    pub fn ensure_water_physics(&mut self) {
        self.ca_enabled = true;
        self.gravity = true;
        self.water_flow = true;
    }
}

/// Re-dirty wet chunks so lateral flow runs after a dry/body-only settle.
pub fn wake_fluid_chunks(world: &mut World) {
    for chunk in world.chunks.values_mut() {
        if chunk.has_wet_air {
            chunk.dirty = Some(Rect::full());
        }
    }
}

/// Enable water CA gates and wake any settled fluid so it can spread.
pub fn enable_water_physics(world: &mut World, cfg: &mut StudioPhysicsConfig) {
    let already = cfg.ca_enabled && cfg.gravity && cfg.water_flow;
    cfg.ensure_water_physics();
    if !already {
        wake_fluid_chunks(world);
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
        // Pre-flow dirty seed — dry sand otherwise starves after clear_all_dirty.
        let tick_seed = plan_active(world);
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

        let mut active = plan_active(world);
        if active.is_empty() && (cfg.grain || cfg.seepage) {
            active = tick_seed;
        }
        if !active.is_empty() {
            if cfg.seepage {
                apply_seepage_regions(world, &active);
            }
            if cfg.grain {
                let passes = partition_checkerboard(&active);
                for pass in &passes {
                    apply_grain_fall_regions(world, pass);
                }
                let mut repose_active = plan_active(world);
                if repose_active.is_empty() {
                    repose_active = active;
                }
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

    #[test]
    fn sandbox_water_spreads_laterally() {
        let mut w = World::new(2);
        for cy in 0..2 {
            for cx in 0..2 {
                w.ensure_chunk(ChunkCoord::new(cx, cy));
            }
        }
        // Floor + a water column with dry air beside it.
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=6 {
            w.set_cell(8, y, Cell::water());
        }
        let cfg = StudioPhysicsConfig::sandbox();
        for _ in 0..80 {
            tick_world_gated(&mut w, &cfg);
        }
        let mut wet_neighbors = 0usize;
        for x in 0..24 {
            if x == 8 {
                continue;
            }
            if let Some(c) = w.get_cell(x, 1) {
                if !c.sat.is_empty() {
                    wet_neighbors += 1;
                }
            }
        }
        assert!(
            wet_neighbors >= 2,
            "sandbox flow should spread water sideways (wet seats={wet_neighbors})"
        );
    }

    #[test]
    fn sandbox_sand_tower_settles() {
        let mut w = World::new(11);
        for cy in 0..2 {
            for cx in 0..2 {
                w.ensure_chunk(ChunkCoord::new(cx, cy));
            }
        }
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=6 {
            w.set_cell(8, y, Cell::solid(MaterialId::Sand));
        }
        let cfg = StudioPhysicsConfig::sandbox();
        for _ in 0..80 {
            tick_world_gated(&mut w, &cfg);
        }
        let max_h = (1..=10)
            .filter(|&y| {
                w.get_cell(8, y)
                    .is_some_and(|c| c.material == MaterialId::Sand)
            })
            .max()
            .unwrap_or(0);
        assert!(
            max_h <= 3,
            "sandbox grain should flatten sand towers (max_h={max_h})"
        );
    }

    #[test]
    fn dry_walk_stacks_until_flow_enabled() {
        let mut w = World::new(2);
        for cy in 0..2 {
            for cx in 0..2 {
                w.ensure_chunk(ChunkCoord::new(cx, cy));
            }
        }
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=6 {
            w.set_cell(8, y, Cell::water());
        }
        let mut cfg = StudioPhysicsConfig::dry_walk();
        for _ in 0..40 {
            tick_world_gated(&mut w, &cfg);
        }
        let stacked = (1..=6)
            .filter(|&y| w.get_cell(8, y).is_some_and(|c| !c.sat.is_empty()))
            .count();
        assert!(stacked >= 4, "dry_walk keeps a tall wet column");
        enable_water_physics(&mut w, &mut cfg);
        for _ in 0..80 {
            tick_world_gated(&mut w, &cfg);
        }
        let spread = (0..24)
            .filter(|&x| x != 8 && w.get_cell(x, 1).is_some_and(|c| !c.sat.is_empty()))
            .count();
        assert!(spread >= 2, "enable_water_physics unlocks lateral flow");
    }
}
