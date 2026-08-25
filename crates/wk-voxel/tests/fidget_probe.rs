//! Diagnostic: what keeps ~57k cells active every tick?
//!
//! `perf_profile` shows grain settle ~32 ms/tick and seepage ~24 ms/tick on
//! the stress world while water flow is ~3.5 ms. Grain settle already
//! early-outs when a pass moves nothing, so those cells really are moving.
//! This probe diffs the whole grid tick over tick and attributes the churn
//! to material transitions, sat-only changes, and A→B→A oscillation.
//!
//! ```text
//! cargo test -p wk-voxel --test fidget_probe --release -- --ignored --nocapture
//! ```

use std::collections::HashMap;

use wk_material::MaterialId;
use wk_voxel::{
    apply_condensation_rain_phased, apply_evaporation_into_humidity, apply_phase, plan_active,
    stamp_world, tick_with_perf, ActiveChunk, ClimateConfig, CloudConfig, CloudStore,
    CondensationConfig, EvapConfig, Humidity, PerfConfig, PhaseConfig, Temperature, Wind, World,
    WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W,
};

const TILE_COLS: i32 = 4;
const WARM: u64 = 60;

struct Scene {
    world: World,
    params: WorldgenParams,
    humidity: Humidity,
    wind: Wind,
    clouds: CloudStore,
    temperature: Temperature,
    evap: EvapConfig,
    cond: CondensationConfig,
    cloud: CloudConfig,
    phase: PhaseConfig,
    climate: ClimateConfig,
    perf: PerfConfig,
}

fn scene(width_chunks: i32) -> Scene {
    let params = WorldgenParams {
        width_cols: CHUNK_CELLS_W as i32 * width_chunks,
        sky_ceiling_y: CHUNK_CELLS_H as i32 * 6,
        ..WorldgenParams::default()
    };
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let mut humidity = Humidity::with_world_bounds(
        TILE_COLS,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
    );
    humidity.wrap_x = params.wrap_x;
    let wind = Wind::climate(
        TILE_COLS,
        0.05,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let temperature = Temperature::with_world_bounds(
        TILE_COLS,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.wrap_x,
    );
    Scene {
        world,
        params,
        humidity,
        wind,
        clouds: CloudStore::new(),
        temperature,
        evap: EvapConfig::default(),
        cond: CondensationConfig {
            top_y: params.sky_ceiling_y - 2,
            ..CondensationConfig::default()
        },
        cloud: CloudConfig::default(),
        phase: PhaseConfig::default(),
        climate: ClimateConfig::default(),
        perf: PerfConfig::default(),
    }
}

/// Demo-like frame: **rain off**, drizzle / evap / phase on (matches the
/// playtest HUD). `weather = false` runs physics alone to see whether the
/// halo is self-sustaining.
fn frame(s: &mut Scene, weather: bool) {
    let tick_no = s.world.tick;
    if weather {
        apply_evaporation_into_humidity(&mut s.world, &mut s.humidity, &s.evap);
        s.humidity.advect(s.wind.climate_vx, s.wind.climate_vy);
        s.clouds.step_with_precip(
            &mut s.world,
            &mut s.humidity,
            &s.wind,
            s.params.sea_level_y,
            s.params.sky_ceiling_y,
            tick_no,
            &s.cloud,
            Some(&s.temperature),
            Some(&s.phase),
        );
        apply_condensation_rain_phased(
            &mut s.world,
            &mut s.humidity,
            &s.cond,
            None,
            Some(&s.temperature),
            Some(&s.phase),
        );
    }
    tick_with_perf(&mut s.world, &s.perf);
    if weather {
        apply_phase(&mut s.world, &s.temperature, &s.phase);
    }
}

type Snap = Vec<(MaterialId, u8)>;

fn snapshot(s: &Scene) -> Snap {
    let mut out = Vec::with_capacity(
        (s.params.width_cols * (s.params.sky_ceiling_y - s.params.bedrock_floor_y)) as usize,
    );
    for x in 0..s.params.width_cols {
        for y in s.params.bedrock_floor_y..s.params.sky_ceiling_y {
            match s.world.get_cell(x, y) {
                Some(c) => out.push((c.material, c.sat.0)),
                None => out.push((MaterialId::Bedrock, 0)),
            }
        }
    }
    out
}

fn active_cells(a: &[ActiveChunk]) -> usize {
    a.iter()
        .map(|r| {
            let w = (r.rect.x1 as usize).saturating_sub(r.rect.x0 as usize) + 1;
            let h = (r.rect.y1 as usize).saturating_sub(r.rect.y0 as usize) + 1;
            w * h
        })
        .sum()
}

fn report(label: &str, s: &mut Scene, weather: bool, ticks: u64) {
    println!("--- {label} (weather={weather}) ---");
    let mut prev = snapshot(s);
    let mut prev_prev: Option<Snap> = None;
    for t in 0..ticks {
        frame(s, weather);
        let now = snapshot(s);
        let mut mat_moves: HashMap<(MaterialId, MaterialId), usize> = HashMap::new();
        let mut sat_only: HashMap<MaterialId, usize> = HashMap::new();
        let mut flip_back = 0usize;
        for (i, (a, b)) in prev.iter().zip(now.iter()).enumerate() {
            if a.0 != b.0 {
                *mat_moves.entry((a.0, b.0)).or_default() += 1;
                if let Some(pp) = &prev_prev {
                    if pp[i].0 == b.0 {
                        flip_back += 1;
                    }
                }
            } else if a.1 != b.1 {
                *sat_only.entry(a.0).or_default() += 1;
            }
        }
        let active = active_cells(&plan_active(&s.world));
        let mat_total: usize = mat_moves.values().sum();
        let sat_total: usize = sat_only.values().sum();
        if t + 1 == ticks || t == 0 {
            let mut mv: Vec<_> = mat_moves.into_iter().collect();
            mv.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            let mut sv: Vec<_> = sat_only.into_iter().collect();
            sv.sort_by_key(|&(_, n)| std::cmp::Reverse(n));
            println!(
                "  tick {t}: active={active} material_changes={mat_total} \
                 (A->B->A {flip_back}) sat_only={sat_total}"
            );
            for ((a, b), n) in mv.iter().take(8) {
                println!("      {a:?} -> {b:?}: {n}");
            }
            for (m, n) in sv.iter().take(6) {
                println!("      sat {m:?}: {n}");
            }
        } else {
            println!(
                "  tick {t}: active={active} material_changes={mat_total} \
                 (A->B->A {flip_back}) sat_only={sat_total}"
            );
        }
        prev_prev = Some(prev);
        prev = now;
    }
}

/// Is the pore churn a moving front, or a ±1 ping-pong that never settles?
#[test]
#[ignore]
fn pore_churn_is_a_ping_pong() {
    let mut s = scene(32);
    for _ in 0..WARM {
        frame(&mut s, true);
    }
    let mut prev = snapshot(&s);
    let mut prev_changed: Option<Vec<usize>> = None;
    for t in 0..6 {
        frame(&mut s, false);
        let now = snapshot(&s);
        let mut changed = Vec::new();
        let mut delta_hist: HashMap<i32, usize> = HashMap::new();
        let mut by_mat: HashMap<MaterialId, usize> = HashMap::new();
        for (i, (a, b)) in prev.iter().zip(now.iter()).enumerate() {
            if a.0 == b.0 && a.1 != b.1 {
                changed.push(i);
                let d = b.1 as i32 - a.1 as i32;
                *delta_hist.entry(d.abs().min(9)).or_default() += 1;
                *by_mat.entry(a.0).or_default() += 1;
            }
        }
        let repeat = match &prev_changed {
            Some(p) => {
                let set: std::collections::HashSet<_> = p.iter().copied().collect();
                changed.iter().filter(|i| set.contains(i)).count()
            }
            None => 0,
        };
        let mut dh: Vec<_> = delta_hist.into_iter().collect();
        dh.sort();
        println!(
            "  tick {t}: sat_changed={} repeat_of_prev_tick={repeat} ({:.0}%)  |delta| hist {dh:?}",
            changed.len(),
            repeat as f64 * 100.0 / changed.len().max(1) as f64
        );
        prev_changed = Some(changed);
        prev = now;
    }
}

#[test]
#[ignore]
fn fidget_attribution() {
    let mut s = scene(32);
    for _ in 0..WARM {
        frame(&mut s, true);
    }
    report("stress 2048x384, full weather", &mut s, true, 6);
    report("stress 2048x384, physics only", &mut s, false, 6);
}
