use wk_material::CHUNK_W;
use wk_world::world::World;

use crate::buffer::{CellTransferBuffer, WorldTransferScratch};

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

        let water_new = col.surface_water + buf.water_delta[i] + inbox_water;
        let mut water_new = if water_new < 0 {
            audit.boundary_out_total += -water_new;
            0
        } else {
            water_new
        };

        let moisture_new = col.moisture + buf.moisture_delta[i] + inbox_moisture;
        let moisture_new = moisture_new.max(0);
        // Discharge: if inflow (from infiltration or lateral groundwater
        // flow) would push this column's aquifer past capacity, the excess
        // has nowhere left to go underground and surfaces as a spring/seep
        // instead — this is what lets a saturated water table feed a lake
        // sitting above it, rather than water just vanishing once "full".
        let cap = col.moisture_cap();
        if moisture_new > cap {
            let overflow = moisture_new - cap;
            col.moisture = cap;
            water_new += overflow;
        } else {
            col.moisture = moisture_new;
        }
        col.surface_water = water_new;

        // Erosion: solid → suspended sediment (actual removed mass only)
        if buf.erosion_request[i] > 0 {
            let (removed, mat) = col.erode_from_top(buf.erosion_request[i]);
            if removed > 0 {
                col.sediment.add(mat, removed);
            }
        }

        let sed_new = col.sediment.total + buf.sediment_delta[i] + inbox_sed.total;
        if sed_new < 0 {
            audit.boundary_out_total += -sed_new;
            col.sediment.total = 0;
        } else {
            col.sediment.total = sed_new;
        }
        if inbox_sed.total > 0 {
            col.sediment.dominant = inbox_sed.dominant;
        }

        if buf.deposit_request[i] > 0 {
            let deposit = buf.deposit_request[i].min(col.sediment.total);
            if deposit > 0 {
                col.sediment.total -= deposit;
                col.deposit_to_top(buf.deposit_material[i], deposit, tick);
            }
        }

        if buf.snow_request[i] > 0 {
            col.deposit_to_top(wk_material::MaterialId::Snow, buf.snow_request[i], tick);
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
        assert_eq!(world.chunks.get(&0).unwrap().columns[0].surface_water, 100);
        assert_eq!(scratch.buffers.get(&0).unwrap().water_delta[0], 0);
    }
}
