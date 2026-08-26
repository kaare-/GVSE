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

    println!(
        "\n{:>7}  {:>11}  {:>11}  {:>12}  {:>7}  {:>7}",
        "tick", "humidity", "world water", "humid+water", "parcels", "raining"
    );
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
        apply_condensation_rain_phased(
            &mut world,
            &mut humidity,
            &cond,
            Some(&oro),
            Some(&temperature),
            Some(&phase),
        );
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
        if t % 300 == 0 {
            let water = wk_voxel::audit::sat_totals(&world).cell_total as f64;
            let humid = humidity.total_mass() as f64;
            println!(
                "{t:>7}  {humid:>11.0}  {water:>11.0}  {:>12.0}  {parcels:>7}  {raining:>7}",
                humid + water
            );
        }
    }

    // Where do the raining parcels actually sit, relative to the ground they
    // are supposed to be raining onto? The streak draw skips a drop when there
    // is no vertical room between cloud and ground, so a cloud deck hugging the
    // terrain would rain in the sim and draw nothing.
    println!("\n  --- raining parcel geometry (fy vs the ground under it) ---");
    println!(
        "  sea_level_y={}  sky_ceiling_y={}",
        params.sea_level_y, params.sky_ceiling_y
    );
    let mut shown = 0;
    let mut no_room = 0;
    for p in clouds.parcels.iter().filter(|p| p.raining) {
        let floor = wk_voxel::cloud_floor_y(&world, &wind, p.fx);
        let gap = p.fy - floor;
        if gap < 2.0 {
            no_room += 1;
        }
        if shown < 8 {
            println!(
                "  fx={:>7.1}  fy={:>6.1}  cloud_floor={:>6.1}  gap={:>6.1}  radius={:.1}",
                p.fx,
                p.fy,
                floor,
                gap,
                p.radius()
            );
            shown += 1;
        }
    }
    println!(
        "  parcels with < 2 cells of room under them: {no_room} / {}",
        clouds.parcels.iter().filter(|p| p.raining).count()
    );

    // How many tiles were *eligible* to be a cloud, against how many got to be
    // one? Parcels are chosen as the globally wettest tiles, and wet tiles
    // cluster, so a cap well below the eligible count concentrates every cloud
    // in the world into one band.
    let cfg_min = cloud.coag_min_hum;
    let sky_hy_min = (params.sea_level_y + cloud.coag_min_above_sea).div_euclid(4);
    let eligible = humidity
        .cells
        .iter()
        .filter(|((_, hy), &m)| *hy >= sky_hy_min && m >= cfg_min)
        .count();
    let xs: Vec<f32> = clouds.parcels.iter().map(|p| p.fx).collect();
    let (lo, hi) = xs.iter().fold((f32::MAX, f32::MIN), |(l, h), &x| (l.min(x), h.max(x)));
    println!("\n  --- cloud coverage ---");
    println!("  eligible sky tiles      {eligible}");
    println!("  parcels drawn           {} (cap {})", clouds.parcels.len(), cloud.max_parcels);
    println!(
        "  parcel x span           {lo:.0}..{hi:.0} of {} columns ({:.1}% of the world)",
        params.width_cols,
        100.0 * (hi - lo + 1.0) / params.width_cols as f32
    );

    println!("\n  --- over {TICKS} ticks ---");
    println!("  peak parcels        {peak_parcels}");
    println!("  peak raining        {peak_raining}");
    println!("  peak parcel wetness {peak_wet:.3}  (streaks need >= 0.42)");
    println!("  ticks with rain     {ever_rained}");
}
