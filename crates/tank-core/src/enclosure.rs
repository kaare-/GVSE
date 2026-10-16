//! Phase 4 — enclosure management: a region of open physics cells
//! bounded by `Glass` walls (roadmap Phase 4). Select by a seed cell
//! inside; delete walls, drain, fill, or release the contents.

use std::collections::VecDeque;

use crate::cell::Cell;
use crate::grid::PhysicsGrid;
use crate::humidity::{is_air_cell, is_water_cell};

/// A flood-filled open region walled by glass (and the tank border).
#[derive(Debug, Clone)]
pub struct Enclosure {
    /// Visible physics cells (open) inside the enclosure.
    pub interior: Vec<(i32, i32)>,
    /// Glass cells that touch the interior.
    pub walls: Vec<(i32, i32)>,
}

impl Enclosure {
    pub fn is_empty(&self) -> bool {
        self.interior.is_empty()
    }

    pub fn area(&self) -> usize {
        self.interior.len()
    }
}

/// Flood-fill from a seed open cell. Returns `None` if the seed is not
/// open, or if the fill escapes to the tank border without hitting glass
/// (not an enclosure — just the open tank).
pub fn find(grid: &PhysicsGrid, seed_x: i32, seed_y: i32) -> Option<Enclosure> {
    let w = grid.visible_w();
    let h = grid.visible_h();
    if seed_x < 0 || seed_y < 0 || seed_x >= w || seed_y >= h {
        return None;
    }
    let seed = grid.get_visible(seed_x, seed_y);
    if !seed.is_open() {
        return None;
    }
    let mut seen = vec![false; (w * h) as usize];
    let mut q = VecDeque::new();
    let mut interior = Vec::new();
    let mut walls: Vec<(i32, i32)> = Vec::new();
    let mut escaped = false;
    q.push_back((seed_x, seed_y));
    seen[(seed_y * w + seed_x) as usize] = true;
    while let Some((x, y)) = q.pop_front() {
        interior.push((x, y));
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (x + dx, y + dy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                escaped = true;
                continue;
            }
            let i = (ny * w + nx) as usize;
            if seen[i] {
                continue;
            }
            let n = grid.get_visible(nx, ny);
            if n.is_glass() {
                seen[i] = true;
                if !walls.contains(&(nx, ny)) {
                    walls.push((nx, ny));
                }
                continue;
            }
            if n.is_open() {
                seen[i] = true;
                q.push_back((nx, ny));
            }
        }
    }
    if walls.is_empty() || escaped {
        // Open to the border with no glass — not a managed enclosure.
        return None;
    }
    Some(Enclosure { interior, walls })
}

/// Replace every glass wall of the enclosure with air.
pub fn delete_walls(grid: &mut PhysicsGrid, enc: &Enclosure) -> u32 {
    let mut n = 0u32;
    for &(x, y) in &enc.walls {
        if grid.get_visible(x, y).is_glass() {
            grid.set_visible(x, y, Cell::air());
            n += 1;
        }
    }
    n
}

/// Drain every open cell inside to dry air (solids untouched).
pub fn drain(grid: &mut PhysicsGrid, enc: &Enclosure) -> u32 {
    let mut n = 0u32;
    for &(x, y) in &enc.interior {
        let c = grid.get_visible(x, y);
        if is_water_cell(c) || (c.is_open() && c.sat > 0) {
            grid.set_visible(x, y, Cell::air());
            n += 1;
        }
    }
    n
}

/// Fill every open cell inside with free water.
pub fn fill(grid: &mut PhysicsGrid, enc: &Enclosure) -> u32 {
    let mut n = 0u32;
    for &(x, y) in &enc.interior {
        let c = grid.get_visible(x, y);
        if is_air_cell(c) || (c.is_open() && c.sat < 255) {
            grid.set_visible(x, y, Cell::water());
            n += 1;
        }
    }
    n
}
