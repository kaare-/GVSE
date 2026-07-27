//! Headless: single side-muscle vertical arm must move under scripted drive.
//!
//! Mirrors the live bench that showed "no reaction on running".
//!
//! ```bash
//! cargo test -p wk-voxel-studio --test hinge_reacts --release -- --nocapture
//! ```

use wk_voxel_studio::{
    ArenaConfig, StudioArena, StudioPhysicsConfig, TissueKind,
};

fn paint_screenshot_arm(arena: &mut StudioArena) {
    let cx = 20i32;
    let top = 50i32;
    // Fixture stem + force sense
    for y in (top - 16)..=top {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Fixture);
    }
    arena
        .body
        .paint
        .set(cx as u32, (top - 8) as u32, TissueKind::ForceSensor);
    // Proximal bone (1px column)
    for y in (top - 22)..(top - 16) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    // Joint
    arena
        .body
        .paint
        .set(cx as u32, (top - 23) as u32, TissueKind::JointHalf);
    // Distal bone + foot
    for y in (top - 32)..(top - 23) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    arena
        .body
        .paint
        .set((cx - 1) as u32, (top - 32) as u32, TissueKind::Bone);
    arena
        .body
        .paint
        .set((cx + 1) as u32, (top - 32) as u32, TissueKind::Bone);
    // Muscle on the right spanning the joint (as in the screenshot)
    for y in (top - 25)..=(top - 22) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Muscle);
    }
    // Neuron on proximal + nerve to muscle
    arena
        .body
        .paint
        .set((cx + 2) as u32, (top - 19) as u32, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 3) as u32, (top - 19) as u32, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 2) as u32, (top - 18) as u32, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 3) as u32, (top - 18) as u32, TissueKind::NeuronBlob);
    for y in (top - 24)..=(top - 19) {
        arena
            .body
            .paint
            .set((cx + 2) as u32, y as u32, TissueKind::Nerve);
    }
}

#[test]
fn screenshot_arm_reacts_under_script() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 48,
        height: 64,
        seed: 7,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    arena.physics.scripted_muscle = true;
    paint_screenshot_arm(&mut arena);

    let g = arena.activate().expect("activate");
    let n_joints = g.joints.len();
    let n_mus = g.muscles.len();
    let n_hinged = g.hinged_bone_count();
    let local_n = g.joints.first().map(|j| j.local_cells.len()).unwrap_or(0);
    let swing = g.joints.first().map(|j| j.swing_part).unwrap_or(0);
    let scripted = arena.physics.scripted_muscle;
    assert!(scripted, "scripted drive must stay on after Enter");
    assert!(
        n_joints >= 1,
        "no joint — bone column may be bypassing the cyan cell (keep bones 1px wide)"
    );
    assert!(
        n_mus >= 1,
        "no muscle link — red must touch the joint or both bones"
    );
    assert!(n_hinged >= 1, "distal not hinged");
    assert!(local_n > 0, "hinge missing local cells");
    let mut max_ang = 0.0f32;
    for _ in 0..200 {
        arena.tick();
        let p = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == swing)
            .unwrap();
        max_ang = max_ang.max(p.hinge_angle.abs());
        assert_eq!(p.offset_x, 0);
        assert_eq!(p.offset_y, 0);
    }
    assert!(
        max_ang > 0.15,
        "no hinge reaction under script (max |θ|={max_ang})"
    );
}
