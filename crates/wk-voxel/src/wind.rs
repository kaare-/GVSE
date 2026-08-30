//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse wind vector field for humidity advection, raft drift, and
//! (later) spore push / canopy bend / local dampening.
//!
//! **Layers**
//! 1. Climate mean (`climate_vx`) + natural variance → tick force/heading
//! 2. Rebuilt per-tile `(vx, vy)` heatmap from pluggable drivers:
//!    terrain (oro / lee), thermal gradients, swirl eddies, optional
//!    canopy drag (filled by the app from tall stems)
//! 3. [`Self::vector_at`] reads the heatmap (falls back to procedural
//!    [`Self::flow_at`] if empty)
//!
//! No pressure solver yet — docs/VOXEL_FIELDS.md §7. Drivers are dials,
//! not hard requirements.

use serde::{Deserialize, Serialize};

use crate::fasthash::FxHashMap;
use crate::grid::World;
use crate::humidity::TileBounds;
use crate::temperature::Temperature;
use crate::worldgen::{continental_surface_y, live_surface_y, LIVE_SURFACE_SEARCH};

fn default_variance() -> f32 {
    0.55
}

fn default_terrain_drive() -> f32 {
    1.0
}

fn default_thermal_drive() -> f32 {
    0.35
}

fn default_swirl() -> f32 {
    0.45
}

fn default_canopy_dampen() -> f32 {
    0.55
}

fn default_field_smooth() -> f32 {
    0.35
}

/// Live-tunable wind field drivers (Tab → Climate → Wind).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WindConfig {
    /// How strongly orography shapes speed / lift / lee sink (0 = flat climate).
    #[serde(default = "default_terrain_drive")]
    pub terrain_drive: f32,
    /// Horizontal −∇T breeze + warm-column lift scale.
    #[serde(default = "default_thermal_drive")]
    pub thermal_drive: f32,
    /// Divergence-free eddy / swirl amplitude (local twirls).
    #[serde(default = "default_swirl")]
    pub swirl: f32,
    /// How hard optional canopy drag (0..1 per tile) slows local wind.
    #[serde(default = "default_canopy_dampen")]
    pub canopy_dampen: f32,
    /// Temporal EMA of the rebuilt field (0 = snap, ~0.9 = very sticky).
    #[serde(default = "default_field_smooth")]
    pub field_smooth: f32,
}

impl Default for WindConfig {
    fn default() -> Self {
        Self {
            terrain_drive: default_terrain_drive(),
            thermal_drive: default_thermal_drive(),
            swirl: default_swirl(),
            canopy_dampen: default_canopy_dampen(),
            field_smooth: default_field_smooth(),
        }
    }
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
    /// Driver dials for the rebuilt vector heatmap.
    #[serde(default)]
    pub config: WindConfig,
    /// Fractional advection residual (legacy; vapour uses flux advection).
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
    /// Rebuilt coarse `(vx, vy)` per humidity tile. Runtime only — not saved.
    #[serde(skip)]
    pub field: FxHashMap<(i32, i32), (f32, f32)>,
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
            config: WindConfig::default(),
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
            field: FxHashMap::default(),
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

    /// Rebuild the coarse vector heatmap for this tick.
    ///
    /// `drag` is optional 0..1 per tile (1 = full stop when
    /// [`WindConfig::canopy_dampen`] is 1). Tall plants / spores fill it later;
    /// the app may already stamp stem occupancy.
    pub fn rebuild_field(
        &mut self,
        world: Option<&World>,
        temp: Option<&Temperature>,
        tick: u64,
        drag: Option<&FxHashMap<(i32, i32), f32>>,
    ) {
        let Some(bounds) = self.bounds else {
            self.field.clear();
            return;
        };
        let evx = self.effective_vx(tick);
        let evy = self.effective_vy(tick);
        let cfg = self.config;
        let smooth = cfg.field_smooth.clamp(0.0, 0.95);
        let prev = std::mem::take(&mut self.field);
        let mut next = FxHashMap::default();
        // Reserve roughly the tile grid.
        let nx = (bounds.hx_max - bounds.hx_min + 1).max(1) as usize;
        let ny = (bounds.hy_max - bounds.hy_min + 1).max(1) as usize;
        next.reserve(nx.saturating_mul(ny));

        for hy in bounds.hy_min..=bounds.hy_max {
            for hx in bounds.hx_min..=bounds.hx_max {
                let (mut vx, mut vy) = self.compose_drivers(
                    world, temp, hx, hy, tick, evx, evy, &cfg,
                );
                if let Some(map) = drag {
                    if let Some(&d) = map.get(&(hx, hy)) {
                        let keep =
                            1.0 - cfg.canopy_dampen.clamp(0.0, 1.0) * d.clamp(0.0, 1.0);
                        vx *= keep;
                        vy *= keep;
                    }
                }
                vx = vx.clamp(-1.0, 1.0);
                vy = vy.clamp(-1.0, 1.0);
                if smooth > 1e-4 {
                    if let Some(&(px, py)) = prev.get(&(hx, hy)) {
                        vx = px * smooth + vx * (1.0 - smooth);
                        vy = py * smooth + vy * (1.0 - smooth);
                    }
                }
                next.insert((hx, hy), (vx, vy));
            }
        }
        self.field = next;
    }

    fn compose_drivers(
        &self,
        world: Option<&World>,
        temp: Option<&Temperature>,
        hx: i32,
        hy: i32,
        tick: u64,
        evx: f32,
        evy: f32,
        cfg: &WindConfig,
    ) -> (f32, f32) {
        let shear = self.height_shear(world, hx, hy);
        let mut vx = evx * shear;
        let mut vy = evy;

        let td = cfg.terrain_drive.clamp(0.0, 2.0);
        if td > 1e-4 {
            let speed = self.orographic_speed_factor(world, hx);
            let lift = self.orographic_lift(world, hx);
            let sink = self.lee_sink(world, hx, hy);
            // Blend oro speed toward free-stream shear when drive is low.
            let sped = 1.0 + (speed - 1.0) * td.min(1.0);
            vx = (evx * shear * sped).clamp(-1.0, 1.0);
            // Extra drive above 1.0 slightly exaggerates oro contrast.
            let boost = if td > 1.0 { 1.0 + (td - 1.0) * 0.35 } else { 1.0 };
            vy += (lift - sink) * td.min(1.0) * boost;
        }

        let th = cfg.thermal_drive.clamp(0.0, 2.0);
        if th > 1e-4 {
            if let Some(t) = temp {
                let (dvx, dvy) = self.thermal_delta(t, hx, hy);
                vx += dvx * th;
                vy += dvy * th;
            }
        }

        let sw = cfg.swirl.clamp(0.0, 2.0);
        if sw > 1e-4 {
            let (sx, sy) = self.swirl_at(hx, hy, tick);
            vx += sx * sw;
            vy += sy * sw;
        }

        (vx, vy)
    }

    /// Sea-breeze style: air slides toward warmer neighbours; warm
    /// anomalies relative to the row mean rise.
    fn thermal_delta(&self, temp: &Temperature, hx: i32, hy: i32) -> (f32, f32) {
        let t0 = temp.at_tile(hx, hy);
        let tx = (temp.at_tile(hx + 1, hy) - temp.at_tile(hx - 1, hy)) * 0.5;
        let ty = (temp.at_tile(hx, hy + 1) - temp.at_tile(hx, hy - 1)) * 0.5;
        // Toward warm (daytime land breeze proxy).
        let mut vx = tx * 0.018;
        let mut vy = ty * 0.012;
        // Column buoyancy from local anomaly vs left/right neighbours.
        let flank = (temp.at_tile(hx - 1, hy) + temp.at_tile(hx + 1, hy)) * 0.5;
        let anomaly = t0 - flank;
        vy += (anomaly / 18.0).clamp(-0.12, 0.12);
        // Buried / deep seats stay quiet.
        let surf = self.surface_tile_hy(None, hx);
        if hy + 1 < surf {
            vx *= 0.15;
            vy *= 0.15;
        }
        (vx, vy)
    }

    /// Divergence-free eddies from a pair of advected potentials.
    fn swirl_at(&self, hx: i32, hy: i32, tick: u64) -> (f32, f32) {
        let t = tick as f32;
        let phase = (self.seed as f32) * 1.0e-9;
        // Large gyre
        let ax = hx as f32 * 0.31 + t * 0.017 + phase;
        let ay = hy as f32 * 0.27 + t * 0.013 + phase * 1.7;
        // ∂φ/∂y , −∂φ/∂x for φ = sin(ax)·cos(ay)
        let dphi_dy = ax.sin() * (-ay.sin()) * 0.27 * 0.055;
        let dphi_dx = ax.cos() * ay.cos() * 0.31 * 0.055;
        let mut sx = dphi_dy;
        let mut sy = -dphi_dx;
        // Smaller counter-eddy
        let bx = hx as f32 * 0.71 + t * 0.041 + phase * 2.1;
        let by = hy as f32 * 0.63 - t * 0.029 + phase * 0.4;
        let dpsi_dy = bx.sin() * (-by.sin()) * 0.63 * 0.028;
        let dpsi_dx = bx.cos() * by.cos() * 0.71 * 0.028;
        sx += dpsi_dy;
        sy += -dpsi_dx;
        (sx, sy)
    }

    /// Local flow for physics / HUD — heatmap if rebuilt, else procedural.
    pub fn vector_at(&self, world: Option<&World>, hx: i32, hy: i32) -> (f32, f32) {
        if let Some(&v) = self.field.get(&(hx, hy)) {
            return v;
        }
        let vx = self.climate_vx;
        let vy = self.climate_vy;
        self.flow_at(world, hx, hy, vx, vy)
    }

    /// Speed |v| at a tile (tiles / tick).
    pub fn speed_at(&self, world: Option<&World>, hx: i32, hy: i32) -> f32 {
        let (vx, vy) = self.vector_at(world, hx, hy);
        (vx * vx + vy * vy).sqrt()
    }

    /// Local vapour-carrying flow at humidity tile `(hx, hy)`.
    ///
    /// `climate_vx` / `climate_vy` should already be the tick's
    /// [`Self::effective_vx`] / [`Self::effective_vy`]. Spatial shaping:
    /// - **height shear** — near-surface air is dragged; free stream aloft
    /// - **windward channel** — climbs slightly faster onto rising terrain
    /// - **lee slow + sink** — air decelerates and drops past a crest
    ///
    /// Prefer [`Self::vector_at`] once [`Self::rebuild_field`] has run.
    pub fn flow_at(
        &self,
        world: Option<&World>,
        hx: i32,
        hy: i32,
        climate_vx: f32,
        climate_vy: f32,
    ) -> (f32, f32) {
        let shear = self.height_shear(world, hx, hy);
        let speed = self.orographic_speed_factor(world, hx);
        let vx = (climate_vx * shear * speed).clamp(-1.0, 1.0);
        let lift = self.orographic_lift(world, hx);
        let sink = self.lee_sink(world, hx, hy);
        let vy = (climate_vy + lift - sink).clamp(-1.0, 1.0);
        (vx, vy)
    }

    /// Horizontal wind at a humidity tile (tiles / tick) — mean climate
    /// only. Prefer [`Self::vector_at`] for vapour advection.
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

    /// 0.2 at the free surface → 1.0 several tiles aloft (rough log profile).
    pub fn height_shear(&self, world: Option<&World>, hx: i32, hy: i32) -> f32 {
        let surf_hy = self.surface_tile_hy(world, hx);
        let above = (hy - surf_hy).max(0) as f32;
        (0.20 + 0.80 * (above / 5.0).clamp(0.0, 1.0)).clamp(0.15, 1.15)
    }

    /// Windward speed-up / lee slow-down from live (or seed) terrain.
    pub fn orographic_speed_factor(&self, world: Option<&World>, hx: i32) -> f32 {
        let ascent = self.ascent_cells(world, hx); // climb *onto* this tile
        let descent = self.descent_cells(world, hx); // drop *leaving* this tile
        if ascent > descent && ascent > 1.5 {
            (1.0 + (ascent / 48.0).clamp(0.0, 0.28)).clamp(0.5, 1.35)
        } else if descent > 1.5 {
            // Lee: decelerate — eddies / separation, not a free slide.
            (0.52 + 0.35 * (1.0 - (descent / 40.0).clamp(0.0, 1.0))).clamp(0.4, 1.0)
        } else {
            1.0
        }
    }

    /// Downward draught past a crest (tiles / tick contribution).
    pub fn lee_sink(&self, world: Option<&World>, hx: i32, hy: i32) -> f32 {
        let descent = self.descent_cells(world, hx);
        if descent <= 1.0 {
            return 0.0;
        }
        let shear = self.height_shear(world, hx, hy);
        // Strongest in the mid wake, weaker at the ground and high aloft.
        let wake = (1.0 - (shear - 0.45).abs() * 1.4).clamp(0.25, 1.0);
        (descent / 55.0).clamp(0.0, 0.14) * wake
    }

    /// Mixing strength 0..1 from |climate wind| — high wind sinks/mixes
    /// the column instead of only translating it.
    pub fn mix_strength(&self, climate_vx: f32, climate_vy: f32) -> f32 {
        let speed = climate_vx.abs().max(climate_vy.abs());
        (speed / 0.22).clamp(0.0, 1.0)
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
        // Windward loft — strong enough that near-surface vapor climbs the
        // face instead of waiting for a horizontal step to tunnel through.
        (ascent / 40.0).clamp(0.0, 0.22)
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

    /// Surface drop looking one tile *downwind* (lee wake).
    pub fn descent_cells(&self, world: Option<&World>, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let sign = if self.climate_vx >= 0.0 { 1 } else { -1 };
        let gx_dn = gx + sign * tc;
        let s0 = self.surface_at(world, gx);
        let s1 = self.surface_at(world, gx_dn);
        ((s0 - s1) as f32).max(0.0)
    }

    pub fn surface_tile_hy(&self, world: Option<&World>, hx: i32) -> i32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let y = self.surface_at(world, gx);
        y.div_euclid(tc)
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

    #[test]
    fn flow_shears_with_height_and_slows_in_the_lee() {
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
        // Find a crest tile with real descent (lee).
        let mut hx_lee = 0;
        let mut best_desc = 0.0f32;
        for hx in 0..(p.width_cols / 4) {
            let d = wind.descent_cells(None, hx);
            if d > best_desc {
                best_desc = d;
                hx_lee = hx;
            }
        }
        assert!(best_desc > 3.0, "need a lee face, got {best_desc}");

        let surf = wind.surface_tile_hy(None, hx_lee);
        let (vx_low, _) = wind.flow_at(None, hx_lee, surf, 0.12, 0.0);
        let (vx_high, vy_high) = wind.flow_at(None, hx_lee, surf + 6, 0.12, 0.0);
        assert!(
            vx_high.abs() > vx_low.abs() * 1.5,
            "aloft wind must outrun the surface layer ({vx_high} vs {vx_low})"
        );
        assert!(
            vx_high.abs() < 0.12 * 0.95,
            "lee must not reach free-stream speed (got {vx_high})"
        );
        assert!(
            vy_high < -0.01,
            "lee wake should sink (got {vy_high})"
        );
    }

    #[test]
    fn mix_strength_tracks_wind_speed() {
        let p = WorldgenParams::default();
        let wind = Wind::climate(
            4,
            0.05,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        assert!(wind.mix_strength(0.0, 0.0) < 0.05);
        assert!(wind.mix_strength(0.22, 0.0) > 0.95);
        assert!(wind.mix_strength(0.05, 0.0) > wind.mix_strength(0.01, 0.0));
    }

    #[test]
    fn rebuild_field_varies_spatially_with_swirl() {
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
        wind.config = WindConfig {
            terrain_drive: 0.0,
            thermal_drive: 0.0,
            swirl: 1.2,
            canopy_dampen: 0.0,
            field_smooth: 0.0,
        };
        wind.rebuild_field(None, None, 40, None);
        assert!(!wind.field.is_empty());
        let speeds: Vec<f32> = wind
            .field
            .values()
            .map(|&(vx, vy)| (vx * vx + vy * vy).sqrt())
            .collect();
        let min = speeds.iter().cloned().fold(f32::INFINITY, f32::min);
        let max = speeds.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            max - min > 0.02,
            "swirl should create local speed differences ({min}..{max})"
        );
        // Directions should not all match the climate +x.
        let vy_spread = wind
            .field
            .values()
            .map(|&(_, vy)| vy)
            .fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(vy_spread > 0.01, "swirl should tilt some tiles vertically");
    }

    #[test]
    fn canopy_drag_slows_local_tiles() {
        let p = WorldgenParams::default();
        let mut wind = Wind::climate(
            4,
            0.20,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        wind.variance = 0.0;
        wind.config.terrain_drive = 0.0;
        wind.config.thermal_drive = 0.0;
        wind.config.swirl = 0.0;
        wind.config.field_smooth = 0.0;
        wind.config.canopy_dampen = 1.0;
        let hx = 10;
        let hy = wind.surface_tile_hy(None, hx) + 2;
        let mut drag = FxHashMap::default();
        drag.insert((hx, hy), 1.0);
        wind.rebuild_field(None, None, 0, Some(&drag));
        let (vx, vy) = wind.vector_at(None, hx, hy);
        assert!(
            vx.abs() < 0.02 && vy.abs() < 0.02,
            "full drag should calm the tile ({vx},{vy})"
        );
        let (ox, _) = wind.vector_at(None, hx + 3, hy);
        assert!(
            ox.abs() > 0.05,
            "undragged neighbour should still blow ({ox})"
        );
    }
}
