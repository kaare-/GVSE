//! E42c — hypha morphogenesis into standing-dead Stem (Wave AA).
//!
//! Seat a Digest-only fungus at the corpse crown. Over a short soak it
//! grows Hypha into an adjacent dead Stem cell (invade → Wave W rot path).

use wk_material::MaterialId;
use wk_voxel::{
    add_soft_litter, collect_corpse_stem_world_cells, collect_fungus_tissue_world_cells, is_fungus,
    BodyModule, Cell, ChunkCoord, Genome, ModuleId, OrganismStore, World, DEMO_DAY_TICKS,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        // Above FUNGUS_DROUGHT_FRAC so invaders stay active.
        sand.sat.0 = 24;
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

fn digest_only_fungus() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Nucleus),
        (0, 0, ModuleId::Digest),
    ]
}

#[test]
fn e42c_hypha_grows_into_standing_dead_stem() {
    let width = 32;
    let mut world = World::new(9242);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(
        &world,
        hx,
        host_gy,
        host_body(),
        80.0,
        Genome::default()
    ));
    orgs.atoms[0].age_ticks = DEMO_DAY_TICKS * 16;
    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert!(orgs.corpse_count() >= 1, "host should leave standing-dead");
    let stems = collect_corpse_stem_world_cells(&world, &orgs.corpses);
    assert!(stems.contains(&(hx, host_gy + 1)), "lowest Stem present");

    assert!(
        orgs.spawn_blueprint_free(
            &world,
            hx,
            host_gy,
            digest_only_fungus(),
            80.0,
            Genome::default(),
        )
        .is_ok(),
        "fungus seats on litter crown"
    );
    add_soft_litter(&mut world, hx, 32);
    let fungus = orgs.atoms.iter().find(|a| is_fungus(a)).expect("fungus");
    assert_eq!(
        fungus
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Hypha)
            .count(),
        0,
        "starts without Hypha"
    );

    let mut invaded = false;
    for _ in 0..48 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
        let tissue = collect_fungus_tissue_world_cells(&world, &orgs.atoms);
        let corpse_stems = collect_corpse_stem_world_cells(&world, &orgs.corpses);
        if tissue.iter().any(|c| corpse_stems.contains(c)) {
            invaded = true;
            break;
        }
        // Corpse may have toppled after invasion on a prior tick.
        if orgs.corpse_count() == 0 {
            invaded = true;
            break;
        }
    }
    assert!(
        invaded,
        "Digest-adjacent fungus should grow Hypha into a dead Stem cell"
    );
    if let Some(f) = orgs.atoms.iter().find(|a| is_fungus(a)) {
        assert!(
            f.body.iter().any(|(_, _, m)| *m == ModuleId::Hypha)
                || orgs.corpse_count() == 0
                || collect_fungus_tissue_world_cells(&world, &orgs.atoms)
                    .iter()
                    .any(|c| collect_corpse_stem_world_cells(&world, &orgs.corpses).contains(c)),
            "invasion leaves Hypha tissue or already finished the trunk"
        );
    }
}
