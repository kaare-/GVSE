//! Evaporation of surface water and pore moisture.

use wk_material::CHUNK_W;
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;
use crate::residual::ResidualAccumulator;

const EVAPORATION_COEFF: f32 = 0.035;
const HUMIDITY: f32 = 0.4;

pub fn run_evaporation(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let (activity, surface_water, moisture) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                (col.activity, col.top_water_mass(), col.moisture)
            };
            if activity == Activity::Dormant {
                continue;
            }
            let evap_factor = 1.0 - HUMIDITY;
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
            }
        }
    }
}
