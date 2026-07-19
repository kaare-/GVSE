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
fn diag_ocean_surface_spikes() {
    // Reproduce the app scenario and dump per-column surface_y / water_top
    // around the ring so we can see which columns diverge from a smooth
    // ocean surface.
    let mut world = app_like_world();
    let mut sim = Simulation::new(&world);
    for _ in 0..800 {
        sim.step(&mut world);
    }
    let width = world.topology().width_columns().unwrap();
    let mut samples: Vec<(i32, f32, f32, i64, bool)> = Vec::new();
    let mut max_dev = 0.0f32;
    let mut prev_top: Option<f32> = None;
    let mut jumps: Vec<(i32, f32, f32, f32)> = Vec::new();
    for x in 0..width {
        let Some(col) = world.column_at(x) else {
            continue;
        };
        // Only look at columns whose water top is below sea (real ocean).
        let top = col.flowable_water().map(|(t, _)| t).unwrap_or(f32::MIN);
        if top > world.sea_level + 5.0 || top < world.sea_level - 5.0 {
            continue;
        }
        let water_top = col
            .flowable_water()
            .map(|(t, _)| t)
            .unwrap_or(col.surface_y);
        let water_mass = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
        samples.push((x, col.surface_y, water_top, water_mass, !col.voids.is_empty()));
        if let Some(prev) = prev_top {
            let d = (water_top - prev).abs();
            if d > 0.5 {
                jumps.push((x, prev, water_top, d));
            }
            max_dev = max_dev.max(d);
        }
        prev_top = Some(water_top);
    }
    let with_voids = samples.iter().filter(|s| s.4).count();
    eprintln!("ocean cols={} with_voids={} max_neighbour_dev={:.3} m", samples.len(), with_voids, max_dev);
    // Which layers are on top? Water spike columns might have Ice / Snow etc.
    use std::collections::HashMap;
    let mut top_layer_hist: HashMap<&'static str, u32> = HashMap::new();
    let mut spike_cols: Vec<i32> = Vec::new();
    for x in 0..width {
        let Some(col) = world.column_at(x) else { continue };
        let top = col.flowable_water().map(|(t, _)| t).unwrap_or(col.surface_y);
        let bed_y = top - col.flowable_water().map(|(_, m)| m as f32 / 250.0).unwrap_or(0.0);
        if bed_y >= world.sea_level - 0.25 {
            continue;
        }
        if top > world.sea_level + 0.5 {
            spike_cols.push(x);
        }
        let top_mat = if col.layer_count > 0 {
            match col.layers[0].material {
                wk_material::MaterialId::Water => "water",
                wk_material::MaterialId::Ice => "ice",
                wk_material::MaterialId::Snow => "snow",
                wk_material::MaterialId::Sand => "sand",
                _ => "other",
            }
        } else {
            "empty"
        };
        *top_layer_hist.entry(top_mat).or_default() += 1;
    }
    eprintln!("top-layer histogram on ocean cells: {top_layer_hist:?}");
    eprintln!("spike columns (top > sea+0.5): {} → {:?}", spike_cols.len(), &spike_cols[..spike_cols.len().min(30)]);
    // Dump every column in a small stripe to see what breaks segments.
    eprintln!("--- consecutive stripe ---");
    for x in 2643..=2670 {
        if let Some(col) = world.column_at(x) {
            let (top, mass) = col
                .flowable_water()
                .unwrap_or((col.surface_y, 0));
            let depth = mass as f32 / 250.0;
            let bed = top - depth;
            eprintln!(
                "  x={x}  bed={bed:.2}  top={top:.2}  water_kg={mass}  surface_y={:.2}",
                col.surface_y
            );
        }
    }
    if let Some(&x) = spike_cols.first() {
        let col = world.column_at(x).unwrap();
        eprintln!("--- spike column x={x} ---");
        for i in 0..col.layer_count as usize {
            eprintln!("  layer[{i}] {:?}  thickness={}kg", col.layers[i].material, col.layers[i].thickness);
        }
        eprintln!("  surface_y={:.3}  moisture={} sediment={}kg voids={}", col.surface_y, col.moisture, col.sediment.total, col.voids.len());
    }
    for (x, a, b, d) in jumps.iter().take(20) {
        eprintln!("  jump at x={x}  prev={a:.2} → this={b:.2}  Δ={d:.3}");
    }
    if !samples.is_empty() {
        // Print a sample stripe of consecutive columns.
        let start = samples.len() / 2;
        for s in samples.iter().skip(start).take(24) {
            eprintln!(
                "  x={}  surface_y={:.3}  water_top={:.3}  water_kg={}  voids={}",
                s.0, s.1, s.2, s.3, s.4
            );
        }
    }
    eprintln!("sea_level={:.3}  tide_eta={:.3}", world.sea_level, world.tide_eta_m(sim.clock.tick));
}

#[test]
#[ignore]
fn perf_profile_ring_app_world_with_creatures() {
    use wk_sim::{Blueprint, Energy, Genome};
    let mut world = app_like_world();
    let mut sim = Simulation::new(&world);
    // Warm the world; then spawn creatures spread around the ring.
    for _ in 0..40 {
        sim.step(&mut world);
    }
    let width = world.topology().width_columns().unwrap();
    let step = (width / 520).max(1);
    let mut spawned = 0;
    let mut x = 0i32;
    while spawned < 500 && x < width {
        if sim
            .agents
            .spawn_from_blueprint(&world, x, Blueprint::atom(Genome::default()), 50.0)
            .is_some()
        {
            spawned += 1;
        }
        x += step;
    }
    // Top all creatures up to max energy so they'll try to fission (stress
    // test the birth / clear-pose / collision path).
    for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
        energy.current = energy.max;
    }
    let pop = sim.agents.organism_count();
    let ticks = 200u64;
    let start = std::time::Instant::now();
    for _ in 0..ticks {
        sim.step(&mut world);
        // Keep the pop at ceiling by re-topping energy each tick.
        for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
    }
    let dt = start.elapsed();
    eprintln!(
        "CREATURES pop_before={pop} pop_after={} births={}: {ticks} ticks in {:?} ({:.2} ms/tick)",
        sim.agents.organism_count(),
        sim.agents.births_total,
        dt,
        dt.as_secs_f32() * 1000.0 / ticks as f32
    );
}

#[test]
#[ignore]
fn perf_profile_creature_kernels() {
    // Profile the individual creature hot paths at MAX_ORGANISMS.
    use wk_sim::{Blueprint, Energy, Genome};
    let mut world = app_like_world();
    let mut sim = Simulation::new(&world);
    for _ in 0..40 {
        sim.step(&mut world);
    }
    let width = world.topology().width_columns().unwrap();
    let step = (width / 520).max(1);
    let mut x = 0i32;
    while sim.agents.organism_count() < 500 && x < width {
        let _ = sim.agents.spawn_from_blueprint(
            &world,
            x,
            Blueprint::atom(Genome::default()),
            50.0,
        );
        x += step;
    }
    for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
        energy.current = energy.max;
    }
    let pop = sim.agents.organism_count();
    eprintln!("--- creature kernels @ pop={pop} ---");

    macro_rules! bench {
        ($name:expr, $reps:expr, $body:block) => {{
            let start = std::time::Instant::now();
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
    // `run_agents` is the top-level pass (grazers + organisms + waking).
    use wk_sim::subsystems::run_agents;
    bench!("run_agents (full pass)", 100, {
        run_agents(&mut world, &mut sim.agents, sim.clock.tick);
        for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
    });
    // Draw list / inspect API (called every render frame).
    bench!("organism_draw_list", 200, {
        let _ = sim.agents.organism_draw_list();
    });

    // Split out step_organisms specifically (grazers are 0 in this scenario).
    bench!("step_organisms", 100, {
        sim.agents.step_organisms(&mut world, sim.clock.tick);
        for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
    });

    // Now kill all creatures then respawn to force many births in one tick
    // (the worst case — every fission attempt fires collect_bodies +
    // find_clear_pose across a full-ring domain).
    let entities: Vec<_> = sim
        .agents
        .ecs
        .query::<&wk_sim::Organism>()
        .iter()
        .map(|(e, _)| e)
        .collect();
    for e in &entities {
        let _ = sim.agents.ecs.despawn(*e);
    }
    // Spawn a handful of parents that will all try to birth this tick.
    for x in (0..width).step_by((width / 40).max(1) as usize).take(40) {
        let _ = sim.agents.spawn_from_blueprint(
            &world,
            x,
            Blueprint::atom(Genome::default()),
            50.0,
        );
    }
    for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
        energy.current = energy.max;
    }
    // Warm one tick, then bench a "birth flurry" tick.
    let start = std::time::Instant::now();
    for _ in 0..50 {
        for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
        run_agents(&mut world, &mut sim.agents, sim.clock.tick);
    }
    let dt = start.elapsed();
    eprintln!(
        "  run_agents (50 flurry ticks, ~40 parents cloning): {:?} ({:.3} ms/call)",
        dt,
        dt.as_secs_f32() * 1000.0 / 50.0
    );
    eprintln!("  final pop={}", sim.agents.organism_count());
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
    measure_without("weather", |w| {
        w.weather.weather_enabled = false;
    });
    measure_without("all_fields", |w| {
        w.thermal_fields_enabled = false;
        w.humidity_fields_enabled = false;
        w.pressure_wind_fields_enabled = false;
    });
    measure_without("all_fields+waves", |w| {
        w.thermal_fields_enabled = false;
        w.humidity_fields_enabled = false;
        w.pressure_wind_fields_enabled = false;
        w.surface_waves_enabled = false;
        w.tide_enabled = false;
    });
}

#[test]
#[ignore]
fn perf_profile_subsystem_kernels() {
    // Time individual subsystem kernels directly for 200 invocations each.
    use wk_sim::subsystems::{
        run_humidity_field, run_lake_level, run_pressure_field, run_slumping, run_surface_waves,
        run_thermal_field, run_wind_field,
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
    // Buffered / every-tick systems (barrier_commit driven).
    use wk_sim::subsystems::{
        run_activity, run_evaporation, run_infiltration, run_layer_merge, run_phase_change,
        run_rain_inject, run_sediment, run_surface_water, run_weather, SimParams,
    };
    let params = SimParams {
        rain_rate: world.rain_rate,
        rain_enabled: world.rain_enabled,
        sea_level: world.sea_level,
    };
    let mut scratch = wk_sim::WorldTransferScratch::default();
    bench!("run_surface_water", 200, {
        run_surface_water(&world, &mut scratch);
    });
    bench!("run_weather", 200, {
        run_weather(&mut world, &mut scratch, 0);
    });
    bench!("run_rain_inject", 200, {
        run_rain_inject(&mut world, &mut scratch, &params, 0);
    });
    bench!("run_evaporation", 200, {
        run_evaporation(&mut world, &mut scratch);
    });
    bench!("run_infiltration", 200, {
        run_infiltration(&mut world, &mut scratch);
    });
    bench!("run_sediment", 200, {
        run_sediment(&world, &mut scratch, 0);
    });
    bench!("run_phase_change", 200, {
        run_phase_change(&mut world, 0);
    });
    bench!("run_activity", 200, {
        run_activity(&mut world);
    });
    bench!("run_layer_merge", 200, {
        run_layer_merge(&mut world, 0);
    });
    // Barrier commit is called once per tick after buffered subsystems.
    bench!("barrier_commit", 200, {
        wk_sim::barrier_commit(&mut world, &mut scratch, 0);
    });

    // Also time the per-tick subsystems via full sim.step in a tight loop
    // and compare against selectively disabling each.
    fn reset() -> (World, Simulation) {
        let mut w = app_like_world();
        let s = Simulation::new(&w);
        // Warm.
        let _ = &mut w;
        (w, s)
    }
    let mut sub_costs: Vec<(&'static str, f32)> = Vec::new();
    // Baseline mean time per step.
    let (mut w, mut s) = reset();
    let start = Instant::now();
    for _ in 0..200 {
        s.step(&mut w);
    }
    let baseline_ms = start.elapsed().as_secs_f32() * 1000.0 / 200.0;
    eprintln!("baseline step: {baseline_ms:.3} ms/tick");
    macro_rules! bench_without {
        ($name:expr, $($setup:tt)+) => {{
            let (mut w, mut s) = reset();
            $($setup)+;
            let start = Instant::now();
            for _ in 0..200 { s.step(&mut w); }
            let dt = start.elapsed().as_secs_f32() * 1000.0 / 200.0;
            let saved = baseline_ms - dt;
            eprintln!("  without {:<22}: {dt:.3} ms/tick (saves {saved:.3})", $name);
            sub_costs.push(($name, saved));
        }};
    }
    bench_without!("all_fields", {
        w.thermal_fields_enabled = false;
        w.humidity_fields_enabled = false;
        w.pressure_wind_fields_enabled = false;
    });
    bench_without!("waves+tide", {
        w.surface_waves_enabled = false;
        w.tide_enabled = false;
    });
    bench_without!("weather", w.weather.weather_enabled = false);
    // Weather-off is the closest proxy we have without adding per-subsystem
    // enable flags; the remaining tick cost is: RainInject, SurfaceWater,
    // Sediment, Infiltration, Evaporation, LayerMerge, Activity,
    // PhaseChange, LakeLevel, Slumping, Karst, RoofCollapse,
    // Speleogenesis, Ecology, Gas, Agents, SurfaceWaves, barrier_commit,
    // recompute_mass_audit. Kernel bench above breaks out the biggest
    // chunks (thermal, slumping, lake_level, mass_audit).
    let _ = sub_costs;
}
