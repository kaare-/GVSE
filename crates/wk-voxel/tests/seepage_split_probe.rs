//! Which part of `seepage` costs 12.6 ms/tick?
//!
//! The perf profile lumps five different calls into one `seepage` bucket:
//! two pore-wake scans, a weep wake, the region pass (deep or contact-only
//! depending on cadence), and vertical seam coupling. They have very
//! different shapes — some are full-chunk scans, some follow the active set —
//! so the bucket alone cannot say what to fix.
//!
//! This probe times each call on a warmed world and reports both the cost per
//! call and the cost amortized over the seepage cadence, which is what
//! actually lands in the frame budget.
//!
//! ```text
//! cargo test -p wk-voxel --release --test seepage_split_probe -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_voxel::{
    apply_seepage_contact_regions, apply_seepage_regions, apply_seepage_seam_coupling, plan_active,
    stamp_world, tick_with_perf, wake_lake_bed_pores, wake_pore_weep_into_air,
    wake_vertical_chunk_seam_pores, PerfConfig, World, WorldgenParams, CHUNK_CELLS_H,
    CHUNK_CELLS_W,
};

const WARMUP: u64 = 40;
const MEASURE: u64 = 60;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

fn stress() -> WorldgenParams {
    WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 32,
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 6,
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

    let mut lake_bed = Duration::ZERO;
    let mut seam_wake = Duration::ZERO;
    let mut weep = Duration::ZERO;
    let mut deep = Duration::ZERO;
    let mut contact = Duration::ZERO;
    let mut seam_couple = Duration::ZERO;
    let mut plan = Duration::ZERO;
    let mut regions = 0usize;
    let mut cells = 0usize;

    // Time the components directly, in the order the tick calls them, so the
    // world state each one sees matches production.
    for _ in 0..MEASURE {
        tick_with_perf(&mut world, &perf);

        let t = Instant::now();
        wake_lake_bed_pores(&mut world);
        lake_bed += t.elapsed();

        let t = Instant::now();
        wake_vertical_chunk_seam_pores(&mut world);
        seam_wake += t.elapsed();

        let t = Instant::now();
        wake_pore_weep_into_air(&mut world);
        weep += t.elapsed();

        let t = Instant::now();
        let active = plan_active(&world);
        plan += t.elapsed();
        regions += active.len();
        cells += active
            .iter()
            .map(|a| {
                let r = &a.rect;
                ((r.x1 - r.x0 + 1) as usize) * ((r.y1 - r.y0 + 1) as usize)
            })
            .sum::<usize>();

        let t = Instant::now();
        apply_seepage_contact_regions(&mut world, &active);
        contact += t.elapsed();

        let t = Instant::now();
        apply_seepage_regions(&mut world, &active);
        deep += t.elapsed();

        let t = Instant::now();
        apply_seepage_seam_coupling(&mut world);
        seam_couple += t.elapsed();
    }

    let n = MEASURE;
    println!("\n=== {label} ===  {} chunks", world.chunks.len());
    println!("  active set          {:>7.1} regions  {:>8} cells", regions as f32 / n as f32, cells / n as usize);
    println!("  -- per call --");
    println!("  wake_lake_bed_pores        {:>8.3} ms", ms(lake_bed, n));
    println!("  wake_seam_pores            {:>8.3} ms", ms(seam_wake, n));
    println!("  wake_pore_weep_into_air    {:>8.3} ms", ms(weep, n));
    println!("  plan_active                {:>8.3} ms", ms(plan, n));
    println!("  seepage_contact_regions    {:>8.3} ms", ms(contact, n));
    println!("  seepage_regions (deep)     {:>8.3} ms", ms(deep, n));
    println!("  seam_coupling              {:>8.3} ms", ms(seam_couple, n));
    println!("  -- full-chunk scans (cadence candidates) --");
    let scans = lake_bed + seam_wake + weep + seam_couple;
    println!("  sum of the four scans      {:>8.3} ms/call", ms(scans, n));
}

#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn seepage_cost_split() {
    report("demo", WorldgenParams::default());
    report("stress", stress());
}
