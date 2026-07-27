//! Light endings (day × column) and vestibular / cochlea-like gyro sensors.
//!
//! ```bash
//! cargo test -p wk-voxel-studio --test light_vestibular --release -- --nocapture
//! ```

use wk_material::MaterialId;
use wk_voxel_studio::{
    ArenaConfig, SensorCounts, StudioArena, StudioNet, StudioPhysicsConfig, TissueKind,
};

#[test]
fn light_ending_dim_under_roof_bright_in_open() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 40,
        height: 32,
        seed: 3,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    // Open-sky photoreceptor on a free bone.
    arena.body.paint.set(8, 8, TissueKind::Bone);
    arena.body.paint.set(8, 9, TissueKind::LightEnding);
    // Roofed photoreceptor: solid stone above.
    arena.body.paint.set(20, 8, TissueKind::Bone);
    arena.body.paint.set(20, 9, TissueKind::LightEnding);
    for x in 18..23 {
        arena.paint_terrain(x, 14, MaterialId::Stone);
    }

    let g = arena.activate().unwrap();
    assert_eq!(g.lights.len(), 2);
    for _ in 0..4 {
        arena.tick();
    }
    let g = arena.body.graph.as_ref().unwrap();
    let open = g.lights.iter().find(|l| l.cells[0].0 == 8).unwrap().light;
    let roofed = g.lights.iter().find(|l| l.cells[0].0 == 20).unwrap().light;
    assert!(
        open > roofed + 0.15,
        "open sky should read brighter than under a roof (open={open}, roofed={roofed})"
    );
    assert!(
        open > 0.2,
        "noon day_factor should leave open photoreceptors lit (open={open})"
    );
}

#[test]
fn vestibular_reads_upright_and_feeds_net() {
    let mut arena = StudioArena::new(ArenaConfig {
        width: 32,
        height: 28,
        seed: 4,
        water_to_y: None,
    });
    arena.physics = StudioPhysicsConfig::body_only();
    // No scripted pull — keep the rest pose for the upright reading.
    arena.physics.scripted_muscle = false;
    for y in 4..16 {
        arena.body.paint.set(2, y as u32, TissueKind::Fixture);
    }
    // Vertical mast: distal bone + cochlea above the pivot (+Y = upright).
    arena.body.paint.set(3, 8, TissueKind::Bone);
    arena.body.paint.set(3, 9, TissueKind::Bone);
    arena.body.paint.set(3, 10, TissueKind::JointHalf);
    arena.body.paint.set(3, 11, TissueKind::Bone);
    arena.body.paint.set(3, 12, TissueKind::Bone);
    arena.body.paint.set(3, 13, TissueKind::Bone);
    arena.body.paint.set(3, 14, TissueKind::VestibularEnding);
    arena.body.paint.set(4, 10, TissueKind::Muscle);
    arena.body.paint.set(4, 11, TissueKind::Muscle);
    // Controller so activate attaches a net sized for the gyro channels.
    arena.body.paint.set(6, 8, TissueKind::NeuronBlob);
    arena.body.paint.set(7, 8, TissueKind::NeuronBlob);
    arena.body.paint.set(6, 9, TissueKind::NeuronBlob);
    arena.body.paint.set(7, 9, TissueKind::NeuronBlob);

    arena.activate().unwrap();
    for _ in 0..4 {
        arena.tick();
    }
    let g = arena.body.graph.as_ref().unwrap();
    assert_eq!(g.vestibulars.len(), 1);
    let v = &g.vestibulars[0];
    assert!(
        v.upright > 0.7,
        "resting upward mast should feel upright (upright={})",
        v.upright
    );
    let sensors = g.sensor_counts();
    assert_eq!(
        sensors,
        SensorCounts {
            pressure: 0,
            light: 0,
            vestibular: 1,
        }
    );
    let n_mus = g.muscles.len();
    let net = arena.body.net.as_ref().expect("controller attaches net");
    assert_eq!(net.n_vestibular, 1);
    assert_eq!(net.n_in, n_mus * 3 + 3);
    let input = StudioNet::encode_inputs(&g.muscle_feedback(), &g.sensor_frame());
    assert_eq!(input.len(), net.n_in);
    assert_eq!(net.kind_label(), "FF-v1");
}
