//! B3 pixel fields: keyed enzyme / toxin / signal plus dissolved glucose
//! and waste. Contact is primary; secreted fields decay and blur (K3#3).
//!
//! The grid is split into 16×16-pixel tiles; a tile carries one activity
//! bit per field and only active tiles are decayed and blurred. Blur is
//! a conservative 4-neighbour exchange (1/8 of a pixel to each open
//! neighbour per blur step) that never crosses a *blocked* pixel: solid
//! substrate, hard living tissue (`PixelLayer::block`), bone or shell.
//! Keys mix by dominance: an inflow larger than half the receiver's amount
//! takes the key.

use crate::bio::program::ring_match_q8;
use crate::consts::PX_PER_CELL;
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;

pub const TILE: usize = 16;

/// Match width assumed when a *field* (whose emitter is unknown) acts on
/// tissue or litter.
pub const FIELD_WIDTH: u8 = 24;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum KeyedField {
    Enzyme,
    Toxin,
    Signal,
}

/// Every diffusing pixel field, with its tile bit.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Field {
    Enzyme,
    Toxin,
    Signal,
    Glucose,
    Waste,
    /// Soft litter / bone: settles, never blurs or decays; digested by
    /// enzyme contact (`chem.rs`) and by the enzyme field (`lyse_litter`).
    Litter,
}

impl Field {
    pub const ALL: [Field; 6] = [
        Field::Enzyme,
        Field::Toxin,
        Field::Signal,
        Field::Glucose,
        Field::Waste,
        Field::Litter,
    ];

    #[inline]
    pub fn bit(self) -> u8 {
        match self {
            Field::Enzyme => 1,
            Field::Toxin => 2,
            Field::Signal => 4,
            Field::Glucose => 8,
            Field::Waste => 16,
            Field::Litter => 32,
        }
    }

    /// Per-tick decay in Q8 (256 = none).
    fn decay_q8(self) -> u32 {
        match self {
            Field::Enzyme | Field::Toxin => 192, // ×0.75, half-life ≈ 2.4 ticks
            Field::Signal => 235,                // ×0.92, half-life ≈ 8 ticks
            Field::Glucose | Field::Waste | Field::Litter => 256,
        }
    }

    /// Blur every n-th tick (0 = never).
    fn blur_every(self) -> u64 {
        match self {
            Field::Glucose | Field::Waste => 4,
            Field::Litter => 0,
            _ => 2,
        }
    }

    fn keyed(self) -> bool {
        matches!(self, Field::Enzyme | Field::Toxin | Field::Signal)
    }
}

impl From<KeyedField> for Field {
    fn from(k: KeyedField) -> Self {
        match k {
            KeyedField::Enzyme => Field::Enzyme,
            KeyedField::Toxin => Field::Toxin,
            KeyedField::Signal => Field::Signal,
        }
    }
}

fn bufs(pixels: &mut PixelLayer, f: Field) -> (&mut Vec<u8>, Option<&mut Vec<u8>>) {
    match f {
        Field::Enzyme => (&mut pixels.enzyme_amt, Some(&mut pixels.enzyme_key)),
        Field::Toxin => (&mut pixels.toxin_amt, Some(&mut pixels.toxin_key)),
        Field::Signal => (&mut pixels.signal_amt, Some(&mut pixels.signal_key)),
        Field::Glucose => (&mut pixels.glucose, None),
        Field::Waste => (&mut pixels.waste, None),
        Field::Litter => (&mut pixels.litter, Some(&mut pixels.litter_key)),
    }
}

/// Add litter mass with its substrate code at a pixel (dominant key).
pub fn add_litter(pixels: &mut PixelLayer, idx: usize, add: u8, code: u8) {
    if add == 0 || idx >= pixels.len() {
        return;
    }
    let cur = pixels.litter[idx];
    if cur == 0 || add >= cur {
        pixels.litter_key[idx] = code;
    }
    pixels.litter[idx] = cur.saturating_add(add);
    touch(pixels, Field::Litter, idx);
}

#[inline]
pub fn tile_of(pixels: &PixelLayer, idx: usize) -> usize {
    let pw = pixels.w as usize;
    let x = idx % pw;
    let y = idx / pw;
    (y / TILE) * pixels.tiles_w as usize + x / TILE
}

/// Mark the tile holding `idx` active for `f`.
#[inline]
pub fn touch(pixels: &mut PixelLayer, f: Field, idx: usize) {
    let t = tile_of(pixels, idx);
    if let Some(a) = pixels.tile_active.get_mut(t) {
        *a |= f.bit();
    }
    if f.keyed() {
        pixels.fields_active = true;
    }
}

/// Deposit a keyed amount at a pixel (dominant-key keep-if-higher).
pub fn deposit(pixels: &mut PixelLayer, which: KeyedField, idx: usize, add: u8, k: u8) {
    if add == 0 || idx >= pixels.len() {
        return;
    }
    let f: Field = which.into();
    {
        let (amt, key) = bufs(pixels, f);
        let cur = amt[idx];
        if add >= cur / 2 {
            if let Some(key) = key {
                key[idx] = k;
            }
        }
        amt[idx] = cur.saturating_add(add);
    }
    touch(pixels, f, idx);
}

/// Add dissolved glucose or waste at a pixel.
pub fn add_plain(pixels: &mut PixelLayer, f: Field, idx: usize, add: u8) {
    if add == 0 || idx >= pixels.len() {
        return;
    }
    {
        let (amt, _) = bufs(pixels, f);
        amt[idx] = amt[idx].saturating_add(add);
    }
    touch(pixels, f, idx);
}

/// Rebuild the tile activity bits from the field contents (after load).
pub fn rescan_tiles(pixels: &mut PixelLayer) {
    for a in pixels.tile_active.iter_mut() {
        *a = 0;
    }
    let n = pixels.len();
    for f in Field::ALL {
        for idx in 0..n {
            let nonzero = {
                let (amt, _) = bufs(pixels, f);
                amt[idx] != 0
            };
            if nonzero {
                touch(pixels, f, idx);
            }
        }
    }
    pixels.fields_active = pixels
        .tile_active
        .iter()
        .any(|&a| a & (Field::Enzyme.bit() | Field::Toxin.bit() | Field::Signal.bit()) != 0);
}

/// Whether diffusion may enter the pixel at `(x, y)`: not a barrier pixel
/// (hard tissue, bone, shell) and not solid substrate.
#[inline]
pub(crate) fn open(grid: &PhysicsGrid, pixels: &PixelLayer, x: usize, y: usize) -> bool {
    let i = y * pixels.w as usize + x;
    if pixels.block[i] != 0 {
        return false;
    }
    let vx = (x / PX_PER_CELL as usize) as i32;
    let vy = grid.visible_h() - 1 - (y / PX_PER_CELL as usize) as i32;
    grid.get_visible(vx, vy).is_open()
}

/// Same test by pixel index.
#[inline]
pub(crate) fn open_idx(grid: &PhysicsGrid, pixels: &PixelLayer, idx: usize) -> bool {
    let pw = pixels.w as usize;
    open(grid, pixels, idx % pw, idx / pw)
}

/// One tick of decay + blur over the active tiles of every field.
pub fn step_fields(pixels: &mut PixelLayer, grid: &PhysicsGrid, tick: u64) {
    let any = pixels.tile_active.iter().any(|&a| a != 0);
    if !any {
        pixels.fields_active = false;
        return;
    }
    for f in Field::ALL {
        let active: Vec<usize> = pixels
            .tile_active
            .iter()
            .enumerate()
            .filter(|(_, &a)| a & f.bit() != 0)
            .map(|(t, _)| t)
            .collect();
        if active.is_empty() {
            continue;
        }
        if f.decay_q8() != 256 {
            decay_tiles(pixels, f, &active);
        }
        if f.blur_every() > 0 && tick % f.blur_every() == 0 {
            blur_tiles(pixels, grid, f, &active);
        }
        if f == Field::Enzyme {
            lyse_litter(pixels, &active, tick);
        }
        if f == Field::Litter && tick % 4 == 2 {
            settle_litter(pixels, grid, &active);
        }
        // Retire tiles that emptied out.
        for &t in &active {
            if tile_is_zero(pixels, f, t) {
                pixels.tile_active[t] &= !f.bit();
            }
        }
    }
    pixels.fields_active = pixels
        .tile_active
        .iter()
        .any(|&a| a & (Field::Enzyme.bit() | Field::Toxin.bit() | Field::Signal.bit()) != 0);
}

fn tile_bounds(pixels: &PixelLayer, t: usize) -> (usize, usize, usize, usize) {
    let tw = pixels.tiles_w as usize;
    let tx = t % tw;
    let ty = t / tw;
    let x0 = tx * TILE;
    let y0 = ty * TILE;
    let x1 = (x0 + TILE).min(pixels.w as usize);
    let y1 = (y0 + TILE).min(pixels.h as usize);
    (x0, y0, x1, y1)
}

fn tile_is_zero(pixels: &mut PixelLayer, f: Field, t: usize) -> bool {
    let (x0, y0, x1, y1) = tile_bounds(pixels, t);
    let pw = pixels.w as usize;
    let (amt, _) = bufs(pixels, f);
    for y in y0..y1 {
        if amt[y * pw + x0..y * pw + x1].iter().any(|&a| a != 0) {
            return false;
        }
    }
    true
}

fn decay_tiles(pixels: &mut PixelLayer, f: Field, active: &[usize]) {
    let q = f.decay_q8();
    let pw = pixels.w as usize;
    for &t in active {
        let (x0, y0, x1, y1) = tile_bounds(pixels, t);
        let (amt, _) = bufs(pixels, f);
        for y in y0..y1 {
            for a in &mut amt[y * pw + x0..y * pw + x1] {
                // Round above the tail so steady states match the §b.3
                // arithmetic; floor the tail so it actually reaches zero.
                let v = *a as u32 * q;
                *a = if *a > 3 { (v + 128) >> 8 } else { v >> 8 } as u8;
            }
        }
    }
}

/// Conservative exchange: every pixel in an active tile gives 1/8 of its
/// amount to each open 4-neighbour. Inactive tiles are all-zero by
/// invariant, so flow only ever originates in active tiles; neighbours
/// that receive are activated.
fn blur_tiles(pixels: &mut PixelLayer, grid: &PhysicsGrid, f: Field, active: &[usize]) {
    let pw = pixels.w as usize;
    let ph = pixels.h as usize;
    // (target idx, give, key) — collected first so all reads see old values.
    let mut moves: Vec<(usize, usize, u8)> = Vec::new();
    for &t in active {
        let (x0, y0, x1, y1) = tile_bounds(pixels, t);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * pw + x;
                let a = {
                    let (amt, _) = bufs(pixels, f);
                    amt[i]
                };
                if a < 2 {
                    continue;
                }
                let give = a / 8;
                if give == 0 {
                    continue;
                }
                if !open(grid, pixels, x, y) {
                    continue;
                }
                for (dx, dy) in [(-1i32, 0), (1, 0), (0, -1), (0, 1)] {
                    let nx = x as i32 + dx;
                    let ny = y as i32 + dy;
                    if nx < 0 || ny < 0 || nx >= pw as i32 || ny >= ph as i32 {
                        continue;
                    }
                    let (nx, ny) = (nx as usize, ny as usize);
                    if !open(grid, pixels, nx, ny) {
                        continue;
                    }
                    moves.push((i, ny * pw + nx, give));
                }
            }
        }
    }
    let keyed = f.keyed();
    for &(from, to, give) in &moves {
        let k_from = if keyed {
            let (_, key) = bufs(pixels, f);
            key.map(|k| k[from]).unwrap_or(0)
        } else {
            0
        };
        {
            let (amt, key) = bufs(pixels, f);
            amt[from] = amt[from].saturating_sub(give);
            let cur = amt[to];
            if keyed && give >= cur / 2 {
                if let Some(key) = key {
                    key[to] = k_from;
                }
            }
            amt[to] = cur.saturating_add(give);
        }
        touch(pixels, f, to);
    }
}

/// Field lysis of litter (§a.4 ENZYME): where the enzyme field lies on
/// litter whose code matches its key, mass converts to dissolved glucose
/// in place at `amount/255 · match` of contact strength.
fn lyse_litter(pixels: &mut PixelLayer, enzyme_tiles: &[usize], tick: u64) {
    let pw = pixels.w as usize;
    for &t in enzyme_tiles {
        if pixels.tile_active[t] & Field::Litter.bit() == 0 {
            continue;
        }
        let (x0, y0, x1, y1) = tile_bounds(pixels, t);
        for y in y0..y1 {
            for x in x0..x1 {
                let i = y * pw + x;
                let (e, l) = (pixels.enzyme_amt[i], pixels.litter[i]);
                if e == 0 || l == 0 {
                    continue;
                }
                let m = ring_match_q8(pixels.enzyme_key[i], pixels.litter_key[i], FIELD_WIDTH) as u32;
                if m == 0 {
                    continue;
                }
                let rate_q8 = e as u32 * m / 128;
                let h = hash_u64(tick, i as u64, 0, 0xF1_15);
                let take = (rate_q8 >> 8) + ((h & 0xFF) < (rate_q8 & 0xFF) as u64) as u32;
                let take = take.min(l as u32).min(255 - pixels.glucose[i] as u32) as u8;
                if take == 0 {
                    continue;
                }
                pixels.litter[i] -= take;
                add_plain(pixels, Field::Glucose, i, take);
            }
        }
    }
}

/// Litter sinks one pixel per settle pass through open, unowned pixels,
/// merging only with litter of the same code. Tiles are walked bottom-up
/// so a grain moves at most once per pass.
fn settle_litter(pixels: &mut PixelLayer, grid: &PhysicsGrid, active: &[usize]) {
    let pw = pixels.w as usize;
    let ph = pixels.h as usize;
    for &t in active.iter().rev() {
        let (x0, y0, x1, y1) = tile_bounds(pixels, t);
        for y in (y0..y1).rev() {
            if y + 1 >= ph {
                continue;
            }
            for x in x0..x1 {
                let i = y * pw + x;
                let l = pixels.litter[i];
                if l == 0 {
                    continue;
                }
                let j = i + pw;
                if pixels.owner[j] != 0 || !open(grid, pixels, x, y + 1) {
                    continue;
                }
                let below = pixels.litter[j];
                if below != 0 && (pixels.litter_key[j] != pixels.litter_key[i] || below as u32 + l as u32 > 255) {
                    continue;
                }
                pixels.litter_key[j] = pixels.litter_key[i];
                pixels.litter[j] = below + l;
                pixels.litter[i] = 0;
                touch(pixels, Field::Litter, j);
            }
        }
    }
}
