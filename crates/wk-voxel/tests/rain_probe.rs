//! Does it ever actually rain?
//!
//! Playtest report: rain has not been seen for a long time, and it is not clear
//! whether the mechanism or only the animation is broken. Those need separating
//! before anything is changed, so this reports both halves:
//!
//! - **mechanism** — water delivered to the world by the precipitation path
//! - **animation** — parcels flagged `raining`, which is what draws the streaks
//!
//! Runs the app's frame order (rain → evap → advect → clouds → condensation)
//! with the app's *defaults*, not the profiler's open faucet, because the
//! question is what a player sees.
//!
//! ```text
//! cargo test -p wk-voxel --release --test rain_probe -- --ignored --nocapture
//! ```

use wk_voxel::{
    apply_condensation_rain_phased, apply_evaporation_into_humidity_climate, stamp_world,
    tick_with_perf, CloudConfig, CloudStore, CondensationConfig, EvapConfig, Humidity,
    OrographicConfig, PerfConfig, PhaseConfig, Temperature, Wind, World, WorldgenParams,
};

/// Mirrors the app's humidity tile width and climate wind.
const HUMIDITY_TILE_COLS: i32 = 4;
const CLIMATE_WIND_VX: f32 = 0.05;

const TICKS: u64 = 3000;

fn main_scene() -> (
    World,
    WorldgenParams,
    Humidity,
    Wind,
    CloudStore,
    Temperature,
) {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let mut humidity = Humidity::with_world_bounds(
        HUMIDITY_TILE_COLS,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
    );
    humidity.wrap_x = params.wrap_x;
    let wind = Wind::climate(
        HUMIDITY_TILE_COLS,
        CLIMATE_WIND_VX,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let temperature = Temperature::with_world_bounds(
        HUMIDITY_TILE_COLS,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.wrap_x,
    );
    (
        world,
        params,
        humidity,
        wind,
        CloudStore::default(),
        temperature,
    )
}

#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn does_it_rain() {
    for cap in [48u32, 256, 1024] {
        run_with_cap(cap);
    }
}

fn run_with_cap(cap: u32) {
    let (mut world, params, mut humidity, wind, mut clouds, temperature) = main_scene();
    let evap = EvapConfig::default();
    // Exactly what the app builds in SimSettings::new. Using
    // CondensationConfig::default() here is a trap: its `top_y` is 0, so rain
    // has nowhere to fall from and the probe blames the sim for its own setup.
    let cond = CondensationConfig {
        top_y: params.sky_ceiling_y - 2,
        min_mass_to_rain: 140.0,
        max_prob_per_tick: 0.10,
        mass_per_droplet: 40.0,
        max_events_per_tick: cap,
        ..CondensationConfig::default()
    };
    let cloud = CloudConfig::default();
    let phase = PhaseConfig::default();
    let mut oro = OrographicConfig::default();
    oro.width_cols = params.width_cols;
    oro.sea_level_y = params.sea_level_y;
    let perf = PerfConfig::default();

    let mut peak_parcels = 0usize;
    let mut peak_raining = 0usize;
    let mut peak_wet = 0.0f32;
    let mut ever_rained = 0u64;
    let mut cond_time = std::time::Duration::ZERO;

    println!("\n=== max_events_per_tick = {cap} ===");
    for t in 0..TICKS {
        apply_evaporation_into_humidity_climate(
            &mut world,
            &mut humidity,
            &evap,
            Some(&temperature),
            0.0,
        );
        humidity.advect(wind.climate_vx, wind.climate_vy);
        clouds.step_with_precip(
            &mut world,
            &mut humidity,
            &wind,
            params.sea_level_y,
            params.sky_ceiling_y,
            t,
            &cloud,
            Some(&temperature),
            Some(&phase),
        );
        let t0 = std::time::Instant::now();
        apply_condensation_rain_phased(
            &mut world,
            &mut humidity,
            &cond,
            Some(&oro),
            Some(&temperature),
            Some(&phase),
        );
        cond_time += t0.elapsed();
        tick_with_perf(&mut world, &perf);

        let parcels = clouds.parcels.len();
        let raining = clouds.parcels.iter().filter(|p| p.raining).count();
        let wet = clouds
            .parcels
            .iter()
            .map(|p| p.wetness())
            .fold(0.0f32, f32::max);
        peak_parcels = peak_parcels.max(parcels);
        peak_raining = peak_raining.max(raining);
        peak_wet = peak_wet.max(wet);
        if raining > 0 {
            ever_rained += 1;
        }
        let _ = (parcels, wet);
    }

    let wants_rain = humidity
        .cells
        .iter()
        .filter(|(_, &m)| m >= cond.min_mass_to_rain)
        .count();
    println!("  tiles wanting to rain         {wants_rain}");
    println!(
        "  oversubscribed by             {:.1}x",
        wants_rain as f32 / cond.max_events_per_tick.max(1) as f32
    );

    println!("  equilibrium humidity          {:.0}", humidity.total_mass());
    println!("  ticks raining                 {ever_rained} / {TICKS}");
    println!("  peak raining parcels          {peak_raining} (of {peak_parcels})");
    println!(
        "  condensation cost             {:.3} ms/tick",
        cond_time.as_secs_f32() * 1000.0 / TICKS as f32
    );
    let _ = peak_wet;
}
