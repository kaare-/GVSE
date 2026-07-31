//! E42d — full standing-dead cascade with fallen-log linger (Wave AE).
//!
//! Smothering epiphyte → host death → Digest/Hypha on the stump → fungal
//! rot topples the trunk into a horizontal grey log → rider dies. Organic
//! on the band waits for log settle (Bezier fall stays out of Core).

use wk_material::MaterialId;
use wk_voxel::{
    add_soft_litter, is_epiphyte, is_fallen_log, is_fungus, is_land_plant, BodyModule, Cell,
    ChunkCoord, Genome, ModuleId, OrganismStore, PixelTraits, World, DEMO_DAY_TICKS,
    INTEGRITY_TOPPLE_THRESHOLD,
};

use crate::helpers::lay_bedrock_floor;

fn moist_floor(world: &mut World, width: i32) {
    lay_bedrock_floor(world, width);
    for x in 0..width {
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat.0 = 24;
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

fn smother_epi_body() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Holdfast),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
        (0, 2, ModuleId::Photosystem),
    ]
}

fn digest_on_lowest_stem() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Digest),
    ]
}

#[test]
fn e42d_smother_death_hypha_topple_fallen_log() {
    let width = 32;
    let mut world = World::new(9250);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 14;
    let host_gy = 2;

    // Host + smothering rider (host_leave = 0).
    assert!(orgs.spawn_blueprint(
        &world,
        hx,
        host_gy,
        host_body(),
        40.0,
        Genome::default()
    ));
    if let Some(host) = orgs.atoms.iter_mut().find(|a| is_land_plant(a)) {
        host.energy = 6.0;
        host.stem_wetness = 0.9;
    }
    let epi_traits: Vec<PixelTraits> = smother_epi_body()
        .iter()
        .map(|&(_, _, m)| {
            let mut t = PixelTraits::default();
            if m == ModuleId::Photosystem {
                t.absorb_bias = 1.35;
                t.host_leave_fraction = 0.0;
            }
            t
        })
        .collect();
    assert!(orgs.spawn_blueprint_with_traits(
        &world,
        hx,
        host_gy + 3,
        smother_epi_body(),
        epi_traits,
        40.0,
        None,
    ));
    assert_eq!(orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(), 1);

    // Force host senescence so the cascade reaches standing-dead quickly;
    // smother stress is the product reason the landlord dies in the wild.
    let host_i = orgs
        .atoms
        .iter()
        .position(|a| is_land_plant(a))
        .expect("host");
    orgs.atoms[host_i].age_ticks = DEMO_DAY_TICKS * 16;
    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert!(
        orgs.corpse_count() >= 1,
        "smothered / senesced host leaves standing-dead"
    );
    assert!(
        orgs.atoms.iter().any(is_epiphyte),
        "epiphyte still seated on grey trunk"
    );

    // Seat Digest on the lowest Stem; soft litter keeps the fungus active.
    assert!(
        orgs.spawn_blueprint_free(
            &world,
            hx,
            host_gy,
            digest_on_lowest_stem(),
            80.0,
            Genome::default(),
        )
        .is_ok()
    );
    add_soft_litter(&mut world, hx, 32);
    assert!(orgs.atoms.iter().any(is_fungus));

    // Accelerate the fail Stem so the window stays headless-friendly;
    // fungal rot is what would drive this in an unforced soak (e42b/c).
    let standing = orgs
        .corpses
        .iter_mut()
        .find(|c| !is_fallen_log(c))
        .expect("standing-dead");
    while standing.body_integrity.len() < standing.body.len() {
        standing.body_integrity.push(1.0);
    }
    if let Some(fail) = standing
        .body
        .iter()
        .enumerate()
        .filter(|(_, (_, _, m))| *m == ModuleId::Stem)
        .min_by_key(|(_, (_, dy, _))| *dy)
        .map(|(i, _)| i)
    {
        standing.body_integrity[fail] = INTEGRITY_TOPPLE_THRESHOLD;
    }

    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }

    assert!(
        orgs.corpses.iter().any(is_fallen_log),
        "topple should re-project the trunk into a horizontal fallen-log Corpse"
    );

    // Unseated epi dies within the Holdfast clock.
    for _ in 0..8 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert_eq!(
        orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(),
        0,
        "epiphyte dies after the host Stem topples into a fallen log"
    );
}
