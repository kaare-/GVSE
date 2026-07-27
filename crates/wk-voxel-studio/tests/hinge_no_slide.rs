//! Headless regression: vertical arm must rotate about the joint, not slide.
//!
//! Run: `cargo test -p wk-voxel-studio --test hinge_no_slide --release`

use wk_voxel_studio::{
    ArenaConfig, JointLimit, StudioArena, StudioPhysicsConfig, TissueKind,
};

/// User-style layout: fixture stem, force sense, bone–joint–bone, side muscle + nerve/neuron.
fn paint_user_vertical_arm(arena: &mut StudioArena) {
    let cx = 16;
    let top = 40;
    for y in (top - 14)..=top {
        arena.body.paint.set(cx, y as u32, TissueKind::Fixture);
    }
    arena.body.paint.set(cx, (top - 7) as u32, TissueKind::ForceSensor);
    // Proximal bone
    arena.body.paint.set(cx, (top - 15) as u32, TissueKind::Bone);
    arena.body.paint.set(cx, (top - 16) as u32, TissueKind::Bone);
    arena.body.paint.set(cx, (top - 17) as u32, TissueKind::Bone);
    // Joint
    arena
        .body
        .paint
        .set(cx, (top - 18) as u32, TissueKind::JointHalf);
    // Distal bone
    for y in (top - 24)..(top - 18) {
        arena.body.paint.set(cx, y as u32, TissueKind::Bone);
    }
    // Muscle on the right of distal (touches joint)
    for y in (top - 21)..=(top - 18) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Muscle);
    }
    // Neuron on proximal + nerve down to muscle
    arena
        .body
        .paint
        .set((cx + 2) as u32, (top - 16) as u32, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 3) as u32, (top - 16) as u32, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 2) as u32, (top - 15) as u32, TissueKind::NeuronBlob);
    arena
        .body
        .paint
        .set((cx + 3) as u32, (top - 15) as u32, TissueKind::NeuronBlob);
    for y in (top - 20)..=(top - 16) {
        arena
            .body
            .paint
            .set((cx + 2) as u32, y as u32, TissueKind::Nerve);
    }
}

fn socket_cell(local: &[(i32, i32)]) -> (i32, i32) {
    // Closest rest cell to the pivot (min radius) — must stay on the pivot ring.
    *local
        .iter()
        .min_by(|a, b| {
            let ra = a.0 * a.0 + a.1 * a.1;
            let rb = b.0 * b.0 + b.1 * b.1;
            ra.cmp(&rb)
        })
        .expect("local cells")
}

#[test]
fn user_vertical_arm_rotates_without_sliding() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 48,
        height: 56,
        seed: 99,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    arena.physics.scripted_muscle = true;
    paint_user_vertical_arm(&mut arena);

    let g = arena.activate().expect("activate");
    assert!(g.joints.len() >= 1, "expected cyan joint");
    assert!(g.muscles.len() >= 1, "expected muscle spanning hinge");
    assert!(g.hinged_bone_count() >= 1, "distal must be hinged");
    let joint = g.joints[0].clone();
    assert!(!joint.local_cells.is_empty(), "hinge needs local socket cells");
    let swing_id = joint.swing_part;
    let socket_local = socket_cell(&joint.local_cells);
    let pivot = joint.pivot;

    let mut max_abs_angle = 0.0f32;
    let mut max_socket_gap = 0i32;
    for _ in 0..180 {
        arena.tick();
        let g = arena.body.graph.as_ref().unwrap();
        let swing = g.parts.iter().find(|p| p.id == swing_id).unwrap();
        assert_eq!(swing.offset_x, 0, "slide: offset_x became {}", swing.offset_x);
        assert_eq!(swing.offset_y, 0, "slide: offset_y became {}", swing.offset_y);
        max_abs_angle = max_abs_angle.max(swing.hinge_angle.abs());

        // Socket world cell must remain adjacent to the pivot.
        let (s, c) = swing.hinge_angle.sin_cos();
        let sx = pivot.0
            + (socket_local.0 as f32 * c - socket_local.1 as f32 * s).round() as i32;
        let sy = pivot.1
            + (socket_local.0 as f32 * s + socket_local.1 as f32 * c).round() as i32;
        let gap = (sx - pivot.0).abs().max((sy - pivot.1).abs());
        max_socket_gap = max_socket_gap.max(gap);
        assert!(
            gap <= 1,
            "socket detached/slid from pivot (gap={gap}, θ={})",
            swing.hinge_angle
        );
    }

    assert!(
        max_abs_angle > 0.15,
        "expected visible hinge rotation, max |θ|={max_abs_angle}"
    );
    let gate = JointLimit::Half.max_turns() * std::f32::consts::TAU + 0.02;
    assert!(
        max_abs_angle <= gate + 0.01,
        "JointHalf gate broken (|θ|={max_abs_angle} > {gate})"
    );
    assert!(max_socket_gap <= 1);
}

#[test]
fn bilateral_arm_keeps_socket_and_swings() {
    let mut arena = wk_voxel_studio::vertical_arm_arena(JointLimit::Half);
    arena.physics.scripted_muscle = true;
    let g = arena.activate().unwrap();
    let swing_id = g.joints[0].swing_part;
    let mut max_ang = 0.0f32;
    for _ in 0..120 {
        arena.tick();
        let p = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == swing_id)
            .unwrap();
        assert_eq!(p.offset_x, 0);
        assert_eq!(p.offset_y, 0);
        max_ang = max_ang.max(p.hinge_angle.abs());
    }
    assert!(max_ang > 0.15, "bilateral arm should swing, |θ|={max_ang}");
}
