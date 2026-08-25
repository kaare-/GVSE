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

/// Mirrors the app's "is this cell painted at all" rule.
fn visible(world: &World, params: &WorldgenParams, x: i32, y: i32) -> Option<(MaterialId, u8)> {
    let cell = world.get_cell(x, y)?;
    if cell.material == MaterialId::Air {
        if cell.sat.is_empty() || cell.sat.0 <= 32 {
            return None;
        }
        if y > params.sea_level_y && !is_standing_water(world, x, y) {
            return None;
        }
    }
    Some((cell.material, cell.sat.0))
}

fn measure(label: &str, params: WorldgenParams) {
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);

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
