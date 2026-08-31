//! Headless perf profile for the wk-voxel demo / stress stack.
//!
//! Ignored by default. Run:
//!
//! ```text
//! cargo test -p wk-voxel --test perf_profile --release -- --ignored --nocapture
//! ```
//!
//! Matches `wk-voxel-app` frame order (rain → evap → humidity advect →
//! clouds → condensation → karst → physics tick → snow drift →
//! suspension → erosion → humidity / temp cadence → phase → organisms).
//! Physics sub-pass times come from the real [`tick_with_perf_profiled`]
//! path (not a post-hoc mirror). Also prints a rayon on/off A/B on the
//! demo world.
//!
//! Snow drift and clay suspension live *outside* `tick_with_perf` in the
//! app. A profile that skipped them was not measuring the frame.

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_cold_avalanche, apply_condensation_rain_phased, apply_evaporation_into_humidity,
    apply_flow_erosion, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    apply_snow_wind_drift,
    find_plant_slot, humidity_diffuse_due, set_parallel_enabled, stamp_world,
    temperature_step_due, tick_with_perf, tick_with_perf_profiled, Blueprint, ClimateConfig,
    CloudConfig, CloudStore, CondensationConfig, EvapConfig, Genome, GrainConfig, Humidity,
    KarstConfig, OrganismPassTimings, OrganismStore, OrographicConfig, PerfConfig, PhaseConfig,
    PhysicsTimings, RainConfig, Temperature, Wind, World, WorldgenParams, CHUNK_CELLS_H,
    CHUNK_CELLS_W, FLOW_SUBSTEPS,
};

const HUMIDITY_TILE_COLS: i32 = 4;
const HUMIDITY_DIFFUSION_ALPHA: f32 = 0.15;
const CLIMATE_WIND_VX: f32 = 0.05;
const WARMUP_TICKS: u64 = 40;
const MEASURE_TICKS: u64 = 200;
const PLANT_COUNT: usize = 48;
/// Creature-count sweep for pop-cap pressure (includes [`wk_voxel::MAX_ATOMS`]).
const CREATURE_SWEEP: &[usize] = &[0, 48, 128, 256];

struct PassAccum {
    rain: Duration,
    evap: Duration,
    humidity_advect: Duration,
    clouds: Duration,
    condensation: Duration,
    karst: Duration,
    physics_tick: Duration,
    snow_drift: Duration,
    suspension: Duration,
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
            snow_drift: Duration::ZERO,
            suspension: Duration::ZERO,
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
            + self.snow_drift
            + self.suspension
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
    /// Match the app's W toggle. Nightly soak leaves climatic rain off
    /// and lets drizzle / evap cycle the water.
    climatic_rain: bool,
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
        climatic_rain: true,
    }
}

fn seed_plants(scene: &mut Scene, count: usize) {
    if count == 0 {
        eprintln!("  seeded 0/0 land plants");
        return;
    }
    scene.organisms.max_atoms = count.max(scene.organisms.atom_cap()).max(1);
    let body = Blueprint::minimal_plant().modules_relative_to_nucleus();
    let mut g = Genome::default();
    wk_voxel::sync_alloc_to_body(&mut g, &body);
    let w = scene.params.width_cols.max(1);
    let mut placed = 0usize;
    // Sweep the full ring with a prime-ish stride so dense caps still seat.
    let stride = ((w as usize).max(1) / count.max(1)).max(1);
    let mut attempts = 0usize;
    let max_attempts = count.saturating_mul(8).max(w as usize * 2);
    while placed < count && attempts < max_attempts {
        let gx = ((attempts * stride) as i32 + (attempts as i32 / 3)) % w;
        attempts += 1;
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
    eprintln!(
        "  seeded {placed}/{count} land plants  (cap={}  attempts={attempts})",
        scene.organisms.atom_cap()
    );
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
            if scene.climatic_rain {
                apply_rain_with_temp(
                    &mut scene.world,
                    &scene.rain,
                    Some(&scene.temperature),
                    Some(&scene.phase),
                    Some(&mut scene.humidity),
                );
            }
            apply_evaporation_into_humidity(&mut scene.world, &mut scene.humidity, &scene.evap);
            scene
                .humidity
                .advect_with_surface(
                    scene.wind.climate_vx,
                    scene.wind.climate_vy,
                    &scene.wind,
                    &scene.world,
                );
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
            apply_snow_wind_drift(&mut scene.world, CLIMATE_WIND_VX, HUMIDITY_TILE_COLS);
            apply_flow_erosion(&mut scene.world, &scene.grain);
            wk_voxel::sediment::apply_suspension(&mut scene.world);
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
            if scene.climatic_rain {
                apply_rain_with_temp(
                    &mut scene.world,
                    &scene.rain,
                    Some(&scene.temperature),
                    Some(&scene.phase),
                    Some(&mut scene.humidity),
                );
            }
            a.rain += t0.elapsed();

            let t0 = Instant::now();
            apply_evaporation_into_humidity(&mut scene.world, &mut scene.humidity, &scene.evap);
            a.evap += t0.elapsed();

            let t0 = Instant::now();
            scene
                .humidity
                .advect_with_surface(
                    scene.wind.climate_vx,
                    scene.wind.climate_vy,
                    &scene.wind,
                    &scene.world,
                );
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
            apply_snow_wind_drift(&mut scene.world, CLIMATE_WIND_VX, HUMIDITY_TILE_COLS);
            a.snow_drift += t0.elapsed();

            let t0 = Instant::now();
            apply_flow_erosion(&mut scene.world, &scene.grain);
            a.erosion += t0.elapsed();

            let t0 = Instant::now();
            wk_voxel::sediment::apply_suspension(&mut scene.world);
            a.suspension += t0.elapsed();

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
        "  snow drift           {:>8.3} ms/tick",
        ms_per(accum.snow_drift, n)
    );
    eprintln!(
        "  flow erosion         {:>8.3} ms/tick",
        ms_per(accum.erosion, n)
    );
    eprintln!(
        "  suspension           {:>8.3} ms/tick",
        ms_per(accum.suspension, n)
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
    eprintln!(
        "  rock bodies          {:>8.3} ms/tick",
        ms_per(phys.bodies, n)
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

fn run_profile(label: &str, params: WorldgenParams, plant_count: usize) -> f32 {
    let mut scene = stamp_scene(params);
    seed_plants(&mut scene, plant_count);
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
    let wall_ms = ms_per(wall, MEASURE_TICKS);
    let org_ms = ms_per(accum.organisms, MEASURE_TICKS);
    eprintln!(
        "  humidity tiles={}/{}  humidity_mass={:.1}  organisms={}  org_share={:.0}%",
        scene.humidity.cells.len(),
        cap,
        scene.humidity.total_mass(),
        scene.organisms.len(),
        if wall_ms > 0.0 {
            100.0 * org_ms / wall_ms
        } else {
            0.0
        }
    );
    eprintln!();
    org_ms
}

/// Demo world, same stack, plant count 0 → max. Prints a compact summary table.
fn run_creature_count_sweep(params: WorldgenParams) {
    eprintln!("=== creature-count sweep (demo world, FPS PerfConfig) ===");
    eprintln!("  count   wall_ms  org_ms   org%   living");
    for &n in CREATURE_SWEEP {
        let mut scene = stamp_scene(params);
        seed_plants(&mut scene, n);
        for _ in 0..WARMUP_TICKS {
            one_stack_tick(&mut scene, None, None);
        }
        let mut accum = PassAccum::zero();
        let mut pass = OrganismPassTimings::default();
        let wall = Instant::now();
        for _ in 0..MEASURE_TICKS {
            one_stack_tick(&mut scene, Some(&mut accum), None);
            let p = scene.organisms.last_pass;
            pass.pose += p.pose;
            pass.canopy += p.canopy;
            pass.reseat += p.reseat;
            pass.float_cols += p.float_cols;
            pass.land_plants += p.land_plants;
            pass.land_seat += p.land_seat;
            pass.land_metab += p.land_metab;
            pass.land_grow += p.land_grow;
            pass.land_disperse += p.land_disperse;
            pass.other_creatures += p.other_creatures;
            pass.post += p.post;
        }
        let wall = wall.elapsed();
        let wall_ms = ms_per(wall, MEASURE_TICKS);
        let org_ms = ms_per(accum.organisms, MEASURE_TICKS);
        let living = scene.organisms.len();
        let share = if wall_ms > 0.0 {
            100.0 * org_ms / wall_ms
        } else {
            0.0
        };
        eprintln!(
            "  {n:>5}  {wall_ms:>7.3}  {org_ms:>7.3}  {share:>5.1}%  {living}"
        );
        if n > 0 {
            let n_ticks = MEASURE_TICKS;
            eprintln!(
                "         org splits ms/tick: pose {:.3}  canopy {:.3}  reseat {:.3}  float {:.3}  land {:.3}  other {:.3}  post {:.3}",
                ms_per(pass.pose, n_ticks),
                ms_per(pass.canopy, n_ticks),
                ms_per(pass.reseat, n_ticks),
                ms_per(pass.float_cols, n_ticks),
                ms_per(pass.land_plants, n_ticks),
                ms_per(pass.other_creatures, n_ticks),
                ms_per(pass.post, n_ticks),
            );
            eprintln!(
                "         land splits ms/tick: seat {:.3}  metab {:.3}  grow {:.3}  disperse {:.3}",
                ms_per(pass.land_seat, n_ticks),
                ms_per(pass.land_metab, n_ticks),
                ms_per(pass.land_grow, n_ticks),
                ms_per(pass.land_disperse, n_ticks),
            );
        }
    }
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
    run_creature_count_sweep(demo_params());
    run_profile("demo (0 plants)", demo_params(), 0);
    run_profile(
        &format!("demo + {PLANT_COUNT} plants"),
        demo_params(),
        PLANT_COUNT,
    );
    run_profile(
        &format!("demo + {} plants (MAX_ATOMS)", wk_voxel::MAX_ATOMS),
        demo_params(),
        wk_voxel::MAX_ATOMS,
    );
    run_perf_knob_ab(demo_params());
    run_profile("stress (32×6 chunks, 0 plants)", stress_params(), 0);
    run_profile(
        &format!("stress + {} plants", wk_voxel::MAX_ATOMS),
        stress_params(),
        wk_voxel::MAX_ATOMS,
    );
}

/// Does organism cost grow with soak age at a *constant* population?
///
/// Playtest FPS fell 33 → 3 over 121k ticks with 256 plants. Physics was ruled
/// out — `soak_drift_probe` shows physics cost *falling* as a world settles, and
/// suspension at 0.042 ms with a bounded map. Organisms are what that probe does
/// not have.
///
/// Population size alone would not explain it: plants settled well under the old
/// module caps, so nothing grew monstrous. Cost rising while `atoms` stays flat
/// would mean **churn** — work per organism increasing over time — which is a bug
/// rather than a consequence of lifting the caps.
///
/// ```text
/// cargo test -p wk-voxel --release --test perf_profile -- --ignored --nocapture organism_cost_versus_soak_age
/// ```
#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn organism_cost_versus_soak_age() {
    const SEG: u64 = 1000;
    const SEGS: usize = 14;

    let mut scene = stamp_scene(demo_params());
    seed_plants(&mut scene, 256);
    for _ in 0..WARMUP_TICKS {
        one_stack_tick(&mut scene, None, None);
    }

    println!(
        "\n{:>8}  {:>10}  {:>11}  {:>7}  {:>9}",
        "tick", "wall", "organisms", "atoms", "per atom"
    );
    for _ in 0..SEGS {
        let mut accum = PassAccum::zero();
        let mut phys = PhysicsTimings::default();
        let wall = Instant::now();
        for _ in 0..SEG {
            one_stack_tick(&mut scene, Some(&mut accum), Some(&mut phys));
        }
        let wall = wall.elapsed();
        let atoms = scene.organisms.len().max(1);
        let org_ms = accum.organisms.as_secs_f32() * 1000.0 / SEG as f32;
        println!(
            "{:>8}  {:>8.3}ms  {:>9.3}ms  {:>7}  {:>7.4}ms",
            scene.world.tick,
            wall.as_secs_f32() * 1000.0 / SEG as f32,
            org_ms,
            atoms,
            org_ms / atoms as f32,
        );
    }
    println!(
        "\n  Flat 'per atom' means cost tracks population (expected).\n  \
         Rising 'per atom' means churn — work per organism growing with age."
    );
}

/// Nightly soak shape: climatic rain off, drizzle + evap on, a full plant
/// cap. Prints pass cost next to the maps and sticky flags that can only
/// grow. A rising `snow` / `buoy` chunk count with a quiet scene is the
/// occupancy leak; rising `mods` with a flat atom count is plant growth;
/// rising `hum n` toward tile capacity is a filled atmosphere, not a leak.
///
/// ```text
/// cargo test -p wk-voxel --release --test perf_profile -- --ignored --nocapture soak_age_inventory
/// ```
#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn soak_age_inventory() {
    const SEG: u64 = 400;
    const SEGS: usize = 8;

    let mut scene = stamp_scene(demo_params());
    scene.climatic_rain = false;
    seed_plants(&mut scene, 256);
    for _ in 0..WARMUP_TICKS {
        one_stack_tick(&mut scene, None, None);
    }

    println!(
        "\n{:>7} {:>7} {:>7} {:>7} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>6} {:>5} {:>5} {:>5} {:>5}",
        "tick", "wall", "phys", "grav", "flow", "seep", "conf", "body", "org",
        "cond", "halo", "diss", "hum n", "buoy", "orgc", "pores", "mods"
    );
    for _ in 0..SEGS {
        let mut accum = PassAccum::zero();
        let mut phys = PhysicsTimings::default();
        let wall = Instant::now();
        for _ in 0..SEG {
            one_stack_tick(&mut scene, Some(&mut accum), Some(&mut phys));
        }
        let wall = wall.elapsed();
        let mut buoy_ch = 0usize;
        let mut org_ch = 0usize;
        let mut pore_ch = 0usize;
        for c in scene.world.chunks.values() {
            if c.has_buoyant {
                buoy_ch += 1;
            }
            if c.has_organic {
                org_ch += 1;
            }
            if c.has_wet_pores {
                pore_ch += 1;
            }
        }
        let halo = if phys.substeps_ran > 0 {
            phys.active_area as f32 / phys.substeps_ran as f32
        } else {
            0.0
        };
        let mods: usize = scene.organisms.atoms.iter().map(|a| a.body.len()).sum();
        let hum_cap = scene
            .humidity
            .bounds
            .map(|b| b.tile_capacity())
            .unwrap_or(0);
        println!(
            "{:>7} {:>6.2} {:>6.2} {:>6.2} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>5.2} {:>6.0} {:>5} {:>5}/{:<4} {:>5} {:>5} {:>5} {:>6}",
            scene.world.tick,
            wall.as_secs_f32() * 1000.0 / SEG as f32,
            accum.physics_tick.as_secs_f32() * 1000.0 / SEG as f32,
            phys.gravity.as_secs_f32() * 1000.0 / SEG as f32,
            phys.water_flow.as_secs_f32() * 1000.0 / SEG as f32,
            phys.seepage.as_secs_f32() * 1000.0 / SEG as f32,
            phys.confined.as_secs_f32() * 1000.0 / SEG as f32,
            phys.bodies.as_secs_f32() * 1000.0 / SEG as f32,
            accum.organisms.as_secs_f32() * 1000.0 / SEG as f32,
            accum.condensation.as_secs_f32() * 1000.0 / SEG as f32,
            halo,
            scene.world.dissolved.len(),
            scene.humidity.cells.len(),
            hum_cap,
            buoy_ch,
            org_ch,
            pore_ch,
            mods,
        );
    }
}
