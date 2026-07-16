//! Evaporation of surface water and pore moisture.

use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;
use crate::residual::ResidualAccumulator;

const EVAPORATION_COEFF: f32 = 0.035;
/// Fallback RH when the humidity field is disabled (pre-6.3 behaviour).
const FALLBACK_HUMIDITY: f32 = 0.4;
/// RH source (per second) injected per kg of evaporated water, scaled
/// down so a lake doesn't instantly saturate the whole chunk.
const EVAP_VAPOR_PER_KG: f32 = 0.00005;

pub fn run_evaporation(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        // Collect vapor injections after the buffered mass deltas so we
        // don't hold a mut borrow across both paths.
        let mut vapor: Vec<(i32, f32, f32)> = Vec::new();
        for i in 0..CHUNK_W {
            let (activity, surface_water, moisture, surface_y, humidity) = {
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
                )
            };
            if activity == Activity::Dormant {
                continue;
            }
            let evap_factor = (1.0 - humidity).max(0.0);
            let from_surface =
                (surface_water as f32 * EVAPORATION_COEFF * evap_factor).max(0.0);
            let from_moisture =
                (moisture as f32 * EVAPORATION_COEFF * 0.5 * evap_factor).max(0.0);

            let col = world.chunks.get_mut(&coord).unwrap();
            let surf_transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.evaporation, from_surface);
            let moist_transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.evaporation, from_moisture);

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
                    // Inject just above the surface into the air band.
                    vapor.push((wx, surface_y + 1.0, vapor_amt));
                }
            }
        }
        for (wx, y, amt) in vapor {
            world.inject_humidity_source(wx, y, amt);
        }
    }
}
