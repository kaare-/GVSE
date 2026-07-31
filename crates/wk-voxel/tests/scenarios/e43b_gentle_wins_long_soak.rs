//! E43 long soak — gentle rider lineages outlast smotherers (Wave AD).
//!
//! Product intent: smotherers (`host_leave = 0`) boom-and-bust with their
//! landlords; gentle riders (`host_leave ≈ 0.85`) keep the host alive and
//! remain seated. After a long strip soak, gentle epis > smotherer epis.

use wk_material::MaterialId;
use wk_voxel::{
    is_epiphyte, is_land_plant, BodyModule, Cell, ChunkCoord, ModuleId, OrganismStore, PixelTraits,
    PlantGrowthCaps, World,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 200;
        world.set_cell(x, 1, sand);
        for y in 2..=12 {
            world.set_cell(x, y, Cell::air());
        }
    }
}

fn host_body() -> Vec<BodyModule> {
    vec![
        (0, -1, ModuleId::Root),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Stem),
        (0, 2, ModuleId::Stem),
        (0, 3, ModuleId::Stem),
        (0, 4, ModuleId::Photosystem),
    ]
}

fn epi_body() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Holdfast),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
        (0, 2, ModuleId::Photosystem),
        (0, 3, ModuleId::Photosystem),
    ]
}

fn seat_pair(orgs: &mut OrganismStore, world: &World, hx: i32, leave: f32) {
    let host_gy = 2;
    let host_traits: Vec<PixelTraits> = host_body()
        .iter()
        .map(|&(_, _, m)| {
            let mut t = PixelTraits::default();
            t.upkeep_bias = 1.8;
            if m == ModuleId::Root {
                t.drink_bias = 0.08;
            }
            if m == ModuleId::Photosystem {
                t.absorb_bias = 0.55;
            }
            if m == ModuleId::Nucleus {
                t.alloc_root = 0.05;
                t.alloc_stem = 0.2;
                t.alloc_leaf = 0.75;
            }
            t
        })
        .collect();
    assert!(orgs.spawn_blueprint_with_traits(
        world,
        hx,
        host_gy,
        host_body(),
        host_traits,
        40.0,
        None,
    ));
    if let Some(host) = orgs.atoms.iter_mut().rev().find(|a| is_land_plant(a)) {
        host.energy = 8.0;
        host.stem_wetness = 0.9;
        host.recompute_body_plan();
    }

    let epi_traits: Vec<PixelTraits> = epi_body()
        .iter()
        .map(|&(_, _, m)| {
            let mut t = PixelTraits::default();
            t.upkeep_bias = 0.35;
            if m == ModuleId::Photosystem {
                // absorb_bias==1.0 remaps to default 0.45 in leaf_absorb_effective.
                t.absorb_bias = 1.35;
                t.host_leave_fraction = leave;
            }
            if m == ModuleId::Holdfast {
                t.attach_prefer = 0.0;
            }
            t
        })
        .collect();
    assert!(orgs.spawn_blueprint_with_traits(
        world,
        hx,
        host_gy + 3,
        epi_body(),
        epi_traits,
        40.0,
        None,
    ));
    if let Some(epi) = orgs.atoms.iter_mut().rev().find(|a| is_epiphyte(a)) {
        epi.energy = 30.0;
        epi.recompute_body_plan();
    }
}

#[test]
fn e43b_gentle_lineages_outlast_smotherers() {
    let width = 48;
    let mut world = World::new(9049);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    orgs.max_atoms = 16;
    orgs.growth_caps = PlantGrowthCaps {
        max_roots: 1,
        max_stems: 3,
        max_photos: 1,
    };

    let smother_xs = [6, 10, 14, 18];
    let gentle_xs = [28, 32, 36, 40];
    for &hx in &smother_xs {
        seat_pair(&mut orgs, &world, hx, 0.0);
    }
    for &hx in &gentle_xs {
        seat_pair(&mut orgs, &world, hx, 0.85);
    }
    assert_eq!(orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(), 8);
    assert_eq!(orgs.atoms.iter().filter(|a| is_land_plant(a)).count(), 8);

    for _ in 0..2000 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }

    let smother_epi = orgs
        .atoms
        .iter()
        .filter(|a| is_epiphyte(a) && a.body_plan.host_leave_fraction < 0.2)
        .count();
    let gentle_epi = orgs
        .atoms
        .iter()
        .filter(|a| is_epiphyte(a) && a.body_plan.host_leave_fraction > 0.5)
        .count();
    let smother_host = orgs
        .atoms
        .iter()
        .filter(|a| is_land_plant(a) && smother_xs.contains(&a.gx))
        .count();
    let gentle_host = orgs
        .atoms
        .iter()
        .filter(|a| is_land_plant(a) && gentle_xs.contains(&a.gx))
        .count();

    assert!(
        gentle_epi > smother_epi,
        "gentle lineages should outnumber smotherers (gentle={gentle_epi} smother={smother_epi}; hosts g={gentle_host} s={smother_host})"
    );
    assert!(
        gentle_host > smother_host,
        "gentle landlords should outlast smothered ones (g={gentle_host} s={smother_host})"
    );
    assert!(
        smother_epi < 4,
        "at least one smotherer lineage should have collapsed with its host"
    );
}
