//! Phase 4 — the air-side humidity field, night condensation and the very
//! slow evaporation of the water surface (`evolution-system.md` §1, K2#5,
//! K2#8; `roadmap.md` Phase 4).
//!
//! `coarse.humidity` is one `u8` per visible physics cell:
//! * free water reads 255 (so `max(humidity, hydration)` is saturated
//!   for a submerged cell), solids read their own moisture;
//! * an air cell is pulled toward an evaporation target by the water and
//!   moist substrate it touches, diffuses to its air neighbours, and
//!   leaks a little each update (the lid is not perfectly tight);
//! * at night humid air (≥ `DEW_H`) condenses on the glass, rock and
//!   sand beside it: glass gets dew (`pad`), rock its wet film, sand a
//!   little pore water up to what it holds against gravity.
//!
//! Evaporation removes water only (the one matter sink besides the clean
//! and remove tools): the free surface of each column loses one sat
//! every `EVAP_EVERY` ticks unless the air above is saturated.

use crate::cell::{retained_sat, Cell};
use crate::consts::{
    DEW_FADE, DEW_GAIN, DEW_H, EVAP_SAT_STEP, EVAP_STOP_H, HUMIDITY_EVAP_BASE, HUMIDITY_LEAK_DIV,
    HUMIDITY_PER_C, MOIST_SAT, WATER_SAT_RENDER,
};
use crate::grid::PhysicsGrid;
use crate::pixels::CoarseField;

/// Open air as the humidity field sees it (a half-evaporated surface
/// cell still counts as water, as in the render).
#[inline]
pub fn is_air_cell(c: Cell) -> bool {
    c.is_open() && c.sat < WATER_SAT_RENDER
}

/// Free water, including a partly drawn-down surface cell.
#[inline]
pub fn is_water_cell(c: Cell) -> bool {
    c.is_open() && c.sat >= WATER_SAT_RENDER
}

/// Moist substrate: damp sand / soil, wet rock film, dewy glass.
#[inline]
pub fn is_moist_substrate(c: Cell) -> bool {
    !c.is_open() && substrate_moisture(c) >= MOIST_SAT
}

/// Wetness of a solid cell, 0..=255 (glass dew lives in `pad`).
#[inline]
pub fn substrate_moisture(c: Cell) -> u8 {
    if c.is_glass() {
        c.pad
    } else {
        c.moisture()
    }
}

/// Day-time humidity target of the air right over free water.
pub fn evap_target(t_tank: f32, night: bool) -> u8 {
    if night {
        // Cool night air saturates over the water.
        return 255;
    }
    (HUMIDITY_EVAP_BASE + HUMIDITY_PER_C * (t_tank - 15.0)).clamp(64.0, 255.0) as u8
}

/// One humidity update (call every `HUMIDITY_EVERY` ticks).
pub fn step(grid: &mut PhysicsGrid, coarse: &mut CoarseField, tick: u64, t_tank: f32, night: bool) {
    let w = coarse.w as i32;
    let h = coarse.h as i32;
    if w <= 0 || h <= 0 {
        return;
    }
    const Q: i32 = 256;
    let target = evap_target(t_tank, night) as i32 * Q;
    let src = coarse.humidity.clone();
    let at = |vx: i32, vy: i32| -> Option<usize> {
        if vx < 0 || vy < 0 || vx >= w || vy >= h {
            None
        } else {
            Some((vy * w + vx) as usize)
        }
    };
    let mut dew: Vec<(i32, i32)> = Vec::new();
    for vy in 0..h {
        for vx in 0..w {
            let i = (vy * w + vx) as usize;
            let here = grid.get_visible(vx, vy);
            if is_water_cell(here) {
                coarse.humidity[i] = 255 * Q as u16;
                continue;
            }
            if !here.is_open() {
                coarse.humidity[i] = substrate_moisture(here) as u16 * Q as u16;
                continue;
            }
            // Air: sources from the four neighbours, diffusion among air.
            let mut source = 0i32;
            let mut air_sum = 0i32;
            let mut air_n = 0i32;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                let (nx, ny) = (vx + dx, vy + dy);
                let n = grid.get_visible(nx, ny);
                if is_water_cell(n) {
                    source = source.max(target);
                } else if !n.is_open() {
                    let m = substrate_moisture(n) as i32;
                    if m >= MOIST_SAT as i32 {
                        source = source.max(m * Q * 3 / 4);
                    }
                } else if let Some(j) = at(nx, ny) {
                    air_sum += src[j] as i32;
                    air_n += 1;
                }
            }
            let cur = src[i] as i32;
            let mut next = cur;
            if source > next {
                next += (source - next) / 2;
            }
            if air_n > 0 {
                next += (air_sum / air_n - next) / 2;
            }
            // Proportional leak (τ ≈ LEAK_DIV updates); the Q8 state keeps
            // the rounding from eating the tail of the field.
            next -= next.max(0) / HUMIDITY_LEAK_DIV as i32;
            let _ = tick;
            let next = next.clamp(0, 255 * Q) as u16;
            coarse.humidity[i] = next;
            if night && next >= DEW_H as u16 * Q as u16 {
                dew.push((vx, vy));
            }
        }
    }
    // Night condensation onto the solids beside humid air.
    for (vx, vy) in dew {
        let mut deposited = false;
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let (nx, ny) = (vx + dx, vy + dy);
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            let mut n = grid.get_visible(nx, ny);
            if n.is_open() {
                continue;
            }
            if n.is_glass() {
                if n.pad < 255 {
                    n.pad = n.pad.saturating_add(DEW_GAIN);
                    deposited = true;
                }
            } else if n.is_rock() {
                if n.pad < 255 {
                    n.set_rock_film(n.pad.saturating_add(DEW_GAIN));
                    deposited = true;
                }
            } else {
                let cap = retained_sat(n.id());
                if n.sat < cap {
                    n.sat = n.sat.saturating_add(DEW_GAIN).min(cap);
                    deposited = true;
                }
            }
            if deposited {
                grid.set_visible(nx, ny, n);
                break;
            }
        }
        if deposited {
            if let Some(i) = at(vx, vy) {
                coarse.humidity[i] = coarse.humidity[i].saturating_sub(DEW_GAIN as u16 / 2 * Q as u16);
            }
        }
    }
    // Glass dew dries off in drier air.
    for vy in 0..h {
        for vx in 0..w {
            let c = grid.get_visible(vx, vy);
            if !c.is_glass() || c.pad == 0 {
                continue;
            }
            let mut humid = false;
            for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                if let Some(j) = at(vx + dx, vy + dy) {
                    let n = grid.get_visible(vx + dx, vy + dy);
                    if is_water_cell(n) || (n.is_open() && coarse.humidity[j] >= DEW_H as u16 * Q as u16) {
                        humid = true;
                        break;
                    }
                }
            }
            if !humid {
                let mut c = c;
                c.pad = c.pad.saturating_sub(DEW_FADE);
                grid.set_visible(vx, vy, c);
            }
        }
    }
}

/// Evaporation from every free surface (call every `EVAP_EVERY` ticks).
/// Returns the sat units removed.
pub fn evaporate(grid: &mut PhysicsGrid, coarse: &CoarseField) -> u32 {
    let w = grid.visible_w();
    let h = grid.visible_h();
    let mut lost = 0u32;
    for vx in 0..w {
        // Top standing-water cell with open air above it.
        for vy in (0..h).rev() {
            let c = grid.get_visible(vx, vy);
            if !is_water_cell(c) {
                continue;
            }
            let above = grid.get_visible(vx, vy + 1);
            if vy + 1 < h && !is_air_cell(above) {
                break;
            }
            let hi = ((vy + 1).min(h - 1) * w + vx) as usize;
            if coarse.humidity.get(hi).is_some_and(|&hu| (hu >> 8) as u8 >= EVAP_STOP_H) {
                break;
            }
            let mut c = c;
            let take = EVAP_SAT_STEP.min(c.sat);
            c.sat -= take;
            lost += take as u32;
            grid.set_visible(vx, vy, c);
            break;
        }
    }
    lost
}

/// Top-up tool (kind 14): pour `rows` cells of pure water onto every free
/// surface in the tank (K2#8). Returns true when anything changed.
pub fn top_up(grid: &mut PhysicsGrid, rows: i32) -> bool {
    let w = grid.visible_w();
    let h = grid.visible_h();
    let rows = rows.max(1);
    let mut changed = false;
    for vx in 0..w {
        let surface = (0..h).rev().find(|&vy| is_water_cell(grid.get_visible(vx, vy)));
        let Some(top) = surface else {
            continue;
        };
        // Fill the partly-full surface cell, then the air above it.
        let mut cur = grid.get_visible(vx, top);
        if cur.sat < 255 {
            cur.sat = 255;
            grid.set_visible(vx, top, cur);
            changed = true;
        }
        for k in 1..=rows {
            let vy = top + k;
            if vy >= h {
                break;
            }
            let c = grid.get_visible(vx, vy);
            if !is_air_cell(c) {
                break;
            }
            grid.set_visible(vx, vy, Cell::water());
            changed = true;
        }
    }
    changed
}
