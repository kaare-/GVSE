//! Headless sim-log harness — short CI test + ignored long soak.
//!
//! Short (always runs in CI):
//! ```bash
//! cargo test -p wk-voxel --test sim_log_soak --release short_logged_life_run -- --nocapture
//! ```
//!
//! Long soak (cloud / local overnight):
//! ```bash
//! GVSE_SIM_LOG=/tmp/gvse-soak.ndjson GVSE_SOAK_TICKS=1000000 GVSE_SIM_LOG_PERIOD=2000 \
//!   cargo test -p wk-voxel --test sim_log_soak --release long_sim_log_soak \
//!   -- --ignored --nocapture
//! ```
//!
//! Env:
//! - `GVSE_SIM_LOG` — NDJSON output path (optional; summary always prints)
//! - `GVSE_SIM_LOG_PERIOD` — sample every N ticks (default 60 / soak 2000)
//! - `GVSE_SOAK_TICKS` — override long soak length (default 50_000)

use std::path::PathBuf;

use wk_material::MaterialId;
use wk_voxel::{
    apply_condensation_rain_phased, apply_evaporation_into_humidity,
    humidity_diffuse_alpha_per_tick,
    infect_mycelium_with_lineage, step_carbon_budget, tick_with_life, Blueprint, CarbonBudget,
    CarbonConfig, Cell, ChunkCoord, ClimateConfig, CloudConfig, CloudStore, CondensationConfig,
    EvapConfig, FailureConfig, FungiConfig, Humidity, ModuleId, OrganismStore, PerfConfig, Sat,
    SimEventKind, SimLog, SporeBankConfig, Wind, World,
};

const WIDTH: i32 = 112;
const SKY_CEILING_Y: i32 = 48;
const SEA_LEVEL_Y: i32 = 12;
const BEDROCK_FLOOR_Y: i32 = 0;
const TILE_COLS: i32 = 4;
/// Small off-map ocean vapor flux so closed humidity→cloud→rain can continue
/// after Organic cream locks free lake water into pores.
const OCEAN_HUMIDITY_FLUX: f32 = 4.0;

/// Moist beach + deep lake, land plants, seaweed, mycelium inoculum (no living stalk).
fn complex_life_world(seed: u64) -> (World, OrganismStore, CarbonBudget, Humidity, Wind, CloudStore, CloudConfig) {
    let mut world = World::new(seed);
    for cx in 0..=1 {
        world.ensure_chunk(ChunkCoord::new(cx, 0));
    }

    for x in 0..WIDTH {
        world.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(160);
        world.set_cell(x, 1, sand);
        let topsoil = if x < 48 {
            let mut s = Cell::solid(MaterialId::Soil);
            s.sat = Sat(120);
            s
        } else {
            let mut s = Cell::solid(MaterialId::Sand);
            s.sat = Sat(190);
            s
        };
        world.set_cell(x, 2, topsoil);

        let water_top = if x < 48 {
            if x >= 40 { 5 } else { 2 }
        } else if x < 60 {
            8
        } else {
            SEA_LEVEL_Y
        };

        for y in 3..=water_top {
            if water_top > 2 {
                world.set_cell(x, y, Cell::water());
            } else {
                world.set_cell(x, y, Cell::air());
            }
        }
        let air_top = (water_top + 10).min(SKY_CEILING_Y);
        for y in (water_top + 1)..air_top {
            world.set_cell(x, y, Cell::air());
        }
    }

    // Grove cream — leave some mineral crowns so roots can reach wet sand.
    for x in 14..36 {
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(90);
        world.set_cell(x, 2, org);
    }
    for x in 40..50 {
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(110);
        world.set_cell(x, 2, org);
    }

    let mut store = OrganismStore::new();
    store.spore_bank = SporeBankConfig {
        germinate_odds: 3,
        max_age_ticks: 500_000,
        max_total: 640,
        ..SporeBankConfig::default()
    };
    store.fungi = FungiConfig {
        soil_convert_odds: 2_000,
        soil_mycelium_threshold: 160,
        ..FungiConfig::default()
    };

    let plant = Blueprint::minimal_plant();
    let mut plant_body = plant.modules_relative_to_nucleus();
    plant_body.push((1, 0, ModuleId::Symbiont));
    let mut plant_g = plant.genome;
    plant_g.clone_fidelity = 0.90;
    plant_g.sym_water = 140;
    plant_g.sym_energy = 120;
    plant_g.root_depth_bias = 0.58;
    for x in [10i32, 14, 18, 22, 26, 30, 34, 8] {
        let ok = store.spawn_blueprint(&world, x, 3, plant_body.clone(), 50.0, plant_g);
        assert!(ok, "land plant should seat at x={x}");
    }

    let seaweed = Blueprint::minimal_seaweed();
    let seaweed_body = seaweed.modules_relative_to_nucleus();
    let mut seaweed_g = seaweed.genome;
    seaweed_g.clone_fidelity = 0.92;
    for x in [56i32, 64, 72, 80, 88, 96, 104, 68, 84] {
        let ok = store.spawn_blueprint(&world, x, 3, seaweed_body.clone(), 42.0, seaweed_g);
        assert!(ok, "seaweed should seat at x={x}");
    }

    let fungus = Blueprint::minimal_fungus();
    let mut fungus_body = fungus.modules_relative_to_nucleus();
    fungus_body.push((0, 1, ModuleId::Symbiont));
    let mut fungus_g = fungus.genome;
    fungus_g.digest_rate = 1.10;
    fungus_g.clone_fidelity = 0.85;
    fungus_g.sym_water = 150;
    fungus_g.sym_energy = 110;
    let lineage = Some((fungus_g, fungus_body));
    for x in [16i32, 24, 32] {
        let hit = infect_mycelium_with_lineage(&mut world, x, 3, lineage.clone());
        assert!(hit.is_some(), "inoculum should hit Organic at x={x}");
    }

    let (p, f, a) = store.habit_counts();
    assert_eq!(f, 0, "no living fruiting body at init (got fungi={f})");
    assert!(p >= 10, "expected land plants + seaweed, got plants={p}");
    assert_eq!(a, 0);

    let mut humidity = Humidity::with_world_bounds(
        TILE_COLS,
        0,
        BEDROCK_FLOOR_Y,
        WIDTH,
        SKY_CEILING_Y,
    );
    for x in (48..WIDTH).step_by(4) {
        humidity.add(x, SEA_LEVEL_Y + 6, 60.0);
    }
    let wind = Wind::climate(
        TILE_COLS,
        0.14,
        seed,
        WIDTH,
        SEA_LEVEL_Y,
        BEDROCK_FLOOR_Y,
        SKY_CEILING_Y,
        false,
    );
    let mut cloud_cfg = CloudConfig::default();
    cloud_cfg.coag_rate = 0.12;
    cloud_cfg.coag_max_take = 22.0;
    cloud_cfg.cloud_alt_above_sea = 28;
    cloud_cfg.coag_min_above_sea = 14;
    cloud_cfg.buoyant_rise = 0.12;
    cloud_cfg.downpour_mass = 160.0;
    cloud_cfg.rain_cells_per_tick = 4;
    cloud_cfg.max_parcels = 40;

    (
        world,
        store,
        CarbonBudget::default(),
        humidity,
        wind,
        CloudStore::new(),
        cloud_cfg,
    )
}

fn run_logged(ticks: u64, sample_period: u64, label: &str) -> SimLog {
    let mut log = SimLog::new(200_000, sample_period);
    if let Ok(p) = std::env::var("GVSE_SIM_LOG") {
        let p = p.trim();
        if !p.is_empty() && p != "0" && p != "false" {
            log.set_path(PathBuf::from(p));
        }
    }

    let (mut world, mut store, mut carbon, mut humidity, wind, mut clouds, cloud_cfg) =
        complex_life_world(0x51_10_60);
    let climate = ClimateConfig::default();
    let carbon_cfg = CarbonConfig::default();
    let perf = PerfConfig {
        parallel_physics: false,
        ..PerfConfig::default()
    };
    let failure = FailureConfig::default();
    // Tab-like evap (period 5) — gentler than crate default period=1.
    let evap = EvapConfig {
        rate_per_tick: 1,
        dry_above_max: 200,
        period_ticks: 5,
    };
    let mut cond = CondensationConfig::default();
    cond.top_y = SKY_CEILING_Y - 2;
    cond.min_mass_to_rain = 72.0;
    cond.max_prob_per_tick = 0.25;
    cond.mass_per_droplet = 72.0;
    let humidity_alpha = 0.15f32;

    let (p0, f0, a0) = store.habit_counts();
    log.note(
        0,
        format!(
            "{label} start p/f/a={p0}/{f0}/{a0} \
             inoculum-only fungi; seaweed+grove; sea_y={SEA_LEVEL_Y}; \
             coag={:.3} evap_period={} ocean_flux={:.1}",
            cloud_cfg.coag_rate, evap.period_ticks, OCEAN_HUMIDITY_FLUX
        ),
    );
    log.push_sample(0, &world, &store, Some(&carbon), None);
    {
        let s = log.samples.last().unwrap();
        assert!(s.woody_plants >= 7, "grove woody plants");
        assert!(s.stemless_wet >= 7, "lake seaweed should be bathing");
    }

    for _ in 0..ticks {
        let tick_no = world.tick;

        apply_evaporation_into_humidity(&mut world, &mut humidity, &evap);
        if tick_no % 2 == 0 {
            humidity.add(WIDTH - 10, SEA_LEVEL_Y + 8, OCEAN_HUMIDITY_FLUX);
        }
        let wind_vx = wind.effective_vx(tick_no);
        humidity.advect(wind_vx, 0.0);
        clouds.step(
            &mut world,
            &mut humidity,
            &wind,
            SEA_LEVEL_Y,
            SKY_CEILING_Y,
            tick_no,
            &cloud_cfg,
        );
        apply_condensation_rain_phased(
            &mut world,
            &mut humidity,
            &cond,
            None,
            None,
            None,
        );

        let fail = tick_with_life(
            &mut world,
            &perf,
            &failure,
            None,
            None,
            None,
            Some(&store.fungi),
            None,
        );
        log.record_geotech(tick_no, fail);
        step_carbon_budget(&mut carbon, &mut world, &carbon_cfg);

        let outcome = store.step_with_carbon(
            &mut world,
            tick_no,
            &climate,
            Some(&mut humidity),
            wind_vx,
            None,
            Some(&mut carbon),
            &carbon_cfg,
        );
        log.record_organism(tick_no, &outcome.stats);

        humidity.diffuse(humidity_diffuse_alpha_per_tick(humidity_alpha));

        log.maybe_sample(tick_no, &world, &store, Some(&carbon), None);

        if ticks >= 10_000 && tick_no > 0 && tick_no % 25_000 == 0 {
            let (p, f, a) = store.habit_counts();
            let s = log.samples.last();
            let msg = format!(
                "{label} @{tick_no} p/f/a={p}/{f}/{a} \
                 woody/wet/dry={}/{}/{} cream={} sugar={} hum={:.0} sat_f={} \
                 dry_sym={}/{} moist_w/d={:.2}/{:.2} \
                 stranded_r={:.1} depth_b_dry={:.2} org_w={:.1} bank={}",
                s.map(|s| s.woody_plants).unwrap_or(0),
                s.map(|s| s.stemless_wet).unwrap_or(0),
                s.map(|s| s.stemless_dry).unwrap_or(0),
                s.map(|s| s.cream_cells).unwrap_or(0),
                s.map(|s| s.sugar_sum).unwrap_or(0),
                humidity.total_mass(),
                s.map(|s| s.sat_free).unwrap_or(0),
                s.map(|s| s.plants_dry_sym_recv).unwrap_or(0),
                s.map(|s| s.plants_drought).unwrap_or(0),
                s.map(|s| s.mean_moist_woody).unwrap_or(0.0),
                s.map(|s| s.mean_moist_stemless_dry).unwrap_or(0.0),
                s.map(|s| s.mean_roots_stemless_dry).unwrap_or(0.0),
                s.map(|s| s.mean_depth_bias_stemless_dry).unwrap_or(0.0),
                s.map(|s| s.mean_org_depth_woody).unwrap_or(0.0),
                s.map(|s| s.spores_bank).unwrap_or(0),
            );
            log.note(tick_no, msg.clone());
            eprintln!("{msg}");
            if let Err(e) = log.flush_env() {
                eprintln!("sim_log checkpoint flush warning: {e}");
            }
        }
    }

    let (p1, f1, a1) = store.habit_counts();
    log.note(
        world.tick,
        format!(
            "{label} end p/f/a={p1}/{f1}/{a1} corpses={} clouds={} hum={:.0}",
            store.corpse_count(),
            clouds.len(),
            humidity.total_mass(),
        ),
    );
    log.push_sample(world.tick, &world, &store, Some(&carbon), None);
    if let Err(e) = log.flush_env() {
        eprintln!("sim_log flush warning: {e}");
    }
    eprintln!("{}", log.summary());
    log
}

#[test]
fn short_logged_life_run() {
    let log_path = std::env::temp_dir().join("gvse-sim-log-short.ndjson");
    std::env::set_var("GVSE_SIM_LOG", &log_path);

    let log = run_logged(600, 30, "short");
    assert!(!log.samples.is_empty(), "expected periodic samples");
    assert!(
        log.events
            .iter()
            .any(|e| matches!(e.kind, SimEventKind::Note { .. })),
        "expected start/end notes"
    );
    let first = log.samples.first().expect("sample");
    assert_eq!(first.fungi, 0, "init must be inoculum-only (no living stalk)");
    assert!(first.plants >= 10, "grove + seaweed should be present");
    assert!(first.cream_cells > 0, "mycelium inoculum should seed cream");
    assert!(first.woody_plants >= 7, "woody grove");
    assert!(first.stemless_wet >= 7, "bathing seaweed");
    assert_eq!(first.stemless_dry, 0, "lake should not be stranded at t=0");
    assert!(first.mean_alloc_root > 0.0, "evolution means should be live");

    let last = log.samples.last().expect("sample");
    assert!(
        last.plants + last.corpses + last.fungi + last.cream_cells > 0,
        "life / cream should still register on the strip"
    );
    let nd = log.to_ndjson();
    assert!(nd.contains("\"type\":\"sample\""));
    assert!(nd.contains("\"stemless_dry\""));
    assert!(nd.contains("\"woody_plants\""));
    assert!(nd.contains("\"mean_roots_stemless_dry\""));
    assert!(nd.lines().count() >= 3);
    assert!(
        log_path.is_file(),
        "GVSE_SIM_LOG should flush NDJSON to {}",
        log_path.display()
    );
    let disk = std::fs::read_to_string(&log_path).expect("read log");
    assert!(disk.contains("\"type\":\"event\""));
}

/// Long headless soak — opt-in with `--ignored`.
#[test]
#[ignore = "long soak; run with --ignored --nocapture"]
fn long_sim_log_soak() {
    let ticks: u64 = std::env::var("GVSE_SOAK_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(50_000);
    let period: u64 = std::env::var("GVSE_SIM_LOG_PERIOD")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(2_000);
    let log = run_logged(ticks, period, "soak");
    assert!(log.samples.len() >= 10, "soak should emit many samples");
    eprintln!(
        "soak wrote {} event lines / {} samples (flush path={:?})",
        log.events.len(),
        log.samples.len(),
        std::env::var_os("GVSE_SIM_LOG")
    );
}
