//! Profile the post-flow "tail" of [`tick_with_life`]: full-grid wakes,
//! punch, buoyancy, soak, failure, mycelium. Ignored by default.
//!
//! ```text
//! cargo test -p wk-voxel --test physics_tail_profile --release -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_failure, apply_rain_with_temp, punch_through_floating_rafts,
    rise_and_soak_buoyant_litter, stamp_world, step_mycelium_field, tick_with_perf,
    wake_confined_head, wake_grains_for_settle, FailureConfig, PerfConfig, PhaseConfig,
    RainConfig, Temperature, World, WorldgenParams,
};

const WARMUP: u64 = 30;
const MEASURE: u64 = 80;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

#[test]
#[ignore]
fn profile_physics_tail_passes() {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let temperature = Temperature::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.wrap_x,
    );
    let rain = RainConfig {
        top_y: params.sky_ceiling_y - 1,
        x_range: (0, params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
        closed_loop: true, // match demo default
        sea_level_y: params.sea_level_y,
        ..RainConfig::default()
    };
    let phase = PhaseConfig::default();
    let perf = PerfConfig::default();
    let failure = FailureConfig::default();

    for _ in 0..WARMUP {
        apply_rain_with_temp(
            &mut world,
            &rain,
            Some(&temperature),
            Some(&phase),
            None,
        );
        tick_with_perf(&mut world, &perf);
    }

    let mut confined = Duration::ZERO;
    let mut wake_settle = Duration::ZERO;
    let mut punch = Duration::ZERO;
    let mut wake_after_punch = Duration::ZERO;
    let mut rise_soak = Duration::ZERO;
    let mut failure_t = Duration::ZERO;
    let mut mycelium = Duration::ZERO;
    let mut full_tick = Duration::ZERO;

    for _ in 0..MEASURE {
        apply_rain_with_temp(
            &mut world,
            &rain,
            Some(&temperature),
            Some(&phase),
            None,
        );

        let t0 = Instant::now();
        tick_with_perf(&mut world, &perf);
        full_tick += t0.elapsed();

        // Isolate tail costs on a second world snapshot path: re-run each
        // pass on the post-tick world (slightly optimistic vs mid-tick, but
        // ranks relative cost of full-grid scans).
        let t0 = Instant::now();
        wake_confined_head(&mut world);
        confined += t0.elapsed();

        let t0 = Instant::now();
        wake_grains_for_settle(&mut world);
        wake_settle += t0.elapsed();

        let t0 = Instant::now();
        let n = punch_through_floating_rafts(&mut world);
        punch += t0.elapsed();
        if n > 0 {
            let t0 = Instant::now();
            wake_grains_for_settle(&mut world);
            wake_after_punch += t0.elapsed();
        }

        let t0 = Instant::now();
        rise_and_soak_buoyant_litter(&mut world);
        rise_soak += t0.elapsed();

        let t0 = Instant::now();
        let _ = apply_failure(&mut world, &failure, None);
        failure_t += t0.elapsed();

        let t0 = Instant::now();
        step_mycelium_field(&mut world);
        mycelium += t0.elapsed();
    }

    eprintln!("=== Physics tail (closed_loop rain, demo world) ===");
    eprintln!("  full tick_with_perf   {:>8.3} ms/tick", ms(full_tick, MEASURE));
    eprintln!("  wake_confined_head    {:>8.3} ms/call (extra)", ms(confined, MEASURE));
    eprintln!("  wake_grains_for_settle{:>8.3} ms/call (extra)", ms(wake_settle, MEASURE));
    eprintln!("  punch_through_rafts   {:>8.3} ms/call (extra)", ms(punch, MEASURE));
    eprintln!("  wake after punch      {:>8.3} ms/call (extra)", ms(wake_after_punch, MEASURE));
    eprintln!("  rise_and_soak         {:>8.3} ms/call (extra)", ms(rise_soak, MEASURE));
    eprintln!("  apply_failure         {:>8.3} ms/call (extra)", ms(failure_t, MEASURE));
    eprintln!("  step_mycelium_field   {:>8.3} ms/call (extra)", ms(mycelium, MEASURE));
    let tail = confined
        + wake_settle
        + punch
        + wake_after_punch
        + rise_soak
        + failure_t
        + mycelium;
    eprintln!("  sum(tail extras)      {:>8.3} ms/tick", ms(tail, MEASURE));
}
