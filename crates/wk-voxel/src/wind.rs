//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse wind field for cloud / humidity advection.
//!
//! Climate wind is mostly a horizontal prevailing flow; orographic
//! lift adds a small upward component where the free surface rises
//! in the wind direction (tall mountains).

use serde::{Deserialize, Serialize};

use crate::humidity::TileBounds;
use crate::worldgen::continental_surface_y;

/// Tile-scale wind used to advect atmospheric water.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wind {
    /// Prevailing horizontal speed in **tiles per tick** (positive = +x).
    pub climate_vx: f32,
    /// Base vertical drift (usually ~0).
    pub climate_vy: f32,
    /// Fractional advection residual (shared; climate is uniform).
    pub residual_x: f32,
    pub residual_y: f32,
    /// Optional tile bounds (same as humidity) for orographic sampling.
    pub tile_cols: i32,
    pub bounds: Option<TileBounds>,
    pub wrap_x: bool,
    /// Worldgen inputs for surface-based lift.
    pub seed: u64,
    pub width_cols: i32,
    pub sea_level_y: i32,
}

impl Wind {
    pub fn climate(
        tile_cols: i32,
        climate_vx: f32,
        seed: u64,
        width_cols: i32,
        sea_level_y: i32,
        bedrock_floor_y: i32,
        sky_ceiling_y: i32,
        wrap_x: bool,
    ) -> Self {
        let mut w = Self {
            climate_vx,
            climate_vy: 0.0,
            residual_x: 0.0,
            residual_y: 0.0,
            tile_cols: tile_cols.max(1),
            bounds: Some(TileBounds::from_world_cells(
                tile_cols.max(1),
                0,
                bedrock_floor_y,
                width_cols,
                sky_ceiling_y,
            )),
            wrap_x,
            seed,
            width_cols: width_cols.max(1),
            sea_level_y,
        };
        let _ = &mut w;
        w
    }

    /// Horizontal wind at a humidity tile (tiles / tick).
    pub fn vx_at(&self, _hx: i32, _hy: i32) -> f32 {
        self.climate_vx
    }

    /// Vertical wind: climate plus orographic lift when terrain rises
    /// downwind of this tile.
    pub fn vy_at(&self, hx: i32, _hy: i32) -> f32 {
        let lift = self.orographic_lift(hx);
        self.climate_vy + lift
    }

    /// Mean ascent (cells of surface gain) looking one tile downwind.
    /// Positive → air is forced up the mountain face.
    pub fn orographic_lift(&self, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let sign = if self.climate_vx >= 0.0 { 1 } else { -1 };
        let gx_dn = gx + sign * tc;
        let s0 = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        let s1 = continental_surface_y(self.seed, gx_dn, self.sea_level_y, self.width_cols);
        let ascent = (s1 - s0) as f32;
        if ascent <= 0.0 {
            return 0.0;
        }
        // Small upward drift — keeps clouds hugging lee slopes a bit.
        (ascent / 80.0).clamp(0.0, 0.08)
    }

    /// True when the tile centre sits over tall land (rain-prone).
    pub fn is_tall_terrain(&self, hx: i32, tall_above_sea: i32) -> bool {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let s = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        s >= self.sea_level_y + tall_above_sea
    }

    /// Surface ascent into the wind (cells) at tile `hx`.
    pub fn ascent_cells(&self, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let sign = if self.climate_vx >= 0.0 { 1 } else { -1 };
        // Look *upwind*: rain as moist air climbs onto this tile.
        let gx_up = gx - sign * tc;
        let s0 = continental_surface_y(self.seed, gx_up, self.sea_level_y, self.width_cols);
        let s1 = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        ((s1 - s0) as f32).max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::worldgen::WorldgenParams;

    #[test]
    fn mountain_tile_has_positive_ascent_from_ocean_side() {
        let p = WorldgenParams::default();
        let wind = Wind::climate(
            4,
            0.12,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        // Scan for a tile with clear upslope; mountains sit mid-ring.
        let mut max_ascent = 0.0f32;
        for hx in 0..(p.width_cols / 4) {
            max_ascent = max_ascent.max(wind.ascent_cells(hx));
        }
        assert!(
            max_ascent > 5.0,
            "expected some orographic ascent across the ring, got {max_ascent}"
        );
    }
}
