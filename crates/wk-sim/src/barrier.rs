use wk_material::{CHUNK_W, MaterialId};
use wk_world::world::World;

use crate::buffer::{CellTransferBuffer, WorldTransferScratch};

/// Apply all buffered per-column deltas to one chunk. Under the unified
/// material model, `water_delta` and `snow_request` don't manipulate
/// special fields on the column any more — they translate into edits on
/// the layer stack (grow / shrink / insert Water, Ice, Snow layers).
pub fn commit_chunk_buffer(
    chunk: &mut wk_world::chunk::Chunk,
    buf: &CellTransferBuffer,
    tick: u64,
    audit: &mut wk_world::world::MassAudit,
) {
    for i in 0..CHUNK_W {
        let inbox_water = chunk.inbox.water_in[i];
        let inbox_sed = chunk.inbox.sediment_in[i];
        let inbox_moisture = chunk.inbox.moisture_in[i];
        // Fast path: nothing to apply here. Skip the whole per-column
        // apply/clamp/recompute chain — on a full 192-chunk ring most
        // ocean columns have no delta any given tick, and clamp+recompute
        // over all 12 288 columns per tick added ~2 ms to barrier_commit.
        if inbox_water == 0
            && inbox_sed.total == 0
            && inbox_moisture == 0
            && buf.water_delta[i] == 0
            && buf.moisture_delta[i] == 0
            && buf.infil_delta[i] <= 0
            && buf.erosion_request[i] == 0
            && buf.sediment_delta[i] == 0
            && buf.sediment_inflow[i].total == 0
            && buf.deposit_request[i] == 0
            && buf.snow_request[i] == 0
        {
            continue;
        }
        let col = &mut chunk.columns[i];

        // Water delta: positive grows a Water layer on top; negative
        // drains water off the top Water layer, up to what's there.
        //
        // Bookkeeping subtlety: the individual subsystems (evap, flow,
        // infiltration) each booked their contribution independently
        // into rain_inject_total / evap_out_total / etc. If their sum
        // asks to drain *more* than this column actually has, the
        // shortfall has to be booked somewhere or the mass-audit
        // equation `current = initial + rain - evap - boundary` breaks.
        // Convention (kept from before the unification): unbookable
        // overshoot goes to boundary_out_total, i.e. the audit records
        // it as "mass left the world via the boundary". No layer-level
        // effect since there's no mass to remove.
        let requested_delta = buf.water_delta[i] + inbox_water;
        let applied = col.adjust_top_water(requested_delta, tick);
        if requested_delta < 0 {
            let shortfall = (-requested_delta) - (-applied);
            if shortfall > 0 {
                // Outboxes/inboxes were exchanged optimistically. If this
                // column couldn't fund its outflow, the receiver still got
                // the water — minting mass. Prefer reversing sink bookings
                // (boundary, then evap); residual becomes a synthetic source.
                let mut left = shortfall;
                let undo_b = left.min(audit.boundary_out_total.max(0));
                audit.boundary_out_total -= undo_b;
                left -= undo_b;
                let undo_e = left.min(audit.evap_out_total.max(0));
                audit.evap_out_total -= undo_e;
                left -= undo_e;
                audit.rain_inject_total += left;
            }
        }

        let infil = buf.infil_delta[i].max(0);
        let infil_applied = if infil > 0 {
            col.take_water_from_cap(infil)
        } else {
            0
        };

        // Moisture: pore-water in the topmost porous solid layer.
        let moisture_new =
            (col.moisture + buf.moisture_delta[i] + inbox_moisture + infil_applied).max(0);
        let cap = col.moisture_cap();
        if moisture_new > cap {
            // Discharge: pore space is full. The overflow surfaces as
            // standing water on top (spring/seep) — it becomes an
            // ordinary Water layer just like rain would.
            let overflow = moisture_new - cap;
            col.moisture = cap;
            col.deposit_to_top(MaterialId::Water, overflow, tick);
        } else {
            col.moisture = moisture_new;
        }

        // Erosion: solid → suspended sediment (actual removed mass only).
        if buf.erosion_request[i] > 0 {
            let (removed, mat) = col.erode_from_top(buf.erosion_request[i]);
            if removed > 0 {
                col.sediment.add(mat, removed);
            }
        }

        // Sediment outflow: `sediment_delta` is negative when water flow
        // has carried this column's sediment away to a neighbour. If the
        // outflow request exceeds what's actually here (subsystems all
        // clamped independently), the shortfall is booked as boundary
        // loss so the audit equation stays exact.
        let sed_new = col.sediment.total + buf.sediment_delta[i];
        if sed_new < 0 {
            audit.boundary_out_total += -sed_new;
            col.sediment.total = 0;
        } else {
            col.sediment.total = sed_new;
        }

        // Sediment inflow: intra-chunk (buf.sediment_inflow) and
        // cross-chunk (chunk.inbox.sediment_in) both preserve the
        // incoming material identity — see SedimentLoad::add.
        let intra_inflow = buf.sediment_inflow[i];
        if intra_inflow.total > 0 {
            col.sediment.add(intra_inflow.dominant, intra_inflow.total);
        }
        if inbox_sed.total > 0 {
            col.sediment.add(inbox_sed.dominant, inbox_sed.total);
        }

        // Deposition: sediment falls out. Just place it on top — the
        // density settle inside `clamp_state` at the end of this loop
        // will sink dense grains (sand/clay/stone) through any lighter
        // fluid cap (water/ice/snow) automatically, so no special-case
        // "insert below fluid cap" is needed.
        if buf.deposit_request[i] > 0 {
            let deposit = buf.deposit_request[i].min(col.sediment.total);
            if deposit > 0 {
                col.sediment.total -= deposit;
                col.deposit_to_top(buf.deposit_material[i], deposit, tick);
            }
        }

        // Snowfall is just a Snow layer deposited on top — same
        // mechanism as any other precipitation now.
        if buf.snow_request[i] > 0 {
            col.deposit_to_top(MaterialId::Snow, buf.snow_request[i], tick);
        }

        col.clamp_state();
        col.recompute_surface_y(chunk.bedrock_y);
    }
    chunk.inbox.clear();
}

pub fn barrier_commit(world: &mut World, scratch: &mut WorldTransferScratch, tick: u64) {
    let boundary_out = crate::subsystems::exchange_outboxes(world, scratch);

    // Borrow buffer immutably + chunk mutably in the same pass — no need
    // to clone the ~4 kB `CellTransferBuffer` per chunk each tick.
    let audit = &mut world.mass_audit;
    for (coord, buf) in scratch.buffers.iter() {
        if let Some(chunk) = world.chunks.get_mut(coord) {
            commit_chunk_buffer(chunk, buf, tick, audit);
        }
    }

    world.mass_audit.boundary_out_total += boundary_out;
    world.mass_audit.tick = tick;
    // The sim step will run `recompute_mass_audit` once at end-of-tick;
    // don't duplicate that here (was a hidden ~1 ms/tick full-ring walk).
    crate::subsystems::update_halos(world);
    scratch.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;
    use wk_world::terrain::generate_flat_sand;
    use wk_world::world::World;

    #[test]
    fn buffer_apply_changes_column() {
        let mut world = World::new(42);
        world.insert_chunk(generate_flat_sand(0, 0.0, 20.0));
        let mut scratch = WorldTransferScratch::default();
        {
            let buf = scratch.buffer_mut(0);
            buf.water_delta[0] = 100;
        }
        barrier_commit(&mut world, &mut scratch, 1);
        assert_eq!(
            world.chunks.get(&0).unwrap().columns[0].top_water_mass(),
            100
        );
        assert_eq!(scratch.buffers.get(&0).unwrap().water_delta[0], 0);
    }

    #[test]
    fn atomic_infil_conserves_water_plus_moisture() {
        let mut world = World::new(1);
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        for i in 0..64 {
            if let Some(col) = world.column_at_mut(i) {
                col.deposit_to_top(MaterialId::Water, 5_000, 0);
                col.moisture = 0;
            }
        }
        world.wake_all();
        let before: i64 = (0..64)
            .map(|i| {
                let c = world.column_at(i).unwrap();
                c.top_water_mass() + c.moisture
            })
            .sum();
        let mut scratch = WorldTransferScratch::default();
        for i in 0..64 {
            scratch.buffer_mut(0).infil_delta[i] = 1_000;
        }
        barrier_commit(&mut world, &mut scratch, 1);
        let after: i64 = (0..64)
            .map(|i| {
                let c = world.column_at(i).unwrap();
                c.top_water_mass() + c.moisture
            })
            .sum();
        assert_eq!(before, after, "infil must conserve water+moisture");
    }
}
