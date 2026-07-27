//! Speleothem growth: reprecipitate dissolved minerals inside voids.
//!
//! Closes the karst mass loop: solid → dissolved → solid (Limestone),
//! shrinking voids as floors/ceilings accrete.

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::{CHUNK_W};
use wk_world::column::Activity;
use wk_world::world::World;

/// Fraction of local dissolved mass converted per speleogenesis step.
const SPELEO_FRAC: f32 = 0.02;

/// Minimum kg to bother precipitating.
const MIN_PRECIP_KG: i64 = 1;

pub fn run_speleogenesis(world: &mut World, tick: u64) {
    if !world.dissolved_fields_enabled {
        return;
    }
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    let mut returned = 0i64;

    for coord in coords {
        let actions: Vec<(usize, usize, i64, f32, f32)> = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(dissolved) = &chunk.dissolved else {
                continue;
            };
            let base = chunk.world_x_base();
            let mut out = Vec::new();
            for i in 0..CHUNK_W {
                let col = &chunk.columns[i];
                for (vi, v) in col.voids.iter().enumerate() {
                    if v.height_m <= 0.05 {
                        continue;
                    }
                    // Prefer damp voids (water present) or ventilated ones
                    // (evaporation-driven precipitation).
                    if v.water_mass <= 0 && v.light < 20 {
                        continue;
                    }
                    let x_m = (base + i as i32) as f32 * SAMPLE_WIDTH_M;
                    let y = v.mid_y();
                    let (cx, cy) = dissolved.0.world_to_cell(x_m, y);
                    let conc = dissolved.0.cell_at(cx, cy).max(0.0);
                    let vol = World::dissolved_cell_volume_m3(dissolved.0.cell_size_m).max(1e-6);
                    let available = (conc * vol) as i64;
                    let take = ((available as f32) * SPELEO_FRAC) as i64;
                    if take >= MIN_PRECIP_KG {
                        out.push((i, vi, take, x_m, y));
                    }
                }
            }
            out
        };

        for (i, vi, take, x_m, y) in actions {
            let bedrock = world.chunks.get(&coord).map(|c| c.bedrock_y).unwrap_or(0.0);
            // Remove from dissolved field first.
            let removed = {
                let Some(chunk) = world.chunks.get_mut(&coord) else {
                    continue;
                };
                let Some(dissolved) = chunk.dissolved.as_mut() else {
                    continue;
                };
                let (cx, cy) = dissolved.0.world_to_cell(x_m, y);
                let vol = World::dissolved_cell_volume_m3(dissolved.0.cell_size_m).max(1e-6);
                let prev = dissolved.0.cell_at(cx, cy).max(0.0);
                let avail = (prev * vol) as i64;
                let t = take.min(avail);
                if t <= 0 {
                    0
                } else {
                    dissolved
                        .0
                        .set_cell(cx, cy, (prev - t as f32 / vol).max(0.0));
                    t
                }
            };
            if removed <= 0 {
                continue;
            }

            let Some(chunk) = world.chunks.get_mut(&coord) else {
                continue;
            };
            let col = &mut chunk.columns[i];
            if vi >= col.voids.len() {
                // Field already reduced — put mass back if void vanished.
                if let Some(dissolved) = chunk.dissolved.as_mut() {
                    let (cx, cy) = dissolved.0.world_to_cell(x_m, y);
                    let vol = World::dissolved_cell_volume_m3(dissolved.0.cell_size_m).max(1e-6);
                    let prev = dissolved.0.cell_at(cx, cy);
                    dissolved.0.set_cell(cx, cy, prev + removed as f32 / vol);
                }
                continue;
            }
            let density = MaterialRegistry::props(MaterialId::Limestone).density.max(1) as f32;
            let dh = (removed as f32 / density) / SAMPLE_WIDTH_M;
            let dh = dh.min(col.voids[vi].height_m * 0.5);
            col.voids[vi].height_m = (col.voids[vi].height_m - dh).max(0.0);
            col.voids[vi].top_y -= dh * 0.5;
            // Accrete limestone into the solid stack.
            col.deposit_to_top(MaterialId::Limestone, removed, tick);
            col.activity = Activity::HydrologyActive;
            col.recompute_surface_y(bedrock);
            returned += removed;
        }
    }

    if returned > 0 {
        world.mass_audit.dissolved_return_total += returned;
        world.mass_audit.dissolved_total = world.dissolved_mass_kg();
    }
}
