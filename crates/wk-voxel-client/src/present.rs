//! Host-side cell → RGBA for [`crate::TerrainStrip`].
//!
//! Palette aligned with `wk-voxel-app::palette` (wetness / perm / mycelium).
//! Water glitter (Phase 7 D) applied on free-water tops when lit.

use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{is_standing_water, Cell, World, GRAIN_REPOSE_HAZE_MAX};

const WATER_FILM_RGB: [u8; 3] = [0x9A, 0xC0, 0xD8];
const MYCELIUM_THREAD_RGB: [u8; 3] = [0xF1, 0xE6, 0xC4];
const PERMEABLE_ROCK_RGB: [u8; 3] = [0xC0, 0x9A, 0x76];
const PERM_TINT_STRENGTH: f32 = 0.55;
const TINT_LEVELS: u8 = 4;
const WET_DARKEN: f32 = 0.45;
/// Default glitter strength (matches demo settings).
pub const WATER_GLITTER_STRENGTH: f32 = 0.75;

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

fn mycelium_blend(material: MaterialId, intensity: u8) -> f32 {
    if intensity == 0 {
        return 0.0;
    }
    let t = intensity as f32 / 255.0;
    if material == MaterialId::Organic {
        (0.08 + t * 0.37).min(0.45)
    } else {
        (0.14 + t * 0.42).min(0.55)
    }
}

fn wetness_bucket(cell: Cell, hydro: &wk_material::HydroOverrides) -> u8 {
    let cap = wk_voxel::water_capacity_cell(cell, hydro);
    if cap == 0 || cell.sat.0 == 0 {
        return 0;
    }
    let frac = (cell.sat.0 as f32 / cap as f32).clamp(0.0, 1.0);
    ((frac * TINT_LEVELS as f32).floor() as u8).min(TINT_LEVELS - 1)
}

fn permeability_tint(cell: Cell, hydro: &wk_material::HydroOverrides) -> f32 {
    if !matches!(cell.material, MaterialId::Stone | MaterialId::Conglomerate) {
        return 0.0;
    }
    let range = MaterialRegistry::hydrology_with(cell.material, hydro).permeability;
    let (lo, hi) = (range.min as f32, range.max as f32);
    let span = (hi - lo).max(1.0);
    let p = wk_voxel::permeability_cell(cell, hydro) as f32;
    let t = ((p - lo) / span).clamp(0.0, 1.0);
    let step = (t * (TINT_LEVELS - 1) as f32).round();
    step / (TINT_LEVELS - 1) as f32
}

/// RGB for a known cell (dry air returns the material sky base — caller
/// replaces clear air with height-phased sky).
pub fn cell_rgb(cell: Cell) -> [u8; 3] {
    let hydro = wk_material::HydroOverrides::default();
    let base = MaterialRegistry::colour_rgb(cell.material);
    if cell.material == MaterialId::Air {
        if cell.sat.is_empty() || cell.sat.0 <= GRAIN_REPOSE_HAZE_MAX {
            return base;
        }
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        let t_vis = ((cell.sat.0 - GRAIN_REPOSE_HAZE_MAX) as f32
            / (255 - GRAIN_REPOSE_HAZE_MAX) as f32)
            .clamp(0.0, 1.0);
        let blend = if t_vis >= 0.55 {
            (0.55 + (t_vis - 0.55) * 1.8).clamp(0.75, 1.0)
        } else {
            t_vis / 0.55 * 0.75
        };
        [
            lerp_u8(WATER_FILM_RGB[0], water[0], blend),
            lerp_u8(WATER_FILM_RGB[1], water[1], blend),
            lerp_u8(WATER_FILM_RGB[2], water[2], blend),
        ]
    } else {
        let bucket = wetness_bucket(cell, &hydro);
        let darken = WET_DARKEN * bucket as f32 / (TINT_LEVELS - 1) as f32;
        let mut rgb = [
            lerp_u8(base[0], 40, darken),
            lerp_u8(base[1], 55, darken),
            lerp_u8(base[2], 85, darken),
        ];
        let perm = permeability_tint(cell, &hydro);
        if perm > 0.0 {
            let w = perm * PERM_TINT_STRENGTH;
            rgb = [
                lerp_u8(rgb[0], PERMEABLE_ROCK_RGB[0], w),
                lerp_u8(rgb[1], PERMEABLE_ROCK_RGB[1], w),
                lerp_u8(rgb[2], PERMEABLE_ROCK_RGB[2], w),
            ];
        }
        let myc = mycelium_blend(cell.material, cell.mycelium());
        if myc > 0.0 {
            rgb = [
                lerp_u8(rgb[0], MYCELIUM_THREAD_RGB[0], myc),
                lerp_u8(rgb[1], MYCELIUM_THREAD_RGB[1], myc),
                lerp_u8(rgb[2], MYCELIUM_THREAD_RGB[2], myc),
            ];
        }
        rgb
    }
}

/// Draw colour for one cell (alpha 0 = clear air for sky fill).
pub fn cell_rgba(cell: Option<Cell>) -> [u8; 4] {
    let Some(cell) = cell else {
        return [12, 14, 20, 255];
    };
    if cell.material == MaterialId::Air
        && (cell.sat.is_empty() || cell.sat.0 <= GRAIN_REPOSE_HAZE_MAX)
    {
        return [0, 0, 0, 0];
    }
    let rgb = cell_rgb(cell);
    [rgb[0], rgb[1], rgb[2], 255]
}

fn low_sun_factor(celestial_local: f32) -> f32 {
    let elev = (celestial_local.clamp(0.0, 1.0) * std::f32::consts::PI)
        .sin()
        .clamp(0.0, 1.0);
    ((0.55 - elev) / 0.55).clamp(0.0, 1.0)
}

/// Soft sheen + sparse sparkle on lit free-water tops (Phase 7 D).
pub fn apply_water_glitter_rgb(
    rgb: [u8; 3],
    x: i32,
    y: i32,
    tick: u64,
    celestial_local: f32,
    is_day: bool,
    key: f32,
    strength: f32,
) -> [u8; 3] {
    let s = strength.clamp(0.0, 1.0);
    let key = key.clamp(0.0, 1.0);
    if s < 0.02 || key < 0.12 {
        return rgb;
    }
    let low = low_sun_factor(celestial_local);
    let s = (s * (1.0 + 0.25 * low)).clamp(0.0, 1.0);
    let h = (x as u32)
        .wrapping_mul(374761393)
        .wrapping_add((y as u32).wrapping_mul(668265263))
        .wrapping_add(((tick / 4) as u32).wrapping_mul(2654435761));
    let spark = ((h >> 16) & 0xFF) as f32 / 255.0;
    let sheen = (key * s * (0.14 + 0.10 * low)).clamp(0.0, 0.35);
    let gloss = if is_day {
        [255.0f32, 240.0, 190.0]
    } else {
        [185.0f32, 205.0, 255.0]
    };
    let mut out = [
        (rgb[0] as f32 * (1.0 - sheen) + gloss[0] * sheen) as u8,
        (rgb[1] as f32 * (1.0 - sheen) + gloss[1] * sheen) as u8,
        (rgb[2] as f32 * (1.0 - sheen) + gloss[2] * sheen) as u8,
    ];
    let threshold = 0.90 - 0.08 * s;
    if spark >= threshold {
        let fleck = (key * s * (0.55 + 0.35 * spark)).clamp(0.0, 0.95);
        let tip = if is_day {
            [255.0f32, 252.0, 230.0]
        } else {
            [230.0f32, 240.0, 255.0]
        };
        out = [
            (out[0] as f32 * (1.0 - fleck) + tip[0] * fleck).clamp(0.0, 255.0) as u8,
            (out[1] as f32 * (1.0 - fleck) + tip[1] * fleck).clamp(0.0, 255.0) as u8,
            (out[2] as f32 * (1.0 - fleck) + tip[2] * fleck).clamp(0.0, 255.0) as u8,
        ];
    }
    out
}

fn is_exposed_water_top(world: &World, x: i32, y: i32) -> bool {
    if !is_standing_water(world, x, y) {
        return false;
    }
    match world.get_cell(x, y + 1) {
        None => true,
        Some(c) => c.material == MaterialId::Air && !is_standing_water(world, x, y + 1),
    }
}

/// Full present sample: palette + optional glitter on free-water tops.
pub fn cell_rgba_lit(
    world: &World,
    gx: i32,
    gy: i32,
    tick: u64,
    celestial_local: f32,
    is_day: bool,
    glitter_strength: f32,
) -> [u8; 4] {
    let cell = world.get_cell(gx, gy);
    let mut px = cell_rgba(cell);
    if px[3] == 0 {
        return px;
    }
    if is_exposed_water_top(world, gx, gy) {
        let elev = (celestial_local.clamp(0.0, 1.0) * std::f32::consts::PI)
            .sin()
            .clamp(0.0, 1.0);
        let key = (0.35 + 0.65 * elev).clamp(0.0, 1.0);
        let rgb = apply_water_glitter_rgb(
            [px[0], px[1], px[2]],
            gx,
            gy,
            tick,
            celestial_local,
            is_day,
            key,
            glitter_strength,
        );
        px = [rgb[0], rgb[1], rgb[2], 255];
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_voxel::{Cell, Sat};

    #[test]
    fn dry_air_is_clear() {
        let c = cell_rgba(Some(Cell::air()));
        assert_eq!(c[3], 0);
    }

    #[test]
    fn stone_is_opaque() {
        let c = cell_rgba(Some(Cell::solid(MaterialId::Stone)));
        assert_eq!(c[3], 255);
        assert!(c[0] > 20 || c[1] > 20 || c[2] > 20);
    }

    #[test]
    fn standing_water_not_clear() {
        let mut air = Cell::air();
        air.sat = Sat(200);
        let c = cell_rgba(Some(air));
        assert_eq!(c[3], 255);
    }

    #[test]
    fn glitter_brightens_when_keyed() {
        let base = [40u8, 80, 120];
        let out = apply_water_glitter_rgb(base, 3, 7, 12, 0.5, true, 0.9, 1.0);
        let sum_base = base[0] as u32 + base[1] as u32 + base[2] as u32;
        let sum_out = out[0] as u32 + out[1] as u32 + out[2] as u32;
        assert!(sum_out >= sum_base);
    }

    #[test]
    fn wet_stone_darker_than_dry() {
        let hydro = wk_material::HydroOverrides::default();
        let dry = Cell::solid(MaterialId::Stone);
        let mut wet = dry;
        wet.sat = Sat(wk_voxel::water_capacity_cell(dry, &hydro));
        let d = cell_rgb(dry);
        let w = cell_rgb(wet);
        let sd = d[0] as i32 + d[1] as i32 + d[2] as i32;
        let sw = w[0] as i32 + w[1] as i32 + w[2] as i32;
        assert!(sw < sd, "wet stone should darken");
    }
}
