//! Serial two-joint chain: distal pivot must follow the parent link.
//!
//! ```bash
//! cargo test -p wk-voxel-studio --test hinge_chain --release -- --nocapture
//! ```

use wk_voxel_studio::{
    ArenaConfig, JointLimit, StudioArena, StudioPhysicsConfig, TissueKind,
};

fn paint_two_joint_arm(arena: &mut StudioArena) {
    let cx = 20i32;
    let top = 55i32;
    for y in (top - 8)..=top {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Fixture);
    }
    // Proximal
    for y in (top - 14)..(top - 8) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    // Joint 1
    arena
        .body
        .paint
        .set(cx as u32, (top - 15) as u32, TissueKind::JointHalf);
    // Middle
    for y in (top - 22)..(top - 15) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    // Joint 2
    arena
        .body
        .paint
        .set(cx as u32, (top - 23) as u32, TissueKind::JointHalf);
    // Distal
    for y in (top - 30)..(top - 23) {
        arena.body.paint.set(cx as u32, y as u32, TissueKind::Bone);
    }
    // Muscles on each hinge (bilateral)
    for y in (top - 16)..=(top - 14) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Muscle);
        arena
            .body
            .paint
            .set((cx - 1) as u32, y as u32, TissueKind::Muscle);
    }
    for y in (top - 24)..=(top - 22) {
        arena
            .body
            .paint
            .set((cx + 1) as u32, y as u32, TissueKind::Muscle);
        arena
            .body
            .paint
            .set((cx - 1) as u32, y as u32, TissueKind::Muscle);
    }
}

#[test]
fn child_joint_pivot_follows_parent_swing() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 48,
        height: 64,
        seed: 3,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    arena.physics.scripted_muscle = true;
    paint_two_joint_arm(&mut arena);

    let g = arena.activate().expect("activate");
    assert_eq!(g.joints.len(), 2, "expected two hinges");
    assert_eq!(g.hinged_bone_count(), 2, "middle + distal hinged");

    // Root-nearest joint is parent hinge; further is child.
    let mut joints = g.joints.clone();
    joints.sort_by_key(|j| {
        // parent_part anchored or smaller root depth first
        j.pivot.1 // higher y first for this vertical paint
    });
    // In our paint, joint1 has higher y than joint2.
    let j1 = joints
        .iter()
        .max_by_key(|j| j.pivot.1)
        .expect("joint1")
        .clone();
    let j2 = joints
        .iter()
        .min_by_key(|j| j.pivot.1)
        .expect("joint2")
        .clone();
    assert_ne!(j1.swing_part, j2.swing_part);
    assert_eq!(j2.parent_part, j1.swing_part, "distal hinge parents to middle bone");

    let p2_rest = j2.pivot;
    let mut saw_parent_angle = false;
    let mut saw_child_pivot_move = false;
    let mut max_child_independent = 0.0f32;

    for _ in 0..220 {
        arena.tick();
        let g = arena.body.graph.as_ref().unwrap();
        let mid = g.parts.iter().find(|p| p.id == j1.swing_part).unwrap();
        let dist = g.parts.iter().find(|p| p.id == j2.swing_part).unwrap();
        if mid.hinge_angle.abs() > 0.15 {
            saw_parent_angle = true;
            // Child world pivot = transform of rest pivot through parent pose.
            let pivots = g.joint_world_pivots();
            // Find world pivot matching j2 by comparing to rotated expectation.
            let parent_cum = mid.hinge_angle; // mid is first link
            let (s, c) = parent_cum.sin_cos();
            let lx = (p2_rest.0 - j1.pivot.0) as f32;
            let ly = (p2_rest.1 - j1.pivot.1) as f32;
            let expect = (
                j1.pivot.0 + (lx * c - ly * s).round() as i32,
                j1.pivot.1 + (lx * s + ly * c).round() as i32,
            );
            let child_world = pivots
                .iter()
                .copied()
                .find(|&p| (p.0 - expect.0).abs() <= 1 && (p.1 - expect.1).abs() <= 1);
            if child_world.is_some() && expect != p2_rest {
                saw_child_pivot_move = true;
            }
        }
        max_child_independent = max_child_independent.max(dist.hinge_angle.abs());
    }

    assert!(saw_parent_angle, "parent hinge should swing under script");
    assert!(
        saw_child_pivot_move,
        "child joint world pivot must move with the parent link (was stuck in paint space)"
    );
    // Child may also articulate on its own; just ensure gates still hold.
    let gate = JointLimit::Half.max_turns() * std::f32::consts::TAU + 0.02;
    assert!(max_child_independent <= gate + 0.01);
}

#[test]
fn two_joint_arm_activates_chain() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 40,
        height: 56,
        seed: 1,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    paint_two_joint_arm(&mut arena);
    let g = arena.activate().unwrap();
    assert_eq!(g.joints.len(), 2);
    let parents: Vec<_> = g.joints.iter().map(|j| j.parent_part).collect();
    let swings: Vec<_> = g.joints.iter().map(|j| j.swing_part).collect();
    assert!(parents.iter().any(|&p| {
        g.parts
            .iter()
            .any(|part| part.id == p && part.anchored)
    }));
    assert_eq!(swings.len(), 2);
    assert_ne!(swings[0], swings[1]);
}
