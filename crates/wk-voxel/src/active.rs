//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Active-chunk planning from dirty rectangles.
//!
//! Each [`Chunk::set`] expands a per-chunk dirty rect. At the start of
//! a physics tick we turn those rects into a scan plan: inflate by one
//! cell (neighbour halo) and wake abutting chunks so water can cross
//! seams. Rules then only visit planned cells; writes during the tick
//! rebuild dirty for the *next* tick.

use crate::fasthash::FxHashMap as HashMap;

use crate::chunk::{ChunkCoord, DirtyBits, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// One chunk region that should be visited by the next rule pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveChunk {
    pub coord: ChunkCoord,
    /// Inclusive local-cell rectangle to scan (bbox of [`Self::bits`]
    /// after inflate, or an explicit full-chunk rect for wakes).
    pub rect: Rect,
    /// Dilated dirty cells. All-zero means **dense**: walk the whole
    /// [`Self::rect`] (standalone tests, occupancy wakes, legacy saves).
    pub bits: DirtyBits,
}

impl ActiveChunk {
    /// Dense rect walk (no per-cell mask).
    pub fn new(coord: ChunkCoord, rect: Rect) -> Self {
        Self {
            coord,
            rect,
            bits: DirtyBits::empty(),
        }
    }

    pub fn with_bits(coord: ChunkCoord, rect: Rect, bits: DirtyBits) -> Self {
        Self { coord, rect, bits }
    }

    pub fn is_dense(self) -> bool {
        self.bits.is_empty()
    }

    #[inline]
    pub fn visits(self, x: u8, y: u8) -> bool {
        self.is_dense() || self.bits.get(x, y)
    }

    /// Cells this region will visit (set bits, or the rect area if dense).
    pub fn cell_count(self) -> usize {
        if self.is_dense() {
            self.rect.area()
        } else {
            self.bits.count()
        }
    }

    pub fn aabb_area(self) -> usize {
        self.rect.area()
    }

    fn row_mask(x0: u8, x1: u8) -> u64 {
        if x0 == 0 && x1 >= 63 {
            u64::MAX
        } else {
            let hi = if x1 >= 63 {
                u64::MAX
            } else {
                (1u64 << (x1 + 1)) - 1
            };
            let lo = (1u64 << x0) - 1;
            hi ^ lo
        }
    }

    /// Visit each planned cell. Sparse regions skip AABB holes.
    pub fn for_each_cell(self, f: impl FnMut(u8, u8)) {
        self.for_each_cell_in_y(self.rect.y0, self.rect.y1, f);
    }

    /// Like [`Self::for_each_cell`], restricted to `y0..=y1` (intersected
    /// with the region rect). Confined rise uses the standing-air band
    /// so a dense period-16 wake does not walk dry sky.
    pub fn for_each_cell_in_y(self, y0: u8, y1: u8, mut f: impl FnMut(u8, u8)) {
        let y0 = y0.max(self.rect.y0);
        let y1 = y1.min(self.rect.y1);
        if y0 > y1 {
            return;
        }
        if self.is_dense() {
            for y in y0..=y1 {
                for x in self.rect.x0..=self.rect.x1 {
                    f(x, y);
                }
            }
            return;
        }
        let mask = Self::row_mask(self.rect.x0, self.rect.x1);
        for y in y0..=y1 {
            let mut row = self.bits.0[y as usize] & mask;
            while row != 0 {
                let x = row.trailing_zeros() as u8;
                row &= row - 1;
                f(x, y);
            }
        }
    }

    /// Visit each local-x column that has at least one planned cell.
    pub fn for_each_x(self, mut f: impl FnMut(u8)) {
        if self.is_dense() {
            for x in self.rect.x0..=self.rect.x1 {
                f(x);
            }
            return;
        }
        let mut cols = 0u64;
        for y in self.rect.y0..=self.rect.y1 {
            cols |= self.bits.0[y as usize];
        }
        cols &= Self::row_mask(self.rect.x0, self.rect.x1);
        while cols != 0 {
            let x = cols.trailing_zeros() as u8;
            cols &= cols - 1;
            f(x);
        }
    }

    /// Visit planned rows in one column (bottom-up, gravity order).
    pub fn for_each_y_in_col(self, x: u8, mut f: impl FnMut(u8)) {
        if self.is_dense() {
            for y in self.rect.y0..=self.rect.y1 {
                f(y);
            }
            return;
        }
        let bit = 1u64 << x;
        for y in self.rect.y0..=self.rect.y1 {
            if self.bits.0[y as usize] & bit != 0 {
                f(y);
            }
        }
    }
}

fn merge_rect(a: Rect, b: Rect) -> Rect {
    Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

fn clamp_u8(v: i32, max: i32) -> u8 {
    v.clamp(0, max) as u8
}

/// Number of chunk columns along x when the world wraps, else `None`.
fn wrap_chunk_span_x(world: &World) -> Option<i32> {
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

fn add_region(map: &mut HashMap<ChunkCoord, Rect>, coord: ChunkCoord, rect: Rect) {
    map.entry(coord)
        .and_modify(|e| *e = merge_rect(*e, rect))
        .or_insert(rect);
}

/// Inflate `rect` by a neighbour halo and merge into `map`, waking
/// abutting chunks when the halo crosses a seam.
///
/// Horizontal halo is 1 cell. Vertical halo is **2** cells so water at
/// the cy seam (world y=64 ↔ y=63) can still plan cascade/soak into
/// y=62 in the same dirty generation — a 1-cell vertical wake left a
/// persistent shelf / dry line at y≈62/63 in playtest.
fn inflate_wake(
    map: &mut HashMap<ChunkCoord, Rect>,
    coord: ChunkCoord,
    rect: Rect,
    span_x: Option<i32>,
) {
    let w = CHUNK_CELLS_W as i32;
    let h = CHUNK_CELLS_H as i32;
    const HALO_X: i32 = 1;
    const HALO_Y: i32 = 2;
    let x0 = rect.x0 as i32 - HALO_X;
    let y0 = rect.y0 as i32 - HALO_Y;
    let x1 = rect.x1 as i32 + HALO_X;
    let y1 = rect.y1 as i32 + HALO_Y;

    // Home chunk (clamped).
    add_region(
        map,
        coord,
        Rect {
            x0: clamp_u8(x0, w - 1),
            y0: clamp_u8(y0, h - 1),
            x1: clamp_u8(x1, w - 1),
            y1: clamp_u8(y1, h - 1),
        },
    );

    let y0c = clamp_u8(y0, h - 1);
    let y1c = clamp_u8(y1, h - 1);
    let x0c = clamp_u8(x0, w - 1);
    let x1c = clamp_u8(x1, w - 1);

    if x0 < 0 {
        let n = ChunkCoord::new(wrap_cx(coord.cx - 1, span_x), coord.cy);
        add_region(
            map,
            n,
            Rect {
                x0: (w - 1) as u8,
                y0: y0c,
                x1: (w - 1) as u8,
                y1: y1c,
            },
        );
    }
    if x1 >= w {
        let n = ChunkCoord::new(wrap_cx(coord.cx + 1, span_x), coord.cy);
        add_region(
            map,
            n,
            Rect {
                x0: 0,
                y0: y0c,
                x1: 0,
                y1: y1c,
            },
        );
    }
    if y0 < 0 {
        // Overlap into cy-1: rows [h+y0, h-1] (e.g. y0=-2 → h-2..=h-1).
        let n = ChunkCoord::new(coord.cx, coord.cy - 1);
        let below_y0 = (h + y0).max(0).min(h - 1) as u8;
        add_region(
            map,
            n,
            Rect {
                x0: x0c,
                y0: below_y0,
                x1: x1c,
                y1: (h - 1) as u8,
            },
        );
    }
    if y1 >= h {
        // Overlap into cy+1: rows [0, y1-h].
        let n = ChunkCoord::new(coord.cx, coord.cy + 1);
        let above_y1 = (y1 - h).max(0).min(h - 1) as u8;
        add_region(
            map,
            n,
            Rect {
                x0: x0c,
                y0: 0,
                x1: x1c,
                y1: above_y1,
            },
        );
    }
    // Diagonals — water can fall/spill across a corner seam.
    if x0 < 0 && y0 < 0 {
        let below_y0 = (h + y0).max(0).min(h - 1) as u8;
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx - 1, span_x), coord.cy - 1),
            Rect {
                x0: (w - 1) as u8,
                y0: below_y0,
                x1: (w - 1) as u8,
                y1: (h - 1) as u8,
            },
        );
    }
    if x1 >= w && y0 < 0 {
        let below_y0 = (h + y0).max(0).min(h - 1) as u8;
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx + 1, span_x), coord.cy - 1),
            Rect {
                x0: 0,
                y0: below_y0,
                x1: 0,
                y1: (h - 1) as u8,
            },
        );
    }
    if x0 < 0 && y1 >= h {
        let above_y1 = (y1 - h).max(0).min(h - 1) as u8;
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx - 1, span_x), coord.cy + 1),
            Rect {
                x0: (w - 1) as u8,
                y0: 0,
                x1: (w - 1) as u8,
                y1: above_y1,
            },
        );
    }
    if x1 >= w && y1 >= h {
        let above_y1 = (y1 - h).max(0).min(h - 1) as u8;
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx + 1, span_x), coord.cy + 1),
            Rect {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: above_y1,
            },
        );
    }
}

/// Dilate one dirty cell into `map` (halo +1 x / +2 y, including
/// neighbour chunks and wrap). Same topology as [`inflate_wake`].
fn paint_dilated(
    map: &mut HashMap<ChunkCoord, DirtyBits>,
    coord: ChunkCoord,
    lx: i32,
    ly: i32,
    span_x: Option<i32>,
) {
    const HALO_X: i32 = 1;
    const HALO_Y: i32 = 2;
    let w = CHUNK_CELLS_W as i32;
    let h = CHUNK_CELLS_H as i32;
    for dy in -HALO_Y..=HALO_Y {
        for dx in -HALO_X..=HALO_X {
            let mut x = lx + dx;
            let mut y = ly + dy;
            let mut cx = coord.cx;
            let mut cy = coord.cy;
            if x < 0 {
                cx = wrap_cx(cx - 1, span_x);
                x += w;
            } else if x >= w {
                cx = wrap_cx(cx + 1, span_x);
                x -= w;
            }
            if y < 0 {
                cy -= 1;
                y += h;
            } else if y >= h {
                cy += 1;
                y -= h;
            }
            if !(0..w).contains(&x) || !(0..h).contains(&y) {
                continue;
            }
            map.entry(ChunkCoord::new(cx, cy))
                .or_default()
                .set(x as u8, y as u8);
        }
    }
}

fn dilate_bits(
    map: &mut HashMap<ChunkCoord, DirtyBits>,
    coord: ChunkCoord,
    src: DirtyBits,
    span_x: Option<i32>,
) {
    for y in 0..CHUNK_CELLS_H as u8 {
        let mut row = src.0[y as usize];
        while row != 0 {
            let x = row.trailing_zeros() as u8;
            row &= row - 1;
            paint_dilated(map, coord, x as i32, y as i32, span_x);
        }
    }
}

/// Build the scan plan from current dirty cells.
///
/// Dilates each write (not the bounding box) so scattered rain does not
/// scan the AABB holes. Legacy saves with a rect and empty bits still
/// inflate the box. Returns empty when the world is quiescent. Only
/// loaded chunks are retained.
pub fn plan_active(world: &World) -> Vec<ActiveChunk> {
    let span_x = wrap_chunk_span_x(world);
    let mut bits_map: HashMap<ChunkCoord, DirtyBits> = HashMap::default();
    let mut rect_map: HashMap<ChunkCoord, Rect> = HashMap::default();
    for (coord, chunk) in &world.chunks {
        let Some(rect) = chunk.dirty else {
            continue;
        };
        if chunk.dirty_bits.is_empty() {
            inflate_wake(&mut rect_map, *coord, rect, span_x);
        } else {
            dilate_bits(&mut bits_map, *coord, chunk.dirty_bits, span_x);
        }
    }
    bits_map.retain(|c, _| world.chunks.contains_key(c));
    rect_map.retain(|c, _| world.chunks.contains_key(c));

    let mut out: Vec<ActiveChunk> = bits_map
        .into_iter()
        .filter_map(|(coord, bits)| {
            let rect = bits.bbox()?;
            Some(ActiveChunk::with_bits(coord, rect, bits))
        })
        .collect();
    for (coord, rect) in rect_map {
        // Legacy dense box — do not OR into a sparse entry.
        if out.iter().any(|a| a.coord == coord) {
            continue;
        }
        out.push(ActiveChunk::new(coord, rect));
    }
    out.sort_by(|a, b| {
        a.coord
            .cy
            .cmp(&b.coord.cy)
            .then(a.coord.cx.cmp(&b.coord.cx))
    });
    out
}

/// Clear every chunk's dirty rect. Called at the start of [`crate::rules::tick`]
/// after [`plan_active`] so rule writes form the *next* tick's plan.
pub fn clear_all_dirty(world: &mut World) {
    for chunk in world.chunks.values_mut() {
        chunk.clear_dirty();
    }
}

/// Checkerboard colour of a chunk: `0=EE, 1=OE, 2=EO, 3=OO`
/// (`E` = even, `O` = odd; first letter is `cx`, second is `cy`).
///
/// Adjacent chunks (orthogonal) always differ in colour, so a serial
/// four-pass sweep — and later a parallel one — never updates two
/// neighbouring chunks in the same sub-pass (Purho / Noita, GDC 2019).
#[inline]
pub(crate) fn checkerboard_phase(coord: ChunkCoord) -> u8 {
    let px = coord.cx.rem_euclid(2) as u8;
    let py = coord.cy.rem_euclid(2) as u8;
    px + 2 * py
}

/// Split an active plan into the four checkerboard sub-passes.
///
/// Pass order is fixed: even-cx/even-cy → odd-cx/even-cy →
/// even-cx/odd-cy → odd-cx/odd-cy. Within each pass, chunks stay
/// sorted by ascending `(cy, cx)` so bottom-up pull rules meet
/// lower rows first.
pub fn partition_checkerboard(active: &[ActiveChunk]) -> [Vec<ActiveChunk>; 4] {
    let mut passes: [Vec<ActiveChunk>; 4] = [Vec::new(), Vec::new(), Vec::new(), Vec::new()];
    for ac in active {
        let phase = checkerboard_phase(ac.coord) as usize;
        passes[phase].push(*ac);
    }
    for pass in &mut passes {
        pass.sort_by(|a, b| {
            a.coord
                .cy
                .cmp(&b.coord.cy)
                .then(a.coord.cx.cmp(&b.coord.cx))
        });
    }
    passes
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use wk_material::MaterialId;

    #[test]
    fn clean_world_plans_nothing() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // ensure_chunk doesn't dirty; explicit clear.
        clear_all_dirty(&mut w);
        assert!(plan_active(&w).is_empty());
    }

    #[test]
    fn two_far_writes_do_not_fill_the_aabb() {
        let mut w = World::new(1);
        w.set_cell(2, 2, Cell::water());
        w.set_cell(50, 50, Cell::water());
        let plan = plan_active(&w);
        assert_eq!(plan.len(), 1);
        let ac = plan[0];
        assert!(!ac.is_dense(), "fresh writes must carry dirty bits");
        assert!(ac.bits.get(2, 2));
        assert!(ac.bits.get(50, 50));
        assert!(
            !ac.bits.get(26, 26),
            "AABB hole between two writes must stay unset"
        );
        assert!(
            ac.cell_count() * 2 < ac.aabb_area(),
            "dilated bits ({}) must be much smaller than the box ({})",
            ac.cell_count(),
            ac.aabb_area()
        );
        let mut n = 0usize;
        ac.for_each_cell(|x, y| {
            n += 1;
            assert!(
                ac.visits(x, y),
                "for_each_cell must only yield planned cells"
            );
        });
        assert_eq!(n, ac.cell_count());
        let mut band = 0usize;
        ac.for_each_cell_in_y(0, 10, |_, _| band += 1);
        assert!(band < n, "y-band walk must skip the high write at y=50");
    }

    #[test]
    fn write_wakes_inflated_rect() {
        let mut w = World::new(1);
        w.set_cell(10, 10, Cell::water());
        let plan = plan_active(&w);
        assert_eq!(plan.len(), 1);
        let r = plan[0].rect;
        assert!(r.contains(10, 10));
        assert!(r.contains(9, 10));
        assert!(r.contains(11, 11));
    }

    #[test]
    fn edge_write_wakes_neighbour_chunk() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        clear_all_dirty(&mut w);
        // Right edge of chunk 0.
        w.set_cell((CHUNK_CELLS_W as i32) - 1, 5, Cell::water());
        let plan = plan_active(&w);
        assert!(plan.iter().any(|a| a.coord == ChunkCoord::new(0, 0)));
        assert!(
            plan.iter().any(|a| a.coord == ChunkCoord::new(1, 0)),
            "neighbour chunk must wake"
        );
    }

    #[test]
    fn vertical_seam_write_wakes_two_rows_below() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        clear_all_dirty(&mut w);
        // Bottom row of cy=1 (world y=64).
        w.set_cell(7, CHUNK_CELLS_H as i32, Cell::water());
        let plan = plan_active(&w);
        let below = plan
            .iter()
            .find(|a| a.coord == ChunkCoord::new(0, 0))
            .expect("cy-1 must wake");
        assert!(
            below.rect.contains(7, (CHUNK_CELLS_H - 1) as u8),
            "must wake y=63"
        );
        assert!(
            below.rect.contains(7, (CHUNK_CELLS_H - 2) as u8),
            "must wake y=62 (2-cell vertical halo across seam)"
        );
    }

    #[test]
    fn wrap_edge_wakes_last_chunk() {
        let mut w = World::new(1);
        w.wrap_width = Some(128); // 2 chunks wide
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        clear_all_dirty(&mut w);
        w.set_cell(0, 3, Cell::solid(MaterialId::Sand));
        let plan = plan_active(&w);
        assert!(
            plan.iter().any(|a| a.coord == ChunkCoord::new(1, 0)),
            "cx=0 left edge should wake cx=1 under wrap"
        );
    }

    #[test]
    fn checkerboard_phase_matches_parity() {
        assert_eq!(checkerboard_phase(ChunkCoord::new(0, 0)), 0);
        assert_eq!(checkerboard_phase(ChunkCoord::new(1, 0)), 1);
        assert_eq!(checkerboard_phase(ChunkCoord::new(0, 1)), 2);
        assert_eq!(checkerboard_phase(ChunkCoord::new(1, 1)), 3);
        assert_eq!(checkerboard_phase(ChunkCoord::new(-1, 0)), 1);
        assert_eq!(checkerboard_phase(ChunkCoord::new(0, -1)), 2);
    }

    #[test]
    fn partition_covers_all_and_neighbours_differ() {
        let active: Vec<ActiveChunk> = [
            ChunkCoord::new(0, 0),
            ChunkCoord::new(1, 0),
            ChunkCoord::new(0, 1),
            ChunkCoord::new(1, 1),
            ChunkCoord::new(2, 0),
        ]
        .into_iter()
        .map(|coord| ActiveChunk::new(coord, Rect::full()))
        .collect();
        let passes = partition_checkerboard(&active);
        let total: usize = passes.iter().map(|p| p.len()).sum();
        assert_eq!(total, active.len());
        for pass in &passes {
            for a in pass {
                for b in pass {
                    if a.coord == b.coord {
                        continue;
                    }
                    let dx = (a.coord.cx - b.coord.cx).abs();
                    let dy = (a.coord.cy - b.coord.cy).abs();
                    assert!(
                        !(dx + dy == 1),
                        "orthogonal neighbours must not share a pass: {:?} vs {:?}",
                        a.coord,
                        b.coord
                    );
                }
            }
        }
    }
}
