//! End-to-end smoke test for the wk-voxel gravity rule.
//!
//! Drop a row of water cells one row above a flat sand bed and tick
//! the world. After one tick:
//!   - The sand row has absorbed water up to its porosity capacity.
//!   - The water row above holds the remainder.
//!   - Dirty rectangles clear after each tick.
//!
//! Once lateral spill lands (follow-up PR), we'll extend this to
//! puddle spread and evaporation.

use wk_material::MaterialId;
use wk_voxel::{tick, water_capacity, Cell, Sat, World};

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

    tick(&mut w);

    let sand_cap = water_capacity(MaterialId::Sand);
    for x in 0..64 {
        let sand = w.get_cell(x, 0).unwrap();
        let above = w.get_cell(x, 1).unwrap();
        assert_eq!(
            sand.sat.0, sand_cap,
            "sand should hold porosity worth of water (x={x})"
        );
        assert_eq!(
            above.sat.0,
            u8::MAX - sand_cap,
            "leftover water sits above sand (x={x})"
        );
        assert_eq!(sand.material, MaterialId::Sand);
        assert_eq!(above.material, MaterialId::Air);
    }

    // Dirty rectangles clear after the tick.
    let (coord, _, _) = World::split(4, 1);
    let chunk = w.chunks.get(&coord).unwrap();
    assert!(chunk.dirty.is_none());
    assert_eq!(w.tick, 1);

    // A second tick doesn't change anything — sand is at capacity,
    // the water above has nowhere left to go (bedrock check would
    // otherwise apply; sand already full).
    tick(&mut w);
    for x in 0..64 {
        assert_eq!(w.get_cell(x, 0).unwrap().sat.0, sand_cap);
        assert_eq!(w.get_cell(x, 1).unwrap().sat.0, u8::MAX - sand_cap);
    }
}

#[test]
fn lone_droplet_falls_over_many_ticks() {
    let mut w = World::new(4321);
    // Bedrock floor at y=0; a lone droplet at y=8; everything else Air.
    for x in 0..64 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let x = 32;
    w.set_cell(x, 8, Cell::water());

    // After each tick the droplet should be one row lower until it
    // rests on top of the bedrock at y=1.
    for expected_y in (1..=7).rev() {
        tick(&mut w);
        let expected = expected_y;
        assert!(
            w.get_cell(x, expected).unwrap().sat.is_full(),
            "droplet should be at y={expected} after {} ticks (tick={})",
            8 - expected_y,
            w.tick
        );
        // Old cell must have emptied.
        assert!(w.get_cell(x, expected + 1).unwrap().sat.is_empty());
    }

    // One more tick: bedrock is impermeable, droplet stays.
    tick(&mut w);
    assert!(w.get_cell(x, 1).unwrap().sat.is_full());
    // No leak into bedrock.
    assert!(w.get_cell(x, 0).unwrap().sat.is_empty());
}
