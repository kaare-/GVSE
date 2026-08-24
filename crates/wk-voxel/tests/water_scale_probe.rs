//! Diagnostic: how fast does a *large* water mound level and drain?
//!
//! Playtest report: "water still flows really painfully slow" on a
//! mountain-scale world at ~5 FPS. Toy-plot tests (a 4×4 blob) pass while
//! a mound hundreds of cells wide crawls, so measure at that scale.
//!
//! ```text
//! cargo test -p wk-voxel --test water_scale_probe --release -- --ignored --nocapture
//! ```

use std::time::Instant;

use wk_material::MaterialId;
use wk_voxel::{tick_with_perf, Cell, ChunkCoord, PerfConfig, Sat, World};

/// Mountain of Stone with a wide, deep water mound sitting on the peak.
fn mountain_with_mound(width: i32, mound_h: i32) -> World {
    let mut w = World::new(9);
    for cx in 0..=(width / 64) {
        for cy in 0..3 {
            w.ensure_chunk(ChunkCoord::new(cx, cy));
        }
    }
    let peak = width / 2;
    for x in 0..width {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        // Symmetric ridge, ~1:3 slope.
        let h = (40 - (x - peak).abs() / 3).max(2);
        for y in 1..=h {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
        // Water mound following the peak, only near the top.
        let top = 40 - (x - peak).abs() / 3;
        if top > 30 {
            for y in (h + 1)..=(h + mound_h) {
                w.set_cell(
                    x,
                    y,
                    Cell {
                        material: MaterialId::Air,
                        sat: Sat(255),
                        ..Cell::default()
                    },
                );
            }
        }
    }
    w
}

fn free_water_above(w: &World, width: i32, y_min: i32) -> i64 {
    (0..width)
        .flat_map(|x| (y_min..70).map(move |y| (x, y)))
        .filter_map(|(x, y)| {
            w.get_cell(x, y).and_then(|c| {
                (c.material == MaterialId::Air).then_some(c.sat.0 as i64)
            })
        })
        .sum()
}

#[test]
#[ignore]
fn mound_drain_rate_and_cost() {
    let width = 256;
    let mut w = mountain_with_mound(width, 6);
    let perf = PerfConfig::default();
    let start = free_water_above(&w, width, 30);
    println!("mound sat above y=30 at t0: {start}");
    let mut t = Instant::now();
    for step in 1..=200 {
        tick_with_perf(&mut w, &perf);
        if step % 25 == 0 {
            let left = free_water_above(&w, width, 30);
            let ms = t.elapsed().as_secs_f64() * 1000.0 / 25.0;
            println!(
                "  tick {step:>3}: {:>5.1}% still on the peak   {ms:>6.2} ms/tick",
                left as f64 * 100.0 / start.max(1) as f64
            );
            t = Instant::now();
        }
    }
}
