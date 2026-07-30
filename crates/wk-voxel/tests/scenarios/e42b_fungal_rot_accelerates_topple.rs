//! E42b — fungal Digest on standing-dead Stem accelerates topple (Wave W).
//!
//! Control: abiotic Stem drain alone does not topple within ~80 ticks.
//! Treatment: living Digest sharing a Stem world cell adds fungal drain
//! so the trunk fails in the same window.

use wk_material::MaterialId;
use wk_voxel::{
    add_soft_litter, BodyModule, Cell, ChunkCoord, Genome, ModuleId, OrganismStore, World,
    DEMO_DAY_TICKS,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 4;
        world.set_cell(x, 1, sand);
        for y in 2..=8 {
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

/// Nucleus on the litter crown; Digest reaches the lowest corpse Stem (`dy=1`).
fn fungus_on_lowest_stem() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Digest),
    ]
}

fn kill_host(orgs: &mut OrganismStore, world: &mut World) {
    let host_i = orgs.atoms.iter().position(|_| true).expect("host");
    orgs.atoms[host_i].age_ticks = DEMO_DAY_TICKS * 16;
    let tick = world.tick;
    orgs.step(world, tick);
    world.tick = tick.wrapping_add(1);
}

fn stem_count(orgs: &OrganismStore) -> usize {
    orgs.corpses
        .first()
        .map(|c| c.body.iter().filter(|(_, _, m)| *m == ModuleId::Stem).count())
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
fn e42b_abiotic_only_no_topple_in_window() {
    let width = 32;
    let mut world = World::new(9043);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(&world, hx, host_gy, host_body(), 80.0, Genome::default()));
    kill_host(&mut orgs, &mut world);
    assert!(orgs.corpse_count() >= 1);
    let stems_before = stem_count(&orgs);
    assert!(stems_before >= 2);

    step_n(&mut orgs, &mut world, 80);

    assert_eq!(
        stem_count(&orgs),
        stems_before,
        "abiotic drain (~0.002/tick) should not topple within 80 ticks"
    );
}

#[test]
fn e42b_fungal_digest_topples_in_window() {
    let width = 32;
    let mut world = World::new(9044);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(&world, hx, host_gy, host_body(), 80.0, Genome::default()));
    kill_host(&mut orgs, &mut world);
    assert!(orgs.corpse_count() >= 1);
    let stems_before = stem_count(&orgs);
    assert!(stems_before >= 2);

    // Seat fungus on the crown; Digest at dy=1 overlaps lowest Stem cell.
    assert!(
        orgs.spawn_blueprint_free(
            &world,
            hx,
            host_gy,
            fungus_on_lowest_stem(),
            80.0,
            Genome::default(),
        )
        .is_ok(),
        "fungus should seat on moist litter crown"
    );
    add_soft_litter(&mut world, hx, 24);

    step_n(&mut orgs, &mut world, 80);

    assert!(
        stem_count(&orgs) < stems_before,
        "Digest on Stem cell should accelerate integrity collapse → topple (before={stems_before})"
    );
}
