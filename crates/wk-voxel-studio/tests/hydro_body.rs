//! Body ↔ water coupling on the shared CA.
//!
//! ```bash
//! cargo test -p wk-voxel-studio --test hydro_body --release -- --nocapture
//! ```

use wk_voxel_studio::{fin_hydro_arena, ArenaConfig, StudioArena, StudioPhysicsConfig, TissueKind};

fn total_sat_in_band(arena: &StudioArena, x0: i32, x1: i32, y0: i32, y1: i32) -> u32 {
    let mut s = 0u32;
    for y in y0..=y1 {
        for x in x0..=x1 {
            if let Some(c) = arena.world.get_cell(x, y) {
                if c.material == wk_material::MaterialId::Air {
                    s += c.sat.0 as u32;
                }
            }
        }
    }
    s
}

#[test]
fn flapping_fin_displaces_tank_water() {
    let mut arena = fin_hydro_arena();
    // Isolate body displace from CA leveling noise.
    arena.physics.water_flow = false;
    let g = arena.activate().expect("activate fin");
    assert!(g.hinged_bone_count() >= 1);
    assert!(!g.muscles.is_empty());

    let mid_y = arena.cfg.height / 2;
    assert!(
        total_sat_in_band(&arena, 5, 14, mid_y - 4, mid_y + 4) > 1000,
        "tank should be wet around the fin"
    );

    let mut swung = false;
    let mut max_imbalance = 0i32;
    for _ in 0..240 {
        arena.tick();
        let g = arena.body.graph.as_ref().unwrap();
        if g.parts.iter().any(|p| p.hinged && p.hinge_angle.abs() > 0.2) {
            swung = true;
        }
        // Stroke should shove water off-center (left vs right of the hinge).
        let left = total_sat_in_band(&arena, 1, 5, mid_y - 3, mid_y + 3) as i32;
        let right = total_sat_in_band(&arena, 9, 16, mid_y - 3, mid_y + 3) as i32;
        max_imbalance = max_imbalance.max((left - right).abs());
    }
    assert!(swung, "scripted fin should articulate in the tank");
    assert!(
        max_imbalance > 80,
        "hinge stroke must shove water sideways (L/R imbalance={max_imbalance})"
    );
}

#[test]
fn free_body_floats_slower_in_water() {
    let mut dry = StudioArena::new(ArenaConfig {
        width: 32,
        height: 40,
        seed: 1,
        water_to_y: None,
    });
    dry.physics = StudioPhysicsConfig::sandbox();
    dry.body.paint.set(10, 20, TissueKind::Bone);
    dry.body.paint.set(11, 20, TissueKind::Bone);
    dry.body.paint.set(12, 20, TissueKind::Bone);
    dry.activate().unwrap();
    for _ in 0..20 {
        dry.tick();
    }
    let dry_y = dry
        .body
        .graph
        .as_ref()
        .unwrap()
        .parts
        .iter()
        .find(|p| p.kind == wk_voxel_studio::PartKind::Bone)
        .unwrap()
        .offset_y;

    let mut wet = StudioArena::new(ArenaConfig {
        width: 32,
        height: 40,
        seed: 1,
        water_to_y: Some(32),
    });
    wet.physics = StudioPhysicsConfig::sandbox();
    // Start already submerged so drag/buoyancy apply immediately.
    wet.body.paint.set(10, 20, TissueKind::Bone);
    wet.body.paint.set(11, 20, TissueKind::Bone);
    wet.body.paint.set(12, 20, TissueKind::Bone);
    wet.activate().unwrap();
    for _ in 0..20 {
        wet.tick();
    }
    let wet_y = wet
        .body
        .graph
        .as_ref()
        .unwrap()
        .parts
        .iter()
        .find(|p| p.kind == wk_voxel_studio::PartKind::Bone)
        .unwrap()
        .offset_y;

    assert!(dry_y < 0, "dry bone falls");
    assert!(
        wet_y > dry_y,
        "water drag/buoyancy should slow the fall (dry={dry_y}, wet={wet_y})"
    );
}
