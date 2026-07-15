use std::collections::BTreeMap;

use wk_material::{CHUNK_W, MaterialId};
use wk_world::column::SedimentLoad;

#[derive(Debug, Clone)]
pub struct CellTransferBuffer {
    pub water_delta: [i64; CHUNK_W],
    pub moisture_delta: [i64; CHUNK_W],
    /// Net signed change (a subsystem removing mass this tick logs a
    /// negative value here; incoming payloads with material identity
    /// go through `sediment_inflow` instead so we don't lose track of
    /// what the material actually is).
    pub sediment_delta: [i64; CHUNK_W],
    /// Sediment arriving into this column *within the same chunk*
    /// (cross-chunk transfer uses the `ChunkBoundaryOutbox` /
    /// `ChunkInbox.sediment_in` pair). Each accumulator remembers
    /// both the total incoming mass and the first material that
    /// arrived, so a sand river doesn't accidentally get reclassified
    /// as clay when it flows into a downstream column.
    pub sediment_inflow: [SedimentLoad; CHUNK_W],
    pub erosion_request: [i64; CHUNK_W],
    pub deposit_request: [i64; CHUNK_W],
    pub deposit_material: [MaterialId; CHUNK_W],
    /// Cold-weather precipitation to deposit as a Snow layer this tick
    /// (kept separate from deposit_request, which pulls from suspended
    /// sediment rather than creating new mass from precipitation).
    pub snow_request: [i64; CHUNK_W],
}

impl Default for CellTransferBuffer {
    fn default() -> Self {
        Self {
            water_delta: [0; CHUNK_W],
            moisture_delta: [0; CHUNK_W],
            sediment_delta: [0; CHUNK_W],
            sediment_inflow: [SedimentLoad::default(); CHUNK_W],
            erosion_request: [0; CHUNK_W],
            deposit_request: [0; CHUNK_W],
            deposit_material: [MaterialId::Sand; CHUNK_W],
            snow_request: [0; CHUNK_W],
        }
    }
}

impl CellTransferBuffer {
    pub fn clear(&mut self) {
        *self = Self::default();
    }
}

#[derive(Debug, Clone, Default)]
pub struct ChunkBoundaryOutbox {
    pub left_water: i64,
    pub right_water: i64,
    pub left_sediment: SedimentLoad,
    pub right_sediment: SedimentLoad,
    pub left_moisture: i64,
    pub right_moisture: i64,
}

#[derive(Debug, Clone, Default)]
pub struct WorldTransferScratch {
    pub buffers: BTreeMap<i32, CellTransferBuffer>,
    pub outbox: BTreeMap<i32, ChunkBoundaryOutbox>,
    pub last_water_flux: BTreeMap<i32, [i64; CHUNK_W]>,
    pub last_erosion_flux: BTreeMap<i32, [i64; CHUNK_W]>,
}

impl WorldTransferScratch {
    pub fn buffer_mut(&mut self, coord: i32) -> &mut CellTransferBuffer {
        self.buffers.entry(coord).or_default();
        self.buffers.get_mut(&coord).unwrap()
    }

    pub fn outbox_mut(&mut self, coord: i32) -> &mut ChunkBoundaryOutbox {
        self.outbox.entry(coord).or_default();
        self.outbox.get_mut(&coord).unwrap()
    }

    pub fn clear(&mut self) {
        for b in self.buffers.values_mut() {
            b.clear();
        }
        self.outbox.clear();
    }
}
