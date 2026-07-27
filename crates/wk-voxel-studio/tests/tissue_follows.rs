//! Soft tissue (skin / nerve / muscle) must ride hinged bones, not stay in paint space.
//!
//! ```bash
//! cargo test -p wk-voxel-studio --test tissue_follows --release -- --nocapture
//! ```

use wk_voxel_studio::{
    ArenaConfig, StudioArena, StudioPhysicsConfig, TissueKind,
};

fn paint_coated_arm(arena: &mut StudioArena) {
    let cx = 22i32;
    let top = 52i32;
    for y in (top - 10)..=top {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Fixture);
    }
    for y in (top - 16)..(top - 10) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    arena
        .body
        .paint
        .set(cx as u32, (top - 17) as u32, TissueKind::JointHalf);
    for y in (top - 26)..(top - 17) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    // Skin coat on distal bone (left)
    for y in (top - 25)..(top - 18) {
        arena
            .body
            .paint
            .set((cx - 1) as u32, y as u32, TissueKind::Skin);
    }
    // Nerve on distal bone (right), away from the hinge muscle
    for y in (top - 25)..(top - 20) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Nerve);
    }
    // Muscle beside the joint (touches proximal + distal via joint)
    for y in (top - 18)..=(top - 15) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Muscle);
    }
}

#[test]
fn skin_nerve_muscle_leave_paint_space_with_swing() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 48,
        height: 64,
        seed: 9,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    arena.physics.scripted_muscle = true;
    paint_coated_arm(&mut arena);

    let g = arena.activate().expect("activate");
    assert!(!g.skins.is_empty(), "skin patches expected");
    assert!(!g.nerves.is_empty(), "nerve strands expected");
    assert!(!g.muscles.is_empty(), "muscle expected");

    let skin_rest: Vec<_> = g.skins.iter().flat_map(|s| s.cells.clone()).collect();
    let nerve_rest: Vec<_> = g.nerves.iter().flat_map(|n| n.cells.clone()).collect();
    let muscle_rest: Vec<_> = g.muscles.iter().flat_map(|m| m.cells.clone()).collect();

    let mut swung = false;
    let mut skin_moved = false;
    let mut nerve_moved = false;
    let mut muscle_moved = false;

    for _ in 0..240 {
        arena.tick();
        let g = arena.body.graph.as_ref().unwrap();
        let distal_angle = g
            .parts
            .iter()
            .filter(|p| p.hinged)
            .map(|p| p.hinge_angle.abs())
            .fold(0.0f32, f32::max);
        if distal_angle < 0.2 {
            continue;
        }
        swung = true;

        // posed soft tissue must appear under kind_at away from rest cells
        let mut skin_world = 0usize;
        let mut nerve_world = 0usize;
        let mut muscle_world = 0usize;
        let mut skin_still_rest = 0usize;
        let mut nerve_still_rest = 0usize;

        for y in 0..64i32 {
            for x in 0..48i32 {
                match g.kind_at(x, y) {
                    Some(TissueKind::Skin) => {
                        skin_world += 1;
                        if skin_rest.contains(&(x, y)) {
                            skin_still_rest += 1;
                        }
                    }
                    Some(TissueKind::Nerve) => {
                        nerve_world += 1;
                        if nerve_rest.contains(&(x, y)) {
                            nerve_still_rest += 1;
                        }
                    }
                    Some(TissueKind::Muscle) => {
                        muscle_world += 1;
                        if !muscle_rest.contains(&(x, y)) {
                            muscle_moved = true;
                        }
                    }
                    _ => {}
                }
            }
        }

        assert!(skin_world > 0, "skin should still draw after pose");
        assert!(nerve_world > 0, "nerve should still draw after pose");
        assert!(muscle_world > 0, "muscle should still draw after pose");

        // Most soft cells should leave their paint seats once the bone swings.
        if skin_still_rest * 2 < skin_rest.len() {
            skin_moved = true;
        }
        if nerve_still_rest * 2 < nerve_rest.len() {
            nerve_moved = true;
        }
        if skin_moved && nerve_moved && muscle_moved {
            break;
        }
    }

    assert!(swung, "hinge should swing under script");
    assert!(skin_moved, "skin stayed in paint space");
    assert!(nerve_moved, "nerve stayed in paint space");
    assert!(muscle_moved, "muscle stayed in paint space");
}
