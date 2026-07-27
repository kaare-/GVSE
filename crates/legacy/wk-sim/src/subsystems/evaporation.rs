//! Evaporation of surface water and pore moisture.

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::{CHUNK_W};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;
use crate::residual::ResidualAccumulator;

const EVAPORATION_COEFF: f32 = 0.03;
const FALLBACK_HUMIDITY: f32 = 0.4;
const EVAP_VAPOR_PER_KG: f32 = 0.00005;
const EVAP_SKIN_DEPTH_M: f32 = 0.08;
/// Pore moisture is buffered soil water, not a free surface. Old
/// `0.5 * et` emptied a plant-covered hill in ~1 s once the organic
/// beach cap shrank `moisture_cap`. Keep canopy ET, but much slower.
const PORE_EVAP_MULT: f32 = 0.04;

fn evap_skin_kg() -> f32 {
    let density = MaterialRegistry::props(MaterialId::Water).density.max(1) as f32;
    EVAP_SKIN_DEPTH_M * SAMPLE_WIDTH_M * density
}

pub fn run_evaporation(world: &mut World, scratch: &mut WorldTransferScratch) {
    let skin_kg = evap_skin_kg();
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let mut vapor: Vec<(i32, f32, f32)> = Vec::new();
        for i in 0..CHUNK_W {
            let (activity, surface_water, moisture, surface_y, humidity, leaf) = {
                let chunk = world.chunks.get(&coord).unwrap();
                let col = &chunk.columns[i];
                let wx = chunk.world_x_base() + i as i32;
                let x_m = wx as f32 * SAMPLE_WIDTH_M;
                let humidity = chunk
                    .humidity
                    .as_ref()
                    .map(|h| h.0.sample_bilinear(x_m, col.surface_y).clamp(0.0, 1.0))
                    .unwrap_or(FALLBACK_HUMIDITY);
                (
                    col.activity,
                    col.top_water_mass(),
                    col.moisture,
                    col.surface_y,
                    humidity,
                    col.ecology.leaf_area.clamp(0.0, 1.0),
                )
            };
            if activity == Activity::Dormant {
                continue;
            }
            let evap_factor = (1.0 - humidity).max(0.0);
            // Mild canopy boost — not a 2× flash-dry multiplier.
            let et = 1.0 + 0.35 * leaf;
            let exposed = (surface_water as f32).min(skin_kg);
            let from_surface = (exposed * EVAPORATION_COEFF * evap_factor).max(0.0);
            let from_moisture = if surface_water > 0 {
                0.0
            } else {
                (moisture as f32 * EVAPORATION_COEFF * PORE_EVAP_MULT * evap_factor * et).max(0.0)
            };

            let col = world.chunks.get_mut(&coord).unwrap();
            let total_rate = from_surface + from_moisture;
            let total_transfer = ResidualAccumulator::drain(
                &mut col.columns[i].residual.evaporation,
                total_rate,
            );
            let (surf_transfer, moist_transfer) = if total_rate > 0.0 {
                let surf_share = from_surface / total_rate;
                let s = ((total_transfer as f32) * surf_share).round() as i64;
                (s, total_transfer - s)
            } else {
                (0, 0)
            };

            let surf_actual = surf_transfer.min(surface_water);
            let moist_actual = moist_transfer.min(moisture);

            if surf_actual > 0 || moist_actual > 0 {
                let buf = scratch.buffer_mut(coord);
                buf.water_delta[i] -= surf_actual;
                buf.moisture_delta[i] -= moist_actual;
                world.mass_audit.evap_out_total += surf_actual + moist_actual;
                let vapor_amt = (surf_actual + moist_actual) as f32 * EVAP_VAPOR_PER_KG;
                if vapor_amt > 0.0 {
                    let wx = world.chunks.get(&coord).unwrap().world_x_base() + i as i32;
                    vapor.push((wx, surface_y + 1.0, vapor_amt));
                }
            }
        }
        for (wx, y, amt) in vapor {
            world.inject_humidity_source(wx, y, amt);
        }
    }
}
