//! E10 — open sinkhole captures surface water (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e10_sinkhole_captures_river.rs`.
//! Product intent: a surface-open pit swallows rim/pond water into the
//! cavity instead of leaving it entirely on top.

use wk_voxel::{tick, Cell};

use crate::helpers::{sat_sum, setup_sinkhole_world};

#[test]
fn e10_sinkhole_captures_surface_water() {
    let pit_x0 = 30;
    let pit_x1 = 34;
    let mut world = setup_sinkhole_world(9010, 64, pit_x0, pit_x1, 3);

    // Pond on the stone rim beside the mouth (both flanks).
    for x in 24..30 {
        world.set_cell(x, 2, Cell::water());
    }
    for x in 35..41 {
        world.set_cell(x, 2, Cell::water());
    }
    let rim_before = sat_sum(&world, 24, 29, 2, 2) + sat_sum(&world, 35, 40, 2, 2);
    let pit_before = sat_sum(&world, pit_x0, pit_x1, 1, 3);
    assert!(rim_before > 2_000, "expected a rim pond (got {rim_before})");
    assert_eq!(pit_before, 0, "pit starts dry");

    for _ in 0..40 {
        tick(&mut world);
    }

    let rim_after = sat_sum(&world, 24, 29, 2, 2) + sat_sum(&world, 35, 40, 2, 2);
    let pit_after = sat_sum(&world, pit_x0, pit_x1, 1, 3);

    assert!(
        pit_after > 500,
        "sinkhole should hold captured water (pit sat={pit_after})"
    );
    assert!(
        rim_after < rim_before,
        "rim pond should drain into the pit ({rim_before} → {rim_after})"
    );
}
