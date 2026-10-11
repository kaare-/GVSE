//! In-memory eviction store for Phase 4 streaming (WORLDGEN Phase 1).
//!
//! When a camera [`StreamWindow`](crate::streaming::StreamWindow) is set,
//! chunks outside the **resident** band (active + [`RESIDENT_MARGIN_CHUNKS`])
//! can be postcard-encoded here and dropped from [`World::chunks`].
//! Reloading restores cells; dirty bits are runtime-only (same as F5 saves).
//!
//! **Not wired into the play loop yet** — call [`sync_residency`] explicitly
//! (or from a future settings flag). TRACKED soaks keep `stream_window =
//! None` so this is a no-op. Side maps (steam, dissolved, …) stay
//! full-resident; WORLDGEN `FrozenBacklog` is still open.

use crate::chunk::{Chunk, ChunkCoord};
use crate::fasthash::FxHashMap;
use crate::grid::World;
use crate::streaming::StreamWindow;

/// Postcard bytes for chunks that left the resident band.
#[derive(Debug, Clone, Default)]
pub struct ChunkStore {
    bytes: FxHashMap<ChunkCoord, Vec<u8>>,
}

/// Counts from one [`sync_residency`] pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncResidencyStats {
    pub evicted: usize,
    pub reloaded: usize,
}

impl ChunkStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.bytes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bytes.is_empty()
    }

    pub fn contains(&self, coord: ChunkCoord) -> bool {
        self.bytes.contains_key(&coord)
    }

    /// Encode `chunk` and park it under `chunk.coord`.
    pub fn encode_put(&mut self, chunk: &Chunk) -> Result<(), String> {
        let bytes = postcard::to_allocvec(chunk).map_err(|e| e.to_string())?;
        self.bytes.insert(chunk.coord, bytes);
        Ok(())
    }

    /// Take and decode one parked chunk. Missing → `Ok(None)`.
    pub fn take_decode(&mut self, coord: ChunkCoord) -> Result<Option<Chunk>, String> {
        let Some(bytes) = self.bytes.remove(&coord) else {
            return Ok(None);
        };
        let chunk: Chunk = postcard::from_bytes(&bytes).map_err(|e| e.to_string())?;
        Ok(Some(chunk))
    }
}

/// Evict chunks outside the resident band; reload parked ones that re-enter.
///
/// No-op when [`World::stream_window`] is `None` (headless / TRACKED path)
/// or when the active band already covers the whole ring.
pub fn sync_residency(world: &mut World) -> Result<SyncResidencyStats, String> {
    let Some(win) = world.stream_window else {
        return Ok(SyncResidencyStats::default());
    };
    // Whole ring fits in active — keep everything resident.
    if win.active_chunk_count(world)
        >= StreamWindow::span_chunk_count(world).unwrap_or(i32::MAX)
    {
        return Ok(SyncResidencyStats::default());
    }

    let mut stats = SyncResidencyStats::default();

    // --- evict ---
    let to_evict: Vec<ChunkCoord> = world
        .chunks
        .keys()
        .copied()
        .filter(|c| !win.contains_resident_cx(world, c.cx))
        .collect();
    for coord in to_evict {
        let Some(chunk) = world.chunks.remove(&coord) else {
            continue;
        };
        world.competent_settled.remove(&coord);
        world.chunk_store.encode_put(&chunk)?;
        stats.evicted += 1;
    }
    if stats.evicted > 0 {
        world.invalidate_chunk_cache_pub();
    }

    // --- reload ---
    let to_load: Vec<ChunkCoord> = world
        .chunk_store
        .bytes
        .keys()
        .copied()
        .filter(|c| win.contains_resident_cx(world, c.cx))
        .collect();
    for coord in to_load {
        if world.chunks.contains_key(&coord) {
            // Already present (reminted blank?) — leave store entry; do not
            // overwrite live terrain. Caller / remint gate is a follow-up.
            continue;
        }
        if let Some(chunk) = world.chunk_store.take_decode(coord)? {
            world.chunks.insert(coord, chunk);
            stats.reloaded += 1;
        }
    }
    if stats.reloaded > 0 {
        world.invalidate_chunk_cache_pub();
    }

    Ok(stats)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::CHUNK_CELLS_W;
    use wk_material::MaterialId;

    fn ring_world(cols: i32) -> World {
        let mut w = World::new(1);
        w.wrap_width = Some(cols);
        w
    }

    #[test]
    fn chunk_postcard_round_trip_keeps_cells() {
        let mut store = ChunkStore::new();
        let coord = ChunkCoord::new(3, -1);
        let mut chunk = Chunk::new(coord);
        chunk.set(4, 5, Cell::solid(MaterialId::Stone));
        assert!(!chunk.dirty_bits.is_empty());
        store.encode_put(&chunk).expect("encode");
        let loaded = store.take_decode(coord).expect("decode").expect("present");
        assert_eq!(loaded.coord, coord);
        assert_eq!(loaded.get(4, 5).material, MaterialId::Stone);
        // Dirty bits are serde-skip — empty after load (same as F5).
        assert!(loaded.dirty_bits.is_empty());
        assert!(store.is_empty());
    }

    #[test]
    fn sync_noop_without_stream_window() {
        let mut w = ring_world(64 * CHUNK_CELLS_W as i32);
        w.set_cell(10, 10, Cell::solid(MaterialId::Stone));
        let before = w.chunks.len();
        let stats = sync_residency(&mut w).expect("sync");
        assert_eq!(stats, SyncResidencyStats::default());
        assert_eq!(w.chunks.len(), before);
        assert!(w.chunk_store.is_empty());
    }

    #[test]
    fn sync_evicts_far_and_reloads_on_focus_move() {
        let cols = 64 * CHUNK_CELLS_W as i32;
        let mut w = ring_world(cols);
        // Stone at cx=0 and cx=32.
        w.set_cell(8, 8, Cell::solid(MaterialId::Stone));
        w.set_cell(32 * CHUNK_CELLS_W as i32 + 8, 8, Cell::solid(MaterialId::Sand));
        w.stream_window = Some(StreamWindow::new(0, 128));

        let stats = sync_residency(&mut w).expect("evict");
        assert!(stats.evicted >= 1, "expected far cx=32 out: {stats:?}");
        assert!(!w.chunks.keys().any(|c| c.cx == 32));
        assert!(
            w.chunk_store.len() >= 1,
            "far chunk should park in store"
        );
        // Near chunk stays.
        assert_eq!(
            w.get_cell(8, 8).map(|c| c.material),
            Some(MaterialId::Stone)
        );
        // Far get is empty while evicted.
        assert!(w
            .get_cell(32 * CHUNK_CELLS_W as i32 + 8, 8)
            .is_none());

        // Pan camera to far side — reload.
        w.stream_window = Some(StreamWindow::new(32 * CHUNK_CELLS_W as i32, 128));
        let stats2 = sync_residency(&mut w).expect("reload");
        assert!(stats2.reloaded >= 1, "expected reload: {stats2:?}");
        assert_eq!(
            w.get_cell(32 * CHUNK_CELLS_W as i32 + 8, 8)
                .map(|c| c.material),
            Some(MaterialId::Sand)
        );
    }

    #[test]
    fn resident_band_wider_than_active() {
        let w = ring_world(64 * CHUNK_CELLS_W as i32);
        let win = StreamWindow::new(0, 128);
        let active_half = win.active_half_chunks();
        let resident_half = win.resident_half_chunks();
        assert!(resident_half > active_half);
        // Just outside active but inside resident margin.
        let edge = active_half + 1;
        assert!(!win.contains_cx(&w, edge));
        assert!(win.contains_resident_cx(&w, edge));
    }
}
