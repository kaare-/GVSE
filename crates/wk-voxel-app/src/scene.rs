//! wk-voxel-app scene state.
//!
//! Isolation: only wk-voxel + wk-material dependencies. No column
//! stack imports.

use wk_voxel::{stamp_world, Humidity, World, WorldgenParams};

/// Humidity tile side (world cells per humidity sample). Coarse
/// enough that a fair map fits comfortably in memory, fine enough
/// that "rain-soaked plains" and "dry mountains" read differently.
const HUMIDITY_TILE_COLS: i32 = 4;

pub struct Scene {
    pub world: World,
    pub params: WorldgenParams,
    pub humidity: Humidity,
}

impl Scene {
    pub fn new(params: WorldgenParams) -> Self {
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        Self {
            world,
            params,
            humidity: Humidity::new(HUMIDITY_TILE_COLS),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scene_new_stamps_world_deterministically() {
        let params = WorldgenParams::default();
        let a = Scene::new(params);
        let b = Scene::new(params);
        // Compare a random cell — full grid compare is covered in
        // wk-voxel's worldgen tests already.
        let ca = a.world.get_cell(100, 20);
        let cb = b.world.get_cell(100, 20);
        assert_eq!(ca, cb);
    }

    #[test]
    fn scene_starts_with_empty_humidity() {
        let s = Scene::new(WorldgenParams::default());
        assert_eq!(s.humidity.total_mass(), 0.0);
    }
}
