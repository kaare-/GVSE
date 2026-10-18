//! Biology pixel layer, coarse fields, and the RGBA composite.
//!
//! Phase 0 leaves owner / glucose / waste / keyed fields at zero.
//! Light is filled every tick so Phase 2 has a column to read.

use crate::cell::Cell;
use crate::clock::day_factor;
use crate::consts::{PX_PER_CELL, WATER_SAT_RENDER};
use crate::grid::PhysicsGrid;

#[derive(Debug, Clone)]
pub struct PixelLayer {
    pub w: u32,
    pub h: u32,
    pub owner: Vec<u32>,
    pub light: Vec<u8>,
    pub glucose: Vec<u8>,
    pub waste: Vec<u8>,
    pub enzyme_amt: Vec<u8>,
    pub enzyme_key: Vec<u8>,
    pub toxin_amt: Vec<u8>,
    pub toxin_key: Vec<u8>,
    pub signal_amt: Vec<u8>,
    pub signal_key: Vec<u8>,
    /// Soft litter: dead soft mass in `GLUCOSE_UNIT_FP` units, with its
    /// substrate code (`bio::chem::LITTER_*_CODE`) — what litter-band and
    /// bone-band enzymes digest. Settles under gravity.
    pub litter: Vec<u8>,
    pub litter_key: Vec<u8>,
    /// Grit: inert residue of lysis (§a.3 third detritus class). Settles,
    /// is never digested, leaves only with the clean tool (K2#8).
    pub grit: Vec<u8>,
    /// Front-pane film. 0 = clean. Phase 2 swipe-clears it.
    pub film: Vec<u8>,
    /// Diffusion barrier mask: hard living tissue, bone, shell (rebuilt
    /// every tick from the cell store; not saved).
    pub block: Vec<u8>,
    /// Field activity bits per 16×16 tile (`bio::fields::Field::bit`).
    pub tiles_w: u32,
    pub tiles_h: u32,
    pub tile_active: Vec<u8>,
    /// True when any keyed field may be non-zero (skip O(n) blur otherwise).
    pub fields_active: bool,
    /// Open-water lysis this tick that drifted off as fines (Phase 4):
    /// `(pixel column, glucose units)`, drained by the tank each tick.
    pub lysis_fines: Vec<(u16, u32)>,
}

impl PixelLayer {
    pub fn new(visible_w: u32, visible_h: u32) -> Self {
        let w = visible_w * PX_PER_CELL;
        let h = visible_h * PX_PER_CELL;
        let n = (w as usize) * (h as usize);
        Self {
            w,
            h,
            owner: vec![0; n],
            light: vec![0; n],
            glucose: vec![0; n],
            waste: vec![0; n],
            enzyme_amt: vec![0; n],
            enzyme_key: vec![0; n],
            toxin_amt: vec![0; n],
            toxin_key: vec![0; n],
            signal_amt: vec![0; n],
            signal_key: vec![0; n],
            litter: vec![0; n],
            litter_key: vec![0; n],
            grit: vec![0; n],
            film: vec![0; n],
            block: vec![0; n],
            tiles_w: w.div_ceil(crate::bio::fields::TILE as u32),
            tiles_h: h.div_ceil(crate::bio::fields::TILE as u32),
            tile_active: vec![
                0;
                (w.div_ceil(crate::bio::fields::TILE as u32)
                    * h.div_ceil(crate::bio::fields::TILE as u32))
                    as usize
            ],
            fields_active: false,
            lysis_fines: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        (self.w as usize) * (self.h as usize)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// Coarse fields, one sample per visible physics cell. Updated properly
/// in later phases; allocated now so the save has a stable shape.
#[derive(Debug, Clone)]
pub struct CoarseField {
    pub w: u32,
    pub h: u32,
    pub minerals: Vec<u8>,
    pub t_local: Vec<i8>,
    /// Air-side humidity in Q8 (`>> 8` is the 0..=255 value a cell reads);
    /// the extra precision keeps the slow leak and diffusion unbiased.
    pub humidity: Vec<u16>,
    /// Sediment nutrient (§a.3): waste that settled into sand / soil;
    /// CHEMO's food.
    pub sediment: Vec<u8>,
    /// Litter digested on or in this sand cell (units / 4); at
    /// `bio::fields::SOIL_DIGEST` the cell turns to `Soil` (fungus-made soil).
    pub digested: Vec<u8>,
}

impl CoarseField {
    pub fn new(visible_w: u32, visible_h: u32) -> Self {
        let n = (visible_w as usize) * (visible_h as usize);
        Self {
            w: visible_w,
            h: visible_h,
            minerals: vec![0; n],
            t_local: vec![0; n],
            humidity: vec![0; n],
            sediment: vec![0; n],
            digested: vec![0; n],
        }
    }
}

/// Incident light, top-down. Water loses 0.1 % per pixel of depth.
#[derive(Clone, Copy)]
pub struct Flashlight {
    /// Visible physics column centre.
    pub vx: i32,
    /// Half-width in physics cells.
    pub radius: i32,
    /// Boost multiplier in 1/256 units (512 = ×2).
    pub boost: u16,
}

#[allow(clippy::too_many_arguments)]
/// Column light at pixel resolution. Water loses 0.1 % per pixel row,
/// the same in every column, so a creature does not paint a dark streak
/// down the glass. Sand, rock, soil and other solids are lit from the
/// side: their own pixels keep the column's incident light, and a roof
/// nicks what is below it once per physics cell. `cloud[x]` still
/// multiplies the column.
pub fn rebuild_light(
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    tick: u64,
    always_day: bool,
    light_slider: f32,
    flash: Option<Flashlight>,
    cloud_q8: &[u8],
) {
    let vis_h = grid.visible_h();
    let pw = pixels.w as usize;
    let ph = pixels.h as usize;
    let df = if always_day { 1.0 } else { day_factor(tick) };
    let slider = light_slider.clamp(0.0, 1.0);
    let mut incident0 = (df * slider * 1000.0).round() as u32;
    incident0 = incident0 * 255 / 1000;
    // Fixed-point ×256 so the per-row 0.1 % water loss is not rounded away.
    let incident0 = incident0 * 256;

    for px in 0..pw {
        let vx = (px / PX_PER_CELL as usize) as i32;
        let mut incoming = incident0;
        if let Some(f) = flash {
            // Full boost inside the cone, fading linearly to ×1 over a
            // penumbra three radii wide (a soft torch edge — and a light
            // gradient a phototactic cell can swim up, §a.6).
            let d = (vx - f.vx).abs();
            let r = f.radius.max(1);
            if d <= 4 * r {
                let boost = if d <= r {
                    f.boost as u32
                } else {
                    256 + (f.boost as u32).saturating_sub(256) * (4 * r - d) as u32 / (3 * r) as u32
                };
                incoming = (incoming * boost / 256).min(255 * 256);
            }
        }
        if let Some(&c) = cloud_q8.get(px) {
            incoming = incoming * c as u32 / 255;
        }
        // Side light for the bed: the incident in this column, before
        // anything in it casts a shadow.
        let side = incoming / 256;
        let mut last_solid_vy = i32::MIN;
        for py in 0..ph {
            let vy_top = (py / PX_PER_CELL as usize) as i32;
            let vy = vis_h - 1 - vy_top;
            let cell = grid.get_visible(vx, vy);
            let i = py * pw + px;
            let solid = !cell.is_open() && !cell.is_glass();
            if solid {
                pixels.light[i] = side.min(255) as u8;
                if vy != last_solid_vy {
                    last_solid_vy = vy;
                    // One mild step per physics cell, so a roof still
                    // dims the water under it without blacking the grain.
                    incoming = incoming * 236 / 256;
                }
                continue;
            }
            pixels.light[i] = (incoming / 256).min(255) as u8;
            if cell.sat >= WATER_SAT_RENDER {
                incoming = incoming * 999 / 1000;
            } else if cell.is_glass() {
                // A pane loses ≈ 7 % per physics cell; a holding tank is
                // not a shadow.
                incoming = incoming * 250 / 256;
            }
        }
    }
}

/// Phase 4 environment layers drawn over open pixels: cloudiness haze
/// per column and the THERMO warm haze per coarse cell.
pub struct Overlay<'a> {
    pub cloud_q8: &'a [u8],
    pub t_local: &'a [i8],
    pub coarse_w: u32,
}

/// RGBA8, row 0 = top of the tank, nearest-neighbour 3×3 blocks.
/// `bio_rgb` paints living owners: `(owner_id, [r,g,b])` lookup via closure.
pub fn write_rgba(
    grid: &PhysicsGrid,
    pixels: &PixelLayer,
    out: &mut Vec<u8>,
    bio_rgb: &dyn Fn(u32) -> Option<[u8; 3]>,
    overlay: &Overlay,
) {
    let pw = pixels.w as usize;
    let ph = pixels.h as usize;
    out.resize(pw * ph * 4, 0);
    let vis_h = grid.visible_h();
    let any_warm = overlay.t_local.iter().any(|&v| v > 0);
    for py in 0..ph {
        for px in 0..pw {
            let vx = (px as i32) / PX_PER_CELL as i32;
            let vy_top = (py as i32) / PX_PER_CELL as i32;
            let vy = vis_h - 1 - vy_top;
            let cell = grid.get_visible(vx, vy);
            let i = py * pw + px;
            let owner = pixels.owner[i];
            let [mut r, mut g, mut b] = if owner != 0 {
                bio_rgb(owner).unwrap_or(albedo(cell))
            } else {
                albedo(cell)
            };
            if owner == 0 && !cell.is_open() && !cell.is_glass() {
                let (jr, jg, jb) = grain_speck(px, py);
                let relief = rock_relief(cell, px, py);
                r = shade(r, jr + relief);
                g = shade(g, jg + relief);
                b = shade(b, jb + relief);
            }
            let li = pixels.light[i] as u32;
            r = ((r as u32) * li / 255) as u8;
            g = ((g as u32) * li / 255) as u8;
            b = ((b as u32) * li / 255) as u8;
            if owner == 0 && cell.is_open() {
                // Milky haze over cloudy water columns, densest where the
                // fines are (alpha ∝ 1 − cloud).
                if cell.sat >= WATER_SAT_RENDER {
                    let c = overlay.cloud_q8.get(px).copied().unwrap_or(255) as u32;
                    if c < 250 {
                        let t = (255 - c) * 3 / 4;
                        let milk = [0xB8u32, 0xC4, 0xB0];
                        r = ((r as u32 * (255 - t) + milk[0] * li / 255 * t) / 255) as u8;
                        g = ((g as u32 * (255 - t) + milk[1] * li / 255 * t) / 255) as u8;
                        b = ((b as u32 * (255 - t) + milk[2] * li / 255 * t) / 255) as u8;
                    }
                }
                // Warm haze over a THERMO halo.
                if any_warm {
                    let ci = (vy as usize) * overlay.coarse_w as usize + vx as usize;
                    let w =
                        crate::thermal::haze_weight(overlay.t_local.get(ci).copied().unwrap_or(0))
                            as u32;
                    if w > 0 {
                        r = ((r as u32 * (255 - w) + 0xE3 * li / 255 * w) / 255) as u8;
                        g = ((g as u32 * (255 - w) + 0x6A * li / 255 * w) / 255) as u8;
                        b = ((b as u32 * (255 - w) + 0x50 * li / 255 * w) / 255) as u8;
                    }
                }
            }
            // Litter: brown speck (bone band ivory, grit grey) scaled by mass.
            let litter = pixels.litter[i];
            let grit = pixels.grit[i];
            if owner == 0 && (litter > 0 || grit > 0) {
                let (c, mass) = if litter >= grit {
                    let c = if pixels.litter_key[i] >= 192 {
                        [0xC9, 0xC2, 0xB6]
                    } else {
                        [0x6B, 0x4F, 0x32]
                    };
                    (c, litter)
                } else {
                    ([0x7E, 0x7A, 0x74], grit)
                };
                let t = (mass as u32 * 3).min(255) * 3 / 4;
                r = ((r as u32 * (255 - t) + c[0] as u32 * li / 255 * t) / 255) as u8;
                g = ((g as u32 * (255 - t) + c[1] as u32 * li / 255 * t) / 255) as u8;
                b = ((b as u32 * (255 - t) + c[2] as u32 * li / 255 * t) / 255) as u8;
            }
            // Keyed-field haze on open pixels, in key-band colours (§a.4).
            if owner == 0 && pixels.fields_active {
                let hazes = [
                    (
                        pixels.enzyme_amt[i],
                        enzyme_band_colour(pixels.enzyme_key[i]),
                    ),
                    (pixels.toxin_amt[i], [0x7B, 0x3F, 0xA0]),
                    (pixels.signal_amt[i], [0x3B, 0xBF, 0xD3]),
                ];
                for (amt, c) in hazes {
                    if amt > 4 {
                        let t = (amt as u32).min(255) / 2; // ≤ 50 % tint
                        r = ((r as u32 * (255 - t) + c[0] as u32 * t) / 255) as u8;
                        g = ((g as u32 * (255 - t) + c[1] as u32 * t) / 255) as u8;
                        b = ((b as u32 * (255 - t) + c[2] as u32 * t) / 255) as u8;
                    }
                }
            }
            // Front-pane film stays in the buffer for the swipe tool, but
            // it is not drawn. It was the dirt on the glass.
            let o = i * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
            out[o + 3] = 255;
        }
    }
}

/// ENZYME key bands (§a.4): litter 0–63 cream, living wall 64–191 dark
/// red, bone 192–255 ivory-grey.
pub fn enzyme_band_colour(key: u8) -> [u8; 3] {
    match key {
        0..=63 => [0xE8, 0xDC, 0xB8],
        64..=191 => [0x8B, 0x1A, 0x1A],
        _ => [0xC9, 0xC4, 0xB4],
    }
}

fn albedo(cell: Cell) -> [u8; 3] {
    let v = cell.colour_variant() as usize;
    if cell.is_glass() {
        // Night dew fogs the pane toward white (Phase 4 condensation).
        let t = (cell.pad as i32) / 2;
        return [mix(0xC4, 0xF2, t), mix(0xE2, 0xF6, t), mix(0xEC, 0xFA, t)];
    }
    if cell.is_open() {
        if cell.sat < WATER_SAT_RENDER {
            // Lit air. The old near-black read as a hole in the glass.
            return [0x6e, 0x8c, 0xa4];
        }
        let t = cell.sat as i32;
        // Deep #285F83 toward shallow #519FBE.
        return [mix(0x28, 0x51, t), mix(0x5F, 0x9F, t), mix(0x83, 0xBE, t)];
    }
    let (dry, wet) = palette(cell, v);
    let t = render_wetness(cell);
    [
        mix(dry[0], wet[0], t),
        mix(dry[1], wet[1], t),
        mix(dry[2], wet[2], t),
    ]
}

/// Pore water is a fraction of capacity, not of 255, so a soaked sand
/// grain reaches the wet palette instead of stopping a third of the way.
/// Rock film is already stored on the 0–255 scale.
fn render_wetness(cell: Cell) -> i32 {
    let m = cell.moisture() as i32;
    if cell.is_rock() {
        return m.min(255);
    }
    let cap = crate::cell::water_capacity(cell.id()) as i32;
    if cap <= 0 {
        return 0;
    }
    (m * 255 / cap).min(255)
}

fn palette(cell: Cell, variant: usize) -> ([u8; 3], [u8; 3]) {
    match cell.id() {
        wk_material::MaterialId::Sand => {
            const SAND: [([u8; 3], [u8; 3]); 4] = [
                ([0xc6, 0x9e, 0x68], [0xa8, 0x84, 0x50]), // warm
                ([0xd8, 0xc4, 0x8c], [0xb0, 0x98, 0x68]), // pale
                ([0xc4, 0x86, 0x52], [0x8e, 0x62, 0x3c]), // red
                ([0xb0, 0xa4, 0x84], [0x78, 0x70, 0x58]), // gray
            ];
            SAND[variant.min(3)]
        }
        wk_material::MaterialId::Stone
        | wk_material::MaterialId::Limestone
        | wk_material::MaterialId::Sandstone
        | wk_material::MaterialId::Conglomerate
        | wk_material::MaterialId::Flowstone => {
            const ROCK: [([u8; 3], [u8; 3]); 4] = [
                ([0x82, 0x77, 0x6a], [0x6a, 0x62, 0x58]), // warm stone
                ([0xa0, 0x8c, 0x74], [0x80, 0x70, 0x5c]), // brown
                ([0x86, 0x90, 0x92], [0x66, 0x70, 0x72]), // blue-gray
                ([0xb0, 0xa6, 0x9a], [0x88, 0x80, 0x76]), // pale
            ];
            ROCK[variant.min(3)]
        }
        wk_material::MaterialId::Gravel | wk_material::MaterialId::LooseRock => {
            const COBBLE: [([u8; 3], [u8; 3]); 4] = [
                ([0x9a, 0x91, 0x86], [0x6e, 0x68, 0x60]),
                ([0xa8, 0x98, 0x80], [0x78, 0x6a, 0x58]),
                ([0x88, 0x8a, 0x90], [0x5e, 0x60, 0x66]),
                ([0xb0, 0xa8, 0x9c], [0x7a, 0x74, 0x6a]),
            ];
            COBBLE[variant.min(3)]
        }
        wk_material::MaterialId::Soil | wk_material::MaterialId::Organic => {
            ([0x6b, 0x4f, 0x32], [0x4a, 0x36, 0x22])
        }
        wk_material::MaterialId::Clay | wk_material::MaterialId::Bentonite => {
            ([0xa4, 0x78, 0x58], [0x74, 0x50, 0x3c])
        }
        wk_material::MaterialId::Bedrock => ([0x2a, 0x24, 0x1c], [0x2a, 0x24, 0x1c]),
        _ => ([0x88, 0x80, 0x78], [0x60, 0x5a, 0x54]),
    }
}

fn mix(a: u8, b: u8, t: i32) -> u8 {
    let a = a as i32;
    let b = b as i32;
    (a + (b - a) * t / 255) as u8
}

fn shade(c: u8, d: i32) -> u8 {
    (c as i32 + d).clamp(0, 255) as u8
}

/// Small per-pixel grain so a flat 3×3 cell is not one colour.
/// Neighbours differ: the mix advances by 3 along x.
fn grain_speck(px: usize, py: usize) -> (i32, i32, i32) {
    let n = (px.wrapping_mul(3).wrapping_add(py.wrapping_mul(5))) & 15;
    let j = n as i32 - 8;
    let warm = ((px.wrapping_add(py.wrapping_mul(2))) & 3) as i32 - 1;
    (j + warm, j, j - warm)
}

/// A lit corner on a cobble. The corner walks as [`Cell::tumble`] advances,
/// so a falling rock reads as a turn.
fn rock_relief(cell: Cell, px: usize, py: usize) -> i32 {
    if !cell.tumbles() {
        return 0;
    }
    let sx = (px as i32) % PX_PER_CELL as i32;
    let sy = (py as i32) % PX_PER_CELL as i32;
    let (hx, hy) = match cell.tumble() {
        0 => (0, 0),
        1 => (2, 0),
        2 => (2, 2),
        _ => (0, 2),
    };
    let dist = (sx - hx).abs() + (sy - hy).abs();
    16 - dist * 8
}
