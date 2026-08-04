//! Expert deep-dive: decompose water flow tick cost by scan geometry.
//!
//! We can't cleanly separate the four flow priorities (they run inside
//! one `accumulate_water_flow_xfers` pass) without invasive changes, so
//! we time proxies that isolate the dominant costs:
//!
//! 1. Total physics tick (baseline).
//! 2. Gravity + water-flow substeps only (skip seepage/grain/etc.).
//! 3. Active area planned per substep — reveals dirty-halo hygiene.
//! 4. Effect of dropping `FLOW_SUBSTEPS` from 12 → 6 (via every-other).
//!
//! Ignored by default:
//! ```text
//! cargo test -p wk-voxel --test water_flow_decomp --release -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_rain_with_temp, plan_active, stamp_world, tick_with_perf, PerfConfig, PhaseConfig,
    RainConfig, Temperature, World, WorldgenParams,
};

const WARM: u64 = 30;
const MEAS: u64 = 120;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

fn active_area(active: &[wk_voxel::ActiveChunk]) -> usize {
    active
        .iter()
        .map(|a| {
            let w = (a.rect.x1 as usize).saturating_sub(a.rect.x0 as usize) + 1;
            let h = (a.rect.y1 as usize).saturating_sub(a.rect.y0 as usize) + 1;
            w.saturating_mul(h)
        })
        .sum()
}

fn build() -> (World, RainConfig, Temperature, PhaseConfig) {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
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
    let rain = RainConfig {
        top_y: params.sky_ceiling_y - 1,
        x_range: (0, params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
        closed_loop: true,
        sea_level_y: params.sea_level_y,
        ..RainConfig::default()
    };
    (world, rain, temperature, PhaseConfig::default())
}

#[test]
#[ignore]
fn water_flow_decomposition() {
    let (mut world, rain, temperature, phase) = build();
    let perf = PerfConfig::default();

    for _ in 0..WARM {
        apply_rain_with_temp(&mut world, &rain, Some(&temperature), Some(&phase), None);
        tick_with_perf(&mut world, &perf);
    }

    // Snapshot active area distribution over MEAS ticks under closed_loop.
    let mut areas: Vec<usize> = Vec::with_capacity(MEAS as usize);
    let mut regions_ct: Vec<usize> = Vec::with_capacity(MEAS as usize);
    let mut ticks = Duration::ZERO;
    for _ in 0..MEAS {
        apply_rain_with_temp(&mut world, &rain, Some(&temperature), Some(&phase), None);
        let plan = plan_active(&world);
        areas.push(active_area(&plan));
        regions_ct.push(plan.len());
        let t0 = Instant::now();
        tick_with_perf(&mut world, &perf);
        ticks += t0.elapsed();
    }
    areas.sort_unstable();
    regions_ct.sort_unstable();
    let p50 = areas[areas.len() / 2];
    let p95 = areas[(areas.len() * 95) / 100];
    let p50r = regions_ct[regions_ct.len() / 2];
    let p95r = regions_ct[(regions_ct.len() * 95) / 100];
    eprintln!("=== Water flow decomposition (demo closed_loop) ===");
    eprintln!(
        "  active plan (per-tick, pre-tick):  p50={p50:>7} cells  p95={p95:>7} cells   regions p50={p50r} p95={p95r}"
    );
    eprintln!("  full tick_with_perf                {:>7.2} ms/tick", ms(ticks, MEAS));

    // A/B on FLOW_SUBSTEPS approximation: every-other flow halves the flow
    // work (gravity keeps ×12).
    let (mut w2, _, _, _) = build();
    for _ in 0..WARM {
        apply_rain_with_temp(&mut w2, &rain, Some(&temperature), Some(&phase), None);
        tick_with_perf(&mut w2, &perf);
    }
    let alt = PerfConfig {
        flow_every_other_substep: true,
        ..PerfConfig::default()
    };
    let mut alt_t = Duration::ZERO;
    for _ in 0..MEAS {
        apply_rain_with_temp(&mut w2, &rain, Some(&temperature), Some(&phase), None);
        let t0 = Instant::now();
        tick_with_perf(&mut w2, &alt);
        alt_t += t0.elapsed();
    }
    eprintln!("  every-other flow tick              {:>7.2} ms/tick", ms(alt_t, MEAS));
    eprintln!(
        "  Δ from ×12→×6 flow substeps:       {:>7.2} ms/tick  (water_flow slice)",
        ms(ticks, MEAS) - ms(alt_t, MEAS)
    );
}
