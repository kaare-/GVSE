//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse air/skin temperature heatmap (°C).
//!
//! Driven by the shared climate clock (solar heat / night cool),
//! shaded by humidity cloud mass, and biased by sea vs land elevation
//! from worldgen. Same 4×4 tile grid as humidity / wind.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

use crate::climate::{day_night_factor_cfg, ClimateConfig};
use crate::humidity::{Humidity, TileBounds};
use crate::worldgen::continental_surface_y;

/// Cadence for temperature steps — same period as humidity diffuse,
/// phase 0 so the two don't always land on the same tick.
pub const TEMP_STEP_PERIOD: u64 = 20;
pub const TEMP_STEP_PHASE: u64 = 0;

pub fn temperature_step_due(tick: u64) -> bool {
    tick % TEMP_STEP_PERIOD == TEMP_STEP_PHASE
}

/// Live-tunable temperature / solar knobs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct TempConfig {
    pub base_temp_c: f32,
    pub sea_bias_c: f32,
    pub land_day_bump_c: f32,
    pub lapse_c: f32,
    pub day_amp_c: f32,
    pub solar_heat_c: f32,
    pub night_cool_c: f32,
    pub cloud_shade: f32,
    pub hum_shade_ref: f32,
    pub sky_relax: f32,
    pub diffuse_alpha: f32,
}

impl Default for TempConfig {
    fn default() -> Self {
        Self {
            base_temp_c: 18.0,
            sea_bias_c: -2.0,
            land_day_bump_c: 1.5,
            lapse_c: 0.08,
            day_amp_c: 6.0,
            solar_heat_c: 0.40,
            night_cool_c: 0.30,
            cloud_shade: 0.55,
            hum_shade_ref: 80.0,
            sky_relax: 0.10,
            diffuse_alpha: 0.12,
        }
    }
}

/// Sparse (but usually dense-filled) temperature field in °C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Temperature {
    pub tile_cols: i32,
    pub cells: HashMap<(i32, i32), f32>,
    pub bounds: Option<TileBounds>,
    pub wrap_x: bool,
    pub seed: u64,
    pub width_cols: i32,
    pub sea_level_y: i32,
    #[serde(default)]
    pub config: TempConfig,
    #[serde(default)]
    pub climate: ClimateConfig,
}

impl Temperature {
    pub fn with_world_bounds(
        tile_cols: i32,
        x0: i32,
        y0: i32,
        x1: i32,
        y1: i32,
        seed: u64,
        width_cols: i32,
        sea_level_y: i32,
        wrap_x: bool,
    ) -> Self {
        let tile_cols = tile_cols.max(1);
        let mut t = Self {
            tile_cols,
            cells: HashMap::new(),
            bounds: Some(TileBounds::from_world_cells(tile_cols, x0, y0, x1, y1)),
            wrap_x,
            seed,
            width_cols: width_cols.max(1),
            sea_level_y,
            config: TempConfig::default(),
            climate: ClimateConfig::default(),
        };
        t.fill_initial(0);
        t
    }

    fn accepts(&self, hx: i32, hy: i32) -> bool {
        self.bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
    }

    fn wrap_hx(&self, hx: i32) -> Option<i32> {
        match self.bounds {
            Some(b) if self.wrap_x => {
                let w = b.hx_max - b.hx_min + 1;
                if w <= 0 {
                    return None;
                }
                Some(b.hx_min + (hx - b.hx_min).rem_euclid(w))
            }
            Some(b) => {
                if hx >= b.hx_min && hx <= b.hx_max {
                    Some(hx)
                } else {
                    None
                }
            }
            None => Some(hx),
        }
    }

    pub fn tile_of(&self, gx: i32, gy: i32) -> (i32, i32) {
        (gx.div_euclid(self.tile_cols), gy.div_euclid(self.tile_cols))
    }

    pub fn at_tile(&self, hx: i32, hy: i32) -> f32 {
        *self
            .cells
            .get(&(hx, hy))
            .unwrap_or(&self.config.base_temp_c)
    }

    pub fn at_cell(&self, gx: i32, gy: i32) -> f32 {
        let (hx, hy) = self.tile_of(gx, gy);
        self.at_tile(hx, hy)
    }

    pub fn mean(&self) -> f32 {
        if self.cells.is_empty() {
            return self.config.base_temp_c;
        }
        self.cells.values().sum::<f32>() / self.cells.len() as f32
    }

    /// Fill every in-bounds tile from climate skin at `tick`.
    pub fn fill_initial(&mut self, tick: u64) {
        let Some(b) = self.bounds else {
            return;
        };
        self.cells.clear();
        for hy in b.hy_min..=b.hy_max {
            for hx in b.hx_min..=b.hx_max {
                self.cells.insert((hx, hy), self.skin_temp(hx, hy, tick));
            }
        }
    }

    fn land_factor(&self, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let s = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        let d = (s - self.sea_level_y) as f32;
        ((d + 2.0) / 4.0).clamp(0.0, 1.0)
    }

    fn elev_cells(&self, hx: i32) -> f32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let s = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        (s - self.sea_level_y).max(0) as f32
    }

    /// Target skin temperature for a tile at climate phase `tick`.
    pub fn skin_temp(&self, hx: i32, hy: i32, tick: u64) -> f32 {
        let _ = hy;
        let cfg = &self.config;
        let dn = day_night_factor_cfg(tick, &self.climate);
        let land = self.land_factor(hx);
        let elev = self.elev_cells(hx);
        let sea_land =
            cfg.sea_bias_c * (1.0 - land) + cfg.land_day_bump_c * land * dn.max(0.0);
        cfg.base_temp_c + sea_land - cfg.lapse_c * elev + cfg.day_amp_c * dn
    }

    /// One climate-driven temperature step + light diffusion.
    pub fn step(&mut self, humidity: &Humidity, tick: u64) {
        if self.cells.is_empty() {
            self.fill_initial(tick);
        }
        let dn = day_night_factor_cfg(tick, &self.climate);
        let cfg = self.config;
        let keys: Vec<(i32, i32)> = self.cells.keys().copied().collect();
        for (hx, hy) in keys {
            let shade = (humidity.at_tile(hx, hy) / cfg.hum_shade_ref.max(1.0)).clamp(0.0, 1.0);
            let solar = cfg.solar_heat_c * dn.max(0.0) * (1.0 - cfg.cloud_shade * shade);
            let cool = cfg.night_cool_c * (-dn).max(0.0);
            let skin = self.skin_temp(hx, hy, tick);
            let t = self.at_tile(hx, hy);
            let mut next = t + solar - cool;
            next = next + (skin - next) * cfg.sky_relax;
            self.cells.insert((hx, hy), next);
        }
        self.diffuse(cfg.diffuse_alpha);
    }

    /// Pairwise temperature diffusion (does not prune tiles — cold air
    /// is still a real temperature).
    pub fn diffuse(&mut self, alpha: f32) {
        let alpha = alpha.clamp(0.0, 0.25);
        if alpha == 0.0 || self.cells.is_empty() {
            return;
        }
        let snap = self.cells.clone();
        let mut sources: Vec<(i32, i32)> = snap.keys().copied().collect();
        sources.sort_unstable();
        sources.dedup();
        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
        let base = self.config.base_temp_c;
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&base);
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) && nx != hx {
                    let n_val = *snap.get(&(nx, hy)).unwrap_or(&base);
                    let flow = (val - n_val) * alpha;
                    if flow.abs() >= 1e-9 {
                        *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                        *deltas.entry((nx, hy)).or_insert(0.0) += flow;
                    }
                }
            }
            let n_key = (hx, hy + 1);
            if self.accepts(n_key.0, n_key.1) {
                let n_val = *snap.get(&n_key).unwrap_or(&base);
                let flow = (val - n_val) * alpha;
                if flow.abs() >= 1e-9 {
                    *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                    *deltas.entry(n_key).or_insert(0.0) += flow;
                }
            }
        }
        for (k, d) in deltas {
            if self.accepts(k.0, k.1) {
                *self.cells.entry(k).or_insert(base) += d;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::climate::DEMO_DAY_TICKS;
    use crate::worldgen::WorldgenParams;

    fn demo_temp() -> (Temperature, Humidity) {
        let p = WorldgenParams::default();
        let t = Temperature::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            true,
        );
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        (t, h)
    }

    #[test]
    fn noon_mean_warmer_than_midnight_after_steps() {
        let (mut day, h) = demo_temp();
        let (mut night, _) = demo_temp();
        // tick 0 = noon, DEMO_DAY_TICKS/2 = midnight.
        for _ in 0..8 {
            day.step(&h, 0);
            night.step(&h, DEMO_DAY_TICKS / 2);
        }
        assert!(
            day.mean() > night.mean() + 1.0,
            "noon mean {:.1} should beat midnight {:.1}",
            day.mean(),
            night.mean()
        );
    }

    #[test]
    fn clouds_shade_daytime_heating() {
        let (mut clear, h_clear) = demo_temp();
        let (mut cloudy, mut h_cloud) = demo_temp();
        // Saturate one map with cloud mass.
        if let Some(b) = h_cloud.bounds {
            for hy in b.hy_min..=b.hy_max {
                for hx in b.hx_min..=b.hx_max {
                    h_cloud
                        .cells
                        .insert((hx, hy), TempConfig::default().hum_shade_ref * 2.0);
                }
            }
        }
        for _ in 0..6 {
            clear.step(&h_clear, 0); // noon
            cloudy.step(&h_cloud, 0);
        }
        assert!(
            clear.mean() > cloudy.mean() + 0.3,
            "clear {:.1} should warm more than cloudy {:.1}",
            clear.mean(),
            cloudy.mean()
        );
    }

    #[test]
    fn temperature_step_due_matches_schedule() {
        assert!(temperature_step_due(0));
        assert!(!temperature_step_due(3));
        assert!(temperature_step_due(20));
    }
}
