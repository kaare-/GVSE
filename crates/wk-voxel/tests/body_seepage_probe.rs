//! Where do `rock bodies` and `seepage` spend their time on a settled world?
//!
//! The perf profile attributes ~7 ms/tick (demo) and ~13 ms/tick (stress) to
//! each of those two passes even when nothing visible is moving. This probe
//! reports the per-tick *work counts* behind that time so the fix targets the
//! cause rather than the symptom.
//!
//! ```text
//! cargo test -p wk-voxel --release --test body_seepage_probe -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    competent_probe, stamp_world, tick_with_perf, tick_with_perf_profiled, PerfConfig,
    PhysicsTimings, World, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W,
};

const WARMUP: u64 = 40;
const MEASURE: u64 = 120;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

fn demo() -> WorldgenParams {
    WorldgenParams::default()
}

fn stress() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 32,
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 6,
        ..WorldgenParams::default()
    }
}

fn report(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();

    for _ in 0..WARMUP {
        tick_with_perf(&mut world, &perf);
    }

    competent_probe::reset();
    let mut phys = PhysicsTimings::default();
    let wall = Instant::now();
    for _ in 0..MEASURE {
        tick_with_perf_profiled(&mut world, &perf, &mut phys);
    }
    let wall = wall.elapsed();
    let p = competent_probe::snapshot();
    let n = MEASURE;

    println!("\n=== {label} ===  {} chunks", world.chunks.len());
    println!("  wall              {:>8.3} ms/tick", ms(wall, n));
    println!("  rock bodies       {:>8.3} ms/tick", ms(phys.bodies, n));
    println!("  seepage           {:>8.3} ms/tick", ms(phys.seepage, n));
    println!("  --- body pass work per tick ---");
    println!("  build_components calls  {:>10.1}", p.build_calls as f32 / n as f32);
    println!("  seed candidates         {:>10.1}", p.seed_candidates as f32 / n as f32);
    println!("  seeds passed gate       {:>10.1}", p.seeds_passed as f32 / n as f32);
    println!("  floods                  {:>10.1}", p.floods as f32 / n as f32);
    println!("  flood cells visited     {:>10.1}", p.flood_cells as f32 / n as f32);
    println!("  strata bailouts         {:>10.1}", p.strata_bailouts as f32 / n as f32);
    println!("  components produced     {:>10.1}", p.components as f32 / n as f32);
    println!("  weld-split calls        {:>10.1}", p.split_calls as f32 / n as f32);
    println!("  weld-split cells        {:>10.1}", p.split_cells as f32 / n as f32);
    println!("  hanging-extract calls   {:>10.1}", p.hang_calls as f32 / n as f32);
    println!("  cargo gather calls      {:>10.1}", p.cargo_calls as f32 / n as f32);
    println!("  cargo cells             {:>10.1}", p.cargo_cells as f32 / n as f32);
    println!("  --- why it ran / what it decided ---");
    println!("  wake cells queued       {:>10.1}", p.wake_cells as f32 / n as f32);
    println!("    from solidity change  {:>10.1}", p.wake_from_solidity as f32 / n as f32);
    println!("    from bodies moved     {:>10.1}", p.wake_from_moved as f32 / n as f32);
    println!("    from cadence float    {:>10.1}", p.wake_from_cadence_float as f32 / n as f32);
    println!("    from cadence seed     {:>10.1}", p.wake_from_cadence_seed as f32 / n as f32);
    println!("  region cells scanned    {:>10.1}", p.region_cells as f32 / n as f32);
    println!("  comps → sleep           {:>10.1}", p.comp_slept as f32 / n as f32);
    println!("  comps floating          {:>10.1}", p.comp_floating as f32 / n as f32);
    println!("  comps unsupported stuck {:>10.1}", p.comp_unsupported_stuck as f32 / n as f32);
    println!("  comps fell              {:>10.1}", p.comp_fell as f32 / n as f32);
    println!("  comps fall refused      {:>10.1}", p.comp_fall_refused as f32 / n as f32);
    println!("  comps rolled            {:>10.1}", p.comp_rolled as f32 / n as f32);
    println!("  comps shattered         {:>10.1}", p.comp_shattered as f32 / n as f32);
}

/// Does the body pass converge on a world where nothing moves, or is it a
/// treadmill? Reports cost per window over a long quiet run.
fn convergence(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();
    println!("\n=== {label} convergence (quiet world) ===");
    for window in 0..8 {
        competent_probe::reset();
        let mut phys = PhysicsTimings::default();
        for _ in 0..100 {
            tick_with_perf_profiled(&mut world, &perf, &mut phys);
        }
        let p = competent_probe::snapshot();
        println!(
            "  ticks {:>4}-{:>4}   bodies {:>6.2} ms   seeds {:>8.0}   comps {:>6.1}   slept {:>5.1}",
            window * 100,
            window * 100 + 99,
            ms(phys.bodies, 100),
            p.seed_candidates as f32 / 100.0,
            p.components as f32 / 100.0,
            p.comp_slept as f32 / 100.0,
        );
    }
}

#[test]
#[ignore = "diagnostic probe; run explicitly"]
fn probe_body_and_seepage_work() {
    report("demo", demo());
    report("stress", stress());
    convergence("demo", demo());
}
