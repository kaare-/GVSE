//! Regression: plant seating used to call `collect_floating_organic_columns`
//! several times **per plant per tick**. On a demo-sized world that is a
//! full grid scan × plants × helpers — enough to tank FPS on startup.
//!
//! This test times the amplification pattern vs a shared once-per-tick map.

use std::time::Instant;

use wk_voxel::{
    collect_floating_organic_columns, stamp_world, World, WorldgenParams, CHUNK_CELLS_H,
    CHUNK_CELLS_W,
};

#[test]
fn shared_float_columns_beat_per_plant_rescans() {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let cells = params.width_cols as i64
        * (params.sky_ceiling_y - params.bedrock_floor_y) as i64;
    let plants = 48usize;
    let helpers_per_plant = 4usize; // holdfast / grounded / raft / tip

    let t0 = Instant::now();
    for _ in 0..plants * helpers_per_plant {
        let _ = collect_floating_organic_columns(&world);
    }
    let amplified = t0.elapsed();

    let t0 = Instant::now();
    let _ = collect_floating_organic_columns(&world);
    let shared = t0.elapsed();

    eprintln!(
        "float-columns: world ~{cells} cells ({}×{} chunks)  plants={plants} helpers={helpers_per_plant}",
        params.width_cols / CHUNK_CELLS_W as i32,
        (params.sky_ceiling_y - params.bedrock_floor_y + CHUNK_CELLS_H as i32 - 1)
            / CHUNK_CELLS_H as i32,
    );
    eprintln!(
        "  amplified ({}) {:>8.3} ms",
        plants * helpers_per_plant,
        amplified.as_secs_f64() * 1000.0
    );
    eprintln!(
        "  shared (1)           {:>8.3} ms   speedup {:.1}×",
        shared.as_secs_f64() * 1000.0,
        amplified.as_secs_f64() / shared.as_secs_f64().max(1e-9)
    );

    // Shared path must be dramatically cheaper than the old O(plants) rescans.
    // Allow some noise but require at least a 10× win (theory: 192×).
    assert!(
        amplified > shared * 10,
        "expected shared collect to beat per-plant rescans by ≥10× (amplified {:?}, shared {:?})",
        amplified,
        shared
    );
}
