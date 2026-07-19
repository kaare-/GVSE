//! wk-voxel-app scene state.
//!
//! Isolation: only wk-voxel + wk-material dependencies. No column
//! stack imports.

use wk_voxel::{stamp_world, Humidity, OrganismStore, World, WorldgenParams};

/// Humidity tile side (world cells per humidity sample). Coarse
/// enough that a fair map fits comfortably in memory, fine enough
/// that "rain-soaked plains" and "dry mountains" read differently.
const HUMIDITY_TILE_COLS: i32 = 4;

pub struct Scene {
    pub world: World,
    pub params: WorldgenParams,
    pub humidity: Humidity,
    pub organisms: OrganismStore,
}

impl Scene {
    pub fn new(params: WorldgenParams) -> Self {
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        // Clamp atmospheric mass to the stamped cell rectangle so
        // diffusion can't grow an unbounded sparse haze off-map.
        // Ring worlds also wrap humidity in x so the atmosphere joins.
        let mut humidity = Humidity::with_world_bounds(
            HUMIDITY_TILE_COLS,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
        );
        humidity.wrap_x = params.wrap_x;
        let mut organisms = OrganismStore::new();
        // Seed Set A Atoms into coastal / ocean wet cells — the
        // visible life layer (black + green pixels), not a biomass wash.
        organisms.seed_coastal_atoms(
            &world,
            params.seed,
            0,
            params.width_cols,
            params.bedrock_floor_y,
            params.sky_ceiling_y,
            8,
            40.0,
        );
        Self {
            world,
            params,
            humidity,
            organisms,
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
        let ca = a.world.get_cell(100, 20);
        let cb = b.world.get_cell(100, 20);
        assert_eq!(ca, cb);
    }

    #[test]
    fn scene_starts_with_empty_humidity() {
        let s = Scene::new(WorldgenParams::default());
        assert_eq!(s.humidity.total_mass(), 0.0);
    }

    #[test]
    fn scene_seeds_set_a_atoms_in_water() {
        let s = Scene::new(WorldgenParams::default());
        assert!(
            !s.organisms.is_empty(),
            "demo scene should seed coastal Atoms"
        );
    }

    #[test]
    fn scene_humidity_is_clamped_to_stamped_world() {
        let s = Scene::new(WorldgenParams::default());
        let b = s.humidity.bounds.expect("demo scene sets bounds");
        assert_eq!(b.hx_min, 0);
        assert!(b.hx_max > 0);
        assert!(b.hy_max > b.hy_min);
        let cells_w = s.params.width_cols;
        let cells_h = s.params.sky_ceiling_y - s.params.bedrock_floor_y;
        let tiles_w = (cells_w + HUMIDITY_TILE_COLS - 1) / HUMIDITY_TILE_COLS;
        let tiles_h = (cells_h + HUMIDITY_TILE_COLS - 1) / HUMIDITY_TILE_COLS;
        assert_eq!(b.tile_capacity(), (tiles_w * tiles_h) as usize);
    }
}
