//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse wind field for cloud / humidity advection and raft drift.
//!
//! Climate wind is a horizontal prevailing flow; orographic lift adds a
//! small upward component where the free surface rises in the wind
//! direction. Gustiness / meander modulate instantaneous force and
//! direction from the mean so weather is not a constant push.

use serde::{Deserialize, Serialize};

use crate::humidity::TileBounds;
use crate::worldgen::continental_surface_y;

/// Tile-scale wind used to advect atmospheric water and shove rafts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wind {
    /// Mean prevailing horizontal speed in **tiles per tick** (positive = +x).
    pub climate_vx: f32,
    /// Base vertical drift (usually ~0).
    pub climate_vy: f32,
    /// Gust amplitude 0..1 — scales instantaneous |vx| around the mean.
    pub gustiness: f32,
    /// Direction meander 0..1 — slow sway that can weaken or reverse the mean.
    pub meander: f32,
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
        Self {
            climate_vx,
            climate_vy: 0.0,
            gustiness: 0.45,
            meander: 0.35,
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
        }
    }

    /// Instantaneous horizontal wind (tiles / tick) at sim `tick`.
    ///
    /// Combines the Tab mean (`climate_vx`) with a gust envelope and a
    /// slow direction meander. `gustiness = meander = 0` recovers a
    /// perfectly steady wind. High meander can reverse the heading.
    pub fn effective_vx(&self, tick: u64) -> f32 {
        let t = tick as f32;
        let g = self.gustiness.clamp(0.0, 1.0);
        let m = self.meander.clamp(0.0, 1.0);
        let gust = 1.0 + g * (0.55 * (t * 0.019).sin() + 0.40 * (t * 0.047 + 2.1).sin());
        // (1−m) keeps the mean heading; m blends in a slow sine that
        // reaches ±1 so full meander can reverse the wind.
        let heading = (1.0 - m) + m * (t * 0.0033).sin();
        (self.climate_vx * gust * heading).clamp(-1.0, 1.0)
    }

    /// Instantaneous vertical climate drift (tiles / tick). Mild gust on `vy`.
    pub fn effective_vy(&self, tick: u64) -> f32 {
        let t = tick as f32;
        let wobble = self.gustiness.clamp(0.0, 1.0) * 0.015 * (t * 0.023 + 0.8).sin();
        self.climate_vy + wobble
    }

    /// Horizontal wind at a humidity tile (tiles / tick).
    ///
    /// Uses the mean climate value for orographic geometry; callers that
    /// advect mass should prefer [`Self::effective_vx`].
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

    #[test]
    fn steady_knobs_recover_climate_vx() {
        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.08,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.gustiness = 0.0;
        wind.meander = 0.0;
        for tick in [0u64, 17, 100, 9999] {
            assert!(
                (wind.effective_vx(tick) - 0.08).abs() < 1e-5,
                "tick={tick} got {}",
                wind.effective_vx(tick)
            );
        }
    }

    #[test]
    fn gust_and_meander_vary_over_time() {
        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.10,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.gustiness = 0.8;
        wind.meander = 0.85;
        let samples: Vec<f32> = (0..2500).map(|t| wind.effective_vx(t)).collect();
        let min = samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(max - min > 0.06, "expected wind to breathe, range={}", max - min);
        assert!(
            samples.iter().any(|&v| v < 0.0),
            "high meander should reverse the mean (min={min} max={max})"
        );
    }
}
