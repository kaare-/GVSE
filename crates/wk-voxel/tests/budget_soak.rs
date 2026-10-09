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
//!   `diffuse` (α=0), `orphan` (evap crest-film 8× off),
//!   `snowfall` (flakes nucleate but do not descend), `snowwet` (no haze/film
//!   snow swap), `slush` (`PhaseConfig::enable_slush = false`),
//!   `snowraft` (Snow sinks through lakes — no float lid),
//!   `snowsurf` (live_surface peels seated Snow; weather ignores pack),
//!   `flow` (skip surface cascade / equalise / throughflow / confined),
//!   `seep` (skip pore seepage + contact wet + seam),
//!   `park` (`park_orphan_water` discards — free-sat park off),
//!   `gravity` (skip free-water / infiltration gravity pulls),
//!   `settle` (skip multi-pass grain fall/repose; airborne snow roll stays)
//!   (`snow` → `PhaseConfig::enable_snow_precip = false`; TRACKED mint kill)

use wk_voxel::{
    set_peel_seated_snow, set_skip_grain_settle, set_skip_gravity, set_skip_park_orphan,
    set_skip_seepage, set_skip_surface_flow,
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
        "probe-W swap={:+} swap_snow={:+} swap_other={:+} park={:+} rej={:+} clamp={:+} hum_adv={:+.0} hum_dif={:+.0} evap_add={:+} evap_debit={:+} orphan_rm={:+} dep_add={:+} dep_debit={:+} flow_air={:+} seep_air={:+} park_air={:+} free_other={:+} par_air={:+} par_snow={:+} grav_air={:+} steam_solid={:+}",
        p.water_swap,
        p.water_swap_snow,
        p.water_swap_other,
        p.water_park,
        p.water_hum_rej,
        p.water_clamp,
        p.water_hum_advect,
        p.water_hum_diffuse,
        p.water_evap_add,
        p.water_evap_debit,
        p.water_orphan_rm,
        p.water_dep_add,
        p.water_dep_debit,
        p.water_flow_air,
        p.water_seep_air,
        p.water_park_air,
        p.water_free_other,
        p.water_par_air,
        p.water_par_snow,
        p.water_grav_air,
        p.steam_on_solid
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

/// Clears soak hunt TLS gates when the soak returns (or panics).
struct HuntGateGuard;
impl Drop for HuntGateGuard {
    fn drop(&mut self) {
        set_peel_seated_snow(false);
        set_skip_surface_flow(false);
        set_skip_seepage(false);
        set_skip_park_orphan(false);
        set_skip_gravity(false);
        set_skip_grain_settle(false);
    }
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
    if soak_off("slush") {
        phase.enable_slush = false;
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
    let mut grain = GrainConfig::default();
    if soak_off("snowfall") {
        // Nucleation stays on; airborne roll + grain-fall flake descent off.
        grain.enable_airborne_snow_fall = false;
    }
    if soak_off("snowwet") {
        // Flakes only swap into empty Air — no haze/film ride.
        grain.enable_snow_wet_fall = false;
    }
    if soak_off("snowraft") {
        // Snow sinks through standing water — no lake raft / evap lid.
        grain.enable_snow_float = false;
    }
    // Weather crest ignores seated snow (physical lid still blocks evap).
    set_peel_seated_snow(soak_off("snowsurf"));
    // Free-sat writers beside landed Snow (post-descent mint hunt).
    set_skip_surface_flow(soak_off("flow"));
    set_skip_seepage(soak_off("seep"));
    set_skip_park_orphan(soak_off("park"));
    set_skip_gravity(soak_off("gravity"));
    set_skip_grain_settle(soak_off("settle"));
    let _hunt_guard = HuntGateGuard;
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
        // Peak steam sitting on non-Air (parallel grain Air→Snow skips evict).
        if period > 0 && i % period == 0 {
            let mut on_solid = 0i64;
            for (&(gx, gy), &amt) in s.world.steam.iter() {
                if amt == 0 {
                    continue;
                }
                if !matches!(
                    s.world.get_cell(gx, gy).map(|c| c.material),
                    Some(wk_material::MaterialId::Air)
                ) {
                    on_solid += i64::from(amt);
                }
            }
            wk_voxel::budget::note_steam_on_solid_peak(on_solid);
        }
        if let (Some(mark), w) = (win_mark.as_ref(), window) {
            if w > 0 && i % w == 0 {
                let now = wk_voxel::BudgetSnap::capture_with(
                    &s.world,
                    &s.humidity,
                    Some(&s.landscape),
                );
                let d = now.delta(*mark);
                let dt = d.ticks.max(1) as f64;
                let triad = d.d_free as f64 + d.d_pore as f64 + d.d_humidity + d.d_snow as f64;
                eprintln!(
                    "win t={}: TRACKED {:+.0} ({:+.2}/t) snow={:+} ice={:+} hum={:+.0} free={:+} pore={:+} steam={:+} triad(f+p+h+s)={:+.0}",
                    now.tick,
                    d.d_tracked,
                    d.d_tracked / dt,
                    d.d_snow,
                    d.d_ice,
                    d.d_humidity,
                    d.d_free,
                    d.d_pore,
                    d.d_steam,
                    triad
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
    // OFF=park deliberately discards orphan water — park leftover is the signal.
    if !soak_off("park") {
        assert_eq!(probe.water_park, 0, "park leftover must stay closed");
    }
    assert_eq!(probe.water_clamp, 0, "humidity clamp must stay closed");
    assert_eq!(probe.mineral_clip, 0, "dissolved clip must stay closed");
    // Absolute leftover over short windows is noisy; rates are the signal.
    let snow_net = probe.snow_exit_yield - probe.snow_enter_yield;
    eprintln!(
        "short soak summary: ticks={ticks} off={off:?} d_tracked={d_tracked} ({:+.2}/t) d_min={d_min} park={} swap={} swap_snow={} swap_other={} flow_air={} seep_air={} park_air={} free_other={} par_air={} par_snow={} grav_air={} steam_solid={} snow_enter={} exit_n={} enter_y={} exit_y={} net_leave={} credit={} bare={} to_ice={}",
        d_tracked as f64 / ticks.max(1) as f64,
        probe.water_park,
        probe.water_swap,
        probe.water_swap_snow,
        probe.water_swap_other,
        probe.water_flow_air,
        probe.water_seep_air,
        probe.water_park_air,
        probe.water_free_other,
        probe.water_par_air,
        probe.water_par_snow,
        probe.water_grav_air,
        probe.steam_on_solid,
        probe.snow_enter_n,
        probe.snow_exit_n,
        probe.snow_enter_yield,
        probe.snow_exit_yield,
        snow_net,
        probe.snow_exit_credit,
        probe.snow_exit_bare,
        probe.snow_to_ice,
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
