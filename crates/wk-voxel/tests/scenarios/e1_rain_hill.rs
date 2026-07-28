//! E1 — rain / water on a hill drains downhill (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e1_rain_hill.rs` (column stack).
//! Product intent: water leaves the crest and accumulates at the base.

use wk_voxel::{apply_rain, tick, RainConfig};

use crate::helpers::{sat_sum, setup_hill_world};

#[test]
fn e1_rain_on_hill_drains_downhill() {
    // Peak at x=32, height 8 → crest surface y=9; bases near x=24 / x=40.
    let mut world = setup_hill_world(12345, 64, 32, 8);

    let rain = RainConfig {
        top_y: 20,
        x_range: (28, 36), // focus on the crest
        prob_per_col_per_tick: 1.0,
        droplet_sat: 128,
        seed_salt: 0xE1,
        closed_loop: false, // open faucet — scenario injection
        sea_level_y: 0,
        max_flood_above_sea: 0,
    };

    // Inject a few rain ticks so the crest is wet, then let physics drain.
    for _ in 0..8 {
        apply_rain(&mut world, &rain);
        tick(&mut world);
    }
    for _ in 0..40 {
        tick(&mut world);
    }

    // Crest band (columns under the peak, high rows) vs base flanks.
    let crest = sat_sum(&world, 30, 34, 6, 12);
    let base_left = sat_sum(&world, 22, 26, 1, 5);
    let base_right = sat_sum(&world, 38, 42, 1, 5);
    let base = base_left + base_right;

    assert!(
        base > crest,
        "water should leave the crest for the base (crest={crest} base={base})"
    );
    assert!(base > 200, "base should hold a meaningful pool (got {base})");
}
