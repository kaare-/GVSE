//! E42 spirit — standing-dead Stem integrity collapses → topple (Wave V / AE).
//!
//! Kill a host plant (grey corpse with Stems), force Stem integrity to the
//! topple threshold, step once: trunk modules above the break leave the
//! standing corpse as a horizontal grey fallen log (Wave AE), soft litter
//! banks at the stump, and a seated epiphyte loses its host. Organic on
//! the ground band arrives when the log settles — not on the topple tick.

use wk_material::MaterialId;
use wk_voxel::{
    is_epiphyte, is_fallen_log, soft_litter_at, BodyModule, Cell, ChunkCoord, Genome, ModuleId,
    OrganismStore, World, CORPSE_SETTLE_LAND_TICKS, DEMO_DAY_TICKS, INTEGRITY_TOPPLE_THRESHOLD,
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

fn epi_body() -> Vec<BodyModule> {
    vec![
        (0, 0, ModuleId::Holdfast),
        (0, 0, ModuleId::Nucleus),
        (0, 1, ModuleId::Photosystem),
    ]
}

fn count_organic_band(world: &World, x0: i32, x1: i32, y0: i32, y1: i32) -> usize {
    let mut n = 0usize;
    for x in x0..=x1 {
        for y in y0..=y1 {
            if world.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic) {
                n += 1;
            }
        }
    }
    n
}

#[test]
fn e42_standing_dead_stem_topples_organic() {
    let width = 32;
    let mut world = World::new(9042);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    moist_floor(&mut world, width);

    let mut orgs = OrganismStore::new();
    let hx = 12;
    let host_gy = 2;
    assert!(orgs.spawn_blueprint(&world, hx, host_gy, host_body(), 80.0, Genome::default()));
    // Seat epi on upper stem (host_gy + 3).
    assert!(orgs.spawn_blueprint(
        &world,
        hx,
        host_gy + 3,
        epi_body(),
        40.0,
        Genome::default()
    ));
    assert_eq!(orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(), 1);

    // Kill host by life-cap → land corpse with Stem stack (leaves/roots stripped).
    let host_i = orgs
        .atoms
        .iter()
        .position(|a| !is_epiphyte(a))
        .expect("host");
    orgs.atoms[host_i].age_ticks = DEMO_DAY_TICKS * 16;
    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert!(
        orgs.corpse_count() >= 1,
        "host should leave a standing-dead corpse"
    );
    assert!(
        orgs.corpses[0]
            .body
            .iter()
            .any(|(_, _, m)| *m == ModuleId::Stem),
        "corpse keeps stems"
    );

    // Force lowest Stem to the topple threshold.
    let corpse = &mut orgs.corpses[0];
    while corpse.body_integrity.len() < corpse.body.len() {
        corpse.body_integrity.push(1.0);
    }
    let fail = corpse
        .body
        .iter()
        .enumerate()
        .filter(|(_, (_, _, m))| *m == ModuleId::Stem)
        .min_by_key(|(_, (_, dy, _))| *dy)
        .map(|(i, _)| i)
        .expect("stem");
    corpse.body_integrity[fail] = INTEGRITY_TOPPLE_THRESHOLD;
    let stems_before = corpse
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Stem)
        .count();
    assert!(stems_before >= 2);

    let organic_before = count_organic_band(&world, hx - 6, hx + 6, 1, 4);
    let litter_before = soft_litter_at(&world, hx);
    let tick = world.tick;
    orgs.step(&mut world, tick);
    world.tick = tick.wrapping_add(1);

    let stems_after = orgs
        .corpses
        .iter()
        .filter(|c| !is_fallen_log(c))
        .map(|c| {
            c.body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Stem)
                .count()
        })
        .sum::<usize>();
    assert!(
        stems_after < stems_before,
        "topple should remove the failing Stem and modules above from the standing corpse"
    );
    let log = orgs
        .corpses
        .iter()
        .find(|c| is_fallen_log(c))
        .expect("Wave AE: topple should leave a horizontal fallen-log Corpse");
    let log_stems = log
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Stem)
        .count();
    assert!(log_stems >= 2, "fallen log keeps the toppled Stems");
    let dys: Vec<i16> = log.body.iter().map(|(_, dy, _)| *dy).collect();
    assert!(
        dys.iter().all(|d| *d == dys[0]),
        "fallen log is a single horizontal row"
    );
    let dx_span = log
        .body
        .iter()
        .map(|(dx, _, _)| *dx)
        .max()
        .unwrap()
        - log
            .body
            .iter()
            .map(|(dx, _, _)| *dx)
            .min()
            .unwrap();
    assert!(dx_span >= 1, "fallen log spreads along x");

    let organic_after = count_organic_band(&world, hx - 6, hx + 6, 1, 4);
    assert_eq!(
        organic_after, organic_before,
        "Organic waits for fallen-log settle (before={organic_before} after={organic_after})"
    );
    assert!(
        soft_litter_at(&world, hx) > litter_before,
        "topple still banks soft litter at the stump"
    );

    // Force settle → Organic band on the ground.
    let log_i = orgs
        .corpses
        .iter()
        .position(|c| is_fallen_log(c))
        .expect("log");
    orgs.corpses[log_i].settled_ticks = CORPSE_SETTLE_LAND_TICKS;
    {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    let organic_settled = count_organic_band(&world, hx - 6, hx + 6, 1, 4);
    assert!(
        organic_settled > organic_before,
        "settled fallen log deposits Organic on the ground band (before={organic_before} after={organic_settled})"
    );
    assert!(
        !orgs.corpses.iter().any(is_fallen_log),
        "fallen log dissolves after settle"
    );

    // Epiphyte was force-unseated; dies on the next tick(s).
    for _ in 0..4 {
        let tick = world.tick;
        orgs.step(&mut world, tick);
        world.tick = tick.wrapping_add(1);
    }
    assert_eq!(
        orgs.atoms.iter().filter(|a| is_epiphyte(a)).count(),
        0,
        "epiphyte should die after host Stem topples"
    );
}
