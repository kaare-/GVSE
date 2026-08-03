//! wk-voxel-app palette adapter.
//!
//! Isolation: this file depends only on `wk_material` and `wk_voxel`.
//! No imports from wk-world / wk-sim / wk-app.
//!
//! Maps a [`wk_voxel::Cell`] to an RGB triple using
//! [`wk_material::MaterialRegistry::colour_rgb`] as the ground truth.
//! Water is `Air + sat` in the voxel model. Dry Air keeps the sky colour;
//! any non-zero sat blends from a faint blue-white film toward the
//! `Water` palette entry so even 1/255 fill reads on screen.

use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::Cell;

/// Faint blue-white film for the tiniest wet Air fill (sat = 1/255).
/// Distinct from dry sky blue so trickle cells don't disappear.
const WATER_FILM_RGB: [u8; 3] = [0xB8, 0xD4, 0xEE];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Faint cream hypha tint blended onto colonized Organic (palette Hypha).
const MYCELIUM_THREAD_RGB: [u8; 3] = [0xF1, 0xE6, 0xC4];

pub fn cell_color(cell: Cell) -> [u8; 3] {
    let base = MaterialRegistry::colour_rgb(cell.material);
    let t = cell.sat.as_f32();
    if cell.material == MaterialId::Air {
        if cell.sat.is_empty() {
            return base;
        }
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        // Floor at the film colour (never sky). Ramp toward lake blue;
        // snap once mostly full so pools read as real water.
        let blend = if t >= 0.55 {
            (0.55 + (t - 0.55) * 1.8).clamp(0.75, 1.0)
        } else {
            (t / 0.55) * 0.75
        };
        [
            lerp_u8(WATER_FILM_RGB[0], water[0], blend),
            lerp_u8(WATER_FILM_RGB[1], water[1], blend),
            lerp_u8(WATER_FILM_RGB[2], water[2], blend),
        ]
    } else {
        // Porous solid cells: nudge base color darker as pore
        // moisture rises. Real palette work can come later; this
        // gives instant visual feedback for infiltration.
        let darken = 0.35 * t;
        let mut rgb = [
            lerp_u8(base[0], 40, darken),
            lerp_u8(base[1], 55, darken),
            lerp_u8(base[2], 85, darken),
        ];
        // Organic with mycelium: faint cream threads (keeps wet/dry darken).
        if cell.material == MaterialId::Organic {
            let myc = (cell.mycelium() as f32 / 255.0) * 0.45;
            if myc > 0.0 {
                rgb = [
                    lerp_u8(rgb[0], MYCELIUM_THREAD_RGB[0], myc),
                    lerp_u8(rgb[1], MYCELIUM_THREAD_RGB[1], myc),
                    lerp_u8(rgb[2], MYCELIUM_THREAD_RGB[2], myc),
                ];
            }
        }
        rgb
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_voxel::{Sat, Cell};

    #[test]
    fn dry_air_stays_sky_blue() {
        let c = Cell::air();
        let rgb = cell_color(c);
        let sky = MaterialRegistry::colour_rgb(MaterialId::Air);
        assert_eq!(rgb, sky);
    }

    #[test]
    fn full_saturated_air_matches_water() {
        let c = Cell::water();
        let rgb = cell_color(c);
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        assert_eq!(rgb, water);
    }

    #[test]
    fn half_saturated_air_is_between_film_and_water() {
        let mut c = Cell::air();
        c.sat = Sat(128);
        let rgb = cell_color(c);
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        for i in 0..3 {
            let lo = WATER_FILM_RGB[i].min(water[i]);
            let hi = WATER_FILM_RGB[i].max(water[i]);
            assert!(rgb[i] >= lo && rgb[i] <= hi, "component {i}: {}", rgb[i]);
        }
    }

    #[test]
    fn one_sat_air_is_faint_blue_white_not_sky() {
        let mut c = Cell::air();
        c.sat = Sat(1);
        let rgb = cell_color(c);
        let sky = MaterialRegistry::colour_rgb(MaterialId::Air);
        assert_ne!(rgb, sky, "1/255 fill must not match dry sky");
        // Within a few steps of the film colour (tiny blend toward water).
        for i in 0..3 {
            let d = (rgb[i] as i16 - WATER_FILM_RGB[i] as i16).unsigned_abs();
            assert!(d <= 4, "component {i}: rgb={} film={}", rgb[i], WATER_FILM_RGB[i]);
        }
    }

    #[test]
    fn wet_sand_darker_than_dry_sand() {
        let dry = Cell::solid(MaterialId::Sand);
        let mut wet = Cell::solid(MaterialId::Sand);
        wet.sat = Sat::FULL;
        let dry_rgb = cell_color(dry);
        let wet_rgb = cell_color(wet);
        let dry_lum = dry_rgb[0] as i32 + dry_rgb[1] as i32 + dry_rgb[2] as i32;
        let wet_lum = wet_rgb[0] as i32 + wet_rgb[1] as i32 + wet_rgb[2] as i32;
        assert!(wet_lum < dry_lum, "wet sand must be darker");
    }
}
