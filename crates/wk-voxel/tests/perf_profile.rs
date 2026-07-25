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
//! temp cadence → phase → organisms). Also prints a physics sub-pass
//! breakdown and a rayon on/off A/B on the demo world.

use std::time::{Duration, Instant};

use wk_material::MaterialId;
use wk_voxel::{
    apply_cold_avalanche, apply_condensation_rain_phased, apply_evaporation_into_humidity,
    apply_failure, apply_flow_erosion, apply_grain_fall_regions, apply_grain_repose_regions,
    apply_gravity_fall_regions, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    apply_seepage_regions, apply_water_flow_regions, clear_all_dirty, find_plant_slot,
    humidity_diffuse_due, partition_checkerboard, plan_active, set_parallel_enabled,
    stamp_world, temperature_step_due, tick_with_perf, Blueprint, Cell, ClimateConfig,
    CloudConfig, CloudStore, CondensationConfig, EvapConfig, FailureConfig, Genome, GrainConfig,
    Humidity, KarstConfig, OrganismStore, OrographicConfig, PerfConfig, PhaseConfig, RainConfig,
    Temperature, Wind, World, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W, FLOW_QUIET_AREA,
    FLOW_SUBSTEPS, FLOW_SUBSTEPS_MIN, MAX_ATOMS,
};

const HUMIDITY_TILE_COLS: i32 = 4;
const HUMIDITY_DIFFUSION_ALPHA: f32 = 0.15;
const CLIMATE_WIND_VX: f32 = 0.05;
const WARMUP_TICKS: u64 = 40;
const MEASURE_TICKS: u64 = 200;
const PHYSICS_BREAKDOWN_TICKS: u64 = 80;
const PLANT_COUNT: usize = 48;
/// Busy-play plant target (store cap raised to fit).
const BUSY_PLANT_COUNT: usize = 180;
/// Terrain dig/place columns touched per tick (editor-like dirty churn).
const TERRAIN_EDIT_COLS: i32 = 24;

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

struct PhysicsAccum {
    plan_clear: Duration,
    gravity: Duration,
    water_flow: Duration,
    seepage: Duration,
    grain_fall: Duration,
    grain_repose: Duration,
    failure: Duration,
    substeps_ran: u64,
    active_regions: u64,
    active_area: u64,
}

impl PhysicsAccum {
    fn zero() -> Self {
        Self {
            plan_clear: Duration::ZERO,
            gravity: Duration::ZERO,
            water_flow: Duration::ZERO,
            seepage: Duration::ZERO,
            grain_fall: Duration::ZERO,
            grain_repose: Duration::ZERO,
            failure: Duration::ZERO,
            substeps_ran: 0,
            active_regions: 0,
            active_area: 0,
        }
    }

    fn total(&self) -> Duration {
        self.plan_clear
            + self.gravity
            + self.water_flow
            + self.seepage
            + self.grain_fall
            + self.grain_repose
            + self.failure
    }
}

fn region_area(active: &[wk_voxel::ActiveChunk]) -> usize {
    active
        .iter()
        .map(|ac| {
            let w = (ac.rect.x1 as usize).saturating_sub(ac.rect.x0 as usize) + 1;
            let h = (ac.rect.y1 as usize).saturating_sub(ac.rect.y0 as usize) + 1;
            w.saturating_mul(h)
        })
        .sum()
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

/// Larger than Tab's common "wide" draft — 48×6 chunks (~1.2M cells).
fn busy_params() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 48,
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
    if count > scene.organisms.max_atoms {
        scene.organisms.max_atoms = count.max(MAX_ATOMS);
    }
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
    eprintln!("  seeded {placed}/{count} land plants (cap={})", scene.organisms.max_atoms);
}

/// Mimic F3 terrain editing: dig a moving strip of hillside and refill
/// with sand so dirty rects stay hot across many chunks.
fn terrain_edit_churn(world: &mut World, params: &WorldgenParams, tick: u64) {
    let w = params.width_cols;
    let base = ((tick as i32 * 3) % w.max(1)).rem_euclid(w.max(1));
    let y0 = params.sea_level_y + 4;
    for i in 0..TERRAIN_EDIT_COLS {
        let gx = (base + i * 5).rem_euclid(w.max(1));
        // Dig a short chimney, then dump sand so grain/repose stay busy.
        for dy in 0..6 {
            let gy = y0 + dy;
            if let Some(c) = world.get_cell(gx, gy) {
                if c.material != MaterialId::Air && c.material != MaterialId::Bedrock {
                    world.set_cell(gx, gy, Cell::air());
                }
            }
        }
        world.set_cell(gx, y0, Cell::solid(MaterialId::Sand));
        world.set_cell(gx, y0 + 1, Cell::solid(MaterialId::Sand));
    }
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

fn one_stack_tick(scene: &mut Scene, accum: Option<&mut PassAccum>, terrain_edits: bool) {
    let tick_no = scene.world.tick;
    if terrain_edits {
        terrain_edit_churn(&mut scene.world, &scene.params, tick_no);
    }
    match accum {
        None => {
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
            tick_with_perf(&mut scene.world, &scene.perf);
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
            if scene.phase.enabled && scene.phase.enable_cold_avalanche {
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

            let t0 = Instant::now();
            tick_with_perf(&mut scene.world, &scene.perf);
            a.physics_tick += t0.elapsed();

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
            if scene.phase.enabled && scene.phase.enable_cold_avalanche {
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

/// Timed mirror of [`tick_with_perf`] — keep in sync with `rules`.
fn timed_physics_tick(world: &mut World, perf: &PerfConfig, a: &mut PhysicsAccum) {
    set_parallel_enabled(perf.parallel_physics);
    for step in 0..FLOW_SUBSTEPS {
        let t0 = Instant::now();
        let active = plan_active(world);
        clear_all_dirty(world);
        a.plan_clear += t0.elapsed();
        if active.is_empty() {
            break;
        }
        a.substeps_ran += 1;
        a.active_regions += active.len() as u64;
        a.active_area += region_area(&active) as u64;

        let passes = partition_checkerboard(&active);
        let t0 = Instant::now();
        for pass in &passes {
            apply_gravity_fall_regions(world, pass);
        }
        a.gravity += t0.elapsed();

        // Match production: even substeps when every-other is on.
        let run_flow = !perf.flow_every_other_substep || (step % 2 == 0);
        if run_flow {
            let t0 = Instant::now();
            apply_water_flow_regions(world, &active);
            a.water_flow += t0.elapsed();
        }

        if perf.flow_quiet_early_out && step + 1 >= FLOW_SUBSTEPS_MIN {
            let next = plan_active(world);
            let area = region_area(&next);
            if next.is_empty() || area <= FLOW_QUIET_AREA {
                break;
            }
        }
    }

    let t0 = Instant::now();
    let active = plan_active(world);
    a.plan_clear += t0.elapsed();
    if !active.is_empty() {
        let t0 = Instant::now();
        apply_seepage_regions(world, &active);
        a.seepage += t0.elapsed();

        let passes = partition_checkerboard(&active);
        let t0 = Instant::now();
        for pass in &passes {
            apply_grain_fall_regions(world, pass);
        }
        a.grain_fall += t0.elapsed();

        let t0 = Instant::now();
        let repose_active = plan_active(world);
        a.plan_clear += t0.elapsed();
        if !repose_active.is_empty() {
            let repose_passes = partition_checkerboard(&repose_active);
            let t0 = Instant::now();
            for pass in &repose_passes {
                apply_grain_repose_regions(world, pass);
            }
            a.grain_repose += t0.elapsed();
        }
    }

    let t0 = Instant::now();
    apply_failure(world, &FailureConfig::default(), None);
    a.failure += t0.elapsed();

    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
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

fn print_physics_table(phys: &PhysicsAccum, n: u64) {
    eprintln!("  --- physics sub-pass (timed mirror of tick, {n} ticks) ---");
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
        "  grain fall           {:>8.3} ms/tick",
        ms_per(phys.grain_fall, n)
    );
    eprintln!(
        "  grain repose         {:>8.3} ms/tick",
        ms_per(phys.grain_repose, n)
    );
    eprintln!(
        "  failure (geotech)    {:>8.3} ms/tick",
        ms_per(phys.failure, n)
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
}

fn run_profile(
    label: &str,
    params: WorldgenParams,
    plant_count: Option<usize>,
    terrain_edits: bool,
) {
    let mut scene = stamp_scene(params);
    if let Some(n) = plant_count {
        seed_plants(&mut scene, n);
    }
    let chunks = scene.world.chunks.len();
    profile_label(label, &scene.params, chunks);
    if terrain_edits {
        eprintln!("  terrain-edit churn: {TERRAIN_EDIT_COLS} cols/tick");
    }

    for _ in 0..WARMUP_TICKS {
        one_stack_tick(&mut scene, None, terrain_edits);
    }

    let mut accum = PassAccum::zero();
    let wall = Instant::now();
    for _ in 0..MEASURE_TICKS {
        one_stack_tick(&mut scene, Some(&mut accum), terrain_edits);
    }
    let wall = wall.elapsed();
    print_pass_table(&accum, MEASURE_TICKS, wall);
    let budget_24hz = 1000.0 / 24.0;
    eprintln!(
        "  vs 24Hz budget       {:>8.1} ms/tick  ({:.0}% of frame)",
        budget_24hz,
        ms_per(wall, MEASURE_TICKS) / budget_24hz * 100.0
    );

    // Fresh physics breakdown on the already-warmed world (rain keeps
    // water active so flow stays representative).
    let mut phys = PhysicsAccum::zero();
    for _ in 0..PHYSICS_BREAKDOWN_TICKS {
        if terrain_edits {
            let tick = scene.world.tick;
            terrain_edit_churn(&mut scene.world, &scene.params, tick);
        }
        apply_rain_with_temp(
            &mut scene.world,
            &scene.rain,
            Some(&scene.temperature),
            Some(&scene.phase),
            Some(&mut scene.humidity),
        );
        timed_physics_tick(&mut scene.world, &scene.perf, &mut phys);
    }
    print_physics_table(&phys, PHYSICS_BREAKDOWN_TICKS);

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
    let variants: [(&str, PerfConfig); 3] = [
        ("defaults (full flow)", PerfConfig::default()),
        (
            "every-other flow",
            PerfConfig {
                flow_every_other_substep: true,
                ..PerfConfig::default()
            },
        ),
        (
            "parallel OFF",
            PerfConfig {
                parallel_physics: false,
                ..PerfConfig::default()
            },
        ),
    ];
    for (label, perf) in variants {
        let mut scene = stamp_scene(params);
        scene.perf = perf;
        for _ in 0..WARMUP_TICKS {
            one_stack_tick(&mut scene, None, false);
        }
        let mut accum = PassAccum::zero();
        let wall = Instant::now();
        for _ in 0..MEASURE_TICKS {
            one_stack_tick(&mut scene, Some(&mut accum), false);
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
    run_profile(
        "demo (WorldgenParams::default)",
        demo_params(),
        None,
        false,
    );
    run_profile("demo + 48 plants", demo_params(), Some(PLANT_COUNT), false);
    run_perf_knob_ab(demo_params());
    run_profile("stress (32×6 chunks)", stress_params(), None, false);
    run_profile(
        "busy play (48×6 chunks + plants + terrain edits)",
        busy_params(),
        Some(BUSY_PLANT_COUNT),
        true,
    );
}
