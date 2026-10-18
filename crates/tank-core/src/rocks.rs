//! Drawn rocks. A stroke while the tank is paused lays down a silhouette.
//! On play, each connected silhouette becomes one stone: it falls through
//! air and water, turns as it drops, and rests on sand or the floor
//! without breaking into tiles.

use std::collections::{HashMap, HashSet};

use crate::cell::Cell;
use crate::grid::PhysicsGrid;

/// Turn silhouettes into rigid rocks, one id per connected stroke.
pub fn commit_silhouettes(grid: &mut PhysicsGrid) {
    let w = grid.visible_w();
    let h = grid.visible_h();
    if w <= 0 || h <= 0 {
        return;
    }
    let mut used = [false; 256];
    used[0] = true;
    for vy in 0..h {
        for vx in 0..w {
            let c = grid.get_visible(vx, vy);
            if c.is_rigid() {
                used[c.pad as usize] = true;
            }
        }
    }
    let mut seen = vec![false; (w * h) as usize];
    for vy in 0..h {
        for vx in 0..w {
            let i = (vy * w + vx) as usize;
            if seen[i] {
                continue;
            }
            let c = grid.get_visible(vx, vy);
            if !c.is_silhouette() {
                continue;
            }
            let mut stack = vec![(vx, vy)];
            let mut body = Vec::new();
            seen[i] = true;
            while let Some((x, y)) = stack.pop() {
                body.push((x, y));
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        if dx == 0 && dy == 0 {
                            continue;
                        }
                        let (nx, ny) = (x + dx, y + dy);
                        if nx < 0 || ny < 0 || nx >= w || ny >= h {
                            continue;
                        }
                        let ni = (ny * w + nx) as usize;
                        if seen[ni] || !grid.get_visible(nx, ny).is_silhouette() {
                            continue;
                        }
                        seen[ni] = true;
                        stack.push((nx, ny));
                    }
                }
            }
            let Some(id) = alloc_id(&mut used) else {
                continue;
            };
            for (x, y) in body {
                let mut cell = grid.get_visible(x, y);
                cell.flags = (cell.flags & !Cell::SILHOUETTE) | Cell::RIGID | Cell::BOUND;
                cell.pad = id;
                grid.set_visible(x, y, cell);
            }
        }
    }
}

/// Drop and turn every rigid rock. Settled rocks stay one piece.
pub fn step(grid: &mut PhysicsGrid, tick: u64) {
    let w = grid.visible_w();
    let h = grid.visible_h();
    let mut bodies: HashMap<u8, Vec<(i32, i32)>> = HashMap::new();
    for vy in 0..h {
        for vx in 0..w {
            let c = grid.get_visible(vx, vy);
            if c.is_rigid() && c.pad != 0 {
                bodies.entry(c.pad).or_default().push((vx, vy));
            }
        }
    }
    let mut ids: Vec<u8> = bodies.keys().copied().collect();
    ids.sort_unstable();
    for id in ids {
        let Some(cells) = bodies.get(&id) else {
            continue;
        };
        let cells = cells.clone();
        if translate(grid, &cells, 0, -1) {
            let fallen: Vec<(i32, i32)> = cells.iter().map(|(x, y)| (*x, y - 1)).collect();
            if tick % 4 == 3 {
                if let Some(turned) = rotated(&fallen) {
                    let _ = occupy(grid, &fallen, &turned);
                }
            }
            continue;
        }
        // Blocked. Tip onto a side only when that lets the stone drop.
        if let Some(turned) = rotated(&cells) {
            if occupy(grid, &cells, &turned) && !translate(grid, &turned, 0, -1) {
                let _ = occupy(grid, &turned, &cells);
            }
        }
    }
}

fn alloc_id(used: &mut [bool; 256]) -> Option<u8> {
    for id in 1..256 {
        if !used[id] {
            used[id] = true;
            return Some(id as u8);
        }
    }
    None
}

/// 90° clockwise around the stone's bounding box, tipping it onto a side.
fn rotated(cells: &[(i32, i32)]) -> Option<Vec<(i32, i32)>> {
    if cells.len() < 2 {
        return None;
    }
    let minx = cells.iter().map(|c| c.0).min()?;
    let maxx = cells.iter().map(|c| c.0).max()?;
    let miny = cells.iter().map(|c| c.1).min()?;
    let next: Vec<(i32, i32)> = cells
        .iter()
        .map(|(x, y)| (minx + (y - miny), miny + (maxx - x)))
        .collect();
    let mut uniq = HashSet::new();
    if next.iter().any(|p| !uniq.insert(*p)) {
        return None;
    }
    if next.len() == cells.len() && next.iter().all(|p| cells.contains(p)) {
        return None;
    }
    Some(next)
}

fn translate(grid: &mut PhysicsGrid, from: &[(i32, i32)], dx: i32, dy: i32) -> bool {
    let to: Vec<(i32, i32)> = from.iter().map(|(x, y)| (x + dx, y + dy)).collect();
    occupy(grid, from, &to)
}

/// Move the stone cells from `from` onto `to`, pouring the displaced
/// air or water back into the cells that were left behind.
fn occupy(grid: &mut PhysicsGrid, from: &[(i32, i32)], to: &[(i32, i32)]) -> bool {
    if from.is_empty() || from.len() != to.len() {
        return false;
    }
    let w = grid.visible_w();
    let h = grid.visible_h();
    let from_set: HashSet<(i32, i32)> = from.iter().copied().collect();
    let mut to_set = HashSet::new();
    for &p in to {
        if !to_set.insert(p) {
            return false;
        }
        if p.0 < 0 || p.1 < 0 || p.0 >= w || p.1 >= h {
            return false;
        }
        if from_set.contains(&p) {
            continue;
        }
        if !grid.get_visible(p.0, p.1).is_air() {
            return false;
        }
    }
    let mut fluids = Vec::new();
    for &p in to {
        if !from_set.contains(&p) {
            fluids.push(grid.get_visible(p.0, p.1));
        }
    }
    let mut vacate = Vec::new();
    for &p in from {
        if !to_set.contains(&p) {
            vacate.push(p);
        }
    }
    if fluids.len() != vacate.len() {
        return false;
    }
    let stones: Vec<Cell> = from.iter().map(|&(x, y)| grid.get_visible(x, y)).collect();
    for (i, &(x, y)) in to.iter().enumerate() {
        grid.set_visible(x, y, stones[i]);
    }
    for (i, &(x, y)) in vacate.iter().enumerate() {
        grid.set_visible(x, y, fluids[i]);
    }
    true
}
