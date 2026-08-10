//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Parallel helpers for one checkerboard colour pass.
//!
//! Within a single colour, orthogonal neighbours never co-occur, and
//! gravity/grain **pull** only writes the active chunk plus (at most)
//! the `cy + 1` neighbour. Those write sets are disjoint across the
//! pass, so we can hand each [`ActiveChunk`] to a rayon task.
//!
//! Concurrent `HashMap` mutation is still UB, so tasks touch chunks
//! only through raw pointers gathered **before** the parallel region
//! (same idea as `slice::split_at_mut`).

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use rayon::prelude::*;

use crate::active::ActiveChunk;
use crate::cell::Cell;
use crate::chunk::{Chunk, ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Global switch — default on. Tests that need a pure serial path
/// call [`set_parallel_enabled`]`(false)`.
static PARALLEL_ENABLED: AtomicBool = AtomicBool::new(true);

/// Enable or disable rayon checkerboard work (process-wide).
pub fn set_parallel_enabled(on: bool) {
    PARALLEL_ENABLED.store(on, Ordering::Relaxed);
}

pub fn parallel_enabled() -> bool {
    PARALLEL_ENABLED.load(Ordering::Relaxed)
}

/// Minimum regions in one colour before we bother with rayon.
const PARALLEL_MIN_REGIONS: usize = 2;

pub(crate) fn should_parallelize(active: &[ActiveChunk]) -> bool {
    parallel_enabled() && active.len() >= PARALLEL_MIN_REGIONS
}

/// Raw chunk pointers for one parallel colour pass.
///
/// # Safety contract
/// - Built from unique coords; HashMap is not resized while alive.
/// - Only `Chunk` payloads are mutated; distinct coords do not alias.
/// - Caller only shares this across rayon tasks for one checkerboard
///   colour (disjoint pull write-sets — see module docs).
pub(crate) struct ChunkPtrMap {
    map: HashMap<ChunkCoord, *mut Chunk>,
}

// SAFETY: checkerboard + pull write-set disjointness (module docs).
unsafe impl Send for ChunkPtrMap {}
unsafe impl Sync for ChunkPtrMap {}

impl ChunkPtrMap {
    pub(crate) fn get(&self, coord: &ChunkCoord) -> Option<*mut Chunk> {
        self.map.get(coord).copied()
    }
}

/// Gather `*mut Chunk` for every coordinate that a pull-pass may write.
pub(crate) fn chunk_ptrs_mut(world: &mut World, coords: &[ChunkCoord]) -> ChunkPtrMap {
    let map = &mut world.chunks as *mut HashMap<ChunkCoord, Chunk>;
    let mut out = HashMap::with_capacity(coords.len());
    for &coord in coords {
        // Sequential get_mut: each &mut is turned into a raw pointer
        // and dropped before the next borrow.
        let ptr = unsafe {
            match (*map).get_mut(&coord) {
                Some(chunk) => chunk as *mut Chunk,
                None => continue,
            }
        };
        out.insert(coord, ptr);
    }
    ChunkPtrMap { map: out }
}

/// Coords an active pull-region may write: itself and `cy + 1` (drain).
pub(crate) fn pull_write_coords(active: &[ActiveChunk]) -> Vec<ChunkCoord> {
    let mut coords = Vec::with_capacity(active.len() * 2);
    for ac in active {
        coords.push(ac.coord);
        coords.push(ChunkCoord::new(ac.coord.cx, ac.coord.cy + 1));
    }
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords.dedup();
    coords
}

/// Moore neighbourhood of every active chunk (self + 8 neighbours).
///
/// Repose / same-Y walk-off write horizontally across chunk seams; the
/// gravity pull set ([`pull_write_coords`]) only includes `cy + 1`, so
/// those slides silently no-op'd and left hard cliff faces on one side
/// of large F3 sand blobs.
pub(crate) fn moore_write_coords(active: &[ActiveChunk]) -> Vec<ChunkCoord> {
    let mut coords = Vec::with_capacity(active.len() * 9);
    for ac in active {
        for dy in -1..=1 {
            for dx in -1..=1 {
                coords.push(ChunkCoord::new(ac.coord.cx + dx, ac.coord.cy + dy));
            }
        }
    }
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords.dedup();
    coords
}

/// True when each region's write set (own + `cy + 1`) is unique in `active`.
///
/// Required for [`ChunkPtrMap`]'s `Sync` contract under rayon. Callers pass
/// one checkerboard colour; overlapping sets mean a data race in parallel.
pub(crate) fn pull_write_coords_disjoint(active: &[ActiveChunk]) -> bool {
    let mut claimed: HashMap<ChunkCoord, ChunkCoord> = HashMap::new();
    for ac in active {
        for c in [
            ac.coord,
            ChunkCoord::new(ac.coord.cx, ac.coord.cy + 1),
        ] {
            if claimed.insert(c, ac.coord).is_some() {
                return false;
            }
        }
    }
    true
}

fn wrap_x(wrap_width: Option<i32>, gx: i32) -> i32 {
    match wrap_width {
        Some(w) if w > 0 => gx.rem_euclid(w),
        _ => gx,
    }
}

fn split(gx: i32, gy: i32) -> (ChunkCoord, usize, usize) {
    let w = CHUNK_CELLS_W as i32;
    let h = CHUNK_CELLS_H as i32;
    let cx = gx.div_euclid(w);
    let cy = gy.div_euclid(h);
    let lx = gx.rem_euclid(w) as usize;
    let ly = gy.rem_euclid(h) as usize;
    (ChunkCoord::new(cx, cy), lx, ly)
}

pub(crate) unsafe fn get_cell(
    ptrs: &ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    gy: i32,
) -> Option<Cell> {
    let gx = wrap_x(wrap_width, gx);
    let (coord, lx, ly) = split(gx, gy);
    let ptr = ptrs.get(&coord)?;
    Some(unsafe { (*ptr).get(lx, ly) })
}

pub(crate) unsafe fn set_cell(
    ptrs: &ChunkPtrMap,
    wrap_width: Option<i32>,
    gx: i32,
    gy: i32,
    cell: Cell,
) {
    let gx = wrap_x(wrap_width, gx);
    let (coord, lx, ly) = split(gx, gy);
    let Some(ptr) = ptrs.get(&coord) else {
        return;
    };
    unsafe {
        (*ptr).set(lx, ly, cell);
    }
}

/// Run `body` on each active region, in parallel when enabled.
pub(crate) fn for_each_region_parallel(
    world: &mut World,
    active: &[ActiveChunk],
    body: impl Fn(&ChunkPtrMap, Option<i32>, &ActiveChunk) + Sync,
) {
    if active.is_empty() {
        return;
    }
    // Dev-only: catch a future rule that widens the pull write-set before
    // we race on aliased `*mut Chunk` in release.
    debug_assert!(
        pull_write_coords_disjoint(active),
        "pull write-sets overlap within a colour pass (own + cy+1 must be unique)"
    );
    let wrap_width = world.wrap_width;
    let coords = pull_write_coords(active);
    let ptrs = chunk_ptrs_mut(world, &coords);
    if should_parallelize(active) {
        active.par_iter().for_each(|ac| body(&ptrs, wrap_width, ac));
    } else {
        for ac in active {
            body(&ptrs, wrap_width, ac);
        }
    }
}

/// Serial region scan with a Moore-neighbour chunk ptr map.
///
/// Used by grain repose: slides write `cx ± 1` and must not silently
/// no-op at chunk seams. Serial so overlapping Moore write sets cannot
/// race (unlike [`for_each_region_parallel`]'s pull-only contract).
pub(crate) fn for_each_region_serial_moore(
    world: &mut World,
    active: &[ActiveChunk],
    body: impl Fn(&ChunkPtrMap, Option<i32>, &ActiveChunk),
) {
    if active.is_empty() {
        return;
    }
    let wrap_width = world.wrap_width;
    let mut coords = moore_write_coords(active);
    coords.retain(|c| world.chunks.contains_key(c));
    let ptrs = chunk_ptrs_mut(world, &coords);
    for ac in active {
        body(&ptrs, wrap_width, ac);
    }
}

/// Soft target cells per compute-then-apply scan job after banding.
///
/// Checkerboard colours often only hold ~2–3 active chunks; without
/// splitting, rayon starves on a 32–128 core box. 8×8 tiles turn one
/// full chunk into 64 jobs while keeping enough work per task.
const SCAN_TILE_CELLS: u8 = 8;

/// Split active rects into stable `(cy, cx, y0, x0)` scan tiles.
///
/// Used only for **read-only** compute-then-apply scans (flow / seepage /
/// spill). Apply stays serial. Tile order matches a serial y-major,
/// x-major walk of each region so transfer lists stay deterministic.
pub(crate) fn expand_scan_tiles(active: &[ActiveChunk]) -> Vec<ActiveChunk> {
    if active.is_empty() {
        return Vec::new();
    }
    let mut regions: Vec<&ActiveChunk> = active.iter().collect();
    regions.sort_by(|a, b| {
        a.coord
            .cy
            .cmp(&b.coord.cy)
            .then(a.coord.cx.cmp(&b.coord.cx))
    });
    let tile = SCAN_TILE_CELLS.max(1);
    let mut out = Vec::with_capacity(regions.len() * 4);
    for ac in regions {
        let mut y = ac.rect.y0;
        loop {
            let y1 = ac.rect.y1.min(y.saturating_add(tile - 1));
            let mut x = ac.rect.x0;
            loop {
                let x1 = ac.rect.x1.min(x.saturating_add(tile - 1));
                out.push(ActiveChunk {
                    coord: ac.coord,
                    rect: Rect {
                        x0: x,
                        y0: y,
                        x1,
                        y1,
                    },
                });
                if x1 >= ac.rect.x1 {
                    break;
                }
                x = x1.saturating_add(1);
            }
            if y1 >= ac.rect.y1 {
                break;
            }
            y = y1.saturating_add(1);
        }
    }
    out
}

/// Parallel spill/seepage/flow scans: each region (or scan tile) produces
/// a local result, then results are concatenated in stable
/// `(cy, cx, y0, x0)` order.
///
/// When rayon is on, tall/wide dirty rects are split into
/// [`SCAN_TILE_CELLS`] tiles so a colour with few chunks still feeds
/// many cores. Serial path keeps whole regions (same cell walk order
/// as tiled concat) to avoid extra allocs in tests.
pub(crate) fn map_regions_parallel<T, F>(active: &[ActiveChunk], f: F) -> Vec<T>
where
    T: Send,
    F: Fn(&ActiveChunk) -> T + Sync,
{
    if active.is_empty() {
        return Vec::new();
    }
    if parallel_enabled() {
        let tiles = expand_scan_tiles(active);
        if tiles.len() >= PARALLEL_MIN_REGIONS {
            return tiles.par_iter().map(|ac| f(ac)).collect();
        }
        return tiles.iter().map(|ac| f(ac)).collect();
    }
    let mut regions: Vec<&ActiveChunk> = active.iter().collect();
    regions.sort_by(|a, b| {
        a.coord
            .cy
            .cmp(&b.coord.cy)
            .then(a.coord.cx.cmp(&b.coord.cx))
    });
    regions.iter().map(|ac| f(ac)).collect()
}

/// Parallel frame-shell scans over sticky-flag chunk lists (evap / karst /
/// flow erosion). `coords` must already be sorted `(cy, cx)` for stable
/// concatenation order. Apply stays serial in the caller.
pub(crate) fn map_chunk_coords_parallel<T, F>(coords: &[ChunkCoord], f: F) -> Vec<T>
where
    T: Send,
    F: Fn(ChunkCoord) -> T + Sync + Send,
{
    if coords.is_empty() {
        return Vec::new();
    }
    if parallel_enabled() && coords.len() >= PARALLEL_MIN_REGIONS {
        coords.par_iter().copied().map(f).collect()
    } else {
        coords.iter().copied().map(f).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::active::{checkerboard_phase, partition_checkerboard};
    use crate::chunk::Rect;

    #[test]
    fn pull_write_coords_include_above_neighbour() {
        let active = [ActiveChunk {
            coord: ChunkCoord::new(0, 0),
            rect: Rect::full(),
        }];
        let coords = pull_write_coords(&active);
        assert!(coords.contains(&ChunkCoord::new(0, 0)));
        assert!(coords.contains(&ChunkCoord::new(0, 1)));
    }

    #[test]
    fn expand_scan_tiles_covers_full_chunk_in_stable_order() {
        let active = [ActiveChunk {
            coord: ChunkCoord::new(1, 2),
            rect: Rect::full(),
        }];
        let tiles = expand_scan_tiles(&active);
        let expect = (CHUNK_CELLS_W / SCAN_TILE_CELLS as usize)
            * (CHUNK_CELLS_H / SCAN_TILE_CELLS as usize);
        assert_eq!(tiles.len(), expect);
        assert_eq!(tiles[0].rect.x0, 0);
        assert_eq!(tiles[0].rect.y0, 0);
        assert_eq!(tiles[0].rect.x1, SCAN_TILE_CELLS - 1);
        assert_eq!(tiles[0].rect.y1, SCAN_TILE_CELLS - 1);
        // y-major, then x within a row of tiles
        assert_eq!(tiles[1].rect.x0, SCAN_TILE_CELLS);
        assert_eq!(tiles[1].rect.y0, 0);
        let last = tiles.last().unwrap();
        assert_eq!(last.rect.x1, (CHUNK_CELLS_W - 1) as u8);
        assert_eq!(last.rect.y1, (CHUNK_CELLS_H - 1) as u8);
        // No gaps / overlaps: cell count matches.
        let cells: usize = tiles
            .iter()
            .map(|t| {
                let w = (t.rect.x1 as usize) - (t.rect.x0 as usize) + 1;
                let h = (t.rect.y1 as usize) - (t.rect.y0 as usize) + 1;
                w * h
            })
            .sum();
        assert_eq!(cells, CHUNK_CELLS_W * CHUNK_CELLS_H);
    }

    #[test]
    fn same_colour_pull_write_sets_are_disjoint() {
        let active: Vec<ActiveChunk> = [
            ChunkCoord::new(0, 0),
            ChunkCoord::new(2, 0),
            ChunkCoord::new(0, 2),
            ChunkCoord::new(2, 2),
            ChunkCoord::new(1, 1),
        ]
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: Rect::full(),
        })
        .collect();
        let passes = partition_checkerboard(&active);
        for pass in &passes {
            assert!(
                pull_write_coords_disjoint(pass),
                "colour {} write-sets must be disjoint",
                pass
                    .first()
                    .map(|ac| checkerboard_phase(ac.coord))
                    .unwrap_or(0)
            );
        }
    }

    #[test]
    fn overlapping_vertical_neighbours_are_not_disjoint() {
        // Same colour would not schedule these together; the helper must
        // still report overlap if a caller ever did.
        let active = [
            ActiveChunk {
                coord: ChunkCoord::new(0, 0),
                rect: Rect::full(),
            },
            ActiveChunk {
                coord: ChunkCoord::new(0, 1),
                rect: Rect::full(),
            },
        ];
        assert!(!pull_write_coords_disjoint(&active));
    }
}
