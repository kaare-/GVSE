//! E6 — water crosses a chunk seam at x=64 (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e6_chunk_seam.rs` (column stack).
//! Product intent: flow is continuous across chunk boundaries.

use wk_voxel::{tick, Cell, CHUNK_CELLS_W};

use crate::helpers::{sat_sum, setup_seam_terrace};

#[test]
fn e6_flow_crosses_chunk_seam() {
    assert_eq!(CHUNK_CELLS_W, 64);
    let mut world = setup_seam_terrace(22222);

    // Pond on the left of the seam; right side starts dry.
    for x in 50..64 {
        world.set_cell(x, 2, Cell::water());
    }
    assert_eq!(sat_sum(&world, 64, 80, 2, 4), 0);

    for _ in 0..60 {
        tick(&mut world);
    }

    let right = sat_sum(&world, 64, 80, 1, 4);
    assert!(
        right > 0,
        "water should cross the chunk seam into x>=64 (got sat={right})"
    );
    // Left side should still have interacted (not vanish into a void).
    let left = sat_sum(&world, 50, 63, 1, 4);
    assert!(
        left > 0 || right > 200,
        "expected water activity around the seam (left={left} right={right})"
    );
}
