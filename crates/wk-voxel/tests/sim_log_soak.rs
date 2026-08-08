//! Headless sim-log harness — short CI test + ignored long soak.
//!
//! Short (always runs in CI):
//! ```bash
//! cargo test -p wk-voxel --test sim_log_soak --release short_logged_life_run -- --nocapture
//! ```
//!
//! Long soak (cloud / local overnight):
//! ```bash
//! GVSE_SIM_LOG=/tmp/gvse-soak.ndjson \
//!   cargo test -p wk-voxel --test sim_log_soak --release long_sim_log_soak \
//!   -- --ignored --nocapture
//! ```
//!
//! Env:
//! - `GVSE_SIM_LOG` — NDJSON output path (optional; summary always prints)
//! - `GVSE_SIM_LOG_PERIOD` — sample every N ticks (default 60 / soak 120)
//! - `GVSE_SOAK_TICKS` — override long soak length (default 50_000)

use std::path::PathBuf;

use wk_material::MaterialId;
use wk_voxel::{
    tick_with_life, Blueprint, CarbonBudget, CarbonConfig, Cell, ChunkCoord, ClimateConfig,
    FailureConfig, OrganismStore, PerfConfig, Sat, SimEventKind, SimLog, World,
};

/// Compact moist beach with a grove + Organic cream strip.
fn demo_life_world(seed: u64) -> (World, OrganismStore, CarbonBudget) {
    let mut world = World::new(seed);
    let width = 96i32;
    for cx in 0..=1 {
        world.ensure_chunk(ChunkCoord::new(cx, 0));
        world.ensure_chunk(ChunkCoord::new(cx, 1));
    }
    for x in 0..width {
        world.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(110);
        world.set_cell(x, 1, sand);
        world.set_cell(x, 2, sand);
        for y in 3..40 {
            world.set_cell(x, y, Cell::air());
        }
    }
    // Standing water pool for climate / raft edge cases.
    for x in 70..90 {
        for y in 3..=8 {
            world.set_cell(x, y, Cell::water());
        }
    }
    // Litter + cream under the grove.
    for x in 20..40 {
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(70);
        org.set_mycelium(48);
        world.set_cell(x, 2, org);
    }

    let mut store = OrganismStore::new();
    let plant = Blueprint::minimal_plant();
    let body = plant.modules_relative_to_nucleus();
    let mut g = plant.genome;
    g.clone_fidelity = 0.85;
    for x in [22i32, 28, 34, 24, 32] {
        let _ = store.spawn_blueprint(&world, x, 3, body.clone(), 45.0, g);
    }
    // One painted fungus fruiting body on the cream.
    let fungus = Blueprint::minimal_fungus();
    let fbody = fungus.modules_relative_to_nucleus();
    let _ = store.spawn_blueprint(&world, 30, 3, fbody, 35.0, fungus.genome);

    (world, store, CarbonBudget::default())
}

fn run_logged(ticks: u64, sample_period: u64, label: &str) -> SimLog {
    let mut log = SimLog::new(80_000, sample_period);
    if let Ok(p) = std::env::var("GVSE_SIM_LOG") {
        let p = p.trim();
        if !p.is_empty() && p != "0" && p != "false" {
            log.set_path(PathBuf::from(p));
        }
    }

    let (mut world, mut store, mut carbon) = demo_life_world(0x51_10_60);
    let climate = ClimateConfig::default();
    let carbon_cfg = CarbonConfig::default();
    let perf = PerfConfig {
        parallel_physics: false,
        ..PerfConfig::default()
    };
    let failure = FailureConfig::default();

    log.note(
        0,
        format!(
            "{label} start p/f/a={:?}",
            store.habit_counts()
        ),
    );
    log.push_sample(0, &world, &store, Some(&carbon), None);

    for _ in 0..ticks {
        let tick_no = world.tick;
        let fail = tick_with_life(&mut world, &perf, &failure, None, None, None, None);
        log.record_geotech(tick_no, fail);

        let outcome = store.step_with_carbon(
            &mut world,
            tick_no,
            &climate,
            None,
            0.12,
            None,
            Some(&mut carbon),
            &carbon_cfg,
        );
        log.record_organism(tick_no, &outcome.stats);
        log.maybe_sample(tick_no, &world, &store, Some(&carbon), None);
    }

    log.note(
        world.tick,
        format!(
            "{label} end p/f/a={:?} corpses={}",
            store.habit_counts(),
            store.corpse_count()
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
    let last = log.samples.last().expect("sample");
    assert!(
        last.plants + last.corpses + last.fungi > 0,
        "life should still register on the strip"
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
        .unwrap_or(120);
    let log = run_logged(ticks, period, "soak");
    assert!(log.samples.len() >= 10, "soak should emit many samples");
    eprintln!(
        "soak wrote {} event lines / {} samples (flush path={:?})",
        log.events.len(),
        log.samples.len(),
        std::env::var_os("GVSE_SIM_LOG")
    );
}
