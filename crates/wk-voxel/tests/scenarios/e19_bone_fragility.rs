//! E19 — Bone fragility under load (Wave N).
//!
//! 1. Wide Bone roof over Air collapses via F1; debris is Sand (Bone is
//!    not a grain, so identity-keep would strand solid Bone mid-air).
//! 2. Dead Bone under a tall overburden stack crushes to Sand when
//!    `enable_bone_crush` is on.
//! 3. Live soft `ModuleId::Bone` under a self-stack fractures and drops Sand.

use wk_material::MaterialId;
use wk_voxel::{
    apply_bone_crush, fracture_overloaded_bones, tick_with_configs, Atom, BodyModule, Cell,
    ChunkCoord, FailureConfig, ModuleId, PerfConfig, PixelTraits, World,
};

use crate::helpers::{count_material, lay_bedrock_floor};

#[test]
fn e19_bone_roof_collapses_to_sand() {
    let mut world = World::new(1919);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, 48);

    // Bone roof_span_max_m = 4.0 → 16 cells; cavity wider than that.
    let x0 = 4;
    let x1 = 24; // 21 Air
    world.set_cell(x0 - 1, 1, Cell::solid(MaterialId::Bone));
    world.set_cell(x1 + 1, 1, Cell::solid(MaterialId::Bone));
    for x in x0..=x1 {
        world.set_cell(x, 1, Cell::air());
        world.set_cell(x, 2, Cell::solid(MaterialId::Bone));
    }

    let roof_before = count_material(&world, MaterialId::Bone, x0, x1, 2, 2);
    assert_eq!(roof_before, (x1 - x0 + 1) as usize);

    let fail = FailureConfig {
        enable_roof_collapse: true,
        enable_bone_crush: false,
        max_roof_events: 64,
        ..FailureConfig::default()
    };
    let perf = PerfConfig {
        parallel_physics: false,
        ..PerfConfig::default()
    };
    for _ in 0..8 {
        tick_with_configs(&mut world, &perf, &fail);
    }

    let roof_after = count_material(&world, MaterialId::Bone, x0, x1, 2, 2);
    let sand_cavity = count_material(&world, MaterialId::Sand, x0, x1, 1, 1);
    assert!(
        roof_after < roof_before,
        "bone ceiling should vacate (before={roof_before} after={roof_after})"
    );
    assert!(
        sand_cavity > 0,
        "collapse debris must be Sand in the cavity (got {sand_cavity})"
    );
}

#[test]
fn e19_dead_bone_crushes_under_overburden() {
    let mut world = World::new(1920);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, 16);
    world.set_cell(4, 1, Cell::solid(MaterialId::Bone));
    for y in 2..=10 {
        world.set_cell(4, y, Cell::solid(MaterialId::Sand));
    }

    let fail = FailureConfig {
        enable_bone_crush: true,
        enable_roof_collapse: false,
        use_geotech_map: false,
        bone_crush_chance_per_mille: 1000,
        max_bone_crush_events: 8,
        ..FailureConfig::default()
    };
    apply_bone_crush(&mut world, &fail, None);
    assert_eq!(
        world.get_cell(4, 1).map(|c| c.material),
        Some(MaterialId::Sand),
        "overburdened dead Bone must crush to Sand"
    );
}

#[test]
fn e19_live_soft_bone_fractures_under_self_stack() {
    let mut world = World::new(1921);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, 16);
    for x in 2..10 {
        world.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        for y in 2..12 {
            world.set_cell(x, y, Cell::air());
        }
    }

    let body: Vec<BodyModule> = vec![
        (0, 0, ModuleId::Nucleus),
        (1, 0, ModuleId::Bone),
        (1, 1, ModuleId::Muscle),
        (1, 2, ModuleId::Muscle),
        (1, 3, ModuleId::Muscle),
        (1, 4, ModuleId::Muscle),
    ];
    let mut traits = vec![PixelTraits::default(); body.len()];
    traits[1].stiffness = 0.15;
    let mut atom = Atom::from_body_with_traits(4, 3, 40.0, body, traits);
    let bones_before = atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Bone)
        .count();
    assert_eq!(bones_before, 1);
    assert!(fracture_overloaded_bones(&mut world, &mut atom));
    assert!(
        atom.body.iter().all(|(_, _, m)| *m != ModuleId::Bone),
        "soft live Bone must fracture off the body"
    );
    assert_eq!(
        world.get_cell(5, 3).map(|c| c.material),
        Some(MaterialId::Sand),
        "fracture drops Sand at the bone world cell"
    );
}
