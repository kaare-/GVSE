//! Karst dissolution: flux-driven soluble-layer removal + void growth.
//!
//! Dissolution is driven by **lateral water flux** through soluble layers
//! (not moisture-in-place). That concentrates cave growth along the water
//! table and yields coherent horizontal passages across columns.
//!
//! The old dissolved-mineral spatial field is gone. Removed rock mass
//! is booked to `MassAudit::dissolved_out_total` and later drawn back
//! into limestone by `run_speleogenesis` from the same counter, so the
//! solid → dissolved → solid loop still closes without a per-cell grid.

use wk_material::{CHUNK_W, MaterialId, MaterialRegistry};
use wk_world::column::{Activity, VoidOrigin};
use wk_world::world::World;

/// Game-tuned scale from flux proxy → kg dissolved per karst step.
/// Tuned so a wet limestone bed under a modest head gradient opens a
/// visible passage in a few thousand ticks.
const KARST_COEFF: f32 = 12.0;

/// Always-on wet-rock contribution (keeps dissolution alive after heads
/// nearly equalize — real aquifers still have seepage). Multiplied by
/// saturation × soluble-layer permeability.
const KARST_SEEPAGE: f32 = 8.0;

fn head_neighbor(world: &World, coord: i32, local: i32) -> f32 {
    let chunk = world.chunks.get(&coord).unwrap();
    if local < 0 {
        return chunk.water_table_neighbor(-1);
    }
    if local >= CHUNK_W as i32 {
        return chunk.water_table_neighbor(CHUNK_W as i32);
    }
    chunk.columns[local as usize].water_table_y()
}

/// Post-barrier direct mutation: dissolve soluble rock under lateral flux
/// and grow voids so caves open without putting Air into the layer stack.
pub fn run_karst(world: &mut World, _tick: u64) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    let sea_level = world.sea_level;
    let mut dissolved_out = 0i64;

    for coord in coords {
        let actions: Vec<(usize, usize, i64, f32, MaterialId)> = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let mut out = Vec::new();
            for i in 0..CHUNK_W {
                let col = &chunk.columns[i];
                if col.activity == Activity::Dormant {
                    continue;
                }
                // Need moving pore water — dry rock doesn't karst.
                if col.moisture <= 0 {
                    continue;
                }
                // Karst is a terrestrial cave process. Submerged shelf
                // beds skip so we don't dissolve rock into the ocean.
                if col.climate_elevation() < sea_level - 0.25 {
                    continue;
                }
                let head_here = col.water_table_y();
                let head_left = head_neighbor(world, coord, i as i32 - 1);
                let head_right = head_neighbor(world, coord, i as i32 + 1);
                let grad = (head_here - head_left).abs() + (head_here - head_right).abs();
                let cap = col.moisture_cap().max(1) as f32;
                let sat = (col.moisture as f32 / cap).clamp(0.0, 1.0);
                // Lateral flux proxy + residual seepage through wet rock.
                let flux = grad * col.moisture as f32 * 0.05 + KARST_SEEPAGE * sat;
                if flux < 0.5 {
                    continue;
                }

                for li in 0..col.layer_count as usize {
                    let layer = col.layers[li];
                    let props = MaterialRegistry::props(layer.material);
                    let sol = props.solubility;
                    if sol == 0 || layer.thickness <= 0 {
                        continue;
                    }
                    let perm = props.permeability as f32 / 255.0;
                    let rate = flux * (sol as f32 / 255.0) * perm * KARST_COEFF;
                    let take = (rate as i64).max(0).min(layer.thickness);
                    if take > 0 {
                        let (top, bot) = col.layer_y_range(li, chunk.bedrock_y);
                        let mid = 0.5 * (top + bot);
                        out.push((i, li, take, mid, layer.material));
                    }
                }
            }
            out
        };

        for (i, li, take, mid_y, mat) in actions {
            let bedrock = world.chunks.get(&coord).map(|c| c.bedrock_y).unwrap_or(0.0);

            let removed = {
                let Some(chunk) = world.chunks.get_mut(&coord) else {
                    continue;
                };
                let col = &mut chunk.columns[i];
                if li >= col.layer_count as usize {
                    continue;
                }
                if col.layers[li].material != mat {
                    continue;
                }
                let t = take.min(col.layers[li].thickness);
                if t <= 0 {
                    continue;
                }
                let dh = col.mass_to_height_delta(mat, t);
                col.layers[li].thickness -= t;
                // Grow void by the height vacated so surface_y holds.
                // Roof is the soluble rock itself — caves form *inside*
                // limestone; the sand/soil cap above is not the cave roof
                // until the void breaches the top of the soluble bed.
                if dh > 1e-5 {
                    col.grow_void_at(mid_y, dh, mat, VoidOrigin::Karst);
                }
                // Drop empty layers.
                if col.layers[li].thickness <= 0 {
                    for j in li..(col.layer_count as usize).saturating_sub(1) {
                        col.layers[j] = col.layers[j + 1];
                    }
                    if col.layer_count > 0 {
                        col.layer_count -= 1;
                        col.layers[col.layer_count as usize] = Default::default();
                    }
                }
                col.activity = Activity::HydrologyActive;
                col.recompute_surface_y(bedrock);
                t
            };
            if removed > 0 {
                dissolved_out += removed;
            }
        }
    }

    if dissolved_out > 0 {
        world.mass_audit.dissolved_out_total += dissolved_out;
    }
}
