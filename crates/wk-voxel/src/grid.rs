//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! World: a sparse map of chunk coordinates to chunks. Chunks are the
//! only unit of storage; the world is thin glue for lookup and
//! iteration.

use std::cell::Cell as StdCell;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};
use wk_material::HydroOverrides;

use crate::cell::Cell;
use crate::chunk::{Chunk, ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::FxHashMap;

/// Live chunk table. FxHash — the hot `get_cell` / `set_cell` path never sees
/// untrusted keys, so SipHash would only add cost.
pub type ChunkMap = FxHashMap<ChunkCoord, Chunk>;

static NEXT_CHUNK_CACHE_ID: AtomicU64 = AtomicU64::new(1);

fn next_chunk_cache_id() -> u64 {
    NEXT_CHUNK_CACHE_ID.fetch_add(1, Ordering::Relaxed)
}

/// Per-instance id so the thread-local last-chunk cache cannot follow a
/// cloned or deserialized world (or a reused heap address after drop).
#[derive(Debug)]
pub(crate) struct ChunkCacheId(u64);

impl Default for ChunkCacheId {
    fn default() -> Self {
        Self(next_chunk_cache_id())
    }
}

impl Clone for ChunkCacheId {
    fn clone(&self) -> Self {
        Self(next_chunk_cache_id())
    }
}

#[derive(Clone, Copy)]
struct LastChunk {
    id: u64,
    coord: ChunkCoord,
    ptr: *const Chunk,
}

thread_local! {
    static LAST_CHUNK: StdCell<Option<LastChunk>> = const { StdCell::new(None) };
}

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
    pub chunks: ChunkMap,
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
    /// Hibernating plant/fungus spores keyed by landing cell (Set E bank).
    #[serde(default)]
    pub spore_bank: crate::spore_bank::SporeBank,
    /// Per-sim hydrology material overrides (saved with the world).
    /// Hot paths read this via [`Self::water_capacity`] /
    /// [`crate::cell::water_capacity_with`] — no process-global install.
    #[serde(default)]
    pub hydro: HydroOverrides,
    /// Sparse mycelium lineage (editor / spore genome+body) keyed by cell.
    /// Emergent stalks prefer the nearest stamp over `minimal_fungus`.
    #[serde(default)]
    pub mycelium_lineage: crate::fungi::MyceliumLineageMap,
    /// Per-cell mycelium strain shares for overlays / competing networks.
    /// Keyed by wrapped `(gx, gy)` → list of `(strain_id, intensity)`.
    /// Share intensities sum to [`Cell::mycelium`] (≤255); cleared at 0.
    #[serde(default)]
    pub mycelium_strains: HashMap<(i32, i32), Vec<(u32, u8)>>,
    /// Next strain id to mint on inoculum (wraps; 0 reserved / unused).
    #[serde(default)]
    pub next_mycelium_strain_id: u32,
    /// Sparse network sugar / glucose analog on colonized cells (0..=255).
    /// Cleared when cream hits 0; migrates with grain/raft moves.
    #[serde(default)]
    pub mycelium_energy: HashMap<(i32, i32), u8>,
    /// Sparse actual symbiont exchange counters keyed by mycelium strain id.
    /// Same strain keeps one book across spatial split / reconnect.
    #[serde(default)]
    pub sym_net_flow: HashMap<u32, crate::symbiosis::SymNetFlow>,
    /// Strain id → lineage (genome+body) for treaty match at strain frontiers.
    /// Stamped on inoculum; survives cream spread far from the spatial stamp.
    #[serde(default)]
    pub mycelium_strain_lineage: HashMap<u32, crate::fungi::MyceliumLineage>,
    /// Competent-fall cargo moves this tick — sync organisms after physics.
    #[serde(skip, default)]
    pub competent_cell_moves: Vec<(i32, i32, i32, i32)>,
    /// **Sleeping rock.** Competent cells already evaluated by the body pass
    /// that could not move. Skipped when seeding until something near them is
    /// written, which is what stops a settled ridge from being re-floodfilled
    /// every tick. Derived state — never saved.
    #[serde(skip, default)]
    pub competent_settled: FxHashMap<ChunkCoord, crate::support_map::ChunkMask>,
    /// Cells a competent body moved into last tick.
    ///
    /// A body in flight marks its cells dirty, but the flow substep loop
    /// runs first and clears dirty, so by the time the body pass runs the
    /// mark is gone. In a lake, water writes keep re-dirtying the region
    /// every tick, so submerged rock kept moving while an airborne one
    /// waited for the every-4-ticks floating wake — the "water is faster
    /// than air" playtest report. Re-dirtying these is O(moves).
    #[serde(skip, default)]
    pub competent_moved_cells: Vec<(i32, i32)>,
    /// Invalidates the thread-local last-chunk pointer after clone / insert.
    #[serde(skip, default)]
    pub(crate) chunk_cache_id: ChunkCacheId,
}

impl World {
    pub fn new(seed: u64) -> Self {
        Self {
            seed: WorldSeed(seed),
            chunks: FxHashMap::default(),
            tick: 0,
            wrap_width: None,
            soft_litter: HashMap::new(),
            spore_bank: crate::spore_bank::SporeBank::default(),
            hydro: HydroOverrides::default(),
            mycelium_lineage: crate::fungi::MyceliumLineageMap::default(),
            mycelium_strains: HashMap::new(),
            next_mycelium_strain_id: 1,
            mycelium_energy: HashMap::new(),
            sym_net_flow: HashMap::new(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_settled: FxHashMap::default(),
            competent_moved_cells: Vec::new(),
            chunk_cache_id: ChunkCacheId::default(),
        }
    }

    /// True when this competent cell is asleep (evaluated, could not move).
    #[inline]
    pub fn competent_is_settled(&self, gx: i32, gy: i32) -> bool {
        let (coord, lx, ly) = Self::split(self.wrap_x(gx), gy);
        self.competent_settled
            .get(&coord)
            .is_some_and(|m| m.get(lx, ly))
    }

    /// Put a competent cell to sleep.
    #[inline]
    pub fn competent_set_settled(&mut self, gx: i32, gy: i32) {
        let (coord, lx, ly) = Self::split(self.wrap_x(gx), gy);
        self.competent_settled
            .entry(coord)
            .or_default()
            .set(lx, ly);
    }

    /// Wake every settled cell inside a world-space rectangle (inclusive).
    pub fn competent_wake_rect(&mut self, x0: i32, y0: i32, x1: i32, y1: i32) {
        if self.competent_settled.is_empty() {
            return;
        }
        for gy in y0..=y1 {
            for gx in x0..=x1 {
                let (coord, lx, ly) = Self::split(self.wrap_x(gx), gy);
                if let Some(m) = self.competent_settled.get_mut(&coord) {
                    m.unset(lx, ly);
                }
            }
        }
    }

    /// Drop all sleep state (editor paint / load / teardown).
    pub fn competent_wake_all(&mut self) {
        self.competent_settled.clear();
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

    /// Bump the last-chunk cache id. Required whenever `chunks` may
    /// reallocate (new key) so cached pointers cannot dangle.
    #[inline]
    fn invalidate_chunk_cache(&mut self) {
        self.chunk_cache_id = ChunkCacheId::default();
    }

    #[inline]
    fn remember_chunk_ptr(id: u64, coord: ChunkCoord, chunk: &Chunk) {
        let ptr = chunk as *const Chunk;
        LAST_CHUNK.with(|slot| {
            slot.set(Some(LastChunk { id, coord, ptr }));
        });
    }

    #[inline]
    fn cached_chunk(&self, coord: ChunkCoord) -> Option<*const Chunk> {
        let id = self.chunk_cache_id.0;
        LAST_CHUNK.with(|slot| {
            slot.get().and_then(|last| {
                if last.id == id && last.coord == coord {
                    Some(last.ptr)
                } else {
                    None
                }
            })
        })
    }

    pub fn get_cell(&self, gx: i32, gy: i32) -> Option<Cell> {
        let gx = self.wrap_x(gx);
        let (coord, lx, ly) = Self::split(gx, gy);
        if let Some(ptr) = self.cached_chunk(coord) {
            // SAFETY: pointer is from this world's map; `chunk_cache_id`
            // is bumped on any insert that could reallocate the table.
            return Some(unsafe { (*ptr).get(lx, ly) });
        }
        let chunk = self.chunks.get(&coord)?;
        Self::remember_chunk_ptr(self.chunk_cache_id.0, coord, chunk);
        Some(chunk.get(lx, ly))
    }

    /// Write a cell at world coordinates, creating the containing
    /// chunk if needed. Marks the chunk's dirty rectangle so rules
    /// re-scan on the next tick.
    pub fn set_cell(&mut self, gx: i32, gy: i32, cell: Cell) {
        let gx = self.wrap_x(gx);
        let (coord, lx, ly) = Self::split(gx, gy);
        // Only a change in *solidity* can destabilise neighbouring competent
        // rock, so that (not every write) is what wakes sleeping bodies. Water
        // changing saturation inside an Air cell must not, or sloshing lakes
        // would wake the whole ridge every tick.
        let track_sleep = !self.competent_settled.is_empty();
        if let Some(ptr) = self.cached_chunk(coord) {
            // SAFETY: exclusive `&mut self`; cached pointer matches this
            // world's current table generation.
            let chunk = unsafe { &mut *(ptr as *mut Chunk) };
            let prev_solid = if track_sleep {
                Some(chunk.get(lx, ly).material.is_solid())
            } else {
                None
            };
            chunk.set(lx, ly, cell);
            if track_sleep && prev_solid != Some(cell.material.is_solid()) {
                self.competent_wake_around(gx, gy);
            }
            return;
        }
        if !self.chunks.contains_key(&coord) {
            self.invalidate_chunk_cache();
        }
        let prev_solid = if track_sleep {
            self.chunks
                .get(&coord)
                .map(|c| c.get(lx, ly).material.is_solid())
        } else {
            None
        };
        let chunk = self.chunks.entry(coord).or_insert_with(|| Chunk::new(coord));
        chunk.set(lx, ly, cell);
        Self::remember_chunk_ptr(self.chunk_cache_id.0, coord, chunk);
        if track_sleep && prev_solid != Some(cell.material.is_solid()) {
            self.competent_wake_around(gx, gy);
        }
    }

    /// Wake sleeping competent rock at and around a cell whose support changed.
    #[inline]
    pub fn competent_wake_around(&mut self, gx: i32, gy: i32) {
        for (dx, dy) in [(0, 0), (0, 1), (0, -1), (1, 0), (-1, 0)] {
            let (coord, lx, ly) = Self::split(self.wrap_x(gx + dx), gy + dy);
            if let Some(m) = self.competent_settled.get_mut(&coord) {
                m.unset(lx, ly);
            }
        }
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
        if !self.chunks.contains_key(&coord) {
            self.invalidate_chunk_cache();
        }
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

    #[test]
    fn last_chunk_cache_survives_sequential_reads_and_clone() {
        let mut w = World::new(3);
        w.wrap_width = Some(128);
        for x in 0..128 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        for x in 0..128 {
            for y in 0..8 {
                assert_eq!(
                    w.get_cell(x, y).map(|c| c.material),
                    Some(MaterialId::Sand),
                    "seq ({x},{y})"
                );
            }
        }
        // Wrap: x=128 is x=0. Cache must not return the unwrapped chunk.
        assert_eq!(
            w.get_cell(128, 3).map(|c| c.material),
            Some(MaterialId::Sand)
        );
        let cloned = w.clone();
        cloned
            .get_cell(10, 2)
            .expect("clone must read through its own table");
        w.set_cell(10, 2, Cell::solid(MaterialId::Stone));
        assert_eq!(
            cloned.get_cell(10, 2).map(|c| c.material),
            Some(MaterialId::Sand),
            "clone must not share the live world's last-chunk pointer"
        );
        assert_eq!(
            w.get_cell(10, 2).map(|c| c.material),
            Some(MaterialId::Stone)
        );
    }
}
