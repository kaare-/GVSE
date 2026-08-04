//! Trace how the active-plan area evolves across the 12 flow substeps
//! (closed_loop demo), so we can pick a good adaptive early-out trigger.
//!
//! ```text
//! cargo test -p wk-voxel --test flow_halo_trace --release -- --ignored --nocapture
//! ```

use wk_voxel::{
    apply_gravity_fall_regions, apply_rain_with_temp, apply_water_flow_regions, clear_all_dirty,
    partition_checkerboard, plan_active, set_parallel_enabled, stamp_world,
    tick_with_perf, ActiveChunk, PerfConfig, PhaseConfig, RainConfig, Temperature, World,
    WorldgenParams,
};

const WARM: u64 = 30;
const TRACE: usize = 12;
const TRACE_TICKS: u64 = 20;

fn active_area(active: &[ActiveChunk]) -> usize {
    active
        .iter()
        .map(|a| {
            let w = (a.rect.x1 as usize).saturating_sub(a.rect.x0 as usize) + 1;
            let h = (a.rect.y1 as usize).saturating_sub(a.rect.y0 as usize) + 1;
            w.saturating_mul(h)
        })
        .sum()
}

#[test]
#[ignore]
fn flow_halo_trace() {
    set_parallel_enabled(true);
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
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
        closed_loop: true,
        sea_level_y: params.sea_level_y,
        ..RainConfig::default()
    };
    let phase = PhaseConfig::default();
    let perf = PerfConfig::default();
    for _ in 0..WARM {
        apply_rain_with_temp(&mut world, &rain, Some(&temperature), Some(&phase), None);
        tick_with_perf(&mut world, &perf);
    }

    eprintln!("=== Active-plan area across 12 flow substeps (closed_loop demo) ===");
    for tick in 0..TRACE_TICKS {
        apply_rain_with_temp(&mut world, &rain, Some(&temperature), Some(&phase), None);
        let mut trace = Vec::with_capacity(TRACE);
        for _step in 0..TRACE {
            let active = plan_active(&world);
            clear_all_dirty(&mut world);
            let area = active_area(&active);
            trace.push(area);
            if active.is_empty() {
                break;
            }
            let passes = partition_checkerboard(&active);
            for pass in &passes {
                apply_gravity_fall_regions(&mut world, pass);
            }
            apply_water_flow_regions(&mut world, &active);
        }
        eprintln!(
            "  tick {tick:>2}:  {}",
            trace
                .iter()
                .map(|a| format!("{a:>5}"))
                .collect::<Vec<_>>()
                .join(" ")
        );
    }
}
