//! Headless scenario builders for tests / training episodes.

use wk_material::MaterialId;

use crate::arena::{ArenaConfig, StudioArena};
use crate::physics::StudioPhysicsConfig;
use crate::tissue::{JointLimit, TissueKind};

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

/// Vertical arm: fixture stem + force sense + bone–joint–bone + bilateral muscle.
pub fn paint_vertical_arm(arena: &mut StudioArena, joint: JointLimit) {
    let cx = arena.cfg.width / 2;
    let top = arena.cfg.height - 6;
    for x in (cx - 6)..(cx + 6) {
        arena.body.paint.set(x as u32, top as u32, TissueKind::Fixture);
    }
    for y in (top - 12)..top {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Fixture);
    }
    arena
        .body
        .paint
        .set(cx as u32, (top - 6) as u32, TissueKind::ForceSensor);

    let joint_kind = match joint {
        JointLimit::Full => TissueKind::JointFull,
        JointLimit::ThreeQuarter => TissueKind::JointThreeQuarter,
        JointLimit::Half => TissueKind::JointHalf,
        JointLimit::Quarter => TissueKind::JointQuarter,
    };
    // Proximal bone, cyan joint, distal bone.
    arena.body.paint.set(cx as u32, (top - 13) as u32, TissueKind::Bone);
    arena.body.paint.set(cx as u32, (top - 14) as u32, TissueKind::Bone);
    arena
        .body
        .paint
        .set(cx as u32, (top - 15) as u32, joint_kind);
    arena.body.paint.set(cx as u32, (top - 16) as u32, TissueKind::Bone);
    arena.body.paint.set(cx as u32, (top - 17) as u32, TissueKind::Bone);
    arena.body.paint.set(cx as u32, (top - 18) as u32, TissueKind::Bone);
    arena.body.paint.set(cx as u32, (top - 19) as u32, TissueKind::Bone);

    // Bilateral antagonists flanking the hinge.
    for y in (top - 18)..=(top - 15) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Muscle);
        arena
            .body
            .paint
            .set((cx - 1) as u32, y as u32, TissueKind::Muscle);
    }

    // Small controller + nerves to each muscle column.
    let ny = (top - 12) as u32;
    arena.body.paint.set((cx + 3) as u32, ny, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 4) as u32, ny, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 3) as u32, ny + 1, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 4) as u32, ny + 1, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 2) as u32, ny, TissueKind::Nerve);
    arena
        .body
        .paint
        .set((cx + 2) as u32, (top - 16) as u32, TissueKind::Nerve);
    arena
        .body
        .paint
        .set((cx - 2) as u32, (top - 16) as u32, TissueKind::Nerve);
    arena
        .body
        .paint
        .set((cx + 2) as u32, (top - 15) as u32, TissueKind::Nerve);
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

pub fn vertical_arm_arena(joint: JointLimit) -> StudioArena {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 48,
        height: 64,
        seed: 0xA8_00_01,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    paint_vertical_arm(&mut arena, joint);
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
        assert!(arena.body.net.is_some(), "controller attaches a StudioNet");
        // Scripted stays on until N / training — random net would idle the fin.
        assert!(arena.physics.scripted_muscle);
    }

    #[test]
    fn vertical_arm_bench_wires_hinge_and_antagonists() {
        let mut arena = vertical_arm_arena(JointLimit::Half);
        let g = arena.activate().unwrap();
        assert!(g.joints.len() >= 1);
        assert!(g.muscles.len() >= 2, "bilateral muscles");
        assert!(g.hinged_bone_count() >= 1);
        assert!(matches!(g.joints[0].limit, JointLimit::Half));
        assert!(g.joints[0].rest_radius >= 1.0);
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
