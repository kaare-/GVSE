//! Build tools and the fill slider.
//!
//! Tool kinds match the C ABI:
//! 1 paint (material byte; `Cell::GLASS` = 18 paints a glass wall) ·
//! 2 erase · 3 pour · 4 drain · 5 set fill level · 6 undo ·
//! 7 swipe film · 8 seed algae · 9 remove anything (organisms whole,
//! corpses, litter, grit) · 10 glucose drip · 11 calcium feed ·
//! 12 detritus feed · 13 clean (litter, grit, corpses only) ·
//! 14 top-up (pure water onto every free surface, `r` rows; Phase 4) ·
//! 15 cut / root-trim (kill cells under the stroke; roots → soft litter).
//! The scoop is not a tool event: `Tank::scoop_region` / `release`.

use wk_material::MaterialId;

use crate::cell::Cell;
use crate::grid::PhysicsGrid;

#[derive(Debug, Clone, Copy)]
pub struct ToolEvent {
    /// 1 paint, 2 erase, 3 pour, 4 drain, 5 fill level, 6 undo, 7 swipe,
    /// 8 seed, 9 remove, 10 glucose, 11 calcium, 12 detritus, 13 clean,
    /// 14 top-up, 15 cut / root-trim.
    pub kind: u8,
    pub material: u8,
    /// Visible physics cell, origin at the bottom-left of the interior.
    pub x: u16,
    pub y: u16,
    pub r: u16,
    /// Colour variant 0..=3 for sand and rock (render only).
    pub variant: u8,
}

pub const UNDO_CAP: usize = 32;

/// Snapshot of the interior cells for undo. The bedrock frame is fixed.
#[derive(Clone)]
pub struct UndoFrame {
    pub cells: Vec<Cell>,
}

pub fn capture(grid: &PhysicsGrid) -> UndoFrame {
    UndoFrame {
        cells: grid.cells().to_vec(),
    }
}

pub fn restore(grid: &mut PhysicsGrid, frame: &UndoFrame) {
    if frame.cells.len() != grid.cells().len() {
        return;
    }
    grid.cells_mut().copy_from_slice(&frame.cells);
}

/// Returns true when the grid changed.
pub fn apply_one(grid: &mut PhysicsGrid, tool: &ToolEvent) -> bool {
    match tool.kind {
        1 => paint(grid, tool),
        2 => erase(grid, tool),
        3 => pour(grid, tool),
        4 => drain(grid, tool),
        5 => set_fill_level(grid, tool.y as i32),
        14 => crate::humidity::top_up(grid, tool.r as i32),
        _ => false,
    }
}

fn paint(grid: &mut PhysicsGrid, tool: &ToolEvent) -> bool {
    let glass = tool.material == Cell::GLASS;
    let id = MaterialId::from_u8(tool.material).unwrap_or(MaterialId::Sand);
    if id == MaterialId::Bedrock && !glass {
        return false;
    }
    let variant = tool.variant & 0b11;
    let mut changed = false;
    for_brush(grid, tool, |g, vx, vy| {
        let mut cell = if glass {
            Cell::glass()
        } else if id == MaterialId::Air {
            Cell::air()
        } else {
            Cell::solid(id)
        };
        cell.set_colour_variant(variant);
        if g.get_visible(vx, vy) != cell {
            g.set_visible(vx, vy, cell);
            changed = true;
        }
    });
    changed
}

fn erase(grid: &mut PhysicsGrid, tool: &ToolEvent) -> bool {
    let mut changed = false;
    for_brush(grid, tool, |g, vx, vy| {
        let empty = Cell::air();
        if g.get_visible(vx, vy) != empty {
            g.set_visible(vx, vy, empty);
            changed = true;
        }
    });
    changed
}

fn pour(grid: &mut PhysicsGrid, tool: &ToolEvent) -> bool {
    let mut changed = false;
    for_brush(grid, tool, |g, vx, vy| {
        let cur = g.get_visible(vx, vy);
        if cur.is_air() {
            if cur.sat != 255 {
                g.set_visible(vx, vy, Cell::water());
                changed = true;
            }
        } else {
            let cap = crate::cell::water_capacity(cur.id());
            if cap > cur.sat {
                let mut c = cur;
                c.sat = cap;
                g.set_visible(vx, vy, c);
                changed = true;
            }
        }
    });
    changed
}

fn drain(grid: &mut PhysicsGrid, tool: &ToolEvent) -> bool {
    let mut changed = false;
    for_brush(grid, tool, |g, vx, vy| {
        let cur = g.get_visible(vx, vy);
        if cur.is_air() {
            if cur.sat != 0 {
                g.set_visible(vx, vy, Cell::air());
                changed = true;
            }
        } else if cur.sat != 0 {
            let mut c = cur;
            c.sat = 0;
            g.set_visible(vx, vy, c);
            changed = true;
        }
    });
    changed
}

/// Water from the bed up to `level` (exclusive of the air above).
/// Solids stay. Air below the level becomes full water; free water
/// above it becomes dry air. `level == 0` empties the tank.
pub fn set_fill_level(grid: &mut PhysicsGrid, level: i32) -> bool {
    let w = grid.visible_w();
    let h = grid.visible_h();
    let level = level.clamp(0, h);
    let mut changed = false;
    for vx in 0..w {
        for vy in 0..h {
            let cur = grid.get_visible(vx, vy);
            if !cur.is_air() {
                continue;
            }
            let want = if vy < level {
                Cell::water()
            } else {
                Cell::air()
            };
            if cur != want {
                grid.set_visible(vx, vy, want);
                changed = true;
            }
        }
    }
    changed
}

fn for_brush(
    grid: &mut PhysicsGrid,
    tool: &ToolEvent,
    mut f: impl FnMut(&mut PhysicsGrid, i32, i32),
) {
    let r = tool.r.max(1) as i32;
    let cx = tool.x as i32;
    let cy = tool.y as i32;
    let vw = grid.visible_w();
    let vh = grid.visible_h();
    for dy in -r..=r {
        for dx in -r..=r {
            if dx * dx + dy * dy > r * r {
                continue;
            }
            let vx = cx + dx;
            let vy = cy + dy;
            if vx < 0 || vy < 0 || vx >= vw || vy >= vh {
                continue;
            }
            f(grid, vx, vy);
        }
    }
}
