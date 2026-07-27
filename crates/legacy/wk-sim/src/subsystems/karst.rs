//! Karst dissolution: flux-driven soluble-layer removal + void growth.
//!
//! Dissolution is driven by **lateral water flux** through soluble layers
//! (not moisture-in-place). That concentrates cave growth along the water
//! table and yields coherent horizontal passages across columns.

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::{CHUNK_W};
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

fn inject_dissolved(world: &mut World, coord: i32, x_m: f32, y_m: f32, kg: i64) {
    if kg <= 0 {
        return;
    }
    let Some(chunk) = world.chunks.get_mut(&coord) else {
        return;
    };
    let Some(dissolved) = chunk.dissolved.as_mut() else {
        // No field — mass still left solid form; stay in dissolved_total
        // via a synthetic bump when fields are off (audit path).
        return;
    };
    let (cx, cy) = dissolved.0.world_to_cell(x_m, y_m);
    let vol = World::dissolved_cell_volume_m3(dissolved.0.cell_size_m).max(1e-6);
    let prev = dissolved.0.cell_at(cx, cy);
    dissolved.0.set_cell(cx, cy, prev + kg as f32 / vol);
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
                // Karst is a terrestrial cave process. Under the ocean the
                // surface_void_capture pass would then suck 35% of the sea
                // into every fresh void per tick, faster than LakeLevel
                // can refill — that's what made the ocean surface spike.
                // Use solid_bed_y (strips fluids *and* cavity height) —
                // climate_elevation still includes voids, so a submerged
                // limestone shelf with sea-cliff mouths looked emergent.
                if col.solid_bed_y() < sea_level - 0.25 {
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
            let base = world.chunks.get(&coord).map(|c| c.world_x_base()).unwrap_or(0);
            let x_m = (base + i as i32) as f32 * SAMPLE_WIDTH_M;

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
                inject_dissolved(world, coord, x_m, mid_y, removed);
                dissolved_out += removed;
            }
        }
    }

    if dissolved_out > 0 {
        world.mass_audit.dissolved_out_total += dissolved_out;
        if world.dissolved_fields_enabled {
            world.mass_audit.dissolved_total = world.dissolved_mass_kg();
        } else {
            // Without a field, dissolved mass still left solid form —
            // park it in dissolved_total so total_tracked stays closed.
            world.mass_audit.dissolved_total += dissolved_out;
        }
    }
}
