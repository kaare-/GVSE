//! Pore water. Free water soaks into porous solids at a permeability
//! rate; excess above field capacity weeps into air below. Pore water
//! never freefalls the way a droplet does.

use crate::cell::{retained_sat, seepage_rate, water_capacity};
use crate::grid::PhysicsGrid;
use crate::rules::plan::ActiveRect;

pub fn apply(grid: &mut PhysicsGrid, active: &ActiveRect) {
    // Top-down, so a lake wets the bed from the contact downward.
    for y in (active.y0..=active.y1).rev() {
        for x in active.x0..=active.x1 {
            soak(grid, x, y);
        }
    }
    for y in active.y0..=active.y1 {
        for x in active.x0..=active.x1 {
            weep(grid, x, y);
        }
    }
}

fn soak(grid: &mut PhysicsGrid, x: i32, y: i32) {
    let dst = grid.get(x, y);
    if dst.is_air() {
        return;
    }
    let id = dst.id();
    let rate = seepage_rate(id) as i32;
    let cap = water_capacity(id) as i32;
    if rate == 0 || cap == 0 {
        return;
    }
    let mut cell = dst;
    let mut room = cap - cell.sat as i32;
    if room <= 0 {
        return;
    }
    let neighbours = [(x, y + 1), (x - 1, y), (x + 1, y)];
    for (sx, sy) in neighbours {
        if room <= 0 {
            break;
        }
        let src = grid.get(sx, sy);
        let offer = if src.is_air() {
            src.sat as i32
        } else if water_capacity(src.id()) > 0 && src.sat > cell.sat {
            (src.sat - cell.sat) as i32
        } else {
            0
        };
        if offer <= 0 {
            continue;
        }
        let mv = offer.min(room).min(rate);
        let mut s = src;
        s.sat -= mv as u8;
        cell.sat += mv as u8;
        room -= mv;
        grid.set(sx, sy, s);
    }
    grid.set(x, y, cell);
}

fn weep(grid: &mut PhysicsGrid, x: i32, y: i32) {
    let dst = grid.get(x, y);
    if dst.is_air() {
        return;
    }
    let id = dst.id();
    let rate = seepage_rate(id) as i32;
    if rate == 0 {
        return;
    }
    let below = grid.get(x, y - 1);
    if !below.is_air() {
        return;
    }
    let retain = retained_sat(id) as i32;
    let excess = dst.sat as i32 - retain;
    let room = 255 - below.sat as i32;
    let mv = excess.min(room).min(rate);
    if mv <= 0 {
        return;
    }
    let mut d = dst;
    let mut b = below;
    d.sat -= mv as u8;
    b.sat += mv as u8;
    grid.set(x, y, d);
    grid.set(x, y - 1, b);
}
