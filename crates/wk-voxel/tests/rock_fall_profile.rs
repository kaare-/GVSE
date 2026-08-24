//! Profile for the rock / landscape fall stack, measured the way the app runs it.
//!
//! `cargo test -p wk-voxel --release --test rock_fall_profile -- --nocapture`
//!
//! Uses `tick_with_life` (the real per-frame entry point) so numbers translate
//! to FPS directly, and A/B's `enable_competent_fall` to isolate the rock cost.

use std::time::{Duration, Instant};

use wk_material::MaterialId;
use wk_voxel::{
    apply_landscape_fall, plan_active, stamp_world, tick_with_life, Cell, CompetentFallConfig,
    FailureConfig, LandscapeBodyStore, PerfConfig, SupportMap, World, WorldgenParams,
};

fn ms(d: Duration, n: u32) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

fn demo_world() -> (World, WorldgenParams) {
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    (world, params)
}

/// Carve a big cavern under a ridge, like an F3 erase sweep in the demo.
fn carve_cavern(world: &mut World, params: &WorldgenParams, x0: i32, x1: i32, h: i32) {
    let mid = (params.bedrock_floor_y + params.sea_level_y) / 2;
    for x in x0..x1 {
        for y in mid..mid + h {
            world.set_cell(x, y, Cell::air());
        }
    }
}

/// Sprinkle boulder-sized competent blobs in the sky (tumble / fall stress).
fn sprinkle_boulders(world: &mut World, params: &WorldgenParams, count: i32, r: i32) {
    let top = params.sky_ceiling_y - 40;
    for i in 0..count {
        let cx = 40 + (i * 37) % (params.width_cols - 80).max(1);
        let cy = top - (i * 13) % 60;
        for x in cx - r..=cx + r {
            for y in cy - r..=cy + r {
                if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= r * r {
                    world.set_cell(x, y, Cell::solid(MaterialId::Stone));
                }
            }
        }
    }
}

fn loose_count(world: &World, params: &WorldgenParams) -> usize {
    let mut n = 0;
    for x in 0..params.width_cols {
        for y in params.bedrock_floor_y..params.sky_ceiling_y {
            if let Some(c) = world.get_cell(x, y) {
                if matches!(
                    c.material,
                    MaterialId::LooseRock | MaterialId::LooseLimestone
                ) {
                    n += 1;
                }
            }
        }
    }
    n
}

/// One frame the way `wk-voxel-app` drives it.
fn run_frame(
    world: &mut World,
    support: &mut SupportMap,
    store: &mut LandscapeBodyStore,
    perf: &PerfConfig,
    failure: &FailureConfig,
    competent: &CompetentFallConfig,
    rebuild_support: bool,
) {
    if rebuild_support {
        support.rebuild(world);
    }
    let dirty = plan_active(world);
    if !dirty.is_empty() || !store.is_empty() {
        let coords: Vec<_> = dirty.iter().map(|a| a.coord).collect();
        let _ = apply_landscape_fall(world, store, support, &coords);
    }
    let _ = tick_with_life(
        world,
        perf,
        failure,
        None,
        None,
        None,
        None,
        Some(competent),
    );
}

fn scenario(
    label: &str,
    setup: impl Fn(&mut World, &WorldgenParams),
    ticks: u32,
) {
    let perf = PerfConfig::default();
    let competent = CompetentFallConfig::default();

    // --- A: rock stack OFF (baseline cost of everything else).
    let (mut w_off, params) = demo_world();
    setup(&mut w_off, &params);
    let mut f_off = FailureConfig::default();
    f_off.enable_competent_fall = false;
    let mut sup_off = SupportMap::new();
    let mut store_off = LandscapeBodyStore::new();
    let t0 = Instant::now();
    for _ in 0..ticks {
        run_frame(
            &mut w_off,
            &mut sup_off,
            &mut store_off,
            &perf,
            &f_off,
            &competent,
            false,
        );
    }
    let off = t0.elapsed();

    // --- B: rock stack ON.
    let (mut w_on, _) = demo_world();
    setup(&mut w_on, &params);
    let loose_before = loose_count(&w_on, &params);
    let f_on = FailureConfig::default();
    let mut sup_on = SupportMap::new();
    let mut store_on = LandscapeBodyStore::new();
    wk_voxel::competent_probe::reset();
    let t1 = Instant::now();
    for t in 0..ticks {
        run_frame(
            &mut w_on,
            &mut sup_on,
            &mut store_on,
            &perf,
            &f_on,
            &competent,
            t % 8 == 1,
        );
    }
    let on = t1.elapsed();
    let loose_after = loose_count(&w_on, &params);

    let on_ms = ms(on, ticks);
    let off_ms = ms(off, ticks);
    println!("\n=== {label} ===");
    println!("  tick, rock stack OFF  {off_ms:>8.3} ms  ({:>5.1} fps)", 1000.0 / off_ms.max(0.001));
    println!("  tick, rock stack ON   {on_ms:>8.3} ms  ({:>5.1} fps)", 1000.0 / on_ms.max(0.001));
    println!("  rock stack cost       {:>8.3} ms/tick", on_ms - off_ms);
    println!(
        "  loose debris          {loose_before} -> {loose_after}  (delta {})",
        loose_after as i64 - loose_before as i64
    );
    let p = wk_voxel::competent_probe::snapshot();
    let per = |v: u64| v as f64 / ticks as f64;
    println!(
        "  per tick: builds {:.1}  seeds {:.0}/{:.0} pass  floods {:.0}  flood cells {:.0}  splits {:.0}  comps {:.0}  cargo {:.0}",
        per(p.build_calls),
        per(p.seeds_passed),
        per(p.seed_candidates),
        per(p.floods),
        per(p.flood_cells),
        per(p.split_calls),
        per(p.components),
        per(p.cargo_calls),
    );
}

#[test]
fn profile_rock_fall_stack() {
    const TICKS: u32 = 60;
    scenario("quiet demo world", |_, _| {}, TICKS);
    scenario(
        "carved cavern under ridge",
        |w, p| carve_cavern(w, p, 200, 400, 40),
        TICKS,
    );
    scenario(
        "60 sky boulders (r=4)",
        |w, p| sprinkle_boulders(w, p, 60, 4),
        TICKS,
    );
    println!();
}
