//! E45 — living Stem load drain / recharge + live topple (Wave X).
//!
//! Control: tall host with energy and no riders keeps its Stem stack.
//! Treatment: several epiphytes on the upper trunk overload the lowest
//! Stem; integrity collapses and a live topple shortens the host.

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

fn tall_host() -> Vec<BodyModule> {
    vec![
        (0, -1, ModuleId::Root),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Stem),
        (0, 2, ModuleId::Stem),
        (0, 3, ModuleId::Stem),
        (0, 4, ModuleId::Stem),
        (0, 5, ModuleId::Photosystem),
    ]
}

fn epi_body() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Holdfast),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
    ]
}

fn host_stem_count(orgs: &OrganismStore) -> usize {
    orgs.atoms
        .iter()
        .find(|a| !is_epiphyte(a))
        .map(|a| a.body.iter().filter(|(_, _, m)| *m == ModuleId::Stem).count())
        .unwrap_or(0)
}

fn step_n(orgs: &mut OrganismStore, world: &mut World, n: u32) {
    for _ in 0..n {
        let tick = world.tick;
        orgs.step(world, tick);
        world.tick = tick.wrapping_add(1);
    }
}

#[test]
fn e45_healthy_host_holds_under_self_load() {
    let width = 32;
    let mut world = World::new(9045);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(&world, hx, host_gy, tall_host(), 120.0, Genome::default()));
    orgs.atoms[0].energy = 120.0;
    let stems_before = host_stem_count(&orgs);
    assert!(stems_before >= 3);

    step_n(&mut orgs, &mut world, 50);

    assert!(
        orgs.atoms.iter().any(|a| !is_epiphyte(a)),
        "host should still be alive"
    );
    assert_eq!(
        host_stem_count(&orgs),
        stems_before,
        "recharge should beat self-load — no live topple"
    );
}

#[test]
fn e45_epiphyte_load_live_topples_host() {
    let width = 32;
    let mut world = World::new(9046);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(&world, hx, host_gy, tall_host(), 120.0, Genome::default()));
    // Upper stems: host_gy+3 and +4 — pile riders to overload the trunk.
    for dy in [3i32, 4, 3, 4] {
        assert!(
            orgs.spawn_blueprint(
                &world,
                hx,
                host_gy + dy,
                epi_body(),
                40.0,
                Genome::default()
            ),
            "epiphyte must seat on host Stem"
        );
    }
    assert_eq!(orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(), 4);
    let stems_before = host_stem_count(&orgs);
    assert!(stems_before >= 3);

    // Cap host energy so recharge cannot fully cancel rider load.
    if let Some(host) = orgs.atoms.iter_mut().find(|a| !is_epiphyte(a)) {
        host.energy = 8.0;
    }

    step_n(&mut orgs, &mut world, 50);

    let stems_after = host_stem_count(&orgs);
    assert!(
        stems_after < stems_before || orgs.atoms.iter().all(is_epiphyte),
        "epiphyte load should live-topple or kill the host (before={stems_before} after={stems_after})"
    );
}