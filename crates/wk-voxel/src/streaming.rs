//! Camera-centered active window for Phase 4 streaming.
//!
//! WORLDGEN view / active / resident / evicted tiers. Active-only CA /
//! field shell filters run against [`StreamWindow`]. Eviction into
//! [`crate::chunk_store::ChunkStore`] via
//! [`crate::chunk_store::sync_residency`] (play app when stream-active);
//! FrozenBacklog still open.

use crate::active::ActiveChunk;
use crate::chunk::CHUNK_CELLS_W;
use crate::grid::World;

/// Halo chunks on each side of the view (WORLDGEN default ≈ 48 m).
pub const HALO_CHUNKS: i32 = 3;

/// Resident margin beyond active (WORLDGEN default ≈ 128 m).
pub const RESIDENT_MARGIN_CHUNKS: i32 = 8;

/// Inclusive wrap-aware span of chunk-x columns that may run CA physics.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamWindow {
    /// Focus column in world cells (wrapped when the world wraps).
    pub focus_col: i32,
    /// Visible column count (screen width / cell size).
    pub view_cols: i32,
    /// Halo on each side of the view, in chunks.
    pub halo_chunks: i32,
}

impl StreamWindow {
    pub fn new(focus_col: i32, view_cols: i32) -> Self {
        Self {
            focus_col,
            view_cols: view_cols.max(1),
            halo_chunks: HALO_CHUNKS,
        }
    }

    /// Half-width of the active set in chunk columns (view/2 + halo).
    pub fn active_half_chunks(&self) -> i32 {
        let view_half = (self.view_cols + CHUNK_CELLS_W as i32 - 1) / (2 * CHUNK_CELLS_W as i32);
        view_half.max(1) + self.halo_chunks.max(0)
    }

    /// Half-width of the resident set (active + [`RESIDENT_MARGIN_CHUNKS`]).
    pub fn resident_half_chunks(&self) -> i32 {
        self.active_half_chunks() + RESIDENT_MARGIN_CHUNKS.max(0)
    }

    /// Wrap-ring chunk-x span, if the world wraps.
    pub fn span_chunk_count(world: &World) -> Option<i32> {
        Self::span_x(world)
    }

    fn span_x(world: &World) -> Option<i32> {
        let w = world.wrap_width?;
        if w <= 0 {
            return None;
        }
        Some((w as i32 + CHUNK_CELLS_W as i32 - 1) / CHUNK_CELLS_W as i32)
    }

    fn wrap_cx(cx: i32, span: Option<i32>) -> i32 {
        match span {
            Some(n) if n > 0 => cx.rem_euclid(n),
            _ => cx,
        }
    }

    /// Center chunk-x of the focus column.
    pub fn focus_cx(&self, world: &World) -> i32 {
        let col = match world.wrap_width {
            Some(w) if w > 0 => self.focus_col.rem_euclid(w),
            _ => self.focus_col,
        };
        Self::wrap_cx(col.div_euclid(CHUNK_CELLS_W as i32), Self::span_x(world))
    }

    /// True when `cx` lies in the active band (wrap-aware).
    pub fn contains_cx(&self, world: &World, cx: i32) -> bool {
        self.contains_band_cx(world, cx, self.active_half_chunks())
    }

    /// True when `cx` lies in the resident band (active + margin).
    pub fn contains_resident_cx(&self, world: &World, cx: i32) -> bool {
        self.contains_band_cx(world, cx, self.resident_half_chunks())
    }

    fn contains_band_cx(&self, world: &World, cx: i32, half: i32) -> bool {
        let span = Self::span_x(world);
        let focus = self.focus_cx(world);
        let cx = Self::wrap_cx(cx, span);
        match span {
            Some(n) if n > 0 => {
                // Full ring fits in the band — no filter.
                if half * 2 + 1 >= n {
                    return true;
                }
                let mut d = (cx - focus).rem_euclid(n);
                if d > n / 2 {
                    d -= n;
                }
                d.abs() <= half
            }
            _ => (cx - focus).abs() <= half,
        }
    }

    /// Count of distinct chunk-x columns in the active band.
    pub fn active_chunk_count(&self, world: &World) -> i32 {
        let span = Self::span_x(world);
        let half = self.active_half_chunks();
        let width = half * 2 + 1;
        match span {
            Some(n) if n > 0 => width.min(n),
            _ => width,
        }
    }

    /// True when world column `gx` lies in the active band (wrap-aware).
    pub fn contains_world_x(&self, world: &World, gx: i32) -> bool {
        let gx = match world.wrap_width {
            Some(w) if w > 0 => gx.rem_euclid(w),
            _ => gx,
        };
        self.contains_cx(world, gx.div_euclid(CHUNK_CELLS_W as i32))
    }

    /// True when humidity/temp tile column `hx` overlaps the active band.
    pub fn contains_hx(&self, world: &World, hx: i32, tile_cols: i32) -> bool {
        let tc = tile_cols.max(1);
        // Tile centre in world cells — same band as CA chunk filter.
        self.contains_world_x(world, hx * tc + tc / 2)
    }
}

/// `true` when there is no stream window, or `gx` is inside it.
#[inline]
pub fn stream_allows_world_x(world: &World, gx: i32) -> bool {
    match world.stream_window {
        Some(win) => win.contains_world_x(world, gx),
        None => true,
    }
}

/// `true` when there is no stream window, or humidity tile `hx` is inside it.
#[inline]
pub fn stream_allows_hx(world: &World, hx: i32, tile_cols: i32) -> bool {
    match world.stream_window {
        Some(win) => win.contains_hx(world, hx, tile_cols),
        None => true,
    }
}

/// Drop planned regions whose chunk-x is outside the world's stream window.
///
/// When [`World::stream_window`] is `None`, returns `plan` unchanged
/// (full-resident / headless soak path).
pub fn filter_plan_to_stream(world: &World, plan: Vec<ActiveChunk>) -> Vec<ActiveChunk> {
    let Some(win) = world.stream_window else {
        return plan;
    };
    plan.into_iter()
        .filter(|a| win.contains_cx(world, a.coord.cx))
        .collect()
}

/// Clear dirty only on chunks that were just planned.
///
/// Preserves dirty outside a stream window so frozen residents thaw with
/// their backlog of wakes when the camera returns.
pub fn clear_planned_dirty(world: &mut World, active: &[ActiveChunk]) {
    for a in active {
        if let Some(chunk) = world.chunks.get_mut(&a.coord) {
            chunk.clear_dirty();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use crate::active::ActiveChunk;
    use crate::chunk::Rect;

    fn ring_world(cols: i32) -> World {
        let mut w = World::new(1);
        w.wrap_width = Some(cols);
        w
    }

    #[test]
    fn active_band_wraps_across_seam() {
        // Compact 64 chunks; focus near x=0 so the band straddles the seam.
        let w = ring_world(64 * CHUNK_CELLS_W as i32);
        let win = StreamWindow::new(8, 320); // ~5 view chunks + halo
        assert!(win.contains_cx(&w, 0));
        assert!(win.contains_cx(&w, 63));
        assert!(win.contains_cx(&w, 1));
        // Far side of the ring stays frozen.
        assert!(!win.contains_cx(&w, 32));
    }

    #[test]
    fn narrow_world_disables_filter() {
        let w = ring_world(8 * CHUNK_CELLS_W as i32);
        let win = StreamWindow::new(0, 2000);
        // View+halo covers the whole 8-chunk ring.
        for cx in 0..8 {
            assert!(win.contains_cx(&w, cx), "cx={cx}");
        }
    }

    #[test]
    fn filter_plan_keeps_only_active_cx() {
        let mut w = ring_world(64 * CHUNK_CELLS_W as i32);
        w.stream_window = Some(StreamWindow::new(0, 128));
        let plan = vec![
            ActiveChunk::new(ChunkCoord::new(0, 0), Rect::full()),
            ActiveChunk::new(ChunkCoord::new(32, 0), Rect::full()),
            ActiveChunk::new(ChunkCoord::new(63, 0), Rect::full()),
        ];
        let filtered = filter_plan_to_stream(&w, plan);
        let cxs: Vec<i32> = filtered.iter().map(|a| a.coord.cx).collect();
        assert!(cxs.contains(&0));
        assert!(cxs.contains(&63));
        assert!(!cxs.contains(&32));
    }

    #[test]
    fn no_window_passes_plan_through() {
        let w = ring_world(64 * CHUNK_CELLS_W as i32);
        let plan = vec![ActiveChunk::new(ChunkCoord::new(32, 0), Rect::full())];
        let filtered = filter_plan_to_stream(&w, plan.clone());
        assert_eq!(filtered, plan);
    }

    #[test]
    fn world_x_and_hx_match_chunk_band() {
        let w = ring_world(64 * CHUNK_CELLS_W as i32);
        let win = StreamWindow::new(0, 128);
        assert!(win.contains_world_x(&w, 10));
        assert!(win.contains_world_x(&w, 64 * CHUNK_CELLS_W as i32 - 1));
        assert!(!win.contains_world_x(&w, 32 * CHUNK_CELLS_W as i32));
        // tile_cols=4 → hx=0 centre at world x=2 (active); hx=512 at x=2048+2 (frozen).
        assert!(win.contains_hx(&w, 0, 4));
        assert!(!win.contains_hx(&w, 512, 4));
        assert!(stream_allows_world_x(&w, 32 * CHUNK_CELLS_W as i32)); // no window on w
        let mut w2 = w;
        w2.stream_window = Some(win);
        assert!(!stream_allows_world_x(&w2, 32 * CHUNK_CELLS_W as i32));
        assert!(!stream_allows_hx(&w2, 512, 4));
    }
}
