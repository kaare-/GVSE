//! How much of the grain-settle active set is non-Air (pore / solid dirty)?
//!
//! Fall and repose are destination-Air pulls. Seepage leaves wet-pore dirty
//! on `has_loose` chunks; sticky-loose filtering keeps the chunk but settle
//! still walks those solid cells.
//!
//! ```text
//! cargo test -p wk-voxel --test settle_air_probe --release -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_material::MaterialId;
use wk_voxel::{
    clear_all_dirty, plan_active, settle_loose_grains_regions, stamp_world, tick_with_perf,
    ActiveChunk, DirtyBits, PerfConfig, World, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W,
    GRAIN_SETTLE_PASSES_SHALLOW,
};

const WARMUP: u64 = 40;
const MEASURE: u64 = 80;

fn ms(d: Duration, n: u64) -> f32 {
    d.as_secs_f32() * 1000.0 / n.max(1) as f32
}

fn filter_loose(world: &World, active: &[ActiveChunk]) -> Vec<ActiveChunk> {
    active
        .iter()
        .copied()
        .filter(|ac| {
            world
                .chunks
                .get(&ac.coord)
                .map(|c| c.has_loose)
                .unwrap_or(false)
        })
        .collect()
}

fn keep_air_dest(world: &World, active: &[ActiveChunk]) -> Vec<ActiveChunk> {
    let mut out = Vec::with_capacity(active.len());
    for ac in active {
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            continue;
        };
        let mut bits = DirtyBits::empty();
        let mut any = false;
        ac.for_each_cell(|x, y| {
            if chunk.get(x as usize, y as usize).material == MaterialId::Air {
                bits.set(x, y);
                any = true;
            }
        });
        if !any {
            continue;
        }
        let Some(rect) = bits.bbox() else {
            continue;
        };
        out.push(ActiveChunk::with_bits(ac.coord, rect, bits));
    }
    out
}

fn cell_count(active: &[ActiveChunk]) -> usize {
    active.iter().map(|a| a.cell_count()).sum()
}

fn report(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();
    for _ in 0..WARMUP {
        tick_with_perf(&mut world, &perf);
    }

    let mut loose_cells = 0usize;
    let mut air_cells = 0usize;
    let mut samples = 0u64;
    let mut settle_base = Duration::ZERO;
    let mut settle_air = Duration::ZERO;

    for _ in 0..MEASURE {
        tick_with_perf(&mut world, &perf);
        let loose = filter_loose(&world, &plan_active(&world));
        if loose.is_empty() {
            continue;
        }
        samples += 1;
        loose_cells += cell_count(&loose);
        let air = keep_air_dest(&world, &loose);
        air_cells += cell_count(&air);

        // Time multi-pass settle on a cloned plan shape (same world — second
        // call sees a quieter world; ranks relative cost of the trim).
        let t0 = Instant::now();
        settle_loose_grains_regions(&mut world, &loose, None, GRAIN_SETTLE_PASSES_SHALLOW);
        settle_base += t0.elapsed();
        clear_all_dirty(&mut world);

        let loose2 = filter_loose(&world, &plan_active(&world));
        let air2 = keep_air_dest(&world, &loose2);
        let t0 = Instant::now();
        settle_loose_grains_regions(&mut world, &air2, None, GRAIN_SETTLE_PASSES_SHALLOW);
        settle_air += t0.elapsed();
        clear_all_dirty(&mut world);
    }

    let n = samples.max(1);
    println!("\n=== {label} ===  samples={samples}");
    println!(
        "  loose plan cells/call   {:>8.1}",
        loose_cells as f32 / n as f32
    );
    println!(
        "  air-dest cells/call     {:>8.1}  ({:.0}% of loose)",
        air_cells as f32 / n as f32,
        100.0 * air_cells as f32 / loose_cells.max(1) as f32
    );
    println!(
        "  settle ×{} on loose     {:>8.3} ms/call",
        GRAIN_SETTLE_PASSES_SHALLOW,
        ms(settle_base, n)
    );
    println!(
        "  settle ×{} on air-trim  {:>8.3} ms/call",
        GRAIN_SETTLE_PASSES_SHALLOW,
        ms(settle_air, n)
    );
}

#[test]
#[ignore = "diagnostic; run with --release --ignored --nocapture"]
fn settle_air_dest_fraction() {
    report("demo", WorldgenParams::default());
    report(
        "stress",
        WorldgenParams {
            width_cols: (CHUNK_CELLS_W as i32) * 32,
            sky_ceiling_y: (CHUNK_CELLS_H as i32) * 6,
            ..WorldgenParams::default()
        },
    );
}
