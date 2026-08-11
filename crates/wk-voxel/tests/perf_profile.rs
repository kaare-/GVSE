//! Headless perf profile for the wk-voxel demo / stress stack.
//!
//! Ignored by default. Run:
//!
//! ```text
//! cargo test -p wk-voxel --test perf_profile --release -- --ignored --nocapture
//! ```
//!
//! Matches `wk-voxel-app` frame order (rain → evap → humidity advect →
//! clouds → condensation → karst → physics tick → erosion → humidity /
//! temp cadence → phase → organisms). Physics sub-pass times come from
//! the real [`tick_with_perf_profiled`] path (not a post-hoc mirror).
//! Also prints a rayon on/off A/B on the demo world.

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_cold_avalanche, apply_condensation_rain_phased, apply_evaporation_into_humidity,
    apply_flow_erosion, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    find_plant_slot, humidity_diffuse_due, set_parallel_enabled, stamp_world,
    temperature_step_due, tick_with_perf, tick_with_perf_profiled, Blueprint, ClimateConfig,
    CloudConfig, CloudStore, CondensationConfig, EvapConfig, Genome, GrainConfig, Humidity,
    KarstConfig, OrganismStore, OrographicConfig, PerfConfig, PhaseConfig, PhysicsTimings,
    RainConfig, Temperature, Wind, World, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W,
    FLOW_SUBSTEPS,
};

const HUMIDITY_TILE_COLS: i32 = 4;
const HUMIDITY_DIFFUSION_ALPHA: f32 = 0.15;
const CLIMATE_WIND_VX: f32 = 0.05;
const WARMUP_TICKS: u64 = 40;
const MEASURE_TICKS: u64 = 200;
const PLANT_COUNT: usize = 48;

struct PassAccum {
    rain: Duration,
    evap: Duration,
    humidity_advect: Duration,
    clouds: Duration,
    condensation: Duration,
    karst: Duration,
    physics_tick: Duration,
    erosion: Duration,
    humidity_diffuse: Duration,
    humidity_diffuse_calls: u64,
    temperature: Duration,
    temperature_calls: u64,
    cold_avalanche: Duration,
    phase: Duration,
    organisms: Duration,
}

impl PassAccum {
    fn zero() -> Self {
        Self {
            rain: Duration::ZERO,
            evap: Duration::ZERO,
            humidity_advect: Duration::ZERO,
            clouds: Duration::ZERO,
            condensation: Duration::ZERO,
            karst: Duration::ZERO,
            physics_tick: Duration::ZERO,
            erosion: Duration::ZERO,
            humidity_diffuse: Duration::ZERO,
            humidity_diffuse_calls: 0,
            temperature: Duration::ZERO,
            temperature_calls: 0,
            cold_avalanche: Duration::ZERO,
            phase: Duration::ZERO,
            organisms: Duration::ZERO,
        }
    }

    fn total(&self) -> Duration {
        self.rain
            + self.evap
            + self.humidity_advect
            + self.clouds
            + self.condensation
            + self.karst
            + self.physics_tick
            + self.erosion
            + self.humidity_diffuse
            + self.temperature
            + self.cold_avalanche
            + self.phase
            + self.organisms
    }
}

struct Scene {
    world: World,
    params: WorldgenParams,
    humidity: Humidity,
    wind: Wind,
    clouds: CloudStore,
    temperature: Temperature,
    organisms: OrganismStore,
    rain: RainConfig,
    evap: EvapConfig,
    cond: CondensationConfig,
    karst: KarstConfig,
    grain: GrainConfig,
    cloud: CloudConfig,
    oro: OrographicConfig,
    phase: PhaseConfig,
    climate: ClimateConfig,
    perf: PerfConfig,
}

fn demo_params() -> WorldgenParams {
    WorldgenParams::default()
}

fn stress_params() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 32,
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 6,
        ..WorldgenParams::default()
    }
}

fn stamp_scene(params: WorldgenParams) -> Scene {
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
    let rain = RainConfig {
        top_y: params.sky_ceiling_y - 1,
        x_range: (0, params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
        // Open faucet keeps flow busy for timing; demo uses closed_loop.
        closed_loop: false,
        sea_level_y: params.sea_level_y,
        ..RainConfig::default()
    };
    let mut oro = OrographicConfig::default();
    oro.width_cols = params.width_cols;
    oro.sea_level_y = params.sea_level_y;
    Scene {
        world,
        params,
        humidity,
        wind,
        clouds: CloudStore::new(),
        temperature,
        organisms: OrganismStore::new(),
        rain,
        evap: EvapConfig::default(),
        cond: CondensationConfig {
            top_y: params.sky_ceiling_y - 2,
            ..CondensationConfig::default()
        },
        karst: KarstConfig::default(),
        grain: GrainConfig::default(),
        cloud: CloudConfig::default(),
        oro,
        phase: PhaseConfig::default(),
        climate: ClimateConfig::default(),
        perf: PerfConfig::default(),
    }
}

fn seed_plants(scene: &mut Scene, count: usize) {
    let body = Blueprint::minimal_plant().modules_relative_to_nucleus();
    let mut g = Genome::default();
    wk_voxel::sync_alloc_to_body(&mut g, &body);
    let w = scene.params.width_cols;
    let mut placed = 0usize;
    // Prefer coastal / mid-land columns so crowns are plantable.
    let start = (w as f32 * 0.35) as i32;
    let step = ((w as f32 * 0.45) / count.max(1) as f32).max(1.0) as i32;
    for i in 0..count * 3 {
        if placed >= count {
            break;
        }
        let gx = start + (i as i32) * step / 3;
        let guess_y = scene.params.sea_level_y + 2;
        let Some(gy) = find_plant_slot(&scene.world, gx, guess_y) else {
            continue;
        };
        if scene
            .organisms
            .spawn_blueprint(&scene.world, gx, gy, body.clone(), 70.0, g)
        {
            placed += 1;
        }
    }
    eprintln!("  seeded {placed}/{count} land plants");
}

fn cell_count(params: &WorldgenParams) -> i64 {
    let h = (params.sky_ceiling_y - params.bedrock_floor_y) as i64;
    params.width_cols as i64 * h
}

fn ms_per(d: Duration, n: u64) -> f32 {
    if n == 0 {
        return 0.0;
    }
    d.as_secs_f32() * 1000.0 / n as f32
}

fn profile_label(label: &str, params: &WorldgenParams, chunks: usize) {
    eprintln!(
        "=== {label} ===  seed={:#x}  {}×{} cells (~{})  chunks={}  wrap={}  warm={} measure={}",
        params.seed,
        params.width_cols,
        params.sky_ceiling_y - params.bedrock_floor_y,
        cell_count(params),
        chunks,
        params.wrap_x,
        WARMUP_TICKS,
        MEASURE_TICKS
    );
}

fn one_stack_tick(
    scene: &mut Scene,
    accum: Option<&mut PassAccum>,
    phys: Option<&mut PhysicsTimings>,
) {
    let tick_no = scene.world.tick;
    match accum {
        None => {
            // Match app: shell scans always parallel; CA follows PerfConfig.
            set_parallel_enabled(true);
            apply_rain_with_temp(
                &mut scene.world,
                &scene.rain,
                Some(&scene.temperature),
                Some(&scene.phase),
                Some(&mut scene.humidity),
            );
            apply_evaporation_into_humidity(&mut scene.world, &mut scene.humidity, &scene.evap);
            scene
                .humidity
                .advect(scene.wind.climate_vx, scene.wind.climate_vy);
            scene.clouds.step_with_precip(
                &mut scene.world,
                &mut scene.humidity,
                &scene.wind,
                scene.params.sea_level_y,
                scene.params.sky_ceiling_y,
                tick_no,
                &scene.cloud,
                Some(&scene.temperature),
                Some(&scene.phase),
            );
            apply_condensation_rain_phased(
                &mut scene.world,
                &mut scene.humidity,
                &scene.cond,
                Some(&scene.oro),
                Some(&scene.temperature),
                Some(&scene.phase),
            );
            apply_karst_dissolution(&mut scene.world, &scene.karst);
            set_parallel_enabled(scene.perf.parallel_physics);
            match phys {
                Some(t) => {
                    let _ = tick_with_perf_profiled(&mut scene.world, &scene.perf, t);
                }
                None => tick_with_perf(&mut scene.world, &scene.perf),
            }
            set_parallel_enabled(true);
            apply_flow_erosion(&mut scene.world, &scene.grain);
            if humidity_diffuse_due(scene.world.tick) {
                scene.humidity.diffuse(HUMIDITY_DIFFUSION_ALPHA);
            }
            if temperature_step_due(scene.world.tick) {
                let t = scene.world.tick;
                scene
                    .temperature
                    .step(Some(&scene.world), &scene.humidity, t);
            }
            if scene.phase.enabled
                && scene.phase.enable_cold_avalanche
                && scene.world.tick % scene.phase.period_ticks.max(1) == 0
            {
                apply_cold_avalanche(
                    &mut scene.world,
                    &scene.temperature,
                    scene.phase.freeze_point_c,
                );
            }
            apply_phase(&mut scene.world, &scene.temperature, &scene.phase);
            if !scene.organisms.is_empty() {
                let t = scene.world.tick;
                scene.organisms.step_with_climate(
                    &mut scene.world,
                    t,
                    &scene.climate,
                    Some(&mut scene.humidity),
                );
            }
        }
        Some(a) => {
            set_parallel_enabled(true);
            let t0 = Instant::now();
            apply_rain_with_temp(
                &mut scene.world,
                &scene.rain,
                Some(&scene.temperature),
                Some(&scene.phase),
                Some(&mut scene.humidity),
            );
            a.rain += t0.elapsed();

            let t0 = Instant::now();
            apply_evaporation_into_humidity(&mut scene.world, &mut scene.humidity, &scene.evap);
            a.evap += t0.elapsed();

            let t0 = Instant::now();
            scene
                .humidity
                .advect(scene.wind.climate_vx, scene.wind.climate_vy);
            a.humidity_advect += t0.elapsed();

            let t0 = Instant::now();
            scene.clouds.step_with_precip(
                &mut scene.world,
                &mut scene.humidity,
                &scene.wind,
                scene.params.sea_level_y,
                scene.params.sky_ceiling_y,
                tick_no,
                &scene.cloud,
                Some(&scene.temperature),
                Some(&scene.phase),
            );
            a.clouds += t0.elapsed();

            let t0 = Instant::now();
            apply_condensation_rain_phased(
                &mut scene.world,
                &mut scene.humidity,
                &scene.cond,
                Some(&scene.oro),
                Some(&scene.temperature),
                Some(&scene.phase),
            );
            a.condensation += t0.elapsed();

            let t0 = Instant::now();
            apply_karst_dissolution(&mut scene.world, &scene.karst);
            a.karst += t0.elapsed();

            set_parallel_enabled(scene.perf.parallel_physics);
            let t0 = Instant::now();
            match phys {
                Some(t) => {
                    let _ = tick_with_perf_profiled(&mut scene.world, &scene.perf, t);
                }
                None => tick_with_perf(&mut scene.world, &scene.perf),
            }
            a.physics_tick += t0.elapsed();

            set_parallel_enabled(true);
            let t0 = Instant::now();
            apply_flow_erosion(&mut scene.world, &scene.grain);
            a.erosion += t0.elapsed();

            if humidity_diffuse_due(scene.world.tick) {
                let t0 = Instant::now();
                scene.humidity.diffuse(HUMIDITY_DIFFUSION_ALPHA);
                a.humidity_diffuse += t0.elapsed();
                a.humidity_diffuse_calls += 1;
            }
            if temperature_step_due(scene.world.tick) {
                let t0 = Instant::now();
                let t = scene.world.tick;
                scene
                    .temperature
                    .step(Some(&scene.world), &scene.humidity, t);
                a.temperature += t0.elapsed();
                a.temperature_calls += 1;
            }
            if scene.phase.enabled
                && scene.phase.enable_cold_avalanche
                && scene.world.tick % scene.phase.period_ticks.max(1) == 0
            {
                let t0 = Instant::now();
                apply_cold_avalanche(
                    &mut scene.world,
                    &scene.temperature,
                    scene.phase.freeze_point_c,
                );
                a.cold_avalanche += t0.elapsed();
            }
            let t0 = Instant::now();
            apply_phase(&mut scene.world, &scene.temperature, &scene.phase);
            a.phase += t0.elapsed();

            if !scene.organisms.is_empty() {
                let t0 = Instant::now();
                let t = scene.world.tick;
                scene.organisms.step_with_climate(
                    &mut scene.world,
                    t,
                    &scene.climate,
                    Some(&mut scene.humidity),
                );
                a.organisms += t0.elapsed();
            }
        }
    }
}

fn print_pass_table(accum: &PassAccum, n: u64, wall: Duration) {
    eprintln!(
        "  wall                 {:>8.3} ms/tick  (total {:?})",
        ms_per(wall, n),
        wall
    );
    eprintln!(
        "  sum(passes)          {:>8.3} ms/tick",
        ms_per(accum.total(), n)
    );
    eprintln!("  ------------------------------------------------------------");
    eprintln!("  rain                 {:>8.3} ms/tick", ms_per(accum.rain, n));
    eprintln!("  evap→humidity        {:>8.3} ms/tick", ms_per(accum.evap, n));
    eprintln!(
        "  humidity.advect      {:>8.3} ms/tick",
        ms_per(accum.humidity_advect, n)
    );
    eprintln!("  clouds+precip        {:>8.3} ms/tick", ms_per(accum.clouds, n));
    eprintln!(
        "  condensation         {:>8.3} ms/tick",
        ms_per(accum.condensation, n)
    );
    eprintln!("  karst                {:>8.3} ms/tick", ms_per(accum.karst, n));
    eprintln!(
        "  physics tick         {:>8.3} ms/tick  (×{FLOW_SUBSTEPS} flow substeps + seepage/grain)",
        ms_per(accum.physics_tick, n)
    );
    eprintln!(
        "  flow erosion         {:>8.3} ms/tick",
        ms_per(accum.erosion, n)
    );
    eprintln!(
        "  humidity.diffuse     {:>8.3} ms/tick amortized  ({:.3} ms/call × {} calls)",
        ms_per(accum.humidity_diffuse, n),
        ms_per(accum.humidity_diffuse, accum.humidity_diffuse_calls.max(1)),
        accum.humidity_diffuse_calls
    );
    eprintln!(
        "  temperature.step     {:>8.3} ms/tick amortized  ({:.3} ms/call × {} calls)",
        ms_per(accum.temperature, n),
        ms_per(accum.temperature, accum.temperature_calls.max(1)),
        accum.temperature_calls
    );
    eprintln!(
        "  cold avalanche       {:>8.3} ms/tick",
        ms_per(accum.cold_avalanche, n)
    );
    eprintln!("  phase                {:>8.3} ms/tick", ms_per(accum.phase, n));
    eprintln!(
        "  organisms            {:>8.3} ms/tick",
        ms_per(accum.organisms, n)
    );
}

fn print_physics_table(phys: &PhysicsTimings, n: u64) {
    eprintln!("  --- physics sub-pass (in-tick, {n} measure ticks) ---");
    eprintln!(
        "  plan+clear dirty     {:>8.3} ms/tick",
        ms_per(phys.plan_clear, n)
    );
    eprintln!(
        "  gravity fall         {:>8.3} ms/tick",
        ms_per(phys.gravity, n)
    );
    eprintln!(
        "  water flow           {:>8.3} ms/tick",
        ms_per(phys.water_flow, n)
    );
    eprintln!("  seepage              {:>8.3} ms/tick", ms_per(phys.seepage, n));
    eprintln!(
        "  wake grains          {:>8.3} ms/tick",
        ms_per(phys.wake_grains, n)
    );
    eprintln!(
        "  settle grains        {:>8.3} ms/tick",
        ms_per(phys.settle, n)
    );
    eprintln!("  punch rafts          {:>8.3} ms/tick", ms_per(phys.punch, n));
    eprintln!(
        "  rise+soak            {:>8.3} ms/tick",
        ms_per(phys.rise_soak, n)
    );
    eprintln!("  failure              {:>8.3} ms/tick", ms_per(phys.failure, n));
    eprintln!(
        "  confined wake        {:>8.3} ms/tick",
        ms_per(phys.confined, n)
    );
    eprintln!(
        "  mycelium field       {:>8.3} ms/tick",
        ms_per(phys.mycelium, n)
    );
    eprintln!(
        "  sum(physics)         {:>8.3} ms/tick  (avg {:.2} flow substeps/tick)",
        ms_per(phys.total(), n),
        phys.substeps_ran as f32 / n as f32
    );
    if phys.substeps_ran > 0 {
        eprintln!(
            "  active plan          {:>8.1} regions/substep   {:>8.0} cells/substep",
            phys.active_regions as f32 / phys.substeps_ran as f32,
            phys.active_area as f32 / phys.substeps_ran as f32
        );
    }
    eprintln!(
        "  deep settle ticks    {:>8} / {n}   punch hits {:>8} / {n}",
        phys.deep_settle_ticks, phys.punch_hits
    );
}

fn run_profile(label: &str, params: WorldgenParams, with_plants: bool) {
    let mut scene = stamp_scene(params);
    if with_plants {
        seed_plants(&mut scene, PLANT_COUNT);
    }
    let chunks = scene.world.chunks.len();
    profile_label(label, &scene.params, chunks);

    for _ in 0..WARMUP_TICKS {
        one_stack_tick(&mut scene, None, None);
    }

    let mut accum = PassAccum::zero();
    let mut phys = PhysicsTimings::default();
    let wall = Instant::now();
    for _ in 0..MEASURE_TICKS {
        one_stack_tick(&mut scene, Some(&mut accum), Some(&mut phys));
    }
    let wall = wall.elapsed();
    print_pass_table(&accum, MEASURE_TICKS, wall);
    print_physics_table(&phys, MEASURE_TICKS);

    let cap = scene
        .humidity
        .bounds
        .map(|b| b.tile_capacity())
        .unwrap_or(0);
    eprintln!(
        "  humidity tiles={}/{}  humidity_mass={:.1}  organisms={}",
        scene.humidity.cells.len(),
        cap,
        scene.humidity.total_mass(),
        scene.organisms.len()
    );
    eprintln!();
}

fn run_perf_knob_ab(params: WorldgenParams) {
    eprintln!("=== PerfConfig A/B (demo stack, no plants) ===");
    let variants: [(&str, PerfConfig); 4] = [
        ("defaults (FPS, par OFF)", PerfConfig::default()),
        ("full_feel (×12, par OFF)", PerfConfig::full_feel()),
        (
            "defaults + parallel ON",
            PerfConfig {
                parallel_physics: true,
                ..PerfConfig::default()
            },
        ),
        (
            "full_feel + parallel ON",
            PerfConfig {
                parallel_physics: true,
                ..PerfConfig::full_feel()
            },
        ),
    ];
    for (label, perf) in variants {
        let mut scene = stamp_scene(params);
        scene.perf = perf;
        for _ in 0..WARMUP_TICKS {
            one_stack_tick(&mut scene, None, None);
        }
        let mut accum = PassAccum::zero();
        let wall = Instant::now();
        for _ in 0..MEASURE_TICKS {
            one_stack_tick(&mut scene, Some(&mut accum), None);
        }
        let wall = wall.elapsed();
        eprintln!(
            "  {label:22}  wall {:>7.3} ms/tick   physics {:>7.3} ms/tick",
            ms_per(wall, MEASURE_TICKS),
            ms_per(accum.physics_tick, MEASURE_TICKS)
        );
    }
    set_parallel_enabled(true);
    eprintln!();
}

#[test]
#[ignore]
fn perf_profile_demo_and_stress() {
    set_parallel_enabled(true);
    run_profile("demo (WorldgenParams::default)", demo_params(), false);
    run_profile(
        "demo + 48 plants",
        demo_params(),
        true,
    );
    run_perf_knob_ab(demo_params());
    run_profile("stress (32×6 chunks)", stress_params(), false);
}
