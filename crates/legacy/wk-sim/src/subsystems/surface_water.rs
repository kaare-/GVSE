//! Lateral surface-water flow under hydrostatic head gradients.

use wk_material::{MaterialId};
use wk_world::{CHUNK_W};
use wk_world::column::{Activity, SedimentLoad};
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

/// Fraction of the "exact equalizing transfer" actually applied each tick.
/// This is mathematically equivalent to the diffusion parameter r in the
/// standard explicit scheme d' = d + r*(neighbour - 2*d + other_neighbour),
/// with r = FLOW_RELAXATION/2. Von Neumann stability requires r <= 0.5, i.e.
/// FLOW_RELAXATION <= 1.0 — at exactly 1.0 the worst-case (checkerboard)
/// mode is damped to zero in a single tick, the fastest possible speed with
/// no oscillation. Above ~1.0 it starts ringing again (the earlier bug);
/// staying at 0.9 keeps a small safety margin for the asymmetric real-world
/// coupling (unequal neighbours, ground slope) that a pure symmetric
/// analysis doesn't capture exactly.
const FLOW_RELAXATION: f32 = 0.97;
/// Surface-waves mode is tide-only now (no momentum seiche to protect),
/// so keep lateral equalization almost as strong as the non-wave path.
/// The old 0.08 value left shelf-edge spikes that the tide re-poked every
/// tick — "standing waves" that faded then came back.
const FLOW_RELAXATION_WITH_WAVES: f32 = 0.85;

pub fn run_surface_water(world: &World, scratch: &mut WorldTransferScratch) {
    let relax = if world.surface_waves_enabled {
        FLOW_RELAXATION_WITH_WAVES
    } else {
        FLOW_RELAXATION
    };
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        if !chunk.any_hydrology_active() {
            continue;
        }

        let mut flux_record = [0i64; CHUNK_W];
        // (out_left, out_right, net_water,
        //  sed_left, sed_right, sed_material)
        let mut deltas =
            [(0i64, 0i64, 0i64, 0i64, 0i64, MaterialId::Sand); CHUNK_W];

        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            // Flow considers any Water in the fluid cap — even water
            // sitting under a Snow or Ice cap can drain sideways, since
            // those caps float on the water rather than sealing it in.
            // Without this a snowfall on a pool would build an unbounded
            // water tower: rain deposits Water on top, settle sinks it
            // below the snow, and (under the old top-only rule) flow
            // then couldn't touch it.
            let Some((water_top_y, water_here)) = col.flowable_water() else {
                continue;
            };
            if col.activity == Activity::Dormant || water_here <= 0 {
                continue;
            }
            // NOTE: no more `skip_deep` bailout. Every wet column
            // diffuses at `relax` (0.08 in wave mode, 0.97 otherwise).
            // The whole-column skip was the bug that let tall pools
            // sit on a single column as an isolated spike.

            let head_here = water_top_y;
            let head_left = chunk.water_top_neighbor(i as i32 - 1);
            let head_right = chunk.water_top_neighbor(i as i32 + 1);

            let grad_left = head_here - head_left;
            let grad_right = head_here - head_right;

            let mut out_left = 0i64;
            let mut out_right = 0i64;

            if grad_left > 0.0 {
                let equalizing = grad_left * WATER_MASS_PER_METRE_DEPTH * 0.5;
                out_left = ((equalizing * relax) as i64).min(water_here);
            }
            if grad_right > 0.0 {
                let remaining = (water_here - out_left).max(0);
                let equalizing = grad_right * WATER_MASS_PER_METRE_DEPTH * 0.5;
                out_right = ((equalizing * relax) as i64).min(remaining);
            }

            // Sediment travels with water proportionally. If half the
            // column's water flows left, half its suspended sediment
            // goes with it — that's how a river carries silt downstream
            // rather than leaving it stranded at the erosion site.
            let (sed_left, sed_right) = if col.sediment.total > 0 {
                let sed = col.sediment.total;
                let sl = (out_left as i128 * sed as i128 / water_here as i128) as i64;
                let sr = (out_right as i128 * sed as i128 / water_here as i128) as i64;
                // Never send more sediment than the column has.
                let (sl, sr) = if sl + sr > sed {
                    let total = sl + sr;
                    let sl2 = (sl as i128 * sed as i128 / total as i128) as i64;
                    let sr2 = sed - sl2;
                    (sl2, sr2)
                } else {
                    (sl, sr)
                };
                (sl, sr)
            } else {
                (0, 0)
            };

            deltas[i] = (
                out_left,
                out_right,
                -(out_left + out_right),
                sed_left,
                sed_right,
                col.sediment.dominant,
            );
            flux_record[i] = out_left + out_right;
        }

        let mut left_water_outbox = 0i64;
        let mut right_water_outbox = 0i64;
        let mut left_sed_outbox = SedimentLoad::default();
        let mut right_sed_outbox = SedimentLoad::default();

        for i in 0..CHUNK_W {
            let (out_left, out_right, net, sed_left, sed_right, sed_mat) = deltas[i];
            let buf = scratch.buffer_mut(coord);
            buf.water_delta[i] += net;
            buf.sediment_delta[i] -= sed_left + sed_right;

            if i == 0 {
                left_water_outbox += out_left;
                if sed_left > 0 {
                    left_sed_outbox.add(sed_mat, sed_left);
                }
            } else {
                buf.water_delta[i - 1] += out_left;
                if sed_left > 0 {
                    buf.sediment_inflow[i - 1].add(sed_mat, sed_left);
                }
            }

            if i == CHUNK_W - 1 {
                right_water_outbox += out_right;
                if sed_right > 0 {
                    right_sed_outbox.add(sed_mat, sed_right);
                }
            } else {
                buf.water_delta[i + 1] += out_right;
                if sed_right > 0 {
                    buf.sediment_inflow[i + 1].add(sed_mat, sed_right);
                }
            }
        }
        let outbox = scratch.outbox_mut(coord);
        outbox.left_water += left_water_outbox;
        outbox.right_water += right_water_outbox;
        if left_sed_outbox.total > 0 {
            outbox
                .left_sediment
                .add(left_sed_outbox.dominant, left_sed_outbox.total);
        }
        if right_sed_outbox.total > 0 {
            outbox
                .right_sediment
                .add(right_sed_outbox.dominant, right_sed_outbox.total);
        }
        scratch.last_water_flux.insert(coord, flux_record);
    }
}
