//! Host-side cell → RGBA for [`crate::TerrainStrip`].
//!
//! Deliberately thinner than `wk-voxel-app::palette` — enough to read
//! stone / sand / water / air. Full parity lands with a later present port.

use wk_material::{MaterialId, MaterialRegistry};
use wk_voxel::{Cell, GRAIN_REPOSE_HAZE_MAX};

const WATER_FILM: [u8; 3] = [0x9A, 0xC0, 0xD8];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Draw colour for one cell (opaque RGBA).
pub fn cell_rgba(cell: Option<Cell>) -> [u8; 4] {
    let Some(cell) = cell else {
        return [12, 14, 20, 255];
    };
    let rgb = if cell.material == MaterialId::Air {
        if cell.sat.is_empty() || cell.sat.0 <= GRAIN_REPOSE_HAZE_MAX {
            // Sky / empty — near-transparent so the clear color shows.
            return [0, 0, 0, 0];
        }
        let water = MaterialRegistry::colour_rgb(MaterialId::Water);
        let t = ((cell.sat.0 - GRAIN_REPOSE_HAZE_MAX) as f32
            / (255 - GRAIN_REPOSE_HAZE_MAX) as f32)
            .clamp(0.0, 1.0);
        [
            lerp_u8(WATER_FILM[0], water[0], t),
            lerp_u8(WATER_FILM[1], water[1], t),
            lerp_u8(WATER_FILM[2], water[2], t),
        ]
    } else {
        let base = MaterialRegistry::colour_rgb(cell.material);
        let cap = wk_voxel::water_capacity_cell(cell, &wk_material::HydroOverrides::default());
        let darken = if cap == 0 {
            0.0
        } else {
            (cell.sat.0 as f32 / cap as f32).clamp(0.0, 1.0) * 0.4
        };
        [
            lerp_u8(base[0], 40, darken),
            lerp_u8(base[1], 55, darken),
            lerp_u8(base[2], 85, darken),
        ]
    };
    [rgb[0], rgb[1], rgb[2], 255]
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
}
