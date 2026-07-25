//! Headless scenario builders for tests / training episodes.

use wk_material::MaterialId;

use crate::arena::{ArenaConfig, StudioArena};
use crate::physics::StudioPhysicsConfig;
use crate::tissue::TissueKind;

/// Fixture + hinged free bone + muscle (flapping fin seed).
pub fn paint_fin_bench(arena: &mut StudioArena) {
    let mid_y = arena.cfg.height / 2;
    for y in (mid_y - 8)..(mid_y + 8) {
        arena.body.paint.set(2, y as u32, TissueKind::Fixture);
    }
    arena.body.paint.set(3, mid_y as u32, TissueKind::Bone);
    arena.body.paint.set(4, mid_y as u32, TissueKind::Bone);
    arena
        .body
        .paint
        .set(5, mid_y as u32, TissueKind::JointHalf);
    arena.body.paint.set(6, mid_y as u32, TissueKind::Bone);
    arena.body.paint.set(7, mid_y as u32, TissueKind::Bone);
    arena.body.paint.set(8, mid_y as u32, TissueKind::Bone);
    arena.body.paint.set(9, mid_y as u32, TissueKind::Bone);
    for x in 4..=7 {
        arena
            .body
            .paint
            .set(x, (mid_y + 1) as u32, TissueKind::Muscle);
    }
    // Controller blob + nerve path toward the muscle (S3 → S4).
    let by = (mid_y - 2) as u32;
    arena.body.paint.set(10, by, TissueKind::NeuronBlob);
    arena.body.paint.set(11, by, TissueKind::NeuronBlob);
    arena.body.paint.set(10, by + 1, TissueKind::NeuronBlob);
    arena.body.paint.set(11, by + 1, TissueKind::NeuronBlob);
    arena.body.paint.set(9, by, TissueKind::Nerve);
    arena.body.paint.set(8, by, TissueKind::Nerve);
    arena.body.paint.set(8, (mid_y + 1) as u32, TissueKind::Nerve);
}

/// Sawtooth rough ground for dry gait benches.
pub fn paint_rough_terrain(arena: &mut StudioArena) {
    let w = arena.cfg.width;
    let floor = 3;
    for x in 1..w - 1 {
        let bump = ((x / 3) % 4) as i32;
        for y in 1..=(floor + bump) {
            let mat = if bump >= 2 {
                MaterialId::Gravel
            } else {
                MaterialId::Sand
            };
            arena.paint_terrain(x, y, mat);
        }
        // Occasional stone ridge.
        if x % 11 == 0 {
            arena.paint_terrain(x, floor + bump + 1, MaterialId::Stone);
        }
    }
}

pub fn fin_hydro_arena() -> StudioArena {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 64,
        height: 48,
        seed: 0xF10B_EAC1,
        water_to_y: Some(30),
    });
    arena.physics = StudioPhysicsConfig::hydro_fin();
    paint_fin_bench(&mut arena);
    arena
}

pub fn rough_walk_arena() -> StudioArena {
    let mut arena = StudioArena::new(ArenaConfig::from_chunks(3, 2, 0xA1C0_0001));
    arena.physics = StudioPhysicsConfig::dry_walk();
    paint_rough_terrain(&mut arena);
    arena
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fin_bench_activates_with_muscle() {
        let mut arena = fin_hydro_arena();
        let g = arena.activate().unwrap();
        assert!(g.muscles.len() >= 1);
        assert!(g.bone_count() >= 2);
        assert!(g.has_controller, "fin example paints a neuron blob");
        assert!(arena.body.net.is_some(), "controller enables a StudioNet");
        assert!(!arena.physics.scripted_muscle);
    }

    #[test]
    fn rough_terrain_has_sand_or_gravel() {
        let arena = rough_walk_arena();
        let mut solids = 0;
        for y in 1..8 {
            for x in 1..arena.cfg.width - 1 {
                let m = arena.world.get_cell(x, y).unwrap().material;
                if matches!(
                    m,
                    MaterialId::Sand | MaterialId::Gravel | MaterialId::Stone
                ) {
                    solids += 1;
                }
            }
        }
        assert!(solids > 20, "expected painted terrain, got {solids} cells");
    }
}
