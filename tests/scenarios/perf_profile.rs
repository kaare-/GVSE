//! Per-subsystem profiler for the app-like ring world.
//!
//! Ignored by default; run explicitly:
//!   cargo test -p wk-sim --test scenarios perf_profile -- --ignored --nocapture

use std::time::{Duration, Instant};
use wk_sim::Simulation;
use wk_world::terrain::{generate_chunk, BEDROCK_FLOOR_M};
use wk_world::world::World;
use wk_world::{WorldGenParams, WorldGenProfile, WorldTopology};

fn app_like_world() -> World {
    let mut world = World::new(42);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.rain_rate = 1.0;
    world.gen = WorldGenParams {
        topology: WorldTopology::Ring { chunks: 192 },
        profile: WorldGenProfile::RingFacies,
    };
    for c in 0..192i32 {
        let chunk = generate_chunk(c, world.seed, BEDROCK_FLOOR_M, world.sea_level, world.gen);
        world.insert_chunk(chunk);
    }
    world.wake_all();
    world.recompute_mass_audit();
    world.enable_thermal_fields();
    world.enable_humidity_fields();
    world.enable_pressure_wind_fields();
    world.enable_groundwater_head_fields();
    world.enable_dissolved_fields();
    world.surface_waves_enabled = true;
    world.tide_enabled = true;
    world
}

fn time_ticks(sim: &mut Simulation, world: &mut World, n: u64) -> Duration {
    let start = Instant::now();
    for _ in 0..n {
        sim.step(world);
    }
    start.elapsed()
}

#[test]
#[ignore]
fn perf_profile_ring_app_world() {
    let mut world = app_like_world();
    let mut sim = Simulation::new(&world);
    let ticks = 200u64;
    let baseline = time_ticks(&mut sim, &mut world, ticks);
    eprintln!(
        "BASELINE (all systems on): {ticks} ticks in {:?}  ({:.2} ms/tick)",
        baseline,
        baseline.as_secs_f32() * 1000.0 / ticks as f32
    );
}

fn measure_without<F: FnOnce(&mut World)>(name: &str, disable: F) -> Duration {
    let mut world = app_like_world();
    disable(&mut world);
    let mut sim = Simulation::new(&world);
    let ticks = 200u64;
    let dt = time_ticks(&mut sim, &mut world, ticks);
    eprintln!(
        "  without {name:<22}: {:?}  ({:.2} ms/tick)",
        dt,
        dt.as_secs_f32() * 1000.0 / ticks as f32
    );
    dt
}

#[test]
#[ignore]
fn perf_profile_disable_each() {
    measure_without("waves", |w| {
        w.surface_waves_enabled = false;
        w.tide_enabled = false;
    });
    measure_without("thermal", |w| {
        w.thermal_fields_enabled = false;
    });
    measure_without("humidity", |w| {
        w.humidity_fields_enabled = false;
    });
    measure_without("pressure/wind", |w| {
        w.pressure_wind_fields_enabled = false;
    });
    measure_without("groundwater_head", |w| {
        w.gw_head_fields_enabled = false;
    });
    measure_without("dissolved", |w| {
        w.dissolved_fields_enabled = false;
    });
    measure_without("weather", |w| {
        w.weather.weather_enabled = false;
    });
    measure_without("all_fields", |w| {
        w.thermal_fields_enabled = false;
        w.humidity_fields_enabled = false;
        w.pressure_wind_fields_enabled = false;
        w.gw_head_fields_enabled = false;
        w.dissolved_fields_enabled = false;
    });
    measure_without("all_fields+waves", |w| {
        w.thermal_fields_enabled = false;
        w.humidity_fields_enabled = false;
        w.pressure_wind_fields_enabled = false;
        w.gw_head_fields_enabled = false;
        w.dissolved_fields_enabled = false;
        w.surface_waves_enabled = false;
        w.tide_enabled = false;
    });
}

#[test]
#[ignore]
fn perf_profile_subsystem_kernels() {
    // Time individual subsystem kernels directly for 200 invocations each.
    use wk_sim::subsystems::{
        run_dissolved_field, run_groundwater_head_field, run_humidity_field, run_lake_level,
        run_pressure_field, run_slumping, run_surface_waves, run_thermal_field, run_wind_field,
    };
    let mut world = app_like_world();
    let mut sim = Simulation::new(&world);
    // Warm the world so activity flags / fields settle.
    for _ in 0..40 {
        sim.step(&mut world);
    }
    macro_rules! bench {
        ($name:expr, $reps:expr, $body:block) => {{
            let start = Instant::now();
            for _ in 0..$reps {
                $body
            }
            let dt = start.elapsed();
            eprintln!(
                "  {:<24} x{:>4} in {:?}  ({:.3} ms/call)",
                $name,
                $reps,
                dt,
                dt.as_secs_f32() * 1000.0 / $reps as f32
            );
        }};
    }
    bench!("run_thermal_field", 50, {
        run_thermal_field(&mut world, 0);
    });
    bench!("run_humidity_field", 50, {
        run_humidity_field(&mut world, 0);
    });
    bench!("run_pressure_field", 50, {
        run_pressure_field(&mut world, 0);
    });
    bench!("run_wind_field", 50, {
        run_wind_field(&mut world, 0);
    });
    bench!("run_groundwater_head_field", 50, {
        run_groundwater_head_field(&mut world, 0);
    });
    bench!("run_dissolved_field", 50, {
        run_dissolved_field(&mut world, 0);
    });
    bench!("run_surface_waves", 200, {
        run_surface_waves(&mut world, 0);
    });
    bench!("run_lake_level", 200, {
        run_lake_level(&mut world);
    });
    bench!("run_slumping", 200, {
        run_slumping(&mut world, 0);
    });
    bench!("recompute_mass_audit", 200, {
        world.recompute_mass_audit();
    });
}
