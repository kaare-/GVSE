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
    // Cap sweep already showed the event cap is not binding (48 -> 1024 changed
    // nothing). Sweep droplet size instead: a deposit that is refused for being
    // too small drains no humidity at all, which would pin the equilibrium.
    // The app's own value. Sweeping this was tried and is not a tuning lever:
    // equilibrium humidity moves non-monotonically (0.30 -> 331k but 0.60 ->
    // 472k) and the eligible-tile count swings 425..1350 between runs, because
    // evaporation, humidity and precipitation are mutually coupled and a
    // 3000-tick run from a fresh stamp has too much variance to separate them.
    run_with_prob(0.10);
}

fn run_with_prob(prob: f32) {
    let drop = 255.0f32;
    let (mut world, params, mut humidity, wind, mut clouds, temperature) = main_scene();
    let evap = EvapConfig::default();
    // Exactly what the app builds in SimSettings::new. Using
    // CondensationConfig::default() here is a trap: its `top_y` is 0, so rain
    // has nowhere to fall from and the probe blames the sim for its own setup.
    let cond = CondensationConfig {
        top_y: params.sky_ceiling_y - 2,
        min_mass_to_rain: 140.0,
        max_prob_per_tick: prob,
        mass_per_droplet: drop,
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
    // Rain against the day/night cycle. The diurnal machinery already exists
    // (solar heat, night cool, saturation-vs-temperature), so the question is
    // whether it actually modulates rain or is swamped.
    let mut day_ticks = 0u64;
    let mut day_rain = 0u64;
    let mut night_ticks = 0u64;
    let mut night_rain = 0u64;
    let mut day_hum = 0.0f64;
    let mut night_hum = 0.0f64;

    println!("\n=== max_prob_per_tick = {prob} ===");
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
        let dn = wk_voxel::day_night_factor(t);
        if dn >= 0.0 {
            day_ticks += 1;
            day_hum += humidity.total_mass() as f64;
            if raining > 0 {
                day_rain += 1;
            }
        } else {
            night_ticks += 1;
            night_hum += humidity.total_mass() as f64;
            if raining > 0 {
                night_rain += 1;
            }
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
    println!(
        "  rain fraction: day {:.0}%  night {:.0}%",
        100.0 * day_rain as f32 / day_ticks.max(1) as f32,
        100.0 * night_rain as f32 / night_ticks.max(1) as f32
    );
    println!(
        "  mean humidity: day {:.0}   night {:.0}",
        day_hum / day_ticks.max(1) as f64,
        night_hum / night_ticks.max(1) as f64
    );
    println!("  ticks raining                 {ever_rained} / {TICKS}");
    println!("  peak raining parcels          {peak_raining} (of {peak_parcels})");
    println!(
        "  condensation cost             {:.3} ms/tick",
        cond_time.as_secs_f32() * 1000.0 / TICKS as f32
    );
    let _ = peak_wet;
}
