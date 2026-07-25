//! Studio arena: scalable `wk_voxel::World` box + tissue overlay.

use wk_material::MaterialId;
use wk_voxel::{Cell, ChunkCoord, World, CHUNK_CELLS_H, CHUNK_CELLS_W};

use crate::body::{activate, step_body, ActivateError, BodyGraph};
use crate::neural::StudioNet;
use crate::physics::{tick_world_gated, StudioPhysicsConfig};
use crate::tissue::{StudioBody, TissuePaint};
use crate::train::apply_net;

/// Soft limits — large enough for rough-terrain walk tracks.
pub const ARENA_MIN: i32 = 32;
pub const ARENA_MAX: i32 = 512;

/// Box arena dimensions in cells.
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
            width: CHUNK_CELLS_W as i32 * 2,
            height: CHUNK_CELLS_H as i32 * 2,
            seed: 0x57_00_10,
            // Dry by default — fill water only when the scenario needs it.
            water_to_y: None,
        }
    }
}

impl ArenaConfig {
    pub fn clamp_size(mut self) -> Self {
        self.width = self.width.clamp(ARENA_MIN, ARENA_MAX);
        self.height = self.height.clamp(ARENA_MIN, ARENA_MAX);
        self
    }

    /// Size in whole chunks (each [`CHUNK_CELLS_W`] / [`CHUNK_CELLS_H`]).
    pub fn from_chunks(w_chunks: i32, h_chunks: i32, seed: u64) -> Self {
        Self {
            width: (w_chunks.max(1) * CHUNK_CELLS_W as i32).clamp(ARENA_MIN, ARENA_MAX),
            height: (h_chunks.max(1) * CHUNK_CELLS_H as i32).clamp(ARENA_MIN, ARENA_MAX),
            seed,
            water_to_y: None,
        }
        .clamp_size()
    }
}

/// Shared-physics studio chamber.
pub struct StudioArena {
    pub world: World,
    pub cfg: ArenaConfig,
    pub body: StudioBody,
    pub physics: StudioPhysicsConfig,
}

impl StudioArena {
    pub fn new(cfg: ArenaConfig) -> Self {
        let cfg = cfg.clamp_size();
        let mut world = World::new(cfg.seed);
        world.wrap_width = None;
        stamp_box(&mut world, &cfg);
        let paint = TissuePaint::new(cfg.width as u32, cfg.height as u32);
        Self {
            world,
            cfg,
            body: StudioBody::from_paint(paint),
            physics: StudioPhysicsConfig::default(),
        }
    }

    /// Rebuild at a new size (clears tissue; keeps physics gates).
    pub fn resize(&mut self, width: i32, height: i32) {
        let physics = self.physics;
        let water = self.cfg.water_to_y;
        let seed = self.cfg.seed;
        *self = Self::new(ArenaConfig {
            width,
            height,
            seed,
            water_to_y: water,
        });
        self.physics = physics;
    }

    pub fn activate(&mut self) -> Result<&BodyGraph, ActivateError> {
        let graph = activate(&self.body.paint)?;
        let n_mus = graph.muscles.len();
        // Prefer neural drive when a controller blob is painted and muscles exist.
        if n_mus > 0 && graph.has_controller {
            let need_new = match self.body.net.as_ref() {
                Some(net) => net.n_out != n_mus,
                None => true,
            };
            if need_new {
                self.body.net = Some(StudioNet::for_muscles(n_mus, self.cfg.seed ^ 0xC011_7E11));
            }
            self.physics.scripted_muscle = false;
        } else if n_mus > 0 {
            // Keep an existing compatible net; otherwise leave scripted mode.
            if let Some(net) = self.body.net.as_ref() {
                if net.n_out != n_mus {
                    self.body.net = Some(StudioNet::for_muscles(n_mus, self.cfg.seed ^ 0xC011_7E11));
                }
            }
        }
        self.body.graph = Some(graph);
        self.body.activated = true;
        Ok(self.body.graph.as_ref().unwrap())
    }

    /// Ensure a net matching current muscle count exists (for train / N toggle).
    pub fn ensure_net(&mut self, seed: u64) -> Option<&StudioNet> {
        let n_mus = self.body.graph.as_ref()?.muscles.len();
        if n_mus == 0 {
            return None;
        }
        let need_new = match self.body.net.as_ref() {
            Some(net) => net.n_out != n_mus,
            None => true,
        };
        if need_new {
            self.body.net = Some(StudioNet::for_muscles(n_mus, seed));
        }
        self.body.net.as_ref()
    }

    /// Paint any world [`MaterialId`] into the interior (shell stays bedrock).
    pub fn paint_terrain(&mut self, x: i32, y: i32, material: MaterialId) {
        let w = self.cfg.width;
        let h = self.cfg.height;
        if x <= 0 || y <= 0 || x >= w - 1 || y >= h - 1 {
            return;
        }
        let cell = match material {
            MaterialId::Air => Cell::air(),
            MaterialId::Water => Cell::water(),
            other => Cell::solid(other),
        };
        self.world.set_cell(x, y, cell);
    }

    /// Gated CA tick, then body step.
    ///
    /// When a net is attached and scripted muscle is off, the controller
    /// writes actuation from muscle feedback before the body step.
    pub fn tick(&mut self) {
        if !self.physics.scripted_muscle {
            if let Some(net) = self.body.net.clone() {
                apply_net(self, &net);
            }
        }
        tick_world_gated(&mut self.world, &self.physics);
        if self.physics.body_enabled {
            if let Some(graph) = self.body.graph.as_mut() {
                step_body(graph, &mut self.world, self.physics.scripted_muscle);
            }
        }
    }

    pub fn set_water_to(&mut self, water_to_y: Option<i32>) {
        self.cfg.water_to_y = water_to_y;
        fill_interior_keep_terrain(&mut self.world, &self.cfg);
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

/// Refill air/water in empty interior cells without wiping solid terrain.
fn fill_interior_keep_terrain(world: &mut World, cfg: &ArenaConfig) {
    let w = cfg.width.max(3);
    let h = cfg.height.max(3);
    for y in 1..h - 1 {
        for x in 1..w - 1 {
            let Some(c) = world.get_cell(x, y) else {
                continue;
            };
            if c.material != MaterialId::Air && c.material != MaterialId::Water {
                continue;
            }
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
    use crate::physics::StudioPhysicsConfig;

    #[test]
    fn arena_uses_real_world_and_ticks() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 64,
            height: 64,
            seed: 1,
            water_to_y: None,
        });
        assert_eq!(
            arena.world.get_cell(0, 0).unwrap().material,
            MaterialId::Bedrock
        );
        assert!(arena.world.get_cell(8, 10).unwrap().sat.is_empty());
        let tick0 = arena.world.tick;
        arena.tick();
        assert_eq!(arena.world.tick, tick0 + 1);
    }

    #[test]
    fn default_arena_is_dry() {
        let arena = StudioArena::new(ArenaConfig::default());
        assert!(arena.cfg.water_to_y.is_none());
        assert!(arena.world.get_cell(8, 8).unwrap().sat.is_empty());
    }

    #[test]
    fn paint_all_solid_materials() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 64,
            height: 64,
            seed: 1,
            water_to_y: None,
        });
        for (i, &mat) in MaterialId::ALL_SOLIDS.iter().enumerate() {
            let x = 2 + i as i32;
            arena.paint_terrain(x, 2, mat);
            assert_eq!(arena.world.get_cell(x, 2).unwrap().material, mat);
        }
    }

    #[test]
    fn resize_clamps_and_rebuilds() {
        let mut arena = StudioArena::new(ArenaConfig::default());
        arena.physics = StudioPhysicsConfig::dry_walk();
        arena.resize(16, 16); // below min → clamp
        assert!(arena.cfg.width >= ARENA_MIN);
        assert!(arena.cfg.height >= ARENA_MIN);
        assert!(!arena.physics.water_flow && arena.physics.grain);
    }

    #[test]
    fn rough_terrain_track_from_chunks() {
        let cfg = ArenaConfig::from_chunks(4, 2, 42);
        assert_eq!(cfg.width, CHUNK_CELLS_W as i32 * 4);
        assert_eq!(cfg.height, CHUNK_CELLS_H as i32 * 2);
        let arena = StudioArena::new(cfg);
        assert_eq!(arena.cfg.width, cfg.width);
    }
}
