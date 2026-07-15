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
        let col = &mut chunk.columns[i];
        let inbox_water = chunk.inbox.water_in[i];
        let inbox_sed = chunk.inbox.sediment_in[i];
        let inbox_moisture = chunk.inbox.moisture_in[i];

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
        let _ = col.adjust_top_water(requested_delta, tick);

        // Moisture: pore-water in the topmost porous solid layer.
        let moisture_new = col.moisture + buf.moisture_delta[i] + inbox_moisture;
        let moisture_new = moisture_new.max(0);
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

        // Deposition: sediment falls out and settles on the bed. Goes
        // *underneath* any Water/Ice/Snow cap so the puddle above stays
        // on top (physically it would just refill from surrounding
        // water anyway); depositing on top would create the [Sand,
        // Water, Sand, ...] sandwich stacks that inflate surface_y and
        // trap water inside solid stratigraphy.
        if buf.deposit_request[i] > 0 {
            let deposit = buf.deposit_request[i].min(col.sediment.total);
            if deposit > 0 {
                col.sediment.total -= deposit;
                col.deposit_below_fluid_cap(buf.deposit_material[i], deposit, tick);
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

    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        if let Some(buf) = scratch.buffers.get(&coord).cloned() {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                commit_chunk_buffer(chunk, &buf, tick, &mut world.mass_audit);
            }
        }
    }

    world.mass_audit.boundary_out_total += boundary_out;
    world.mass_audit.tick = tick;
    world.recompute_mass_audit();
    crate::subsystems::update_halos(world);
    scratch.clear();
}

#[cfg(test)]
mod tests {
    use super::*;
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
}
