//! Microbench: `World::get_cell` vs a chunk-local read.
//!
//! Hot inner loops (water flow, wakes, plant seating) currently pay:
//!   - wrap_x + HashMap lookup + local index for every neighbour probe.
//! A chunk-local read avoids the hash lookup entirely.
//!
//! ```text
//! cargo test -p wk-voxel --test get_cell_microbench --release -- --ignored --nocapture
//! ```

use std::hint::black_box;
use std::time::Instant;

use wk_voxel::{stamp_world, World, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W};

#[test]
#[ignore]
fn get_cell_vs_chunk_local() {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let cells = params.width_cols as i64
        * (params.sky_ceiling_y - params.bedrock_floor_y) as i64;

    // Warm caches.
    for _ in 0..3 {
        let mut acc: u64 = 0;
        for gy in params.bedrock_floor_y..params.sky_ceiling_y {
            for gx in 0..params.width_cols {
                if let Some(c) = world.get_cell(gx, gy) {
                    acc = acc.wrapping_add(c.sat.0 as u64);
                }
            }
        }
        black_box(acc);
    }

    let t0 = Instant::now();
    let mut acc: u64 = 0;
    for gy in params.bedrock_floor_y..params.sky_ceiling_y {
        for gx in 0..params.width_cols {
            if let Some(c) = world.get_cell(gx, gy) {
                acc = acc.wrapping_add(c.sat.0 as u64);
            }
        }
    }
    let get_cell_t = t0.elapsed();
    black_box(acc);

    let t0 = Instant::now();
    let mut acc2: u64 = 0;
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let c = chunk.get(lx, ly);
                acc2 = acc2.wrapping_add(c.sat.0 as u64);
            }
        }
    }
    let chunk_t = t0.elapsed();
    black_box(acc2);

    eprintln!(
        "get_cell path:      {:>7.3} ms  ({} cells, ~{:.1} ns/cell)",
        get_cell_t.as_secs_f64() * 1000.0,
        cells,
        get_cell_t.as_secs_f64() * 1e9 / cells as f64
    );
    eprintln!(
        "chunk-local path:   {:>7.3} ms  ({} cells, ~{:.1} ns/cell)   speedup {:.1}×",
        chunk_t.as_secs_f64() * 1000.0,
        cells,
        chunk_t.as_secs_f64() * 1e9 / cells as f64,
        get_cell_t.as_secs_f64() / chunk_t.as_secs_f64().max(1e-9)
    );
}
