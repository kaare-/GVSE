//! E46 spirit — canopy gap flash after stem topple (Wave AF / PLANTS.md).
//!
//! A short understory plant next to a tall neighbour gets more light once
//! the tall trunk topples (standing-dead shade lifts + gap-flash bonus)
//! than while the grey standing-dead stem still casts.

use wk_material::MaterialId;
use wk_voxel::{
    build_canopy_index_full, canopy_top_y, effective_photo_light, is_fallen_log, is_land_plant,
    BodyModule, Cell, ChunkCoord, Genome, ModuleId, OrganismStore, World, DEMO_DAY_TICKS,
    INTEGRITY_TOPPLE_THRESHOLD,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 8;
        world.set_cell(x, 1, sand);
        for y in 2..=12 {
            world.set_cell(x, y, Cell::air());
        }
    }
}

fn short_body() -> Vec<BodyModule> {
    vec![
        (0, -1, ModuleId::Root),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Stem),
        (0, 2, ModuleId::Photosystem),
    ]
}

fn tall_body() -> Vec<BodyModule> {
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

#[test]
fn e46_topple_opens_light_for_understory() {
    let width = 32;
    let mut world = World::new(9260);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let short_x = 10;
    let tall_x = 11;
    let gy = 2;

    let mut short_g = Genome::default();
    short_g.shade_efficiency = 0.85;
    short_g.leaf_absorb = 0.35;
    let mut tall_g = Genome::default();
    tall_g.leaf_absorb = 0.95;
    tall_g.shade_efficiency = 0.05;

    assert!(orgs.spawn_blueprint(&world, short_x, gy, short_body(), 40.0, short_g));
    assert!(orgs.spawn_blueprint(&world, tall_x, gy, tall_body(), 80.0, tall_g));

    // Kill tall → standing-dead still shades the short neighbour.
    let tall_i = orgs
        .atoms
        .iter()
        .position(|a| is_land_plant(a) && a.gx == tall_x)
        .expect("tall");
    orgs.atoms[tall_i].age_ticks = DEMO_DAY_TICKS * 16;
    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert!(
        orgs.corpses.iter().any(|c| c.gx == tall_x && !is_fallen_log(c)),
        "tall host should leave standing-dead"
    );

    let short = orgs
        .atoms
        .iter()
        .find(|a| is_land_plant(a) && a.gx == short_x)
        .expect("short")
        .clone();
    let sample_y = canopy_top_y(&short);
    let canopy_dead = build_canopy_index_full(&orgs.atoms, &orgs.corpses);
    let lit_standing = effective_photo_light(
        &canopy_dead,
        short_x,
        sample_y,
        1.0,
        0,
        short.photosystem_count(),
        &Genome::default(),
        1.0,
    );

    // Force topple of the standing-dead trunk.
    let corpse = orgs
        .corpses
        .iter_mut()
        .find(|c| c.gx == tall_x && !is_fallen_log(c))
        .expect("standing-dead");
    while corpse.body_integrity.len() < corpse.body.len() {
        corpse.body_integrity.push(1.0);
    }
    if let Some(fail) = corpse
        .body
        .iter()
        .enumerate()
        .filter(|(_, (_, _, m))| *m == ModuleId::Stem)
        .min_by_key(|(_, (_, dy, _))| *dy)
        .map(|(i, _)| i)
    {
        corpse.body_integrity[fail] = INTEGRITY_TOPPLE_THRESHOLD;
    }
    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert!(
        orgs.corpses.iter().any(is_fallen_log),
        "topple should leave a fallen log"
    );
    assert!(
        !orgs.gap_flash.is_empty(),
        "topple should register a canopy-gap flash"
    );

    let short = orgs
        .atoms
        .iter()
        .find(|a| is_land_plant(a) && a.gx == short_x)
        .expect("short still alive")
        .clone();
    let sample_y = canopy_top_y(&short);
    let canopy_gap = build_canopy_index_full(&orgs.atoms, &orgs.corpses);
    let mut lit_gap = effective_photo_light(
        &canopy_gap,
        short_x,
        sample_y,
        1.0,
        0,
        short.photosystem_count(),
        &Genome::default(),
        1.0,
    );
    lit_gap = (lit_gap * wk_voxel::gap_flash_transmit(&orgs.gap_flash, short_x)).clamp(0.0, 1.0);

    assert!(
        lit_gap > lit_standing + 0.05,
        "gap flash + cleared standing-dead should open light (standing={lit_standing} gap={lit_gap})"
    );
}
