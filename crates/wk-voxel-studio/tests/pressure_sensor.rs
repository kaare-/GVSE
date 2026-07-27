//! Pressure nerve endings sample contact / water and feed the net.
//!
//! ```bash
//! cargo test -p wk-voxel-studio --test pressure_sensor --release -- --nocapture
//! ```

use wk_material::MaterialId;
use wk_voxel_studio::{
    ArenaConfig, SensorFrame, StudioArena, StudioNet, StudioPhysicsConfig, TissueKind,
};

#[test]
fn pressure_ending_reads_under_skin_and_on_sand() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 40,
        height: 32,
        seed: 2,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::sandbox();
    // Floor
    for x in 1..39 {
        arena.paint_terrain(x, 1, MaterialId::Sand);
        arena.paint_terrain(x, 2, MaterialId::Sand);
    }
    // Bone resting on sand with skin coat + pressure under the skin.
    arena.body.paint.set(20, 3, TissueKind::Bone);
    arena.body.paint.set(21, 3, TissueKind::Bone);
    arena.body.paint.set(20, 4, TissueKind::Skin);
    arena.body.paint.set(21, 4, TissueKind::Skin);
    // Pressure between bone and sand (under skin laterally / at contact).
    arena
        .body
        .paint
        .set(20, 3, TissueKind::Bone);
    arena
        .body
        .paint
        .set(19, 3, TissueKind::PressureEnding);
    arena
        .body
        .paint
        .set(22, 3, TissueKind::PressureEnding);

    let g = arena.activate().unwrap();
    assert_eq!(g.pressures.len(), 2, "two pressure endings");
    for _ in 0..10 {
        arena.tick();
    }
    let g = arena.body.graph.as_ref().unwrap();
    let mean = g.mean_pressure();
    assert!(
        mean > 0.05,
        "pressure endings next to sand should read contact (P={mean})"
    );
}

#[test]
fn net_grows_pressure_inputs() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 32,
        height: 24,
        seed: 1,
        water_to_y: None,
    });
    for y in 4..12 {
        arena.body.paint.set(2, y as u32, TissueKind::Fixture);
    }
    arena.body.paint.set(3, 8, TissueKind::Bone);
    arena.body.paint.set(4, 8, TissueKind::Bone);
    arena.body.paint.set(5, 8, TissueKind::JointHalf);
    arena.body.paint.set(6, 8, TissueKind::Bone);
    arena.body.paint.set(7, 8, TissueKind::Bone);
    arena.body.paint.set(4, 9, TissueKind::Muscle);
    arena.body.paint.set(5, 9, TissueKind::Muscle);
    arena.body.paint.set(6, 9, TissueKind::Muscle);
    arena.body.paint.set(8, 8, TissueKind::PressureEnding);
    // Controller blob so activate attaches a net.
    arena.body.paint.set(9, 6, TissueKind::NeuronBlob);
    arena.body.paint.set(10, 6, TissueKind::NeuronBlob);
    arena.body.paint.set(9, 7, TissueKind::NeuronBlob);
    arena.body.paint.set(10, 7, TissueKind::NeuronBlob);

    arena.activate().unwrap();
    let g = arena.body.graph.as_ref().unwrap();
    assert_eq!(g.pressures.len(), 1);
    let n_mus = g.muscles.len();
    let fb = g.muscle_feedback();
    let frame = SensorFrame {
        pressure: g.pressure_samples(),
        ..SensorFrame::default()
    };
    let summary = g.neural_summary();
    assert_eq!(summary.n_effectors, n_mus);
    assert_eq!(summary.n_pressure, 1);
    assert_eq!(summary.n_light, 0);
    assert_eq!(summary.n_vestibular, 0);
    let net = arena.body.net.as_ref().expect("controller attaches net");
    assert_eq!(net.n_pressure, 1);
    assert_eq!(net.n_in, n_mus * 3 + 1);
    assert_eq!(net.kind_label(), "FF-v1");
    let input = StudioNet::encode_inputs(&fb, &frame);
    assert_eq!(input.len(), net.n_in);
}
