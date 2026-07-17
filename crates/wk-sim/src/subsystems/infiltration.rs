//! Surface water soaking into the top porous solid layer.

use wk_material::{CHUNK_W, MaterialRegistry};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;
use crate::residual::ResidualAccumulator;

const INFILTRATION_COEFF: f32 = 0.01;
const HYDRAULIC_CONTACT_MIN_KG: i64 = 500;

/// Post-slump / post-barrier: re-saturate beds under deep free water.
/// Runs after slumping so empty-layer flushes cannot leave ocean beds dry
/// for a full infiltration period.
pub fn recharge_deep_water_tables(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let col = &mut world.chunks.get_mut(&coord).unwrap().columns[i];
            let available = col.top_water_mass();
            if available <= HYDRAULIC_CONTACT_MIN_KG {
                continue;
            }
            let cap = col.moisture_cap();
            let need = cap.saturating_sub(col.moisture);
            if need == 0 || available <= need {
                continue;
            }
            let took = col.take_water_from_cap(need);
            col.moisture += took;
        }
    }
}

pub fn run_infiltration(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let (activity, available, moisture, cap, perm) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                let base_perm = col
                    .top_porous_layer()
                    .map(|l| MaterialRegistry::props(l.material).permeability as f32 / 255.0)
                    .unwrap_or(0.0);
                let root = col.ecology.root_density.clamp(0.0, 1.0);
                let perm = base_perm * (1.0 + 0.8 * root);
                (
                    col.activity,
                    col.top_water_mass(),
                    col.moisture,
                    col.moisture_cap(),
                    perm,
                )
            };
            if activity == Activity::Dormant || available <= 0 || perm <= 0.0 {
                continue;
            }
            let need = cap.saturating_sub(moisture);
            if need == 0 {
                continue;
            }
            if available > HYDRAULIC_CONTACT_MIN_KG && available > need {
                scratch.buffer_mut(coord).infil_delta[i] += need;
                continue;
            }
            let rate = available as f32 * perm * INFILTRATION_COEFF;
            let col = world.chunks.get_mut(&coord).unwrap();
            let transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.infiltration, rate);
            let actual = transfer.min(available).min(need);
            if actual > 0 {
                scratch.buffer_mut(coord).infil_delta[i] += actual;
            }
        }
    }
}
