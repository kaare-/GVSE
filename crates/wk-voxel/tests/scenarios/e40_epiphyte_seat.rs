//! E40 — epiphyte seats on host Stem (Wave U).
//!
//! Product intent: pink Holdfast must share a world cell with a living
//! host Stem; lose the host and the freeloader dies within a few ticks.

use wk_material::MaterialId;
use wk_voxel::{
    collect_live_stem_world_cells, is_epiphyte, is_holdfast_anchored, BodyModule, Cell, ChunkCoord,
    Genome, ModuleId, OrganismStore, World,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 4;
        world.set_cell(x, 1, sand);
        for y in 2..=6 {
            world.set_cell(x, y, Cell::air());
        }
    }
}

fn host_body() -> Vec<BodyModule> {
    // Nucleus at crown (y=2 Air above sand); stem up through +1/+2; leaf at +3.
    vec![
        (0, -1, ModuleId::Root),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Stem),
        (0, 2, ModuleId::Stem),
        (0, 3, ModuleId::Photosystem),
    ]
}

fn epi_body() -> Vec<BodyModule> {
    // Holdfast + nucleus share (0,0); leaf above — seat nucleus on host upper Stem.
    vec![
        (0, 0, ModuleId::Holdfast),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
    ]
}

#[test]
fn e40_epiphyte_seats_on_host_and_dies_without() {
    let width = 32;
    let mut world = World::new(9040);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 10;
    let host_gy = 2; // Air above moist sand
    assert!(
        orgs.spawn_blueprint(
            &world,
            hx,
            host_gy,
            host_body(),
            80.0,
            Genome::default(),
        ),
        "host plant must spawn"
    );
    assert_eq!(orgs.atoms.len(), 1);

    // Upper stem world cell: host at (hx, host_gy) + (0,2) → (hx, 4).
    let stem_y = host_gy + 2;
    assert!(
        orgs.spawn_blueprint(&world, hx, stem_y, epi_body(), 40.0, Genome::default()),
        "epiphyte must seat on host Stem"
    );
    assert_eq!(orgs.atoms.len(), 2);
    let epi = orgs.atoms.iter().find(|a| is_epiphyte(a)).expect("epi");
    let stems = collect_live_stem_world_cells(&world, &orgs.atoms);
    assert!(
        is_holdfast_anchored(epi, &stems, |x| world.wrap_x(x)),
        "Holdfast must share a Stem cell"
    );

    for _ in 0..20 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert_eq!(
        orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(),
        1,
        "seated epiphyte should still be alive"
    );

    // Strip host stems → epiphyte loses purchase.
    let host = orgs
        .atoms
        .iter_mut()
        .find(|a| !is_epiphyte(a))
        .expect("host");
    host.body.retain(|(_, _, m)| *m != ModuleId::Stem);
    host.body_traits.clear();
    host.recompute_body_plan();

    for _ in 0..12 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert_eq!(
        orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(),
        0,
        "unseated epiphyte should die within a few ticks"
    );
}

#[test]
fn e40_epiphyte_refuses_air_without_host() {
    let width = 32;
    let mut world = World::new(9041);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    assert!(
        !orgs.spawn_blueprint(&world, 8, 4, epi_body(), 40.0, Genome::default()),
        "habitat spawn must refuse an unseated epiphyte"
    );
    assert!(orgs.atoms.is_empty());
}
