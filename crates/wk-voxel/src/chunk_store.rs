//! In-memory eviction store for Phase 4 streaming (WORLDGEN Phase 1).
//!
//! When a camera [`StreamWindow`](crate::streaming::StreamWindow) is set,
//! chunks outside the **resident** band (active + [`RESIDENT_MARGIN_CHUNKS`])
//! can be postcard-encoded here and dropped from [`World::chunks`].
//! Reloading restores cells; dirty bits are runtime-only (same as F5 saves).
//!
//! Play app calls [`sync_residency`] when stream-active physics is on.
//! [`restore_all_from_store`] runs when streaming turns off. TRACKED soaks
//! keep `stream_window = None` (no-op). Mass-carrying sparse side maps
//! (steam, dissolved, pipe, …) park with chunks via [`SideMapStore`].
//! WORLDGEN `FrozenBacklog` is still open.
//!
//! Far [`World::set_cell`](crate::grid::World::set_cell) reloads from the
//! store or skips blank remint outside the resident band.

use crate::chunk::{Chunk, ChunkCoord};
use crate::fasthash::FxHashMap;
use crate::grid::World;
use crate::streaming::StreamWindow;

/// Postcard bytes for chunks that left the resident band.
#[derive(Debug, Clone, Default)]
pub struct ChunkStore {
    bytes: FxHashMap<ChunkCoord, Vec<u8>>,
}

/// Parked sparse `(gx, gy)` / column books for far resident eviction.
///
/// Keeps TRACKED-ish mass ledgers + spatial mycelium books from orphaning
/// when cells leave RAM. Spore bank / organisms still live (follow-up).
#[derive(Debug, Clone, Default)]
pub struct SideMapStore {
    pub steam: FxHashMap<(i32, i32), u8>,
    pub cave_humidity: FxHashMap<(i32, i32), u8>,
    pub dissolved: FxHashMap<(i32, i32), u16>,
    pub suspended: FxHashMap<(i32, i32), u16>,
    pub pore_ice: FxHashMap<(i32, i32), u8>,
    pub water_temp: FxHashMap<(i32, i32), f32>,
    pub pipe_steam: FxHashMap<(i32, i32), u32>,
    pub pipe_res: FxHashMap<(i32, i32), u32>,
    pub pipe_steam_t: FxHashMap<(i32, i32), f32>,
    pub soft_litter: FxHashMap<i32, u16>,
    pub mycelium_strains: FxHashMap<(i32, i32), Vec<(u32, u8)>>,
    pub mycelium_energy: FxHashMap<(i32, i32), u8>,
    pub mycelium_lineage: FxHashMap<(i32, i32), crate::fungi::MyceliumLineage>,
}

impl SideMapStore {
    pub fn is_empty(&self) -> bool {
        self.steam.is_empty()
            && self.cave_humidity.is_empty()
            && self.dissolved.is_empty()
            && self.suspended.is_empty()
            && self.pore_ice.is_empty()
            && self.water_temp.is_empty()
            && self.pipe_steam.is_empty()
            && self.pipe_res.is_empty()
            && self.pipe_steam_t.is_empty()
            && self.soft_litter.is_empty()
            && self.mycelium_strains.is_empty()
            && self.mycelium_energy.is_empty()
            && self.mycelium_lineage.is_empty()
    }

    pub fn entry_count(&self) -> usize {
        self.steam.len()
            + self.cave_humidity.len()
            + self.dissolved.len()
            + self.suspended.len()
            + self.pore_ice.len()
            + self.water_temp.len()
            + self.pipe_steam.len()
            + self.pipe_res.len()
            + self.pipe_steam_t.len()
            + self.soft_litter.len()
            + self.mycelium_strains.len()
            + self.mycelium_energy.len()
            + self.mycelium_lineage.len()
    }
}

/// Counts from one [`sync_residency`] pass.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncResidencyStats {
    pub evicted: usize,
    pub reloaded: usize,
    pub side_parked: usize,
    pub side_restored: usize,
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

fn park_xy<V>(
    live: &mut FxHashMap<(i32, i32), V>,
    park: &mut FxHashMap<(i32, i32), V>,
    wrap_width: Option<i32>,
    win: StreamWindow,
) -> usize {
    let keys: Vec<(i32, i32)> = live
        .keys()
        .copied()
        .filter(|&(gx, _)| win.world_x_outside_resident(wrap_width, gx))
        .collect();
    let mut n = 0;
    for k in keys {
        if let Some(v) = live.remove(&k) {
            park.insert(k, v);
            n += 1;
        }
    }
    n
}

fn restore_xy<V>(
    live: &mut FxHashMap<(i32, i32), V>,
    park: &mut FxHashMap<(i32, i32), V>,
    wrap_width: Option<i32>,
    win: Option<StreamWindow>,
) -> usize {
    let keys: Vec<(i32, i32)> = park
        .keys()
        .copied()
        .filter(|&(gx, _)| match win {
            None => true,
            Some(w) => !w.world_x_outside_resident(wrap_width, gx),
        })
        .collect();
    let mut n = 0;
    for k in keys {
        if let Some(v) = park.remove(&k) {
            live.insert(k, v);
            n += 1;
        }
    }
    n
}

fn park_col<V>(
    live: &mut FxHashMap<i32, V>,
    park: &mut FxHashMap<i32, V>,
    wrap_width: Option<i32>,
    win: StreamWindow,
) -> usize {
    let keys: Vec<i32> = live
        .keys()
        .copied()
        .filter(|&gx| win.world_x_outside_resident(wrap_width, gx))
        .collect();
    let mut n = 0;
    for k in keys {
        if let Some(v) = live.remove(&k) {
            park.insert(k, v);
            n += 1;
        }
    }
    n
}

fn restore_col<V>(
    live: &mut FxHashMap<i32, V>,
    park: &mut FxHashMap<i32, V>,
    wrap_width: Option<i32>,
    win: Option<StreamWindow>,
) -> usize {
    let keys: Vec<i32> = park
        .keys()
        .copied()
        .filter(|&gx| match win {
            None => true,
            Some(w) => !w.world_x_outside_resident(wrap_width, gx),
        })
        .collect();
    let mut n = 0;
    for k in keys {
        if let Some(v) = park.remove(&k) {
            live.insert(k, v);
            n += 1;
        }
    }
    n
}

fn sync_side_maps(world: &mut World, win: StreamWindow) -> (usize, usize) {
    let wrap = world.wrap_width;
    let mut parked = 0;
    let mut restored = 0;
    let mut store = std::mem::take(&mut world.side_map_store);
    parked += park_xy(&mut world.steam, &mut store.steam, wrap, win);
    parked += park_xy(&mut world.cave_humidity, &mut store.cave_humidity, wrap, win);
    parked += park_xy(&mut world.dissolved, &mut store.dissolved, wrap, win);
    parked += park_xy(&mut world.suspended, &mut store.suspended, wrap, win);
    parked += park_xy(&mut world.pore_ice, &mut store.pore_ice, wrap, win);
    parked += park_xy(&mut world.water_temp, &mut store.water_temp, wrap, win);
    parked += park_xy(&mut world.pipe_steam, &mut store.pipe_steam, wrap, win);
    parked += park_xy(&mut world.pipe_res, &mut store.pipe_res, wrap, win);
    parked += park_xy(&mut world.pipe_steam_t, &mut store.pipe_steam_t, wrap, win);
    parked += park_col(&mut world.soft_litter, &mut store.soft_litter, wrap, win);
    parked += park_xy(&mut world.mycelium_strains, &mut store.mycelium_strains, wrap, win);
    parked += park_xy(&mut world.mycelium_energy, &mut store.mycelium_energy, wrap, win);
    parked += park_xy(
        &mut world.mycelium_lineage.cells,
        &mut store.mycelium_lineage,
        wrap,
        win,
    );

    restored += restore_xy(&mut world.steam, &mut store.steam, wrap, Some(win));
    restored += restore_xy(&mut world.cave_humidity, &mut store.cave_humidity, wrap, Some(win));
    restored += restore_xy(&mut world.dissolved, &mut store.dissolved, wrap, Some(win));
    restored += restore_xy(&mut world.suspended, &mut store.suspended, wrap, Some(win));
    restored += restore_xy(&mut world.pore_ice, &mut store.pore_ice, wrap, Some(win));
    restored += restore_xy(&mut world.water_temp, &mut store.water_temp, wrap, Some(win));
    restored += restore_xy(&mut world.pipe_steam, &mut store.pipe_steam, wrap, Some(win));
    restored += restore_xy(&mut world.pipe_res, &mut store.pipe_res, wrap, Some(win));
    restored += restore_xy(&mut world.pipe_steam_t, &mut store.pipe_steam_t, wrap, Some(win));
    restored += restore_col(&mut world.soft_litter, &mut store.soft_litter, wrap, Some(win));
    restored += restore_xy(
        &mut world.mycelium_strains,
        &mut store.mycelium_strains,
        wrap,
        Some(win),
    );
    restored += restore_xy(
        &mut world.mycelium_energy,
        &mut store.mycelium_energy,
        wrap,
        Some(win),
    );
    restored += restore_xy(
        &mut world.mycelium_lineage.cells,
        &mut store.mycelium_lineage,
        wrap,
        Some(win),
    );
    world.side_map_store = store;
    (parked, restored)
}

fn restore_all_side_maps(world: &mut World) -> usize {
    let wrap = world.wrap_width;
    let mut store = std::mem::take(&mut world.side_map_store);
    let mut n = 0;
    n += restore_xy(&mut world.steam, &mut store.steam, wrap, None);
    n += restore_xy(&mut world.cave_humidity, &mut store.cave_humidity, wrap, None);
    n += restore_xy(&mut world.dissolved, &mut store.dissolved, wrap, None);
    n += restore_xy(&mut world.suspended, &mut store.suspended, wrap, None);
    n += restore_xy(&mut world.pore_ice, &mut store.pore_ice, wrap, None);
    n += restore_xy(&mut world.water_temp, &mut store.water_temp, wrap, None);
    n += restore_xy(&mut world.pipe_steam, &mut store.pipe_steam, wrap, None);
    n += restore_xy(&mut world.pipe_res, &mut store.pipe_res, wrap, None);
    n += restore_xy(&mut world.pipe_steam_t, &mut store.pipe_steam_t, wrap, None);
    n += restore_col(&mut world.soft_litter, &mut store.soft_litter, wrap, None);
    n += restore_xy(&mut world.mycelium_strains, &mut store.mycelium_strains, wrap, None);
    n += restore_xy(&mut world.mycelium_energy, &mut store.mycelium_energy, wrap, None);
    n += restore_xy(
        &mut world.mycelium_lineage.cells,
        &mut store.mycelium_lineage,
        wrap,
        None,
    );
    world.side_map_store = store;
    n
}

/// Evict chunks outside the resident band; reload parked ones that re-enter.
///
/// Also parks / restores mass-carrying sparse side maps ([`SideMapStore`]).
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
            // Remint gate should prevent blank overlays; drop stale park.
            world.chunk_store.bytes.remove(&coord);
            continue;
        }
        if let Some(chunk) = world.chunk_store.take_decode(coord)? {
            world.chunks.insert(coord, chunk);
            stats.reloaded += 1;
        }
    }
    if stats.reloaded > 0 {
        world.invalidate_chunk_cache_pub();
        // Thaw water booked while the slab was missing.
        let thawed: Vec<crate::chunk::ChunkCoord> = world
            .frozen_backlog
            .keys()
            .copied()
            .filter(|c| world.chunks.contains_key(c))
            .collect();
        for coord in thawed {
            crate::frozen_backlog::apply_frozen_backlog(world, coord);
        }
    }

    let (side_parked, side_restored) = sync_side_maps(world, win);
    stats.side_parked = side_parked;
    stats.side_restored = side_restored;

    Ok(stats)
}

/// Reload every parked chunk + side map (stream off / full-resident).
pub fn restore_all_from_store(world: &mut World) -> Result<usize, String> {
    let keys: Vec<ChunkCoord> = world.chunk_store.bytes.keys().copied().collect();
    let mut n = 0;
    for coord in keys {
        if world.chunks.contains_key(&coord) {
            world.chunk_store.bytes.remove(&coord);
            continue;
        }
        if let Some(chunk) = world.chunk_store.take_decode(coord)? {
            world.chunks.insert(coord, chunk);
            n += 1;
        }
    }
    if n > 0 {
        world.invalidate_chunk_cache_pub();
    }
    n += restore_all_side_maps(world);
    let thawed: Vec<crate::chunk::ChunkCoord> = world
        .frozen_backlog
        .keys()
        .copied()
        .filter(|c| world.chunks.contains_key(c))
        .collect();
    for coord in thawed {
        crate::frozen_backlog::apply_frozen_backlog(world, coord);
    }
    Ok(n)
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

    #[test]
    fn set_cell_skips_far_blank_remint_when_streaming() {
        let cols = 64 * CHUNK_CELLS_W as i32;
        let mut w = ring_world(cols);
        w.set_cell(32 * CHUNK_CELLS_W as i32 + 8, 8, Cell::solid(MaterialId::Sand));
        w.stream_window = Some(StreamWindow::new(0, 128));
        sync_residency(&mut w).expect("evict");
        assert!(w
            .get_cell(32 * CHUNK_CELLS_W as i32 + 8, 8)
            .is_none());
        // Far write must not mint a blank chunk over the park.
        w.set_cell(
            32 * CHUNK_CELLS_W as i32 + 8,
            9,
            Cell::solid(MaterialId::Stone),
        );
        assert!(
            !w.chunks.keys().any(|c| c.cx == 32),
            "far remint must be skipped"
        );
        // Store still holds sand.
        w.stream_window = Some(StreamWindow::new(32 * CHUNK_CELLS_W as i32, 128));
        sync_residency(&mut w).expect("reload");
        assert_eq!(
            w.get_cell(32 * CHUNK_CELLS_W as i32 + 8, 8)
                .map(|c| c.material),
            Some(MaterialId::Sand)
        );
    }

    #[test]
    fn set_cell_reloads_from_store_inside_resident() {
        let cols = 64 * CHUNK_CELLS_W as i32;
        let mut w = ring_world(cols);
        let gx = 8;
        w.set_cell(gx, 8, Cell::solid(MaterialId::Stone));
        // Manually park cx=0 while keeping a window that still wants it resident.
        let coord = ChunkCoord::new(0, 0);
        let chunk = w.chunks.remove(&coord).expect("chunk");
        w.chunk_store.encode_put(&chunk).expect("park");
        w.invalidate_chunk_cache_pub();
        w.stream_window = Some(StreamWindow::new(0, 128));
        assert!(w.get_cell(gx, 8).is_none());
        // Write inside resident → reload-on-touch, then apply.
        w.set_cell(gx, 9, Cell::solid(MaterialId::Sand));
        assert_eq!(
            w.get_cell(gx, 8).map(|c| c.material),
            Some(MaterialId::Stone),
            "reload-on-touch must restore parked stone"
        );
        assert_eq!(
            w.get_cell(gx, 9).map(|c| c.material),
            Some(MaterialId::Sand)
        );
    }

    #[test]
    fn restore_all_brings_parked_chunks_home() {
        let cols = 64 * CHUNK_CELLS_W as i32;
        let mut w = ring_world(cols);
        w.set_cell(32 * CHUNK_CELLS_W as i32 + 8, 8, Cell::solid(MaterialId::Sand));
        w.stream_window = Some(StreamWindow::new(0, 128));
        sync_residency(&mut w).expect("evict");
        w.stream_window = None;
        let n = restore_all_from_store(&mut w).expect("restore");
        assert!(n >= 1);
        assert!(w.chunk_store.is_empty());
        assert_eq!(
            w.get_cell(32 * CHUNK_CELLS_W as i32 + 8, 8)
                .map(|c| c.material),
            Some(MaterialId::Sand)
        );
    }

    #[test]
    fn sync_parks_and_restores_far_steam() {
        let cols = 64 * CHUNK_CELLS_W as i32;
        let mut w = ring_world(cols);
        let far_gx = 32 * CHUNK_CELLS_W as i32 + 8;
        w.set_cell(far_gx, 8, Cell::solid(MaterialId::Stone));
        w.steam.insert((far_gx, 10), 40);
        w.dissolved.insert((far_gx, 8), 12);
        w.soft_litter.insert(far_gx, 3);
        w.stream_window = Some(StreamWindow::new(0, 128));
        let stats = sync_residency(&mut w).expect("evict");
        assert!(stats.side_parked >= 3, "steam+dissolved+litter: {stats:?}");
        assert!(!w.steam.contains_key(&(far_gx, 10)));
        assert!(!w.dissolved.contains_key(&(far_gx, 8)));
        assert!(!w.soft_litter.contains_key(&far_gx));
        assert_eq!(w.side_map_store.steam.get(&(far_gx, 10)), Some(&40));

        w.stream_window = Some(StreamWindow::new(far_gx, 128));
        let stats2 = sync_residency(&mut w).expect("reload");
        assert!(stats2.side_restored >= 3, "{stats2:?}");
        assert_eq!(w.steam.get(&(far_gx, 10)), Some(&40));
        assert_eq!(w.dissolved.get(&(far_gx, 8)), Some(&12));
        assert_eq!(w.soft_litter.get(&far_gx), Some(&3));
        assert!(w.side_map_store.steam.is_empty());
    }

    #[test]
    fn sync_parks_and_restores_mycelium_books() {
        let cols = 64 * CHUNK_CELLS_W as i32;
        let mut w = ring_world(cols);
        let far_gx = 32 * CHUNK_CELLS_W as i32 + 8;
        w.set_cell(far_gx, 8, Cell::solid(MaterialId::Organic));
        w.mycelium_energy.insert((far_gx, 8), 90);
        w.mycelium_strains
            .insert((far_gx, 8), vec![(1, 200), (2, 55)]);
        w.stream_window = Some(StreamWindow::new(0, 128));
        let stats = sync_residency(&mut w).expect("evict");
        assert!(stats.side_parked >= 2, "{stats:?}");
        assert!(!w.mycelium_energy.contains_key(&(far_gx, 8)));
        assert!(!w.mycelium_strains.contains_key(&(far_gx, 8)));
        assert_eq!(w.side_map_store.mycelium_energy.get(&(far_gx, 8)), Some(&90));

        w.stream_window = Some(StreamWindow::new(far_gx, 128));
        sync_residency(&mut w).expect("reload");
        assert_eq!(w.mycelium_energy.get(&(far_gx, 8)), Some(&90));
        assert_eq!(
            w.mycelium_strains.get(&(far_gx, 8)).map(|v| v.len()),
            Some(2)
        );
    }
}
