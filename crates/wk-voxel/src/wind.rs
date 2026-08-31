//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse wind field for cloud / humidity advection and raft drift.
//!
//! Climate wind is a horizontal prevailing mean; **natural variance**
//! modulates instantaneous force and direction over gust / breeze /
//! weather timescales so the push is not constant. Orographic lift still
//! adds a small upward component where terrain rises downwind.

use serde::{Deserialize, Serialize};

use crate::grid::World;
use crate::humidity::TileBounds;
use crate::worldgen::{continental_surface_y, live_surface_y, LIVE_SURFACE_SEARCH};

fn default_variance() -> f32 {
    0.55
}

/// Tile-scale wind used to advect atmospheric water and shove rafts.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wind {
    /// Mean prevailing horizontal speed in **tiles per tick** (positive = +x).
    pub climate_vx: f32,
    /// Base vertical drift (usually ~0).
    pub climate_vy: f32,
    /// Natural variance 0..1 — how much force and heading wander around the mean.
    ///
    /// `0` = perfectly steady. Mid values breathe in strength and gently
    /// swing heading. Near `1` the wind can lull, gust, and reverse.
    #[serde(default = "default_variance")]
    pub variance: f32,
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
            variance: default_variance(),
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
    /// Applies multi-timescale natural variance from [`Self::variance`]
    /// around the Tab mean (`climate_vx`). Seeded phases keep each world
    /// from sharing the same weather clock.
    pub fn effective_vx(&self, tick: u64) -> f32 {
        let v = self.variance.clamp(0.0, 1.0);
        if v < 1e-4 {
            return self.climate_vx.clamp(-1.0, 1.0);
        }
        let t = tick as f32;
        // Stable per-world phase so two seeds don't lockstep.
        let phase = (self.seed as f32) * 1.0e-9 + (self.seed.rotate_left(13) as f32) * 1.0e-10;

        // Force envelope: gusts + breeze pulses + slow weather strength.
        let force = 1.0
            + v * (0.48 * (t * 0.021 + phase).sin()
                + 0.28 * (t * 0.055 + phase * 2.3).sin()
                + 0.24 * (t * 0.0085 + phase * 0.6).sin());

        // Heading wander: slow weather turn + quicker breeze swings.
        // At high variance the blend can cross zero (reverse).
        let dir = (0.70 * (t * 0.0026 + phase * 1.4).sin()
            + 0.30 * (t * 0.0074 + phase * 0.5).sin())
            .clamp(-1.0, 1.0);
        let heading = (1.0 - v) + v * dir;

        (self.climate_vx * force * heading).clamp(-1.0, 1.0)
    }

    /// Instantaneous vertical climate drift (tiles / tick). Mild variance wobble.
    pub fn effective_vy(&self, tick: u64) -> f32 {
        let v = self.variance.clamp(0.0, 1.0);
        let t = tick as f32;
        let phase = (self.seed as f32) * 1.0e-9;
        let wobble = v * 0.018 * (t * 0.019 + phase + 0.8).sin();
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
    ///
    /// Pass the live [`World`] so lift follows erosion and deposition.
    /// `None` falls back to the seed profile — tests and HUD that have no
    /// grid yet.
    pub fn vy_at(&self, world: Option<&World>, hx: i32, _hy: i32) -> f32 {
        let lift = self.orographic_lift(world, hx);
        self.climate_vy + lift
    }

    /// Mean ascent (cells of surface gain) looking one tile downwind.
    /// Positive → air is forced up the mountain face.
    pub fn orographic_lift(&self, world: Option<&World>, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let sign = if self.climate_vx >= 0.0 { 1 } else { -1 };
        let gx_dn = gx + sign * tc;
        let s0 = self.surface_at(world, gx);
        let s1 = self.surface_at(world, gx_dn);
        let ascent = (s1 - s0) as f32;
        if ascent <= 0.0 {
            return 0.0;
        }
        // Small upward drift — keeps clouds hugging lee slopes a bit.
        (ascent / 80.0).clamp(0.0, 0.08)
    }

    /// True when the tile centre sits over tall land (rain-prone).
    pub fn is_tall_terrain(&self, world: Option<&World>, hx: i32, tall_above_sea: i32) -> bool {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let s = self.surface_at(world, gx);
        s >= self.sea_level_y + tall_above_sea
    }

    /// Surface ascent into the wind (cells) at tile `hx`.
    pub fn ascent_cells(&self, world: Option<&World>, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let sign = if self.climate_vx >= 0.0 { 1 } else { -1 };
        // Look *upwind*: rain as moist air climbs onto this tile.
        let gx_up = gx - sign * tc;
        let s0 = self.surface_at(world, gx_up);
        let s1 = self.surface_at(world, gx);
        ((s1 - s0) as f32).max(0.0)
    }

    fn surface_at(&self, world: Option<&World>, gx: i32) -> i32 {
        let hint = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        match world {
            Some(w) => live_surface_y(w, gx, hint, LIVE_SURFACE_SEARCH),
            None => hint,
        }
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
        let mut max_ascent = 0.0f32;
        for hx in 0..(p.width_cols / 4) {
            max_ascent = max_ascent.max(wind.ascent_cells(None, hx));
        }
        assert!(
            max_ascent > 5.0,
            "expected some orographic ascent across the ring, got {max_ascent}"
        );
    }

    #[test]
    fn ascent_reads_the_live_hill_not_the_seed_profile() {
        use crate::cell::Cell;
        use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
        use wk_material::MaterialId;

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
        let mut hx_peak = 0;
        let mut best = 0.0f32;
        for hx in 0..(p.width_cols / 4) {
            let a = wind.ascent_cells(None, hx);
            if a > best {
                best = a;
                hx_peak = hx;
            }
        }
        assert!(best > 5.0, "need a seed-profile climb, got {best}");

        let tc = 4;
        let gx = hx_peak * tc + tc / 2;
        let hint = crate::worldgen::continental_surface_y(
            p.seed,
            gx,
            p.sea_level_y,
            p.width_cols,
        );
        let mut w = crate::grid::World::new(p.seed);
        for y in p.sea_level_y..=hint {
            w.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
            w.set_cell(gx, y, Cell::solid(MaterialId::Stone));
        }
        let before = wind.ascent_cells(Some(&w), hx_peak);
        assert!(
            (before - best).abs() < 2.0,
            "an untouched stacked column should agree with the seed profile ({best} vs {before})"
        );

        for y in (p.sea_level_y + 1)..=hint {
            w.set_cell(gx, y, Cell::air());
        }
        w.set_cell(gx, p.sea_level_y, Cell::solid(MaterialId::Stone));
        let after = wind.ascent_cells(Some(&w), hx_peak);
        assert!(
            after + 4.0 < before,
            "flattening the live hill should cut ascent ({before} → {after}); \
             the seed profile is still a climb of {best}"
        );
    }

    #[test]
    fn zero_variance_is_steady_mean() {
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
        wind.variance = 0.0;
        for tick in [0u64, 17, 100, 9999] {
            assert!(
                (wind.effective_vx(tick) - 0.08).abs() < 1e-5,
                "tick={tick} got {}",
                wind.effective_vx(tick)
            );
        }
    }

    #[test]
    fn natural_variance_changes_force_and_direction() {
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
        wind.variance = 0.9;
        let samples: Vec<f32> = (0..3000).map(|t| wind.effective_vx(t)).collect();
        let min = samples.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = samples.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max - min > 0.08,
            "natural variance should change force (range={})",
            max - min
        );
        assert!(
            samples.iter().any(|&v| v < 0.0),
            "high variance should reverse heading (min={min} max={max})"
        );
        // Strength should sometimes rise well above the mean magnitude.
        assert!(
            samples.iter().any(|&v| v.abs() > 0.12),
            "gusts should exceed the mean |vx|"
        );
    }
}
