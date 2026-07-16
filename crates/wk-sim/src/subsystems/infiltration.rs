//! Surface water soaking into the top porous solid layer.

use wk_material::{CHUNK_W, MaterialRegistry};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;
use crate::residual::ResidualAccumulator;

// Slow enough that standing water lingers as a visible pool for a while
// instead of soaking away within a few dozen ticks, but fast enough that a
// basin collecting runoff from a whole mountainside still reaches a real
// equilibrium below the surrounding peaks instead of climbing forever.
const INFILTRATION_COEFF: f32 = 0.01;

pub fn run_infiltration(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let (activity, available, moisture, cap, perm) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                // Permeability comes from the porous *substrate* that
                // will absorb the water, not the material sitting on
                // top (which is Water itself, permeability 0, which
                // would incorrectly block all infiltration under a
                // puddle). Root channels (stage 8) boost the effective
                // rate without rewriting the material table.
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
            let rate = available as f32 * perm * INFILTRATION_COEFF;
            let col = world.chunks.get_mut(&coord).unwrap();
            let transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.infiltration, rate);
            let actual = transfer.min(available).min(cap.saturating_sub(moisture));
            if actual > 0 {
                let buf = scratch.buffer_mut(coord);
                buf.water_delta[i] -= actual;
                buf.moisture_delta[i] += actual;
            }
        }
    }
}
