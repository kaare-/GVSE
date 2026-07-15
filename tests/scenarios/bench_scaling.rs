//! Scaling benchmark: measure tps at multiple chunk-count scales.
//!
//! Ignored by default so the normal test run stays quick; opt in with
//!     cargo test --release -p wk-sim --test scenarios -- --ignored --nocapture bench_scaling

use std::time::Instant;

fn bench_continental(chunks_min: i32, chunks_max: i32, ticks: u64, rain: bool) -> (usize, f64) {
    let mut world = wk_world::world::World::new(42);
    world.sea_level = 12.0;
    world.rain_enabled = rain;
    world.rain_rate = 1.0;
    world.weather.weather_enabled = false;
    for c in chunks_min..chunks_max {
        world.insert_chunk(wk_world::terrain::generate_chunk_continental(
            c,
            world.seed,
            wk_world::terrain::BEDROCK_FLOOR_M,
            world.sea_level,
        ));
    }
    world.wake_all();
    world.recompute_mass_audit();

    let mut sim = wk_sim::Simulation::new(&world);
    let column_count = world.chunks.len() * wk_material::CHUNK_W;
    let start = Instant::now();
    sim.run_ticks(&mut world, ticks);
    let elapsed = start.elapsed();
    let tps = ticks as f64 / elapsed.as_secs_f64().max(0.000_001);
    eprintln!(
        "chunks={} cols={} rain={} ticks={} elapsed={:?} tps={:.0}",
        chunks_max - chunks_min,
        column_count,
        rain,
        ticks,
        elapsed,
        tps
    );
    (column_count, tps)
}

#[test]
#[ignore]
fn bench_scaling() {
    // Cold-start scaling (no rain, world settles first).
    let ticks = 2000;
    bench_continental(0, 4, ticks, false);
    bench_continental(0, 16, ticks, false);
    bench_continental(0, 32, ticks, false);
    bench_continental(0, 64, ticks, false);
    // Match the shipped map (88 chunks × 64 cols = 5632).
    bench_continental(-8, 80, ticks, false);

    // Now with rain enabled — this is the interesting steady-state
    // load for a live game.
    let ticks = 2000;
    bench_continental(0, 4, ticks, true);
    bench_continental(0, 16, ticks, true);
    bench_continental(-8, 80, ticks, true);
}
