//! Chunk-boundary halo updates and outbox exchange.

use wk_world::CHUNK_W;
use wk_world::world::World;

use crate::buffer::{ChunkBoundaryOutbox, WorldTransferScratch};

pub fn update_halos(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let (left_c, right_c) = world.neighbor_chunks(coord);
        let left = world.chunks.get(&left_c).cloned();
        let right = world.chunks.get(&right_c).cloned();
        let chunk = world.chunks.get_mut(&coord).unwrap();
        chunk.update_halos_from_neighbors(left.as_ref(), right.as_ref());
    }
}

pub fn exchange_outboxes(world: &mut World, scratch: &WorldTransferScratch) -> i64 {
    let mut boundary_out = 0i64;
    let pairs: Vec<(i32, ChunkBoundaryOutbox)> = scratch
        .outbox
        .iter()
        .map(|(&c, o)| (c, o.clone()))
        .collect();

    for (coord, outbox) in pairs {
        let (left_c, right_c) = world.neighbor_chunks(coord);
        if let Some(right) = world.chunks.get_mut(&right_c) {
            right.inbox.water_in[0] += outbox.right_water;
            right.inbox.moisture_in[0] += outbox.right_moisture;
            right.inbox.sediment_in[0].add(
                outbox.right_sediment.dominant,
                outbox.right_sediment.total,
            );
        } else {
            boundary_out += outbox.right_water + outbox.right_sediment.total + outbox.right_moisture;
        }

        if let Some(left) = world.chunks.get_mut(&left_c) {
            let last = CHUNK_W - 1;
            left.inbox.water_in[last] += outbox.left_water;
            left.inbox.moisture_in[last] += outbox.left_moisture;
            left.inbox.sediment_in[last].add(
                outbox.left_sediment.dominant,
                outbox.left_sediment.total,
            );
        } else {
            boundary_out += outbox.left_water + outbox.left_sediment.total + outbox.left_moisture;
        }
    }
    boundary_out
}
