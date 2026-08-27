//! How many draw calls does terrain need if vertical runs are merged?
//!
//! The app draws terrain column by column, bottom to top. Strata are horizontal
//! layers, so consecutive cells in a column usually resolve to the same colour
//! and can share one rectangle. This measures the reduction on a real stamped
//! world using cell identity (material + sat) as the colour proxy — the app's
//! celestial key only perturbs the top few cells of a stack.
//!
//! ```text
//! cargo test -p wk-voxel --release --test draw_run_probe -- --ignored --nocapture
//! ```

use wk_material::MaterialId;
use wk_voxel::{is_standing_water, stamp_world, World, WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W};

/// Mirrors the app's run key: what makes two stacked cells share a rectangle.
///
/// Solid wetness is **quantized** into four buckets by the palette, so cells a
/// few sat apart still merge — the reason a continuous tint was rejected. Air
/// keeps its full sat ramp (the water colour is continuous).
fn visible(world: &World, params: &WorldgenParams, x: i32, y: i32) -> Option<(MaterialId, u8)> {
    let cell = world.get_cell(x, y)?;
    if cell.material == MaterialId::Air {
        if cell.sat.is_empty() || cell.sat.0 <= 32 {
            return None;
        }
        if y > params.sea_level_y && !is_standing_water(world, x, y) {
            return None;
        }
        return Some((cell.material, cell.sat.0));
    }
    let cap = wk_voxel::water_capacity_cell(cell, &world.hydro);
    let bucket = if cap == 0 || cell.sat.0 == 0 {
        0
    } else {
        (((cell.sat.0 as f32 / cap as f32).clamp(0.0, 1.0) * 4.0).floor() as u8).min(3)
    };
    Some((cell.material, bucket))
}

fn measure(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    // Wet the ground first: on a bone-dry world every cell is in wetness bucket
    // 0 and the quantization cannot be observed. Sea beds and shores soaking is
    // the representative case.
    let perf = wk_voxel::PerfConfig::default();
    for _ in 0..300 {
        wk_voxel::tick_with_perf(&mut world, &perf);
    }

    // Same walk, keyed on raw sat instead of the bucket — what a continuous
    // tint would have cost.
    let mut unquantized_runs = 0u64;
    for x in 0..params.width_cols {
        let mut open: Option<(MaterialId, u8)> = None;
        for y in params.bedrock_floor_y..params.sky_ceiling_y {
            let key = world
                .get_cell(x, y)
                .filter(|c| visible(&world, &params, x, y).is_some())
                .map(|c| (c.material, c.sat.0));
            if key.is_some() && key != open {
                unquantized_runs += 1;
            }
            open = key;
        }
    }

    let mut cells = 0u64;
    let mut runs = 0u64;
    let mut longest = 0u64;
    for x in 0..params.width_cols {
        let mut open: Option<(MaterialId, u8)> = None;
        let mut run_len = 0u64;
        for y in params.bedrock_floor_y..params.sky_ceiling_y {
            match visible(&world, &params, x, y) {
                Some(key) => {
                    cells += 1;
                    if open == Some(key) {
                        run_len += 1;
                    } else {
                        if open.is_some() {
                            longest = longest.max(run_len);
                        }
                        runs += 1;
                        run_len = 1;
                        open = Some(key);
                    }
                }
                None => {
                    if open.is_some() {
                        longest = longest.max(run_len);
                    }
                    open = None;
                    run_len = 0;
                }
            }
        }
        longest = longest.max(run_len);
    }

    println!("\n=== {label} ===  {}×{} cells", params.width_cols, params.sky_ceiling_y - params.bedrock_floor_y);
    println!("  painted cells (draw calls before)  {cells:>10}");
    println!("  merged runs   (draw calls after)   {runs:>10}");
    println!("  reduction                          {:>9.1}×", cells as f64 / runs.max(1) as f64);
    println!("  longest single run                 {longest:>10} cells");
    println!(
        "  unquantized (continuous tint)      {unquantized_runs:>10}  → only {:.1}×",
        cells as f64 / unquantized_runs.max(1) as f64
    );

    // The pore stipple is one sub-cell dot per qualifying cell and cannot be
    // merged into a run, so it is a draw call the run-batching cannot help.
    // Counted separately because it lands on top of the merged terrain.
    let mut stipples = 0u64;
    for x in 0..params.width_cols {
        for y in params.bedrock_floor_y..params.sky_ceiling_y {
            if visible(&world, &params, x, y).is_none() {
                continue;
            }
            let Some(cell) = world.get_cell(x, y) else {
                continue;
            };
            // Mirrors the app's `shows_pore_stipple`: conglomerate only, on
            // identity. Permeability moved to a quantized hue tint, which merges
            // into runs; speckling every permeable material cost 23k unmergeable
            // draw calls to say something sand already says by being sand.
            if cell.material == MaterialId::Conglomerate {
                stipples += 1;
            }
        }
    }
    println!("  pore stipple dots (unmergeable)    {stipples:>10}");
    println!(
        "  total draw calls                   {:>10}  ({:.0}% stipple)",
        runs + stipples,
        100.0 * stipples as f64 / (runs + stipples).max(1) as f64
    );
}

#[test]
#[ignore = "diagnostic probe; run explicitly"]
fn probe_draw_run_merging() {
    measure("demo", WorldgenParams::default());
    measure(
        "stress",
        WorldgenParams {
            width_cols: (CHUNK_CELLS_W as i32) * 32,
            sky_ceiling_y: (CHUNK_CELLS_H as i32) * 6,
            ..WorldgenParams::default()
        },
    );
}
