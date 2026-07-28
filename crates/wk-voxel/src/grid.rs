//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! World: a sparse map of chunk coordinates to chunks. Chunks are the
//! only unit of storage; the world is thin glue for lookup and
//! iteration.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wk_material::HydroOverrides;

use crate::cell::Cell;
use crate::chunk::{Chunk, ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};

/// Deterministic world seed used to salt per-chunk / per-tick RNG.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct WorldSeed(pub u64);

impl Default for WorldSeed {
    fn default() -> Self {
        Self(0xC0FF_EE00_1234_5678)
    }
}

/// Sparse chunk map. Chunks are created on first access.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct World {
    pub seed: WorldSeed,
    pub chunks: HashMap<ChunkCoord, Chunk>,
    /// Global tick counter across the whole sim. Chunk local `tick`
    /// counters are kept for RNG salting; this one drives rule
    /// scheduling.
    pub tick: u64,
    /// When `Some(w)`, world-x is toroidal with period `w`:
    /// `get_cell` / `set_cell` map every `gx` into `[0, w)`.
    /// Used by ring worldgen so the left edge joins the right.
    pub wrap_width: Option<i32>,
    /// Soft labile litter (Set E) keyed by wrapped world-x column.
    /// Death deposits units here; fungi digest them before Organic cells.
    #[serde(default)]
    pub soft_litter: HashMap<i32, u16>,
    /// Per-sim hydrology material overrides (saved with the world).
    /// Hot paths read this via [`Self::water_capacity`] /
    /// [`crate::cell::water_capacity_with`] — no process-global install.
    #[serde(default)]
    pub hydro: HydroOverrides,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed: WorldSeed(seed),
            chunks: HashMap::new(),
            tick: 0,
            wrap_width: None,
            soft_litter: HashMap::new(),
            hydro: HydroOverrides::default(),
        }
    }

    /// Water capacity for `material` under this world's hydro overrides.
    #[inline]
    pub fn water_capacity(&self, material: wk_material::MaterialId) -> u8 {
        crate::cell::water_capacity_with(material, &self.hydro)
    }

    /// Map a world-x into the stored range when wrap is enabled.
    pub fn wrap_x(&self, gx: i32) -> i32 {
        match self.wrap_width {
            Some(w) if w > 0 => gx.rem_euclid(w),
            _ => gx,
        }
    }

    /// Split a world-cell coordinate `(gx, gy)` into `(chunk_coord,
    /// local_x, local_y)`.
    pub fn split(gx: i32, gy: i32) -> (ChunkCoord, usize, usize) {
        let w = CHUNK_CELLS_W as i32;
        let h = CHUNK_CELLS_H as i32;
        let cx = gx.div_euclid(w);
        let cy = gy.div_euclid(h);
        let lx = gx.rem_euclid(w) as usize;
        let ly = gy.rem_euclid(h) as usize;
        (ChunkCoord::new(cx, cy), lx, ly)
    }

    pub fn get_cell(&self, gx: i32, gy: i32) -> Option<Cell> {
        let gx = self.wrap_x(gx);
        let (coord, lx, ly) = Self::split(gx, gy);
        self.chunks.get(&coord).map(|c| c.get(lx, ly))
    }

    /// Write a cell at world coordinates, creating the containing
    /// chunk if needed. Marks the chunk's dirty rectangle so rules
    /// re-scan on the next tick.
    pub fn set_cell(&mut self, gx: i32, gy: i32, cell: Cell) {
        let gx = self.wrap_x(gx);
        let (coord, lx, ly) = Self::split(gx, gy);
        let chunk = self.chunks.entry(coord).or_insert_with(|| Chunk::new(coord));
        chunk.set(lx, ly, cell);
    }

    /// Dirty a cell without rewriting it (wake quiescent physics).
    pub fn touch_dirty(&mut self, gx: i32, gy: i32) {
        let gx = self.wrap_x(gx);
        let (coord, lx, ly) = Self::split(gx, gy);
        if let Some(chunk) = self.chunks.get_mut(&coord) {
            chunk.touch_dirty(lx, ly);
        }
    }

    pub fn ensure_chunk(&mut self, coord: ChunkCoord) -> &mut Chunk {
        self.chunks.entry(coord).or_insert_with(|| Chunk::new(coord))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;

    #[test]
    fn split_roundtrip() {
        for &(gx, gy) in &[(0, 0), (63, 63), (64, 64), (-1, -1), (-64, 0), (129, -130)] {
            let (coord, lx, ly) = World::split(gx, gy);
            assert!(lx < CHUNK_CELLS_W);
            assert!(ly < CHUNK_CELLS_H);
            let back_x = coord.cx * CHUNK_CELLS_W as i32 + lx as i32;
            let back_y = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
            assert_eq!(back_x, gx);
            assert_eq!(back_y, gy);
        }
    }

    #[test]
    fn set_get_across_chunk_boundary() {
        let mut w = World::new(7);
        // Write into chunk (1, 0); reading (0, 0) touches a
        // never-instantiated chunk and returns None (sparse map).
        w.set_cell(65, 0, Cell::solid(MaterialId::Sand));
        assert_eq!(
            w.get_cell(65, 0).map(|c| c.material),
            Some(MaterialId::Sand)
        );
        assert_eq!(w.get_cell(0, 0), None);
        // Ensuring the neighbouring chunk exists then reading falls
        // through to the default (Air).
        w.ensure_chunk(ChunkCoord::new(0, 0));
        assert_eq!(w.get_cell(0, 0).map(|c| c.material), Some(MaterialId::Air));
    }
}
