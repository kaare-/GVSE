//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse **horizontal** wind for humidity advection, raft drift, and
//! convection-driven breezes.
//!
//! Layers:
//! 1. Climate mean + natural variance → tick force / heading
//! 2. Optional per-tile `(vx, vy)` heatmap from terrain, thermal ∇T,
//!    swirl, canopy drag — rebuilt on a cadence, **occupied tiles only**
//!    (FPS: do not fill the whole sky every frame)
//! 3. [`Self::vector_at`] reads the heatmap, else procedural [`Self::flow_at`]
//!
//! Vertical residual is capped. No pressure solver.

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
    0.70
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

/// Rebuild the local wind heatmap this often. Full-sky every frame was
/// the climate-stack FPS cliff; occupied + halo + a thin surface band
/// every 4 ticks is enough for humidity flux / evap / thermal mix.
pub const WIND_FIELD_PERIOD: u64 = 4;

/// Tab → Climate wind drivers.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct WindConfig {
    #[serde(default = "default_terrain_drive")]
    pub terrain_drive: f32,
    #[serde(default = "default_thermal_drive")]
    pub thermal_drive: f32,
    #[serde(default = "default_swirl")]
    pub swirl: f32,
    #[serde(default = "default_canopy_dampen")]
    pub canopy_dampen: f32,
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
    #[serde(default)]
    pub config: WindConfig,
    /// Fractional advection residual (shared; climate is uniform).
    pub residual_x: f32,
    pub residual_y: f32,
    /// Rebuilt `(vx, vy)` on occupied / near-surface tiles. Runtime only.
    #[serde(skip)]
    pub field: FxHashMap<(i32, i32), (f32, f32)>,
    /// Column surface y, filled during [`Self::rebuild_field`].
    #[serde(skip)]
    surf_cache: FxHashMap<i32, i32>,
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
            config: WindConfig::default(),
            residual_x: 0.0,
            residual_y: 0.0,
            field: FxHashMap::default(),
            surf_cache: FxHashMap::default(),
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
        if let Some(&y) = self.surf_cache.get(&gx) {
            return y;
        }
        let hint = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        match world {
            Some(w) => live_surface_y(w, gx, hint, LIVE_SURFACE_SEARCH),
            None => hint,
        }
    }

    fn cache_surface(&mut self, world: Option<&World>, gx: i32) -> i32 {
        if let Some(&y) = self.surf_cache.get(&gx) {
            return y;
        }
        let y = {
            let hint = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
            match world {
                Some(w) => live_surface_y(w, gx, hint, LIVE_SURFACE_SEARCH),
                None => hint,
            }
        };
        self.surf_cache.insert(gx, y);
        y
    }

    /// Rebuild local vectors for `occupied` humidity seats + a 1-tile halo
    /// and a thin near-surface band. Skips empty sky (the old full-grid
    /// rebuild was a multi-ms FPS cliff).
    pub fn rebuild_field(
        &mut self,
        world: Option<&World>,
        temp: Option<&Temperature>,
        tick: u64,
        occupied: &[(i32, i32)],
        drag: Option<&FxHashMap<(i32, i32), f32>>,
    ) {
        let Some(bounds) = self.bounds else {
            self.field.clear();
            return;
        };
        self.surf_cache.clear();
        let evx = self.effective_vx(tick);
        let evy = self.effective_vy(tick);
        let cfg = self.config;
        let smooth = cfg.field_smooth.clamp(0.0, 0.95);
        let prev = std::mem::take(&mut self.field);

        let mut keys: FxHashMap<(i32, i32), ()> = FxHashMap::default();
        for &(hx, hy) in occupied {
            for dx in -1..=1 {
                for dy in -1..=1 {
                    let nhx = if self.wrap_x {
                        let w = (bounds.hx_max - bounds.hx_min + 1).max(1);
                        bounds.hx_min + (hx + dx - bounds.hx_min).rem_euclid(w)
                    } else {
                        hx + dx
                    };
                    if !bounds.contains(nhx, hy + dy) {
                        continue;
                    }
                    keys.insert((nhx, hy + dy), ());
                }
            }
        }
        // Near-surface band so evap / T coupling have a breeze even
        // where humidity has not arrived yet. Sample every 2nd column.
        let tc = self.tile_cols.max(1);
        let mut hx = bounds.hx_min;
        while hx <= bounds.hx_max {
            let gx = hx * tc + tc / 2;
            let sy = self.cache_surface(world, gx);
            let shy = sy.div_euclid(tc);
            for hy in (shy - 1)..=(shy + 2) {
                if bounds.contains(hx, hy) {
                    keys.insert((hx, hy), ());
                }
            }
            hx += 2;
        }

        // Fill surf_cache for every column the compose pass will touch
        // (tile centre ± one tile) so later `vector_at` / evap reads
        // never walk the world.
        let mut unique_hx: Vec<i32> = keys.keys().map(|&(hx, _)| hx).collect();
        unique_hx.sort_unstable();
        unique_hx.dedup();
        for hx in unique_hx {
            let gx = hx * tc + tc / 2;
            self.cache_surface(world, gx);
            self.cache_surface(world, gx + tc);
            self.cache_surface(world, gx - tc);
        }

        let mut next = FxHashMap::default();
        next.reserve(keys.len());
        for &(hx, hy) in keys.keys() {
            let (mut vx, mut vy) =
                self.compose_drivers(world, temp, hx, hy, tick, evx, evy, &cfg);
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
        let mut blended = Self::spatial_blend_field(next, bounds, self.wrap_x, 1);
        if let Some(map) = drag {
            let damp = cfg.canopy_dampen.clamp(0.0, 1.0);
            if damp > 1e-4 {
                for (&(hx, hy), &d) in map.iter() {
                    if let Some(e) = blended.get_mut(&(hx, hy)) {
                        let keep = 1.0 - damp * d.clamp(0.0, 1.0);
                        e.0 *= keep;
                        e.1 *= keep;
                    }
                }
            }
        }
        self.field = blended;
    }

    fn spatial_blend_field(
        mut field: FxHashMap<(i32, i32), (f32, f32)>,
        bounds: TileBounds,
        wrap_x: bool,
        passes: u32,
    ) -> FxHashMap<(i32, i32), (f32, f32)> {
        if field.is_empty() || passes == 0 {
            return field;
        }
        let hx_span = (bounds.hx_max - bounds.hx_min + 1).max(1);
        for _ in 0..passes {
            let snap = field.clone();
            let mut out = FxHashMap::default();
            out.reserve(snap.len());
            for (&(hx, hy), &(vx, vy)) in &snap {
                let mut sx = vx * 2.0;
                let mut sy = vy * 2.0;
                let mut w = 2.0;
                for dx in [-1, 1] {
                    let nhx = if wrap_x {
                        let mut x = hx + dx;
                        if x < bounds.hx_min {
                            x += hx_span;
                        } else if x > bounds.hx_max {
                            x -= hx_span;
                        }
                        x
                    } else if hx + dx < bounds.hx_min || hx + dx > bounds.hx_max {
                        continue;
                    } else {
                        hx + dx
                    };
                    if let Some(&(nvx, nvy)) = snap.get(&(nhx, hy)) {
                        sx += nvx * 2.0;
                        sy += nvy * 2.0;
                        w += 2.0;
                    }
                }
                for dy in [-1, 1] {
                    let nhy = hy + dy;
                    if nhy < bounds.hy_min || nhy > bounds.hy_max {
                        continue;
                    }
                    if let Some(&(nvx, nvy)) = snap.get(&(hx, nhy)) {
                        sx += nvx;
                        sy += nvy;
                        w += 1.0;
                    }
                }
                out.insert(
                    (hx, hy),
                    ((sx / w).clamp(-1.0, 1.0), (sy / w).clamp(-1.0, 1.0)),
                );
            }
            field = out;
        }
        field
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
        let mut vy = evy * 0.35;
        let td = cfg.terrain_drive.clamp(0.0, 2.0);
        if td > 1e-4 {
            let (speed, lift, sink) = self.orographic_soft(world, hx, hy);
            let sped = 1.0 + (speed - 1.0) * td.min(1.0);
            vx = (evx * shear * sped).clamp(-1.0, 1.0);
            let height_fade = (1.0 - (shear - 0.2) * 0.7).clamp(0.2, 1.0);
            vy += (lift - sink) * td.min(1.0) * height_fade * 0.45;
            vx *= 1.0 - (lift * td.min(1.0) * height_fade * 0.35).clamp(0.0, 0.25);
        }
        let th = cfg.thermal_drive.clamp(0.0, 2.0);
        if th > 1e-4 {
            if let Some(t) = temp {
                let (dvx, dvy) = self.thermal_delta(t, hx, hy);
                vx += dvx * th;
                vy += dvy * th * 0.35;
            }
        }
        let sw = cfg.swirl.clamp(0.0, 2.0);
        if sw > 1e-4 {
            let (sx, sy) = self.swirl_at(hx, hy, tick);
            vx += sx * sw;
            vy += sy * sw * 0.4;
        }
        let v_cap = (vx.abs() * 0.45 + 0.04).min(0.35);
        vy = vy.clamp(-v_cap, v_cap);
        (vx.clamp(-1.0, 1.0), vy.clamp(-1.0, 1.0))
    }

    fn orographic_soft(
        &self,
        world: Option<&World>,
        hx: i32,
        hy: i32,
    ) -> (f32, f32, f32) {
        let mut speed = 0.0;
        let mut lift = 0.0;
        let mut sink = 0.0;
        let mut w = 0.0;
        for (dx, wt) in [(-1, 0.25f32), (0, 0.50), (1, 0.25)] {
            let x = hx + dx;
            speed += self.orographic_speed_factor(world, x) * wt;
            lift += self.orographic_lift(world, x) * wt;
            sink += self.lee_sink(world, x, hy) * wt;
            w += wt;
        }
        (speed / w, lift / w, sink / w)
    }

    fn thermal_delta(&self, temp: &Temperature, hx: i32, hy: i32) -> (f32, f32) {
        let t0 = temp.at_tile(hx, hy);
        let tx = (temp.at_tile(hx + 1, hy) - temp.at_tile(hx - 1, hy)) * 0.5;
        let ty = (temp.at_tile(hx, hy + 1) - temp.at_tile(hx, hy - 1)) * 0.5;
        let mut vx = tx * 0.055;
        let mut vy = ty * 0.012;
        let flank = (temp.at_tile(hx - 1, hy) + temp.at_tile(hx + 1, hy)) * 0.5;
        vy += ((t0 - flank) / 18.0).clamp(-0.10, 0.10);
        let surf = self.surface_tile_hy(None, hx);
        let above = (hy - surf).max(0) as f32;
        let near = (1.0 - above / 6.0).clamp(0.25, 1.0);
        vx *= near;
        vy *= near * 0.6;
        if hy + 1 < surf {
            vx *= 0.15;
            vy *= 0.15;
        }
        (vx, vy)
    }

    fn swirl_at(&self, hx: i32, hy: i32, tick: u64) -> (f32, f32) {
        let t = tick as f32;
        let phase = (self.seed as f32) * 1.0e-9;
        let ax = hx as f32 * 0.31 + t * 0.017 + phase;
        let ay = hy as f32 * 0.27 + t * 0.013 + phase * 1.7;
        let mut sx = ax.sin() * (-ay.sin()) * 0.27 * 0.055;
        let mut sy = -(ax.cos() * ay.cos() * 0.31 * 0.055);
        let bx = hx as f32 * 0.71 + t * 0.041 + phase * 2.1;
        let by = hy as f32 * 0.63 - t * 0.029 + phase * 0.4;
        sx += bx.sin() * (-by.sin()) * 0.63 * 0.028;
        sy += -(bx.cos() * by.cos() * 0.71 * 0.028);
        (sx, sy)
    }

    pub fn vector_at(&self, world: Option<&World>, hx: i32, hy: i32) -> (f32, f32) {
        if let Some(&v) = self.field.get(&(hx, hy)) {
            return v;
        }
        // Miss: climate mean only. `flow_at` walks the live surface and
        // was the humidity-advect FPS cliff when the field was empty or
        // a new seat appeared between rebuilds.
        let _ = (world, hx, hy);
        (self.climate_vx, self.climate_vy)
    }

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

    pub fn height_shear(&self, world: Option<&World>, hx: i32, hy: i32) -> f32 {
        let surf_hy = self.surface_tile_hy(world, hx);
        let above = (hy - surf_hy).max(0) as f32;
        (0.20 + 0.80 * (above / 5.0).clamp(0.0, 1.0)).clamp(0.15, 1.15)
    }

    pub fn orographic_speed_factor(&self, world: Option<&World>, hx: i32) -> f32 {
        let ascent = self.ascent_cells(world, hx);
        let descent = self.descent_cells(world, hx);
        if ascent > descent && ascent > 1.5 {
            (1.0 + (ascent / 48.0).clamp(0.0, 0.28)).clamp(0.5, 1.35)
        } else if descent > 1.5 {
            (0.52 + 0.35 * (1.0 - (descent / 40.0).clamp(0.0, 1.0))).clamp(0.4, 1.0)
        } else {
            1.0
        }
    }

    pub fn lee_sink(&self, world: Option<&World>, hx: i32, hy: i32) -> f32 {
        let descent = self.descent_cells(world, hx);
        if descent <= 1.0 {
            return 0.0;
        }
        let shear = self.height_shear(world, hx, hy);
        let wake = (1.0 - (shear - 0.45).abs() * 1.4).clamp(0.25, 1.0);
        (descent / 55.0).clamp(0.0, 0.14) * wake
    }

    pub fn mix_strength(&self, climate_vx: f32, climate_vy: f32) -> f32 {
        let speed = climate_vx.abs().max(climate_vy.abs());
        (speed / 0.22).clamp(0.0, 1.0)
    }

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
        self.surface_at(world, gx).div_euclid(tc)
    }

    /// Mean |vx| on near-surface field tiles (evap / thermal mix).
    pub fn near_surface_abs(&self, world: Option<&World>) -> f32 {
        if self.field.is_empty() {
            return self.climate_vx.abs().max(self.climate_vy.abs());
        }
        let mut sum = 0.0f32;
        let mut n = 0u32;
        for (&(hx, hy), &(vx, _)) in &self.field {
            let surf = self.surface_tile_hy(world, hx);
            if hy >= surf && hy <= surf + 2 {
                sum += vx.abs();
                n += 1;
            }
        }
        if n == 0 {
            self.climate_vx.abs()
        } else {
            sum / n as f32
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
        wind.config.swirl = 1.2;
        wind.config.thermal_drive = 0.0;
        wind.config.terrain_drive = 0.0;
        wind.config.field_smooth = 0.0;
        let occupied: Vec<(i32, i32)> = (0..16).map(|hx| (hx, 20)).collect();
        wind.rebuild_field(None, None, 40, &occupied, None);
        assert!(!wind.field.is_empty());
        let vals: Vec<f32> = wind.field.values().map(|&(vx, _)| vx).collect();
        let lo = vals.iter().cloned().fold(f32::INFINITY, f32::min);
        let hi = vals.iter().cloned().fold(f32::NEG_INFINITY, f32::max);
        assert!(
            hi - lo > 0.02,
            "swirl should vary local vx (range={})",
            hi - lo
        );
    }
}
