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
    apply_condensation_rain_phased, apply_evaporation_into_humidity, humidity_diffuse_due,
    infect_mycelium_with_lineage, tick_with_life, Blueprint, CarbonBudget, CarbonConfig, Cell,
    ChunkCoord, ClimateConfig, CloudConfig, CloudStore, CondensationConfig, EvapConfig,
    FailureConfig, Humidity, OrganismStore, PerfConfig, Sat, SimEventKind, SimLog, Wind, World,
};

const WIDTH: i32 = 128;
const SKY_CEILING_Y: i32 = 96;
const SEA_LEVEL_Y: i32 = 16;
const BEDROCK_FLOOR_Y: i32 = 0;
const TILE_COLS: i32 = 4;

/// Moist beach + deep lake, land plants, seaweed, mycelium inoculum (no living stalk).
fn complex_life_world(seed: u64) -> (World, OrganismStore, CarbonBudget, Humidity, Wind, CloudStore, CloudConfig) {
    let mut world = World::new(seed);
    for cx in 0..=3 {
        for cy in 0..=2 {
            world.ensure_chunk(ChunkCoord::new(cx, cy));
        }
    }

    // Terrain: beach (x < 56) then deeper lake basin.
    for x in 0..WIDTH {
        world.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(140);
        world.set_cell(x, 1, sand);
        let topsoil = if x < 56 {
            let mut s = Cell::solid(MaterialId::Soil);
            s.sat = Sat(100);
            s
        } else {
            let mut s = Cell::solid(MaterialId::Sand);
            s.sat = Sat(180);
            s
        };
        world.set_cell(x, 2, topsoil);

        let water_top = if x < 56 {
            // Shallow puddles / wet beach film near the shore.
            if x >= 48 { 6 } else { 2 }
        } else if x < 70 {
            // Shelving shore.
            10
        } else {
            // Deep basin — more standing water for the climate pump.
            SEA_LEVEL_Y
        };

        for y in 3..=water_top {
            if water_top > 2 {
                world.set_cell(x, y, Cell::water());
            } else {
                world.set_cell(x, y, Cell::air());
            }
        }
        for y in (water_top + 1)..SKY_CEILING_Y {
            world.set_cell(x, y, Cell::air());
        }
    }

    // Grove litter / cream strip (land) — fungus infection hosts.
    for x in 14..46 {
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(90);
        world.set_cell(x, 2, org);
    }
    // Shore litter tongue into the shallows.
    for x in 46..58 {
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(110);
        world.set_cell(x, 2, org);
    }

    let mut store = OrganismStore::new();

    // Land plants on the cream grove (Air crown above Organic).
    let plant = Blueprint::minimal_plant();
    let plant_body = plant.modules_relative_to_nucleus();
    let mut plant_g = plant.genome;
    plant_g.clone_fidelity = 0.88;
    for x in [16i32, 22, 28, 34, 40, 20, 32] {
        let ok = store.spawn_blueprint(&world, x, 3, plant_body.clone(), 48.0, plant_g);
        assert!(ok, "land plant should seat at x={x}");
    }

    // Seaweed ribbons on the lake bed (nucleus in wet Air above sand).
    let seaweed = Blueprint::minimal_seaweed();
    let seaweed_body = seaweed.modules_relative_to_nucleus();
    let mut seaweed_g = seaweed.genome;
    seaweed_g.clone_fidelity = 0.90;
    for x in [64i32, 72, 80, 88, 96, 104, 112, 76, 92] {
        let ok = store.spawn_blueprint(&world, x, 3, seaweed_body.clone(), 42.0, seaweed_g);
        assert!(ok, "seaweed should seat at x={x}");
    }

    // Fungi: design fruiting body → infect Organic. No living stalk until
    // the network emerges and fruits on its own.
    let fungus = Blueprint::minimal_fungus();
    let fungus_body = fungus.modules_relative_to_nucleus();
    let mut fungus_g = fungus.genome;
    fungus_g.digest_rate = 1.15;
    fungus_g.clone_fidelity = 0.82;
    let lineage = Some((fungus_g, fungus_body));
    for x in [18i32, 26, 36, 50] {
        let hit = infect_mycelium_with_lineage(&mut world, x, 3, lineage.clone());
        assert!(hit.is_some(), "inoculum should hit Organic at x={x}");
    }

    let (p, f, a) = store.habit_counts();
    assert_eq!(f, 0, "no living fruiting body at init (got fungi={f})");
    assert!(p >= 10, "expected land plants + seaweed, got plants={p}");
    assert_eq!(a, 0);

    let humidity = Humidity::with_world_bounds(
        TILE_COLS,
        0,
        BEDROCK_FLOOR_Y,
        WIDTH,
        SKY_CEILING_Y,
    );
    let wind = Wind::climate(
        TILE_COLS,
        0.18,
        seed,
        WIDTH,
        SEA_LEVEL_Y,
        BEDROCK_FLOOR_Y,
        SKY_CEILING_Y,
        false,
    );
    // Wetter sky loop: coagulate faster, form parcels higher above the sea.
    let mut cloud_cfg = CloudConfig::default();
    cloud_cfg.coag_rate = 0.12;
    cloud_cfg.coag_max_take = 22.0;
    cloud_cfg.cloud_alt_above_sea = 70;
    cloud_cfg.coag_min_above_sea = 32;
    cloud_cfg.buoyant_rise = 0.12;
    cloud_cfg.max_parcels = 48;

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
    let evap = EvapConfig::default();
    let cond = CondensationConfig::default();
    let humidity_alpha = 0.15f32;

    let (p0, f0, a0) = store.habit_counts();
    log.note(
        0,
        format!(
            "{label} start p/f/a={p0}/{f0}/{a0} \
             inoculum-only fungi; seaweed+grove; water→sea_y={SEA_LEVEL_Y}; \
             coag_rate={:.3} cloud_alt={} coag_min_alt={}",
            cloud_cfg.coag_rate, cloud_cfg.cloud_alt_above_sea, cloud_cfg.coag_min_above_sea
        ),
    );
    log.push_sample(0, &world, &store, Some(&carbon), None);

    for _ in 0..ticks {
        let tick_no = world.tick;

        apply_evaporation_into_humidity(&mut world, &mut humidity, &evap);
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

        let fail = tick_with_life(&mut world, &perf, &failure, None, None, None, None);
        log.record_geotech(tick_no, fail);

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

        if humidity_diffuse_due(world.tick) {
            humidity.diffuse(humidity_alpha);
        }

        log.maybe_sample(tick_no, &world, &store, Some(&carbon), None);

        // Sparse progress breadcrumbs on long soaks.
        if ticks >= 50_000 && tick_no > 0 && tick_no % 50_000 == 0 {
            let (p, f, a) = store.habit_counts();
            log.note(
                tick_no,
                format!(
                    "{label} @{tick_no} p/f/a={p}/{f}/{a} \
                     cream={} sugar={} clouds={} hum={:.0}",
                    log.samples.last().map(|s| s.cream_cells).unwrap_or(0),
                    log.samples.last().map(|s| s.sugar_sum).unwrap_or(0),
                    clouds.len(),
                    humidity.total_mass(),
                ),
            );
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

    let last = log.samples.last().expect("sample");
    assert!(
        last.plants + last.corpses + last.fungi + last.cream_cells > 0,
        "life / cream should still register on the strip"
    );
    assert!(
        last.cream_cells > 0 || last.sat_total > 0,
        "world sample should see cream or water"
    );
    let nd = log.to_ndjson();
    assert!(nd.contains("\"type\":\"sample\""));
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
