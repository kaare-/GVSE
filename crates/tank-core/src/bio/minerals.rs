//! Dissolved minerals (§a.3): one `u8` per visible physics cell in
//! `CoarseField::minerals`, in quarter-m units (`MINERAL_Q` field units
//! = 1 m). Consumed by WALL hardness, BONE and SHELL; returned by
//! bone-band lysis of bone litter; raised by the player's calcium feed.
//! Diffuses slowly through water (pairwise exchange every 8 ticks).
//! K2#1: when the local field is empty, hard structures wait.

use crate::grid::PhysicsGrid;
use crate::pixels::CoarseField;

/// Field units per mineral unit (m).
pub const MINERAL_Q: u32 = 4;
/// Background hardness of fresh tank water (16 m per physics cell).
pub const MINERAL_DEFAULT: u8 = 64;
/// BONE: `1 g + 2 m` per pixel (§a.4 S1).
pub const BONE_M_UNITS: u8 = 8;
/// SHELL: `0.8 g + 3 m` per pixel (§a.4 S2).
pub const SHELL_M_UNITS: u8 = 12;
/// Bone litter lysis also frees the organic matrix: glucose units
/// (`GLUCOSE_UNIT_FP`) per bone-litter unit (half the g of a bone pixel;
/// the rest is grit, 2.3.5).
pub const BONE_GLUCOSE_PER_UNIT: u8 = 4;
/// Minerals exchange between water cells every this many ticks.
pub const DIFFUSE_EVERY: u64 = 8;

/// Coarse index of the physics cell under pixel `(px, py)`.
#[inline]
pub fn idx_of_pixel(coarse: &CoarseField, px: u16, py: u16) -> usize {
    let vx = px as u32 / crate::consts::PX_PER_CELL;
    let vy_top = py as u32 / crate::consts::PX_PER_CELL;
    let vy = coarse.h.saturating_sub(1).saturating_sub(vy_top);
    (vy as usize) * (coarse.w as usize) + (vx as usize).min(coarse.w as usize - 1)
}

/// WALL mineral cost per pixel in field units: `2·h·units` m (§a.4 #7),
/// rounded to the nearest quarter.
#[inline]
pub fn wall_units_px(wall_units: u8, wall_h: u8) -> u8 {
    ((2 * MINERAL_Q * wall_units as u32 * wall_h as u32 + 127) / 255).min(255) as u8
}

/// The neighbourhood a cell may draw from: its own physics cell first,
/// then the four cardinal neighbours.
fn neighbourhood(coarse: &CoarseField, idx: usize) -> [Option<usize>; 5] {
    let w = coarse.w as usize;
    let h = coarse.h as usize;
    let x = idx % w;
    let y = idx / w;
    [
        Some(idx),
        (x > 0).then(|| idx - 1),
        (x + 1 < w).then(|| idx + 1),
        (y > 0).then(|| idx - w),
        (y + 1 < h).then(|| idx + w),
    ]
}

/// Minerals available to a pixel's neighbourhood.
pub fn available(coarse: &CoarseField, idx: usize) -> u32 {
    neighbourhood(coarse, idx)
        .iter()
        .flatten()
        .map(|&i| coarse.minerals[i] as u32)
        .sum()
}

/// Take `need` field units from the neighbourhood of `idx` (own cell
/// first). Returns false — and takes nothing — when there is not enough.
pub fn take(coarse: &mut CoarseField, idx: usize, need: u32) -> bool {
    if need == 0 {
        return true;
    }
    if available(coarse, idx) < need {
        return false;
    }
    let mut left = need;
    for i in neighbourhood(coarse, idx).into_iter().flatten() {
        if left == 0 {
            break;
        }
        let got = (coarse.minerals[i] as u32).min(left);
        coarse.minerals[i] -= got as u8;
        left -= got;
    }
    true
}

/// Return `units` to the field at `idx`, spilling into the neighbourhood
/// when the cell is saturated.
pub fn give(coarse: &mut CoarseField, idx: usize, units: u32) {
    let mut left = units;
    for i in neighbourhood(coarse, idx).into_iter().flatten() {
        if left == 0 {
            break;
        }
        let room = 255 - coarse.minerals[i] as u32;
        let put = room.min(left);
        coarse.minerals[i] += put as u8;
        left -= put;
    }
}

pub fn total(coarse: &CoarseField) -> u64 {
    coarse.minerals.iter().map(|&m| m as u64).sum()
}

/// Slow conservative diffusion through water: every `DIFFUSE_EVERY`
/// ticks each water cell levels 1/8 of its difference with the water cell
/// to its right and the one above.
pub fn diffuse(coarse: &mut CoarseField, grid: &PhysicsGrid, tick: u64) {
    if tick % DIFFUSE_EVERY != 0 {
        return;
    }
    let w = coarse.w as usize;
    let h = coarse.h as usize;
    let water = |vx: usize, vy: usize| -> bool {
        let c = grid.get_visible(vx as i32, vy as i32);
        c.is_open() && c.sat > 40
    };
    for vy in 0..h {
        for vx in 0..w {
            let i = vy * w + vx;
            if !water(vx, vy) {
                continue;
            }
            if vx + 1 < w && water(vx + 1, vy) {
                level(&mut coarse.minerals, i, i + 1);
            }
            if vy + 1 < h && water(vx, vy + 1) {
                level(&mut coarse.minerals, i, i + w);
            }
        }
    }
}

#[inline]
fn level(m: &mut [u8], a: usize, b: usize) {
    let d = m[a] as i32 - m[b] as i32;
    let move_ = d / 8;
    match move_.cmp(&0) {
        std::cmp::Ordering::Greater => {
            m[a] -= move_ as u8;
            m[b] += move_ as u8;
        }
        std::cmp::Ordering::Less => {
            m[a] += (-move_) as u8;
            m[b] -= (-move_) as u8;
        }
        std::cmp::Ordering::Equal => {}
    }
}
