//! Where do `rock bodies` and `seepage` spend their time on a settled world?
//!
//! The perf profile attributes ~7 ms/tick (demo) and ~13 ms/tick (stress) to
//! each of those two passes even when nothing visible is moving. This probe
//! reports the per-tick *work counts* behind that time so the fix targets the
//! cause rather than the symptom.
//!
//! ```text
//! cargo test -p wk-voxel --release --test body_seepage_probe -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_rain_with_temp, competent_probe, stamp_world, step_world, tick_with_perf,
    tick_with_perf_profiled, CarbonBudget, CarbonConfig, ClimateConfig, CloudConfig, CloudStore,
    CompetentFallConfig, CondensationConfig, EvapConfig, FailureConfig, FungiConfig, GrainConfig,
    Humidity, KarstConfig, OrographicConfig, PerfConfig, PhaseConfig, PhysicsTimings, RainConfig,
    SteamConfig, Temperature, Wind, World, WorldStep, WorldStepConfig, WorldgenParams,
    CHUNK_CELLS_H, CHUNK_CELLS_W,
};

const WARMUP: u64 = 40;
const MEASURE: u64 = 120;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

fn demo() -> WorldgenParams {
    WorldgenParams::default()
}

fn stress() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 32,
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 6,
        ..WorldgenParams::default()
    }
}

fn stretch() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 64,
        sky_ceiling_y: wk_voxel::TROPOSPHERE_TOP_Y + wk_voxel::STRATOSPHERE_CELLS,
        ..WorldgenParams::default()
    }
}

fn report(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();

    for _ in 0..WARMUP {
        tick_with_perf(&mut world, &perf);
    }

    competent_probe::reset();
    let mut phys = PhysicsTimings::default();
    let wall = Instant::now();
    for _ in 0..MEASURE {
        tick_with_perf_profiled(&mut world, &perf, &mut phys);
    }
    let wall = wall.elapsed();
    let p = competent_probe::snapshot();
    let n = MEASURE;

    println!("\n=== {label} ===  {} chunks", world.chunks.len());
    println!("  wall              {:>8.3} ms/tick", ms(wall, n));
    println!("  rock bodies       {:>8.3} ms/tick", ms(phys.bodies, n));
    println!("  seepage           {:>8.3} ms/tick", ms(phys.seepage, n));
    println!("  --- body pass work per tick ---");
    println!("  build_components calls  {:>10.1}", p.build_calls as f32 / n as f32);
    println!("  seed candidates         {:>10.1}", p.seed_candidates as f32 / n as f32);
    println!("  seeds passed gate       {:>10.1}", p.seeds_passed as f32 / n as f32);
    println!("  floods                  {:>10.1}", p.floods as f32 / n as f32);
    println!("  flood cells visited     {:>10.1}", p.flood_cells as f32 / n as f32);
    println!("  strata bailouts         {:>10.1}", p.strata_bailouts as f32 / n as f32);
    println!("  components produced     {:>10.1}", p.components as f32 / n as f32);
    println!("  weld-split calls        {:>10.1}", p.split_calls as f32 / n as f32);
    println!("  weld-split cells        {:>10.1}", p.split_cells as f32 / n as f32);
    println!("  hanging-extract calls   {:>10.1}", p.hang_calls as f32 / n as f32);
    println!("  cargo gather calls      {:>10.1}", p.cargo_calls as f32 / n as f32);
    println!("  cargo cells             {:>10.1}", p.cargo_cells as f32 / n as f32);
    println!("  --- why it ran / what it decided ---");
    println!("  wake cells queued       {:>10.1}", p.wake_cells as f32 / n as f32);
    println!("    from solidity change  {:>10.1}", p.wake_from_solidity as f32 / n as f32);
    println!("    from bodies moved     {:>10.1}", p.wake_from_moved as f32 / n as f32);
    println!("    from cadence float    {:>10.1}", p.wake_from_cadence_float as f32 / n as f32);
    println!("    from cadence seed     {:>10.1}", p.wake_from_cadence_seed as f32 / n as f32);
    println!("  region cells scanned    {:>10.1}", p.region_cells as f32 / n as f32);
    println!("  comps → sleep           {:>10.1}", p.comp_slept as f32 / n as f32);
    println!("  comps floating          {:>10.1}", p.comp_floating as f32 / n as f32);
    println!("  comps unsupported stuck {:>10.1}", p.comp_unsupported_stuck as f32 / n as f32);
    println!("  comps fell              {:>10.1}", p.comp_fell as f32 / n as f32);
    println!("  comps fall refused      {:>10.1}", p.comp_fall_refused as f32 / n as f32);
    println!("  comps rolled            {:>10.1}", p.comp_rolled as f32 / n as f32);
    println!("  comps shattered         {:>10.1}", p.comp_shattered as f32 / n as f32);
}

/// Does the body pass converge on a world where nothing moves, or is it a
/// treadmill? Reports cost per window over a long quiet run.
fn convergence(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();
    println!("\n=== {label} convergence (quiet world) ===");
    for window in 0..8 {
        competent_probe::reset();
        let mut phys = PhysicsTimings::default();
        for _ in 0..100 {
            tick_with_perf_profiled(&mut world, &perf, &mut phys);
        }
        let p = competent_probe::snapshot();
        println!(
            "  ticks {:>4}-{:>4}   bodies {:>6.2} ms   seeds {:>8.0}   comps {:>6.1}   slept {:>5.1}",
            window * 100,
            window * 100 + 99,
            ms(phys.bodies, 100),
            p.seed_candidates as f32 / 100.0,
            p.components as f32 / 100.0,
            p.comp_slept as f32 / 100.0,
        );
    }
}

/// Full climatic stack (`step_world` + rain) — tick-only probes miss the
/// Phase 4 Compact bodies cost (solidity wakes from phase/steam/rain).
fn report_climatic(label: &str, params: WorldgenParams) {
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
    let mut wind = Wind::climate(
        4,
        0.05,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let mut temperature = Temperature::with_world_bounds(
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
    let mut clouds = CloudStore::new();
    let mut carbon = CarbonBudget::default();
    let rain = RainConfig {
        top_y: params.sky_ceiling_y - 1,
        x_range: (0, params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
        closed_loop: false,
        sea_level_y: params.sea_level_y,
        ..RainConfig::default()
    };
    let perf = PerfConfig::default();
    let failure = FailureConfig::default();
    let evap = EvapConfig::default();
    let cond = CondensationConfig {
        top_y: params.sky_ceiling_y - 2,
        ..CondensationConfig::default()
    };
    let mut oro = OrographicConfig::default();
    oro.width_cols = params.width_cols;
    oro.sea_level_y = params.sea_level_y;
    let karst = KarstConfig::default();
    let cloud = CloudConfig::default();
    let phase = PhaseConfig::default();
    let steam = SteamConfig::default();
    let climate = ClimateConfig::default();
    let carbon_cfg = CarbonConfig::default();
    let grain = GrainConfig::default();
    let fungi = FungiConfig::default();
    let competent = CompetentFallConfig::default();
    let cfg = WorldStepConfig {
        perf: &perf,
        failure: &failure,
        evap: &evap,
        cond: &cond,
        oro: Some(&oro),
        karst: &karst,
        cloud: &cloud,
        phase: &phase,
        steam: &steam,
        climate: &climate,
        carbon: &carbon_cfg,
        grain: &grain,
        fungi: &fungi,
        competent: &competent,
        humidity_diffusion_alpha: 0.15,
        sea_level_y: params.sea_level_y,
        sky_ceiling_y: params.sky_ceiling_y,
        evap_on: true,
        cond_rain_on: true,
        karst_on: true,
        organisms_on: false,
    };

    for _ in 0..WARMUP {
        apply_rain_with_temp(
            &mut world,
            &rain,
            Some(&temperature),
            Some(&phase),
            Some(&mut humidity),
        );
        let step = WorldStep {
            world: &mut world,
            humidity: &mut humidity,
            wind: &mut wind,
            temperature: &mut temperature,
            clouds: &mut clouds,
            carbon: &mut carbon,
            organisms: None,
            geotech: None,
            support: None,
            landscape: None,
        };
        let _ = step_world(step, &cfg, None);
    }

    competent_probe::reset();
    let mut bodies = Duration::ZERO;
    let mut seepage = Duration::ZERO;
    let wall = Instant::now();
    for _ in 0..MEASURE {
        apply_rain_with_temp(
            &mut world,
            &rain,
            Some(&temperature),
            Some(&phase),
            Some(&mut humidity),
        );
        let mut step_t = wk_voxel::WorldStepTimings::default();
        let mut step = WorldStep {
            world: &mut world,
            humidity: &mut humidity,
            wind: &mut wind,
            temperature: &mut temperature,
            clouds: &mut clouds,
            carbon: &mut carbon,
            organisms: None,
            geotech: None,
            support: None,
            landscape: None,
        };
        let _ = step_world(step, &cfg, Some(&mut step_t));
        bodies += step_t.physics.bodies;
        seepage += step_t.physics.seepage;
    }
    let wall = wall.elapsed();
    let p = competent_probe::snapshot();
    let n = MEASURE;
    println!("\n=== {label} climatic ===  {} chunks", world.chunks.len());
    println!("  wall              {:>8.3} ms/tick", ms(wall, n));
    println!("  rock bodies       {:>8.3} ms/tick", ms(bodies, n));
    println!("  seepage           {:>8.3} ms/tick", ms(seepage, n));
    println!("  --- body pass work per tick ---");
    println!(
        "  build_components calls  {:>10.1}",
        p.build_calls as f32 / n as f32
    );
    println!(
        "  seed candidates         {:>10.1}",
        p.seed_candidates as f32 / n as f32
    );
    println!(
        "  seeds passed gate       {:>10.1}",
        p.seeds_passed as f32 / n as f32
    );
    println!("  floods                  {:>10.1}", p.floods as f32 / n as f32);
    println!(
        "  flood cells visited     {:>10.1}",
        p.flood_cells as f32 / n as f32
    );
    println!(
        "  strata bailouts         {:>10.1}",
        p.strata_bailouts as f32 / n as f32
    );
    println!(
        "  components produced     {:>10.1}",
        p.components as f32 / n as f32
    );
    println!("  --- why it ran ---");
    println!(
        "  wake cells queued       {:>10.1}",
        p.wake_cells as f32 / n as f32
    );
    println!(
        "    from solidity change  {:>10.1}",
        p.wake_from_solidity as f32 / n as f32
    );
    println!(
        "    from bodies moved     {:>10.1}",
        p.wake_from_moved as f32 / n as f32
    );
    println!(
        "    from cadence float    {:>10.1}",
        p.wake_from_cadence_float as f32 / n as f32
    );
    println!(
        "    from cadence seed     {:>10.1}",
        p.wake_from_cadence_seed as f32 / n as f32
    );
    println!(
        "  region cells scanned    {:>10.1}",
        p.region_cells as f32 / n as f32
    );
    println!(
        "  comps → sleep           {:>10.1}",
        p.comp_slept as f32 / n as f32
    );
    println!(
        "  comps unsupported stuck {:>10.1}",
        p.comp_unsupported_stuck as f32 / n as f32
    );
    println!("  comps fell              {:>10.1}", p.comp_fell as f32 / n as f32);
}

#[test]
#[ignore = "diagnostic probe; run explicitly"]
fn probe_body_and_seepage_work() {
    report("demo", demo());
    report("stress", stress());
    report("stretch", stretch());
    convergence("demo", demo());
}

#[test]
#[ignore = "diagnostic; climatic Compact body cost"]
fn probe_body_climatic_stretch() {
    report_climatic("stretch", stretch());
    report_climatic("demo", demo());
}
