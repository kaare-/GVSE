//! Headless B-overlay soak — print BudgetSnap + probes without the GUI.
//!
//! Short (CI):
//! ```bash
//! cargo test -p wk-voxel --test budget_soak --release short_budget_soak -- --nocapture
//! ```
//!
//! Long / overnight (delegate to a subagent; do not block the main agent):
//! ```bash
//! GVSE_SOAK_TICKS=250000 GVSE_BUDGET_PERIOD=60 \
//!   cargo test -p wk-voxel --test budget_soak --release long_budget_soak \
//!   -- --ignored --nocapture
//! ```
//!
//! Env:
//! - `GVSE_SOAK_TICKS` — soak length for short (default 120) and long (default 50_000)
//! - `GVSE_BUDGET_PERIOD` — sample every N ticks (default 60)
//! - `GVSE_BUDGET_WARM` — ticks before the mark (default 40)
//! - `GVSE_SOAK_OFF` — comma list: `evap`, `cond`, `steam`, `leftover`, `cadence`,
//!   `karst`, `competent`, `phase`, `cull`, `failure`, `snow`, `surplus`,
//!   `diffuse` (α=0), `orphan` (evap crest-film 8× off)
//!   (`snow` → `PhaseConfig::enable_snow_precip = false`; TRACKED mint kill)

use wk_voxel::{
    snow_mint_probe_reset, snow_mint_probe_snapshot, stamp_world, step_world, BudgetLedger,
    BudgetProbe, CarbonBudget, CarbonConfig, ClimateConfig, CloudConfig, CloudStore,
    CompetentFallConfig, CondensationConfig, EvapConfig, FailureConfig, FungiConfig, GrainConfig,
    Humidity, KarstConfig, LandscapeBodyStore, OrographicConfig, PerfConfig, PhaseConfig,
    SteamConfig, Temperature, Wind, World, WorldStep, WorldStepConfig, WorldgenParams,
};

fn env_u64(key: &str, default: u64) -> u64 {
    std::env::var(key)
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(default)
}

struct SoakScene {
    world: World,
    params: WorldgenParams,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    carbon: CarbonBudget,
    landscape: LandscapeBodyStore,
}

fn stamped_demo() -> SoakScene {
    let params = WorldgenParams::default();
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
        4,
        0.05,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let temperature = Temperature::with_world_bounds(
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
    SoakScene {
        world,
        params,
        humidity,
        wind,
        temperature,
        clouds: CloudStore::new(),
        carbon: CarbonBudget::default(),
        landscape: LandscapeBodyStore::new(),
    }
}

fn print_budget(led: &BudgetLedger, land: usize, label: &str) {
    let Some(mark) = led.mark() else {
        return;
    };
    let Some(now) = led.now() else {
        return;
    };
    let d = now.delta(mark);
    let p = BudgetProbe::snapshot();
    let dt = d.ticks.max(1) as f64;
    eprintln!("=== budget soak {label} ===");
    eprintln!(
        "mark t={} now t={} dt={} land={}",
        mark.tick, now.tick, d.ticks, land
    );
    eprintln!(
        "TRACKED d={:+.0} ({:+.2}/t)  min.tot d={:+} ({:+.2}/t)  body_w/m={}/{}",
        d.d_tracked,
        d.d_tracked / dt,
        d.d_min_total,
        d.d_min_total as f64 / dt,
        now.body_water,
        now.body_mineral
    );
    eprintln!(
        "stores d free={:+} pore={:+} steam={:+} cave_h={:+} pipe={:+} hum={:+.0} ice={:+} snow={:+} body={:+}",
        d.d_free,
        d.d_pore,
        d.d_steam,
        d.d_cave,
        d.d_pipe,
        d.d_humidity,
        d.d_ice,
        d.d_snow,
        d.d_body
    );
    eprintln!(
        "mineral d sol={:+} load={:+} body={:+} tot={:+}",
        d.d_min_solid, d.d_min_load, d.d_min_body, d.d_min_total
    );
    eprintln!(
        "probe-W swap={:+} park={:+} rej={:+} clamp={:+} hum_adv={:+.0} hum_dif={:+.0} evap_add={:+} evap_debit={:+} orphan_rm={:+}",
        p.water_swap,
        p.water_park,
        p.water_hum_rej,
        p.water_clamp,
        p.water_hum_advect,
        p.water_hum_diffuse,
        p.water_evap_add,
        p.water_evap_debit,
        p.water_orphan_rm
    );
    eprintln!(
        "probe-M bare={:+} credit={:+} clip={:+}",
        p.mineral_bare, p.mineral_credit, p.mineral_clip
    );
    let flags = format!(
        "{}{}",
        if d.unexplained_water() {
            " UNEXPL-W"
        } else {
            ""
        },
        if d.unexplained_mineral() {
            " UNEXPL-M"
        } else {
            ""
        }
    );
    eprintln!("read{flags}");
}

fn soak_off(flag: &str) -> bool {
    std::env::var("GVSE_SOAK_OFF")
        .ok()
        .map(|s| s.split(',').any(|p| p.trim() == flag))
        .unwrap_or(false)
}

fn run_soak(ticks: u64, warm: u64, period: u64, label: &str) -> (i64, i64, BudgetProbe) {
    let mut s = stamped_demo();
    let perf = PerfConfig::default();
    let mut failure = FailureConfig::default();
    if soak_off("failure") {
        failure.enable_roof_collapse = false;
        failure.enable_shear_weaken = false;
        failure.enable_compaction = false;
    }
    let mut evap = EvapConfig::default();
    if soak_off("orphan") {
        evap.enable_orphan_boost = false;
    }
    let cond = CondensationConfig {
        top_y: s.params.sky_ceiling_y - 2,
        ..CondensationConfig::default()
    };
    let oro = OrographicConfig {
        width_cols: s.params.width_cols,
        sea_level_y: s.params.sea_level_y,
        ..OrographicConfig::default()
    };
    let karst = KarstConfig::default();
    let cloud = CloudConfig::default();
    let mut phase = PhaseConfig::default();
    if soak_off("phase") {
        phase.enabled = false;
    }
    if soak_off("cull") {
        phase.enable_cull = false;
    }
    if soak_off("snow") {
        // Airborne flake paths (cond lottery + thermal surplus) refuse Snow.
        // 5k soak: TRACKED ~+136/t → ~0 with this flag (see VOXEL_BUDGET_SOAK).
        phase.enable_snow_precip = false;
    }
    let mut steam = SteamConfig::default();
    if soak_off("steam") {
        steam.enabled = false;
        steam.enable_pipe = false;
        steam.enable_leftover_field = false;
    }
    if soak_off("leftover") {
        steam.enable_leftover_field = false;
    }
    if soak_off("cadence") {
        // Cadence boil/flood/escape is period-gated; park it far away.
        steam.period_ticks = u64::MAX / 4;
    }
    let climate = ClimateConfig::default();
    let carbon_cfg = CarbonConfig::default();
    let grain = GrainConfig::default();
    let fungi = FungiConfig::default();
    let mut competent = CompetentFallConfig::default();
    if soak_off("competent") {
        competent.enable = false;
    }

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
        humidity_diffusion_alpha: if soak_off("diffuse") { 0.0 } else { 0.15 },
        sea_level_y: s.params.sea_level_y,
        sky_ceiling_y: s.params.sky_ceiling_y,
        evap_on: !soak_off("evap"),
        cond_rain_on: !soak_off("cond"),
        karst_on: !soak_off("karst"),
        organisms_on: false,
    };

    for _ in 0..warm {
        let _ = step_world(
            WorldStep {
                world: &mut s.world,
                humidity: &mut s.humidity,
                wind: &mut s.wind,
                temperature: &mut s.temperature,
                clouds: &mut s.clouds,
                carbon: &mut s.carbon,
                organisms: None,
                landscape: Some(&mut s.landscape),
                geotech: None,
                support: None,
            },
            &cfg,
            None,
        );
    }

    let mut led = BudgetLedger::default();
    led.period = period;
    led.enable_with(&s.world, &s.humidity, Some(&s.landscape));
    snow_mint_probe_reset();

    // Optional windowed attribution: GVSE_SOAK_WINDOW=N prints per-window
    // ΔTRACKED / Δsnow / Δhum so mint onset (~2k→5k) is visible without a
    // full mark remake. Default 0 = off.
    let window = env_u64("GVSE_SOAK_WINDOW", 0);
    let mut win_mark = if window > 0 {
        Some(wk_voxel::BudgetSnap::capture_with(
            &s.world,
            &s.humidity,
            Some(&s.landscape),
        ))
    } else {
        None
    };
    for i in 1..=ticks {
        let _ = step_world(
            WorldStep {
                world: &mut s.world,
                humidity: &mut s.humidity,
                wind: &mut s.wind,
                temperature: &mut s.temperature,
                clouds: &mut s.clouds,
                carbon: &mut s.carbon,
                organisms: None,
                landscape: Some(&mut s.landscape),
                geotech: None,
                support: None,
            },
            &cfg,
            None,
        );
        led.sample_if_due_with(&s.world, &s.humidity, Some(&s.landscape));
        if let (Some(mark), w) = (win_mark.as_ref(), window) {
            if w > 0 && i % w == 0 {
                let now = wk_voxel::BudgetSnap::capture_with(
                    &s.world,
                    &s.humidity,
                    Some(&s.landscape),
                );
                let d = now.delta(*mark);
                let dt = d.ticks.max(1) as f64;
                eprintln!(
                    "win t={}: TRACKED {:+.0} ({:+.2}/t) snow={:+} ice={:+} hum={:+.0} free={:+} pore={:+} steam={:+}",
                    now.tick,
                    d.d_tracked,
                    d.d_tracked / dt,
                    d.d_snow,
                    d.d_ice,
                    d.d_humidity,
                    d.d_free,
                    d.d_pore,
                    d.d_steam
                );
                win_mark = Some(now);
            }
        }
        if i == ticks || (period > 0 && i % (period * 20).max(1) == 0) {
            led.refresh_with(&s.world, &s.humidity, Some(&s.landscape));
            print_budget(&led, s.landscape.len(), label);
        }
    }

    led.refresh_with(&s.world, &s.humidity, Some(&s.landscape));
    print_budget(
        &led,
        s.landscape.len(),
        &format!("{label} final"),
    );
    let d = led.delta().expect("ledger on");
    let snow = snow_mint_probe_snapshot();
    let phase_store = d.d_ice + d.d_snow;
    eprintln!(
        "snow-mint-probe seated={} paid={:.0} under={:.0} Δsnow+ice={:+} ΔTRACKED={:+.0} paid−phase={:+.0}",
        snow.seated,
        snow.paid,
        snow.under,
        phase_store,
        d.d_tracked,
        snow.paid - phase_store as f64,
    );
    (d.d_min_total, d.d_tracked as i64, BudgetProbe::snapshot())
}

#[test]
fn short_budget_soak() {
    let warm = env_u64("GVSE_BUDGET_WARM", 20);
    let period = env_u64("GVSE_BUDGET_PERIOD", 30);
    let ticks = env_u64("GVSE_SOAK_TICKS", 120);
    let off = std::env::var("GVSE_SOAK_OFF").unwrap_or_default();
    let label = if off.is_empty() {
        format!("short/{ticks}")
    } else {
        format!("short/{ticks}/OFF={off}")
    };
    let (d_min, d_tracked, probe) = run_soak(ticks, warm, period, &label);
    assert_eq!(probe.water_park, 0, "park leftover must stay closed");
    assert_eq!(probe.water_clamp, 0, "humidity clamp must stay closed");
    assert_eq!(probe.mineral_clip, 0, "dissolved clip must stay closed");
    // Absolute leftover over short windows is noisy; rates are the signal.
    eprintln!(
        "short soak summary: ticks={ticks} off={off:?} d_tracked={d_tracked} ({:+.2}/t) d_min={d_min} park={}",
        d_tracked as f64 / ticks.max(1) as f64,
        probe.water_park
    );
}

#[test]
#[ignore = "long soak; set GVSE_SOAK_TICKS and run with --ignored --nocapture"]
fn long_budget_soak() {
    let warm = env_u64("GVSE_BUDGET_WARM", 40);
    let period = env_u64("GVSE_BUDGET_PERIOD", 60);
    let ticks = env_u64("GVSE_SOAK_TICKS", 50_000);
    let (d_min, d_tracked, probe) = run_soak(ticks, warm, period, "long");
    assert_eq!(probe.water_park, 0, "park leftover must stay closed");
    eprintln!(
        "long soak summary: ticks={ticks} d_tracked={d_tracked} d_min={d_min} park={}",
        probe.water_park
    );
}
