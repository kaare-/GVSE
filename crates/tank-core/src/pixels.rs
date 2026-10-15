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
    /// Front-pane film. 0 = clean. Phase 2 swipe-clears it.
    pub film: Vec<u8>,
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
            film: vec![0; n],
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
    pub humidity: Vec<u8>,
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
        }
    }
}

/// Incident light, top-down. Water loses 0.1 % per pixel of depth.
pub fn rebuild_light(
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    tick: u64,
    always_day: bool,
    light_slider: f32,
) {
    let vis_w = grid.visible_w();
    let vis_h = grid.visible_h();
    let pw = pixels.w as i32;
    let df = if always_day { 1.0 } else { day_factor(tick) };
    let slider = light_slider.clamp(0.0, 1.0);
    let mut incident0 = (df * slider * 1000.0).round() as u32;
    incident0 = incident0 * 255 / 1000;

    for vx in 0..vis_w {
        let mut incoming = incident0;
        for vy_top in 0..vis_h {
            let gy = 1 + (vis_h - 1 - vy_top);
            let gx = 1 + vx;
            let cell = grid.get(gx, gy);
            let lit = incoming.min(255) as u8;
            for suby in 0..PX_PER_CELL as i32 {
                let py = vy_top * PX_PER_CELL as i32 + suby;
                let row = (py * pw) as usize;
                for subx in 0..PX_PER_CELL as i32 {
                    let px = vx * PX_PER_CELL as i32 + subx;
                    pixels.light[row + px as usize] = lit;
                }
            }
            if cell.is_air() {
                if cell.sat >= WATER_SAT_RENDER {
                    for _ in 0..PX_PER_CELL {
                        incoming = incoming * 999 / 1000;
                    }
                }
            } else {
                incoming = incoming * 45 / 100;
            }
        }
    }
}

/// RGBA8, row 0 = top of the tank, nearest-neighbour 3×3 blocks.
pub fn write_rgba(grid: &PhysicsGrid, pixels: &PixelLayer, out: &mut Vec<u8>) {
    let pw = pixels.w as usize;
    let ph = pixels.h as usize;
    out.resize(pw * ph * 4, 0);
    let vis_h = grid.visible_h();
    for py in 0..ph {
        for px in 0..pw {
            let vx = (px as i32) / PX_PER_CELL as i32;
            let vy_top = (py as i32) / PX_PER_CELL as i32;
            let vy = vis_h - 1 - vy_top;
            let cell = grid.get_visible(vx, vy);
            let [mut r, mut g, mut b] = albedo(cell);
            let i = py * pw + px;
            let li = pixels.light[i] as u32;
            r = ((r as u32) * li / 255) as u8;
            g = ((g as u32) * li / 255) as u8;
            b = ((b as u32) * li / 255) as u8;
            let film = pixels.film[i];
            if film > 0 {
                let t = film as u32;
                r = ((r as u32 * (255 - t) + 150 * t) / 255) as u8;
                g = ((g as u32 * (255 - t) + 190 * t) / 255) as u8;
                b = ((b as u32 * (255 - t) + 120 * t) / 255) as u8;
            }
            let o = i * 4;
            out[o] = r;
            out[o + 1] = g;
            out[o + 2] = b;
            out[o + 3] = 255;
        }
    }
}

fn albedo(cell: Cell) -> [u8; 3] {
    let v = cell.colour_variant() as usize;
    if cell.is_air() {
        if cell.sat < WATER_SAT_RENDER {
            return [0x1b, 0x24, 0x30];
        }
        let t = cell.sat as i32;
        return [mix(0x16, 0x4c, t), mix(0x32, 0x93, t), mix(0x4f, 0xd4, t)];
    }
    let (dry, wet) = palette(cell, v);
    let t = cell.moisture() as i32;
    [
        mix(dry[0], wet[0], t),
        mix(dry[1], wet[1], t),
        mix(dry[2], wet[2], t),
    ]
}

fn palette(cell: Cell, variant: usize) -> ([u8; 3], [u8; 3]) {
    match cell.id() {
        wk_material::MaterialId::Sand => {
            const SAND: [([u8; 3], [u8; 3]); 4] = [
                ([0xc6, 0xa3, 0x6a], [0x7a, 0x62, 0x40]), // warm
                ([0xd4, 0xc2, 0x8a], [0x8a, 0x78, 0x50]), // pale
                ([0xb0, 0x7a, 0x4a], [0x6e, 0x4c, 0x30]), // red
                ([0x9c, 0x90, 0x70], [0x5e, 0x56, 0x42]), // gray
            ];
            SAND[variant.min(3)]
        }
        wk_material::MaterialId::Stone
        | wk_material::MaterialId::Limestone
        | wk_material::MaterialId::Sandstone
        | wk_material::MaterialId::Conglomerate
        | wk_material::MaterialId::Flowstone => {
            const ROCK: [([u8; 3], [u8; 3]); 4] = [
                ([0x7d, 0x7a, 0x74], [0x5c, 0x5a, 0x56]), // gray
                ([0x8a, 0x7a, 0x68], [0x64, 0x56, 0x48]), // brown
                ([0x6e, 0x78, 0x7a], [0x4e, 0x56, 0x58]), // blue-gray
                ([0x90, 0x88, 0x80], [0x68, 0x62, 0x5c]), // pale
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
