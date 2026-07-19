//! wk-voxel-app scene state.
//!
//! Isolation: only wk-voxel + wk-material dependencies. No column
//! stack imports.

use wk_voxel::{stamp_world, World, WorldgenParams};

pub struct Scene {
    pub world: World,
    pub params: WorldgenParams,
}

impl Scene {
    pub fn new(params: WorldgenParams) -> Self {
        let mut world = World::new(params.seed);
        stamp_world(&mut world, &params);
        Self { world, params }
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
}
