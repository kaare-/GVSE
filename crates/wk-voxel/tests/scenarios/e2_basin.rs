//! E2 — closed basin retains a pool (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e2_basin.rs` (column stack).
//! Product intent: a bowl holds water instead of draining to empty.

use wk_voxel::{tick, Cell};

use crate::helpers::{sat_sum, setup_basin_world};

#[test]
fn e2_basin_holds_a_pool() {
    // Interior columns x=11..20, walls at 10 and 21, wall height 8.
    let mut world = setup_basin_world(54321, 10, 21, 8);

    // Fill the bowl with standing water (rows 2..=5).
    for x in 11..=20 {
        for y in 2..=5 {
            world.set_cell(x, y, Cell::water());
        }
    }
    let before = sat_sum(&world, 11, 20, 2, 6);
    assert!(before > 5_000);

    for _ in 0..80 {
        tick(&mut world);
    }

    let inside = sat_sum(&world, 11, 20, 2, 7);
    let outside = sat_sum(&world, 0, 9, 1, 8) + sat_sum(&world, 22, 30, 1, 8);

    assert!(
        inside > before / 2,
        "basin should retain most of its water (before={before} inside={inside})"
    );
    assert!(
        outside < 64,
        "walls should keep the pool from escaping (outside={outside})"
    );
}
