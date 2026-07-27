//! Erosion, suspended-sediment transport, and deposition.

use wk_material::{MaterialId, MaterialRegistry};
use wk_world::{CHUNK_W};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;

const EROSION_FLUX_COEFF: f32 = 0.004;
const SEDIMENT_CAPACITY_COEFF: f32 = 0.02;
/// Fraction of suspended sediment that settles out of the water column
/// onto the bed each tick, regardless of flow speed. Continuous Stokes-
/// like settling — without this term sediment carried by a permanently
/// tilted river never actually deposits: capacity-based rules only fire
/// when the flow stops entirely, which for a steady stream it never
/// does. Real particles fall at their terminal velocity even in fast
/// flow; fast flow just keeps re-eroding what fell. Rate is small
/// enough that the sediment still visibly travels a fair distance
/// downstream before it's fully out of the water.
const SEDIMENT_SETTLE_FRACTION: f32 = 0.008;

pub fn run_sediment(world: &World, scratch: &mut WorldTransferScratch, tick: u64) {
    let sea_level = world.sea_level;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        if !chunk.any_hydrology_active() {
            continue;
        }

        let mut erosion_record = [0i64; CHUNK_W];
        let mut erode_req = [0i64; CHUNK_W];
        let sed_delta = [0i64; CHUNK_W];
        let mut dep_req = [0i64; CHUNK_W];
        let mut dep_mat = [MaterialId::Sand; CHUNK_W];

        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            if col.activity == Activity::Dormant {
                continue;
            }
            // Submerged ocean bed: skip sediment settling AND erosion.
            // Neighbour water-top wobbles give `flux_indicator > 0.01`
            // which used to fire erosion here, and the mandatory
            // `settle.max(1)` re-deposited that 1 kg every tick as a
            // brand-new "sand" layer. The result was every ocean column
            // rebuilding a 1-kg sand cap at ~60 Hz — visible as blinking
            // moisture / surface_y / sand-age in the inspect panel and
            // spiky ocean rendering. Real underwater sedimentation is
            // slow enough that we can ignore it for game purposes.
            if col.climate_elevation() < sea_level - 0.5 {
                continue;
            }

            // Continuous Stokes-like settling: some fraction of any
            // suspended sediment falls onto the bed each tick, whether
            // or not the flow is fast. This is what makes eroded sand
            // actually accumulate as a deposit downstream instead of
            // circulating in suspension forever on a permanently-tilted
            // stream (fast flow will then re-erode most of it back into
            // suspension, matching how a real river bed exchanges
            // sediment continuously with its water column).
            if col.sediment.total > 0 && col.top_water_mass() > 0 {
                let settle = (col.sediment.total as f32 * SEDIMENT_SETTLE_FRACTION) as i64;
                let settle = settle.max(1).min(col.sediment.total);
                dep_req[i] += settle;
                dep_mat[i] = col.sediment.dominant;
            }

            let y_here = col.surface_y;
            let y_left = chunk.surface_y_neighbor(i as i32 - 1);
            let y_right = chunk.surface_y_neighbor(i as i32 + 1);
            let flux_indicator =
                ((y_here - y_left).max(0.0) + (y_here - y_right).max(0.0)).sqrt();

            // Under the unified model the top layer might be Water
            // (a river / pond). Erosion still targets the erodible bed
            // *underneath* that water, so we look for the top erodible
            // layer here rather than blindly taking `top_material`.
            let Some(bed_idx) = (0..col.layer_count as usize).find(|&j| {
                let m = col.layers[j].material;
                m.is_erodible() && MaterialRegistry::props(m).erosion_resistance < 150
            }) else {
                continue;
            };
            let material = col.layers[bed_idx].material;
            let props = MaterialRegistry::props(material);

            let water = col.top_water_mass().max(0) as f32;
            if water < 1.0 || flux_indicator < 0.01 {
                continue;
            }

            // Waterlogged instability: if the erodible bed *is* the
            // column's aquifer layer (top porous layer, the one
            // accumulating pore-water in `col.moisture`), its effective
            // cohesion collapses as saturation approaches 1. Physically
            // this is the pore-pressure collapse behind a sand dam or a
            // river bank slumping under a saturated shore — dry sand
            // holds its angle of repose; saturated sand doesn't.
            //
            // At full saturation resistance drops to ~30% of dry.
            let saturation = if col.top_porous_layer_index() == Some(bed_idx) {
                let cap = col.moisture_cap().max(1) as f32;
                (col.moisture as f32 / cap).clamp(0.0, 1.0)
            } else {
                0.0
            };
            // Roots bind the topsoil — dense mats substantially cut
            // erosion (stage 8 ecology feedback).
            let root_bind = 1.0 + 2.5 * col.ecology.root_density.clamp(0.0, 1.0);
            let effective_resistance =
                (props.erosion_resistance as f32) * (1.0 - saturation * 0.7) * root_bind;
            let erosion_rate =
                water * flux_indicator * EROSION_FLUX_COEFF / effective_resistance.max(1.0);
            let erode_mass = erosion_rate as i64;
            if erode_mass <= 0 {
                continue;
            }

            erode_req[i] += erode_mass;
            erosion_record[i] = erode_mass;

            let capacity = (water * flux_indicator * SEDIMENT_CAPACITY_COEFF * 1000.0) as i64;
            let current_sed = col.sediment.total + erode_req[i] + sed_delta[i];
            if current_sed > capacity {
                let excess = current_sed - capacity;
                dep_req[i] += excess;
                dep_mat[i] = material;
            }

            if flux_indicator < 0.05 && col.sediment.total > 50 {
                let deposit = (col.sediment.total / 16).max(1);
                dep_req[i] += deposit;
                dep_mat[i] = col.sediment.dominant;
            }
        }

        let buf = scratch.buffer_mut(coord);
        for i in 0..CHUNK_W {
            buf.erosion_request[i] += erode_req[i];
            buf.sediment_delta[i] += sed_delta[i];
            buf.deposit_request[i] += dep_req[i];
            if dep_req[i] > 0 {
                buf.deposit_material[i] = dep_mat[i];
            }
        }
        scratch.last_erosion_flux.insert(coord, erosion_record);
        let _ = tick;
    }
}
