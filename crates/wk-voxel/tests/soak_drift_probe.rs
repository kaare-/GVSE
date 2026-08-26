//! Does tick cost grow as a world soaks?
//!
//! The playtest report is not a flat low frame rate but a *decaying* one: a
//! fresh world ran acceptably, and after a night of simulation the same world
//! was at 5 FPS. That shape points at state that accumulates rather than at
//! any single expensive pass, so this probe walks a world forward and reports
//! cost next to the sizes of the containers that could be growing under it.
//!
//! `dissolved` is the prime suspect: it is a per-cell `HashMap` fed by karst
//! and aperture growth, and every seepage transfer consults it once it is
//! non-empty.
//!
//! ```text
//! cargo test -p wk-voxel --release --test soak_drift_probe -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    competent_probe, stamp_world, tick_with_perf, tick_with_perf_profiled, PerfConfig,
    PhysicsTimings, World, WorldgenParams,
};

const SEGMENT: u64 = 400;
const SEGMENTS: usize = 12;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn cost_versus_soak_age() {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();
    for _ in 0..40 {
        tick_with_perf(&mut world, &perf);
    }

    println!(
        "\n{:>7}  {:>9}  {:>9}  {:>9}  {:>9}  {:>10}  {:>9}",
        "tick", "wall", "seepage", "bodies", "flow", "dissolved", "wet chunks"
    );
    for _ in 0..SEGMENTS {
        competent_probe::reset();
        let mut phys = PhysicsTimings::default();
        let wall = Instant::now();
        for _ in 0..SEGMENT {
            tick_with_perf_profiled(&mut world, &perf, &mut phys);
        }
        let wall = wall.elapsed();
        let wet = world
            .chunks
            .values()
            .filter(|c| c.has_wet_pores || c.has_wet_air)
            .count();
        println!(
            "{:>7}  {:>7.3}ms  {:>7.3}ms  {:>7.3}ms  {:>7.3}ms  {:>10}  {:>9}",
            world.tick,
            ms(wall, SEGMENT),
            ms(phys.seepage, SEGMENT),
            ms(phys.bodies, SEGMENT),
            ms(phys.water_flow, SEGMENT),
            world.dissolved.len(),
            wet,
        );
    }
    // What is the body pass actually doing on an aged world? A settled world
    // should drive these toward zero.
    let p = competent_probe::snapshot();
    let n = SEGMENT;
    println!("\n  --- competent body work, per tick, final segment ---");
    println!("  wake cells        {:>9.1}", p.wake_cells as f64 / n as f64);
    println!("    from solidity   {:>9.1}", p.wake_from_solidity as f64 / n as f64);
    println!("    from moved      {:>9.1}", p.wake_from_moved as f64 / n as f64);
    println!("    cadence float   {:>9.1}", p.wake_from_cadence_float as f64 / n as f64);
    println!("    cadence seed    {:>9.1}", p.wake_from_cadence_seed as f64 / n as f64);
    println!("  region cells      {:>9.1}", p.region_cells as f64 / n as f64);
    println!("  build calls       {:>9.1}", p.build_calls as f64 / n as f64);
    println!("  flood cells       {:>9.1}", p.flood_cells as f64 / n as f64);
    println!("  split cells       {:>9.1}", p.split_cells as f64 / n as f64);
    println!("  components        {:>9.1}", p.components as f64 / n as f64);
    println!("  cargo cells       {:>9.1}", p.cargo_cells as f64 / n as f64);
    println!("  slept             {:>9.1}", p.comp_slept as f64 / n as f64);
    println!("  floating          {:>9.1}", p.comp_floating as f64 / n as f64);
    println!("  unsupported stuck {:>9.1}", p.comp_unsupported_stuck as f64 / n as f64);
    println!("  fell / refused    {:>9.1} / {:.1}", p.comp_fell as f64 / n as f64, p.comp_fall_refused as f64 / n as f64);
    println!("  rolled            {:>9.1}", p.comp_rolled as f64 / n as f64);
    println!("  shattered         {:>9.1}", p.comp_shattered as f64 / n as f64);
}

/// Where are the endless rolls happening, and are they a ping-pong?
///
/// `cost_versus_soak_age` shows 3.4 rolls/tick on a world aged 4800 ticks with
/// nothing falling, shattering or floating. Every roll keeps the topology loop
/// alive for another pass, so a handful of restless cells pay for six full
/// component rebuilds. This lists the moves so the shape is visible.
#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn who_keeps_rolling() {
    use std::collections::HashMap;
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();
    for _ in 0..2000 {
        tick_with_perf(&mut world, &perf);
    }

    // Count how often each (from -> to) edge is traversed, and how often each
    // cell moves at all. A cell that moves every tick is not settling.
    let mut edges: HashMap<((i32, i32), (i32, i32)), u32> = HashMap::new();
    let mut movers: HashMap<(i32, i32), u32> = HashMap::new();
    const TICKS: u32 = 200;
    for _ in 0..TICKS {
        tick_with_perf(&mut world, &perf);
        for &(fx, fy, tx, ty) in &world.competent_cell_moves {
            *edges.entry(((fx, fy), (tx, ty))).or_default() += 1;
            *movers.entry((fx, fy)).or_default() += 1;
        }
    }

    let mut top: Vec<_> = edges.into_iter().collect();
    top.sort_by(|a, b| b.1.cmp(&a.1));
    println!("\n  --- most-travelled roll edges over {TICKS} ticks ---");
    for ((from, to), n) in top.iter().take(14) {
        println!(
            "  {:>5},{:<5} -> {:>5},{:<5}   {n:>4}×",
            from.0, from.1, to.0, to.1
        );
    }
    let mut busiest: Vec<_> = movers.into_iter().collect();
    busiest.sort_by(|a, b| b.1.cmp(&a.1));
    println!("  distinct source cells: {}", busiest.len());
    println!("  busiest cell moved {} / {TICKS} ticks", busiest.first().map(|b| b.1).unwrap_or(0));
}
