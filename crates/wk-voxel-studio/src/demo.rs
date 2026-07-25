//! Fin-bench paint helpers for headless tests.

use crate::tissue::{TissueKind, TissuePaint};

/// Fixture wall + joint + bone fin + muscle + force sensor (+ optional nerve).
///
/// Layout (x right, y up) near the left side of a `w×h` arena:
/// ```text
///  fixture column | joint | bone beam
///                 | muscle band under beam
///  force sensor on fixture edge
///  nerve → neuron blob above the joint
/// ```
pub fn fin_bench_paint(width: u32, height: u32) -> TissuePaint {
    let mut paint = TissuePaint::new(width, height);
    let fy = (height / 2).max(8);
    let fx = 4u32;

    // Fixture column.
    for y in (fy.saturating_sub(6))..(fy + 6).min(height - 1) {
        paint.set(fx, y, TissueKind::Fixture);
        paint.set(fx - 1, y, TissueKind::Fixture);
    }

    // Joint between fixture and bone (not direct bone↔fixture contact).
    paint.set(fx + 1, fy, TissueKind::JointHalf);

    // Bone fin extending right.
    for x in (fx + 2)..(fx + 8).min(width - 1) {
        paint.set(x, fy, TissueKind::Bone);
        if x < fx + 6 {
            paint.set(x, fy + 1, TissueKind::Bone);
        }
    }

    // Muscle linking bone tip region back toward fixture (touches both).
    // Path: under the bone, left to fixture face.
    let my = fy.saturating_sub(1);
    for x in fx..=(fx + 6).min(width - 1) {
        paint.set(x, my, TissueKind::Muscle);
    }
    // Ensure muscle 4-touches fixture and bone.
    paint.set(fx, my, TissueKind::Muscle);
    paint.set(fx + 5, fy, TissueKind::Bone); // already bone; keep
    paint.set(fx + 5, my, TissueKind::Muscle);

    // Force sensor adjacent to fixture (right face).
    paint.set(fx + 1, fy.saturating_sub(2), TissueKind::ForceSensor);
    // Sensor must be on/near fixture — place on fixture edge cell too.
    paint.set(fx, fy.saturating_sub(2), TissueKind::ForceSensor);

    // Nerve + neuron blob for neural controller.
    paint.set(fx + 1, fy + 2, TissueKind::Nerve);
    paint.set(fx + 1, fy + 3, TissueKind::Nerve);
    paint.set(fx + 2, fy + 3, TissueKind::NeuronBlob);
    paint.set(fx + 3, fy + 3, TissueKind::NeuronBlob);
    paint.set(fx + 2, fy + 4, TissueKind::NeuronBlob);
    paint.set(fx + 3, fy + 4, TissueKind::NeuronBlob);

    paint
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::body::activate;

    #[test]
    fn fin_bench_activates_with_joint_muscle_sensor() {
        let paint = fin_bench_paint(32, 32);
        let g = activate(&paint).unwrap();
        assert_eq!(g.fixture_count(), 1);
        assert_eq!(g.bone_count(), 1);
        assert_eq!(g.anchored_bone_count(), 0);
        assert!(!g.joints.is_empty());
        assert!(!g.muscles.is_empty());
        assert!(!g.sensors.is_empty());
        assert!(g.neural.is_some());
    }
}
