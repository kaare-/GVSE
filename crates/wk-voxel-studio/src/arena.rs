//! Studio arena: a `wk_voxel::World` box that ticks with production CA.

use wk_material::MaterialId;
use wk_voxel::{tick_with_perf, Cell, ChunkCoord, PerfConfig, World, CHUNK_CELLS_H, CHUNK_CELLS_W};

use crate::body::{activate, step_body, ActivateError, BodyGraph};
use crate::tissue::{StudioBody, TissuePaint};

/// Box arena dimensions in cells (kept small for interactive benches).
#[derive(Debug, Clone, Copy)]
pub struct ArenaConfig {
    pub width: i32,
    pub height: i32,
    pub seed: u64,
    /// Fill standing water up to this y (inclusive). `None` = dry air.
    pub water_to_y: Option<i32>,
}

impl Default for ArenaConfig {
    fn default() -> Self {
        Self {
            // Two chunks wide × two tall — enough for a fin bench.
            width: CHUNK_CELLS_W as i32 * 2,
            height: CHUNK_CELLS_H as i32 * 2,
            seed: 0x57_00_10,
            water_to_y: Some(CHUNK_CELLS_H as i32 + 20),
        }
    }
}

/// Shared-physics studio chamber.
pub struct StudioArena {
    pub world: World,
    pub cfg: ArenaConfig,
    pub body: StudioBody,
    pub perf: PerfConfig,
}

impl StudioArena {
    /// Build a bedrock box, optional water fill, empty tissue paint.
    pub fn new(cfg: ArenaConfig) -> Self {
        let mut world = World::new(cfg.seed);
        world.wrap_width = None;
        stamp_box(&mut world, &cfg);
        let paint = TissuePaint::new(cfg.width as u32, cfg.height as u32);
        Self {
            world,
            cfg,
            body: StudioBody::from_paint(paint),
            perf: PerfConfig::default(),
        }
    }

    /// Paint → [`BodyGraph`] (bones + fixtures). Clears prior offsets.
    pub fn activate(&mut self) -> Result<&BodyGraph, ActivateError> {
        let graph = activate(&self.body.paint)?;
        self.body.graph = Some(graph);
        self.body.activated = true;
        Ok(self.body.graph.as_ref().unwrap())
    }

    /// One production physics tick (same path as the world demo).
    pub fn tick_physics(&mut self) {
        tick_with_perf(&mut self.world, &self.perf);
    }

    /// CA tick, then body step (STUDIO.md frame order).
    pub fn tick(&mut self) {
        self.tick_physics();
        if let Some(graph) = self.body.graph.as_mut() {
            step_body(graph, &self.world);
        }
    }

    /// Flood or drain the interior (leaves the bedrock shell).
    pub fn set_water_to(&mut self, water_to_y: Option<i32>) {
        self.cfg.water_to_y = water_to_y;
        fill_interior(&mut self.world, &self.cfg);
    }
}

fn stamp_box(world: &mut World, cfg: &ArenaConfig) {
    let w = cfg.width.max(3);
    let h = cfg.height.max(3);
    let cx_max = (w + CHUNK_CELLS_W as i32 - 1) / CHUNK_CELLS_W as i32;
    let cy_max = (h + CHUNK_CELLS_H as i32 - 1) / CHUNK_CELLS_H as i32;
    for cy in 0..cy_max {
        for cx in 0..cx_max {
            world.ensure_chunk(ChunkCoord::new(cx, cy));
        }
    }
    for y in 0..h {
        for x in 0..w {
            let on_shell = x == 0 || y == 0 || x == w - 1 || y == h - 1;
            let cell = if on_shell {
                Cell::solid(MaterialId::Bedrock)
            } else if cfg.water_to_y.is_some_and(|wy| y <= wy) {
                Cell::water()
            } else {
                Cell::air()
            };
            world.set_cell(x, y, cell);
        }
    }
}

fn fill_interior(world: &mut World, cfg: &ArenaConfig) {
    let w = cfg.width.max(3);
    let h = cfg.height.max(3);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let cell = if cfg.water_to_y.is_some_and(|wy| y <= wy) {
                Cell::water()
            } else {
                Cell::air()
            };
            world.set_cell(x, y, cell);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_voxel::plan_active;

    #[test]
    fn arena_uses_real_world_and_ticks() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 64,
            height: 64,
            seed: 1,
            water_to_y: Some(20),
        });
        assert_eq!(
            arena.world.get_cell(0, 0).unwrap().material,
            MaterialId::Bedrock
        );
        assert!(arena.world.get_cell(8, 10).unwrap().sat.0 > 0);
        let tick0 = arena.world.tick;
        arena.tick_physics();
        assert_eq!(arena.world.tick, tick0 + 1);
        let _ = plan_active(&arena.world);
    }
}
