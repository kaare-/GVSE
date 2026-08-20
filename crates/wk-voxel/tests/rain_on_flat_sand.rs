//! End-to-end smoke test for the wk-voxel gravity rule.
//!
//! Drop a row of water cells one row above a flat sand bed and tick
//! the world. An open sheet soaks through seepage (not instant gravity
//! pore-fill). After enough ticks:
//!   - The sand row has absorbed water up to its porosity capacity.
//!   - The water row above holds the remainder.
//!   - A settled bed leaves no dirty plan.
//!
//! Surface flow / seepage behaviour is covered in `rules.rs` unit
//! tests and documented in `docs/VOXEL_WATER.md`.

use wk_material::MaterialId;
use wk_voxel::{apply_gravity_fall, is_grain, plan_active, tick, water_capacity, Cell, World};

#[test]
fn rain_row_saturates_sand_over_one_tick() {
    let mut w = World::new(1234);

    // Flat sand bed across a full chunk width; water row directly on top.
    for x in 0..64 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Sand));
        w.set_cell(x, 1, Cell::water());
    }
    assert!(w.get_cell(4, 1).unwrap().sat.is_full());
    assert!(w.get_cell(4, 0).unwrap().sat.is_empty());
    // Setup writes must wake physics for the first tick.
    let (coord, _, _) = World::split(4, 1);
    assert!(
        w.chunks.get(&coord).unwrap().dirty.is_some(),
        "set_cell should dirty the chunk before tick"
    );

    // Open sheet: seepage soaks the bed (gravity no longer dumps a
    // wet-Air row into pores). Sand cap 180 / rate ~20 → ~9 ticks.
    for _ in 0..16 {
        tick(&mut w);
    }

    let sand_cap = water_capacity(MaterialId::Sand);
    for x in 8..56 {
        let sand = w.get_cell(x, 0).unwrap();
        let above = w.get_cell(x, 1).unwrap();
        assert_eq!(
            sand.sat.0, sand_cap,
            "sand should hold porosity worth of water (x={x})"
        );
        assert!(
            above.sat.0 > 0,
            "leftover water sits above sand (x={x} sat={})",
            above.sat.0
        );
        assert_eq!(sand.material, MaterialId::Sand);
        assert_eq!(above.material, MaterialId::Air);
    }
}

#[test]
fn lone_droplet_falls_over_many_gravity_passes() {
    // Gravity-only, so we can assert exact per-pass positions of a
    // single droplet without lateral spill smearing it out.
    let mut w = World::new(4321);
    for x in 0..64 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let x = 32;
    w.set_cell(x, 8, Cell::water());

    for expected_y in (1..=7).rev() {
        apply_gravity_fall(&mut w);
        assert!(
            w.get_cell(x, expected_y).unwrap().sat.is_full(),
            "droplet should be at y={expected_y} after {} passes",
            8 - expected_y,
        );
        assert!(w.get_cell(x, expected_y + 1).unwrap().sat.is_empty());
    }

    // One more pass: bedrock is impermeable, droplet stays.
    apply_gravity_fall(&mut w);
    assert!(w.get_cell(x, 1).unwrap().sat.is_full());
    assert!(w.get_cell(x, 0).unwrap().sat.is_empty());
}

#[test]
fn sand_settles_below_water_in_a_bucket() {
    // Bucket: bedrock floor at y=0, stone walls at x=10 and x=25 for
    // rows y=1..=15. Fill the column x=11..=24 with water rows 1..8
    // and drop a row of sand grains at y=10 above the water surface.
    //
    // Sand should sink through the water, water rises above it, and
    // the final resting state has sand at the bottom of the bucket
    // and water floating on top. Total sat is conserved.
    let mut w = World::new(2025);
    // Floor.
    for x in 0..64 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Walls.
    for y in 1..=15 {
        w.set_cell(10, y, Cell::solid(MaterialId::Stone));
        w.set_cell(25, y, Cell::solid(MaterialId::Stone));
    }
    // Water inside the bucket, rows 1..=8.
    for x in 11..=24 {
        for y in 1..=8 {
            w.set_cell(x, y, Cell::water());
        }
    }
    // Sand grains at row 10, above the water surface.
    for x in 11..=24 {
        w.set_cell(x, 10, Cell::solid(MaterialId::Sand));
    }
    // Baseline totals.
    let start_water: i32 = (11..=24)
        .flat_map(|x: i32| (0..=15).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    let start_grains: i32 = (11..=24)
        .flat_map(|x: i32| (0..=15).map(move |y| (x, y)))
        .filter(|(x, y)| {
            w.get_cell(*x, *y)
                .map(|c| is_grain(c.material))
                .unwrap_or(false)
        })
        .count() as i32;
    assert!(start_water > 0);
    assert_eq!(start_grains, 14);

    // Enough ticks for the whole scene to settle. Sand needs ~10
    // ticks to fall + a few more for water to redistribute.
    for _ in 0..80 {
        tick(&mut w);
    }

    // Mass conservation.
    let end_water: i32 = (0..64)
        .flat_map(|x: i32| (0..=15).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    let end_grains: i32 = (0..64)
        .flat_map(|x: i32| (0..=15).map(move |y| (x, y)))
        .filter(|(x, y)| {
            w.get_cell(*x, *y)
                .map(|c| is_grain(c.material))
                .unwrap_or(false)
        })
        .count() as i32;
    assert_eq!(end_water, start_water, "water sat conserved");
    assert_eq!(end_grains, start_grains, "grain count conserved");

    // Sand should be at the bottom of the bucket now.
    let bottom_row_grains: i32 = (11..=24)
        .filter(|&x| {
            w.get_cell(x, 1)
                .map(|c| c.material == MaterialId::Sand)
                .unwrap_or(false)
        })
        .count() as i32;
    assert!(
        bottom_row_grains >= 10,
        "expected ~14 sand grains at the bottom row after settling, got {bottom_row_grains}"
    );
}

#[test]
fn droplet_falls_then_spreads_over_many_ticks() {
    // Full tick pass drives both gravity + lateral spill. A droplet
    // dropped over a flat sand bed should end up as a spread-out
    // shallow puddle rather than a single tall column.
    let mut w = World::new(9999);
    for x in 0..64 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(32, 10, Cell::water());
    let start_mass: i32 = 255;

    // Enough ticks for the droplet to land (10 fall steps) plus
    // several ticks to spread laterally.
    for _ in 0..30 {
        tick(&mut w);
    }

    // Mass conservation across the whole chunk.
    let mass: i32 = (0..64i32)
        .flat_map(|x| (0..64i32).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    assert_eq!(
        mass, start_mass,
        "total sat conserved across gravity + spill"
    );

    // The droplet must have both fallen and spread — assert a
    // widened wet footprint on the bedrock-adjacent row.
    let wet_cols_at_bottom: i32 = (0..64i32)
        .filter(|&x| w.get_cell(x, 1).map(|c| c.sat.0 > 0).unwrap_or(false))
        .count() as i32;
    assert!(
        wet_cols_at_bottom >= 3,
        "expected the puddle to spread across several cells, got {wet_cols_at_bottom}"
    );
}
