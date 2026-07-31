//! E41 — epiphyte stem_wetness drink / drought trap (Wave Z).
//!
//! Control: moist host keeps a seated epiphyte alive.
//! Treatment: dry the pore bed → host stem_wetness decays → epiphyte
//! dies while the rooted host survives in dormancy.

use wk_material::MaterialId;
use wk_voxel::{
    is_epiphyte, BodyModule, Cell, ChunkCoord, Genome, ModuleId, OrganismStore, World,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 24;
        world.set_cell(x, 1, sand);
        for y in 2..=8 {
            world.set_cell(x, y, Cell::air());
        }
    }
}

fn dry_floor_pores(world: &mut World, width: i32) {
    for x in 0..width {
        if let Some(mut c) = world.get_cell(x, 1) {
            if c.material != MaterialId::Air {
                c.sat.0 = 0;
                world.set_cell(x, 1, c);
            }
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
    ]
}

fn step_n(orgs: &mut OrganismStore, world: &mut World, n: u32) {
    for _ in 0..n {
        let tick = world.tick;
        orgs.step(world, tick);
        world.tick = tick.wrapping_add(1);
    }
}

fn seed_pair(world: &mut World) -> OrganismStore {
    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(
        world,
        hx,
        host_gy,
        host_body(),
        100.0,
        Genome::default()
    ));
    assert!(orgs.spawn_blueprint(
        world,
        hx,
        host_gy + 3,
        epi_body(),
        40.0,
        Genome::default()
    ));
    assert_eq!(orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(), 1);
    orgs
}

#[test]
fn e41_moist_host_keeps_epiphyte() {
    let width = 32;
    let mut world = World::new(9141);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);
    let mut orgs = seed_pair(&mut world);

    step_n(&mut orgs, &mut world, 40);

    assert!(
        orgs.atoms.iter().any(|a| !is_epiphyte(a)),
        "host should survive"
    );
    assert_eq!(
        orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(),
        1,
        "seated epiphyte should drink stem_wetness and live"
    );
}

#[test]
fn e41_dry_stem_kills_epiphyte_host_lives() {
    let width = 32;
    let mut world = World::new(9142);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);
    let mut orgs = seed_pair(&mut world);

    // Establish wet stems, then drought the bed.
    step_n(&mut orgs, &mut world, 10);
    dry_floor_pores(&mut world, width);
    step_n(&mut orgs, &mut world, 40);

    assert!(
        orgs.atoms.iter().any(|a| !is_epiphyte(a)),
        "rooted host should survive drought dormancy"
    );
    assert_eq!(
        orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(),
        0,
        "epiphyte should die as stem_wetness decays"
    );
}

