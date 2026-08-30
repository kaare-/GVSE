//! One-shot expert snapshot: closed-loop demo stack + A/B knobs.
//!
//! ```text
//! cargo test -p wk-voxel --test expert_perf_snapshot --release -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_cold_avalanche, apply_condensation_rain_phased, apply_evaporation_into_humidity,
    apply_flow_erosion, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    find_plant_slot, humidity_diffuse_alpha_per_tick, set_parallel_enabled, stamp_world,
    temperature_step_due, tick_with_perf, Blueprint, ClimateConfig, CloudConfig, CloudStore,
    CondensationConfig, EvapConfig, Genome, GrainConfig, Humidity, KarstConfig, OrganismStore,
    OrographicConfig, PerfConfig, PhaseConfig, RainConfig, Temperature, Wind, World,
    WorldgenParams,
};

const WARM: u64 = 25;
const MEAS: u64 = 100;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
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

fn stamp(params: WorldgenParams, plants: bool, closed_loop: bool) -> Scene {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let mut humidity = Humidity::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
    );
    humidity.wrap_x = params.wrap_x;
    let wind = Wind::climate(
        4, 0.05, params.seed, params.width_cols, params.sea_level_y,
        params.bedrock_floor_y, params.sky_ceiling_y, params.wrap_x,
    );
    let temperature = Temperature::with_world_bounds(
        4, 0, params.bedrock_floor_y, params.width_cols, params.sky_ceiling_y,
        params.seed, params.width_cols, params.sea_level_y, params.wrap_x,
    );
    let rain = RainConfig {
        top_y: params.sky_ceiling_y - 1,
        x_range: (0, params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
        closed_loop,
        sea_level_y: params.sea_level_y,
        ..RainConfig::default()
    };
    let mut oro = OrographicConfig::default();
    oro.width_cols = params.width_cols;
    oro.sea_level_y = params.sea_level_y;
    let mut scene = Scene {
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
    };
    if plants {
        let body = Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut g = Genome::default();
        wk_voxel::sync_alloc_to_body(&mut g, &body);
        let w = scene.params.width_cols;
        let start = (w as f32 * 0.35) as i32;
        let step = ((w as f32 * 0.45) / 48.0).max(1.0) as i32;
        let mut placed = 0;
        for i in 0..48 * 3 {
            if placed >= 48 {
                break;
            }
            let gx = start + (i as i32) * step / 3;
            if let Some(gy) = find_plant_slot(&scene.world, gx, scene.params.sea_level_y + 2) {
                if scene
                    .organisms
                    .spawn_blueprint(&scene.world, gx, gy, body.clone(), 70.0, g)
                {
                    placed += 1;
                }
            }
        }
        eprintln!("  seeded {placed} plants");
    }
    scene
}

fn stack_tick(s: &mut Scene) -> (Duration, Duration, Duration) {
    let t_all = Instant::now();
    apply_rain_with_temp(
        &mut s.world, &s.rain, Some(&s.temperature), Some(&s.phase), Some(&mut s.humidity),
    );
    apply_evaporation_into_humidity(&mut s.world, &mut s.humidity, &s.evap);
    s.humidity.advect(s.wind.climate_vx, s.wind.climate_vy);
    let tick_no = s.world.tick;
    s.clouds.step_with_precip(
        &mut s.world, &mut s.humidity, &s.wind, s.params.sea_level_y,
        s.params.sky_ceiling_y, tick_no, &s.cloud, Some(&s.temperature), Some(&s.phase),
    );
    apply_condensation_rain_phased(
        &mut s.world, &mut s.humidity, &s.cond, Some(&s.oro),
        Some(&s.temperature), Some(&s.phase),
    );
    apply_karst_dissolution(&mut s.world, &s.karst);
    let t_phys = Instant::now();
    tick_with_perf(&mut s.world, &s.perf);
    let phys = t_phys.elapsed();
    apply_flow_erosion(&mut s.world, &s.grain);
    s.humidity.diffuse(humidity_diffuse_alpha_per_tick(0.15));
    if temperature_step_due(s.world.tick) {
        let t = s.world.tick;
        s.temperature.step(Some(&s.world), &s.humidity, t);
    }
    if s.phase.enabled
        && s.phase.enable_cold_avalanche
        && s.world.tick % s.phase.period_ticks.max(1) == 0
    {
        apply_cold_avalanche(&mut s.world, &s.temperature, s.phase.freeze_point_c);
    }
    apply_phase(&mut s.world, &s.temperature, &s.phase);
    let t_org = Instant::now();
    if !s.organisms.is_empty() {
        let t = s.world.tick;
        s.organisms.step_with_climate(&mut s.world, t, &s.climate, Some(&mut s.humidity));
    }
    let org = t_org.elapsed();
    (t_all.elapsed(), phys, org)
}

fn run_variant(label: &str, closed_loop: bool, plants: bool, perf: PerfConfig) {
    set_parallel_enabled(perf.parallel_physics);
    let mut s = stamp(WorldgenParams::default(), plants, closed_loop);
    s.perf = perf;
    for _ in 0..WARM {
        let _ = stack_tick(&mut s);
    }
    let mut wall = Duration::ZERO;
    let mut phys = Duration::ZERO;
    let mut org = Duration::ZERO;
    for _ in 0..MEAS {
        let (w, p, o) = stack_tick(&mut s);
        wall += w;
        phys += p;
        org += o;
    }
    eprintln!(
        "  {label:28}  wall {:>7.2}  phys {:>7.2}  org {:>6.3}  ms/tick  (~{:.0} sim-FPS)",
        ms(wall, MEAS),
        ms(phys, MEAS),
        ms(org, MEAS),
        1000.0 / ms(wall, MEAS).max(0.001)
    );
}

#[test]
#[ignore]
fn expert_perf_snapshot() {
    eprintln!("=== Expert perf snapshot (demo world, warm={WARM} meas={MEAS}) ===");
    run_variant(
        "demo closed_loop +plants",
        true,
        true,
        PerfConfig::default(),
    );
    run_variant(
        "demo closed_loop no plants",
        true,
        false,
        PerfConfig::default(),
    );
    run_variant(
        "demo open faucet +plants",
        false,
        true,
        PerfConfig::default(),
    );
    run_variant(
        "full_feel closed",
        true,
        true,
        PerfConfig::full_feel(),
    );
    run_variant(
        "defaults every-other only",
        true,
        true,
        PerfConfig {
            flow_quiet_early_out: false,
            ..PerfConfig::default()
        },
    );
    run_variant(
        "defaults quiet EO only",
        true,
        true,
        PerfConfig {
            flow_every_other_substep: false,
            ..PerfConfig::default()
        },
    );
    run_variant(
        "parallel OFF closed",
        true,
        true,
        PerfConfig {
            parallel_physics: false,
            ..PerfConfig::default()
        },
    );
    set_parallel_enabled(true);
}
