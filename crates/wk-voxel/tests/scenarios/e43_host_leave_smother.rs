//! E43 spirit — HostLeaveFraction gentle vs smotherer (Wave Y).
//!
//! Same tall host, same high LeafAbsorb epiphyte seat. Smotherer
//! (`host_leave = 0`) drains host energy harder than a gentle rider
//! (`host_leave = 0.85`) over a short soak.

use wk_material::MaterialId;
use wk_voxel::{
    is_epiphyte, BodyModule, Cell, ChunkCoord, Genome, ModuleId, OrganismStore, World,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 4;
        world.set_cell(x, 1, sand);
        for y in 2..=10 {
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
    ]
}

fn run_with_leave(leave: f32) -> f32 {
    let width = 32;
    let mut world = World::new(9048);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 14;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(
        &world,
        hx,
        host_gy,
        host_body(),
        100.0,
        Genome::default()
    ));
    orgs.atoms[0].energy = 40.0;

    let mut epi_g = Genome::default();
    epi_g.leaf_absorb = 0.95;
    epi_g.host_leave_fraction = leave;
    // Holdfast on upper Stem; epi leaf shares host crown height.
    assert!(orgs.spawn_blueprint(
        &world,
        hx,
        host_gy + 3,
        epi_body(),
        40.0,
        epi_g
    ));

    for _ in 0..80 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }

    orgs.atoms
        .iter()
        .find(|a| !is_epiphyte(a))
        .map(|a| a.energy)
        .unwrap_or(0.0)
}

#[test]
fn e43_gentle_leaves_more_host_energy_than_smotherer() {
    let smother_e = run_with_leave(0.0);
    let gentle_e = run_with_leave(0.85);
    assert!(
        gentle_e > smother_e + 1.0,
        "gentle rider should leave host healthier (gentle={gentle_e} smother={smother_e})"
    );
}
