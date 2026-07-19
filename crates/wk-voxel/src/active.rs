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

use std::collections::HashMap;

use crate::chunk::{ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// One chunk region that should be visited by the next rule pass.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ActiveChunk {
    pub coord: ChunkCoord,
    /// Inclusive local-cell rectangle to scan.
    pub rect: Rect,
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

/// Inflate `rect` by one cell and merge into `map`, waking neighbour
/// chunks when the halo crosses a chunk seam.
fn inflate_wake(
    map: &mut HashMap<ChunkCoord, Rect>,
    coord: ChunkCoord,
    rect: Rect,
    span_x: Option<i32>,
) {
    let w = CHUNK_CELLS_W as i32;
    let h = CHUNK_CELLS_H as i32;
    let x0 = rect.x0 as i32 - 1;
    let y0 = rect.y0 as i32 - 1;
    let x1 = rect.x1 as i32 + 1;
    let y1 = rect.y1 as i32 + 1;

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
        let n = ChunkCoord::new(coord.cx, coord.cy - 1);
        add_region(
            map,
            n,
            Rect {
                x0: x0c,
                y0: (h - 1) as u8,
                x1: x1c,
                y1: (h - 1) as u8,
            },
        );
    }
    if y1 >= h {
        let n = ChunkCoord::new(coord.cx, coord.cy + 1);
        add_region(
            map,
            n,
            Rect {
                x0: x0c,
                y0: 0,
                x1: x1c,
                y1: 0,
            },
        );
    }
    // Diagonals — water can fall/spill across a corner seam.
    if x0 < 0 && y0 < 0 {
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx - 1, span_x), coord.cy - 1),
            Rect {
                x0: (w - 1) as u8,
                y0: (h - 1) as u8,
                x1: (w - 1) as u8,
                y1: (h - 1) as u8,
            },
        );
    }
    if x1 >= w && y0 < 0 {
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx + 1, span_x), coord.cy - 1),
            Rect {
                x0: 0,
                y0: (h - 1) as u8,
                x1: 0,
                y1: (h - 1) as u8,
            },
        );
    }
    if x0 < 0 && y1 >= h {
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx - 1, span_x), coord.cy + 1),
            Rect {
                x0: (w - 1) as u8,
                y0: 0,
                x1: (w - 1) as u8,
                y1: 0,
            },
        );
    }
    if x1 >= w && y1 >= h {
        add_region(
            map,
            ChunkCoord::new(wrap_cx(coord.cx + 1, span_x), coord.cy + 1),
            Rect {
                x0: 0,
                y0: 0,
                x1: 0,
                y1: 0,
            },
        );
    }
}

/// Build the scan plan from current dirty rectangles.
///
/// Returns an empty list when the world is fully quiescent (no chunk
/// has a dirty rect). Only chunks that already exist in `world.chunks`
/// are retained — waking a neighbour that was never stamped is a no-op.
pub fn plan_active(world: &World) -> Vec<ActiveChunk> {
    let span_x = wrap_chunk_span_x(world);
    let mut map: HashMap<ChunkCoord, Rect> = HashMap::new();
    for (coord, chunk) in &world.chunks {
        if let Some(rect) = chunk.dirty {
            inflate_wake(&mut map, *coord, rect, span_x);
        }
    }
    // Drop wakes for chunks that aren't loaded.
    map.retain(|c, _| world.chunks.contains_key(c));

    let mut out: Vec<ActiveChunk> = map
        .into_iter()
        .map(|(coord, rect)| ActiveChunk { coord, rect })
        .collect();
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
}
