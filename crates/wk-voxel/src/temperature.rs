//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse **thermal field** (°C) on the same 4×4 tile grid as humidity /
//! wind.
//!
//! - **Air** tiles track the climate skin (day/night, clouds).
//! - **Surface** (rock / sand / lakes / snow) has high heat capacity and
//!   only weakly couples to the skin — water and rock do not slam from
//!   +20 °C to −10 °C in one night.
//! - **Buried** rock ignores solar/night air; it relaxes toward a
//!   geothermal profile and slowly leaks heat upward by diffusion.
//!
//! Cadence: [`TEMP_STEP_PERIOD`] = 20 — not every physics tick.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry};

use crate::cell::Cell;
use crate::climate::{day_night_factor_cfg, ClimateConfig};
use crate::grid::World;
use crate::humidity::{Humidity, TileBounds};
use crate::worldgen::{continental_surface_y, live_surface_at, live_surface_y, LIVE_SURFACE_SEARCH};

/// Cadence for temperature steps — same period as humidity diffuse,
/// phase 0 so the two don't always land on the same tick.
pub const TEMP_STEP_PERIOD: u64 = 20;
pub const TEMP_STEP_PHASE: u64 = 0;
/// Rebuild cached per-tile surface props every N temperature steps.
/// World scans dominate `step`; stale props for a few steps are fine
/// (materials change slowly vs the thermal field).
/// Was 4; 8 halves props world scans with little thermal lag (materials
/// change slowly vs the field). Super-Server temp ~17 ms/call.
pub const TEMP_PROPS_REFRESH_STEPS: u32 = 8;

pub fn temperature_step_due(tick: u64) -> bool {
    tick % TEMP_STEP_PERIOD == TEMP_STEP_PHASE
}

/// Live-tunable temperature / solar / inertia / geothermal knobs.
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
    /// Base relax rate toward climate skin (air-like surfaces).
    pub sky_relax: f32,
    pub diffuse_alpha: f32,
    /// Scales material heat capacity into surface inertia:
    /// `relax = sky_relax / (1 + capacity * inertia_scale)`.
    pub inertia_scale: f32,
    pub min_relax: f32,
    pub max_relax: f32,
    /// Extra capacity per standing-water / ice cell in the surface stack.
    pub water_stack_cap: f32,
    /// Night radiative cool multiplier on surface water (<< rock/air).
    pub water_night_cool_scale: f32,
    /// Deep-rock geothermal target at the surface interface (°C).
    pub geothermal_surface_c: f32,
    /// Extra °C per cell of depth below the free surface.
    pub geothermal_gradient_c_per_cell: f32,
    /// Relax rate of buried tiles toward the geothermal profile.
    pub geothermal_relax: f32,
    /// Constant heat added each thermal step to the deepest buried band
    /// (slow upward leak once diffusion carries it).
    pub geothermal_flux_c: f32,
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
            sky_relax: 0.12,
            diffuse_alpha: 0.10,
            inertia_scale: 1.6,
            min_relax: 0.003,
            max_relax: 0.28,
            water_stack_cap: 1.4,
            water_night_cool_scale: 0.15,
            geothermal_surface_c: 10.0,
            geothermal_gradient_c_per_cell: 0.35,
            geothermal_relax: 0.018,
            geothermal_flux_c: 0.04,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum TileLayer {
    Air,
    Surface { watery: bool },
    Buried { depth_cells: f32 },
}

#[derive(Debug, Clone, Copy)]
struct TileThermal {
    layer: TileLayer,
    capacity: f32,
    albedo: f32,
}

impl Default for TileThermal {
    fn default() -> Self {
        Self {
            layer: TileLayer::Air,
            capacity: 1.0,
            albedo: 0.0,
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
    /// Cached [`tile_thermal_props`] results — rebuilt every
    /// [`TEMP_PROPS_REFRESH_STEPS`] steps (not serialized).
    #[serde(skip)]
    props_cache: HashMap<(i32, i32), TileThermal>,
    #[serde(skip)]
    props_cache_age: u32,
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
            props_cache: HashMap::new(),
            props_cache_age: TEMP_PROPS_REFRESH_STEPS,
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

    /// Geothermal target (°C) at `depth_cells` below the free surface.
    pub fn geothermal_at_depth(&self, depth_cells: f32) -> f32 {
        let cfg = &self.config;
        cfg.geothermal_surface_c + cfg.geothermal_gradient_c_per_cell * depth_cells.max(0.0)
    }

    /// Fill tiles: air/surface from climate skin; buried from geothermal.
    pub fn fill_initial(&mut self, tick: u64) {
        let Some(b) = self.bounds else {
            return;
        };
        self.cells.clear();
        self.props_cache.clear();
        self.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let tc = self.tile_cols.max(1);
        for hy in b.hy_min..=b.hy_max {
            for hx in b.hx_min..=b.hx_max {
                let surf = self.column_surface_y_estimate(None, hx);
                let mid = hy * tc + tc / 2;
                let depth = (surf - mid) as f32;
                let t0 = if depth > tc as f32 {
                    self.geothermal_at_depth(depth)
                } else {
                    self.skin_temp(hx, hy, tick)
                };
                self.cells.insert((hx, hy), t0);
            }
        }
    }

    fn refresh_props_cache(&mut self, world: Option<&World>, keys: &[(i32, i32)]) {
        self.props_cache.clear();
        self.props_cache.reserve(keys.len());
        for &(hx, hy) in keys {
            let props = tile_thermal_props(self, world, hx, hy);
            self.props_cache.insert((hx, hy), props);
        }
        self.props_cache_age = 0;
    }

    fn column_surface_y_estimate(&self, world: Option<&World>, hx: i32) -> i32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let hint = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        match world {
            Some(w) => live_surface_y(w, gx, hint, LIVE_SURFACE_SEARCH),
            None => hint,
        }
    }

    fn land_factor(&self, world: Option<&World>, hx: i32) -> f32 {
        let s = self.column_surface_y_estimate(world, hx);
        let d = (s - self.sea_level_y) as f32;
        ((d + 2.0) / 4.0).clamp(0.0, 1.0)
    }

    fn elev_cells(&self, world: Option<&World>, hx: i32) -> f32 {
        (self.column_surface_y_estimate(world, hx) - self.sea_level_y).max(0) as f32
    }

    /// Target skin temperature for air / surface coupling at `tick`.
    pub fn skin_temp(&self, hx: i32, hy: i32, tick: u64) -> f32 {
        self.skin_temp_on(None, hx, hy, tick)
    }

    fn skin_temp_on(&self, world: Option<&World>, hx: i32, hy: i32, tick: u64) -> f32 {
        let _ = hy;
        let cfg = &self.config;
        let dn = day_night_factor_cfg(tick, &self.climate);
        let land = self.land_factor(world, hx);
        let elev = self.elev_cells(world, hx);
        let sea_land =
            cfg.sea_bias_c * (1.0 - land) + cfg.land_day_bump_c * land * dn.max(0.0);
        cfg.base_temp_c + sea_land - cfg.lapse_c * elev + cfg.day_amp_c * dn
    }

    /// One thermal step: layered forcing + inertia + diffusion.
    ///
    /// `world` supplies surface materials. Pass `None` only for air-only tests.
    pub fn step(&mut self, world: Option<&World>, humidity: &Humidity, tick: u64) {
        if self.cells.is_empty() {
            self.fill_initial(tick);
        }
        let dn = day_night_factor_cfg(tick, &self.climate);
        let cfg = self.config;
        let keys: Vec<(i32, i32)> = self.cells.keys().copied().collect();
        // Lowest world-y tile band (= deepest underground).
        let mut deepest_hy = i32::MAX;
        for &(_, hy) in &keys {
            deepest_hy = deepest_hy.min(hy);
        }
        if self.props_cache_age >= TEMP_PROPS_REFRESH_STEPS
            || self.props_cache.len() != keys.len()
        {
            self.refresh_props_cache(world, &keys);
        }
        self.props_cache_age = self.props_cache_age.saturating_add(1);
        for (hx, hy) in keys {
            let props = self
                .props_cache
                .get(&(hx, hy))
                .copied()
                .unwrap_or_else(|| tile_thermal_props(self, world, hx, hy));
            let t = self.at_tile(hx, hy);
            let next = match props.layer {
                TileLayer::Air => {
                    let shade =
                        (humidity.at_tile(hx, hy) / cfg.hum_shade_ref.max(1.0)).clamp(0.0, 1.0);
                    let solar = cfg.solar_heat_c
                        * dn.max(0.0)
                        * (1.0 - cfg.cloud_shade * shade);
                    let cool = cfg.night_cool_c * (-dn).max(0.0);
                    let skin = self.skin_temp_on(world, hx, hy, tick);
                    let relax = cfg.sky_relax.clamp(cfg.min_relax, cfg.max_relax);
                    let n = t + solar - cool;
                    n + (skin - n) * relax
                }
                TileLayer::Surface { watery } => {
                    let shade =
                        (humidity.at_tile(hx, hy) / cfg.hum_shade_ref.max(1.0)).clamp(0.0, 1.0);
                    let solar = cfg.solar_heat_c
                        * dn.max(0.0)
                        * (1.0 - cfg.cloud_shade * shade)
                        * (1.0 - props.albedo.clamp(0.0, 0.95));
                    let cool_scale = if watery {
                        cfg.water_night_cool_scale
                    } else {
                        1.0
                    };
                    let cool = cfg.night_cool_c * (-dn).max(0.0) * cool_scale;
                    let skin = self.skin_temp_on(world, hx, hy, tick);
                    let relax = (cfg.sky_relax
                        / (1.0 + props.capacity.max(0.05) * cfg.inertia_scale))
                        .clamp(cfg.min_relax, cfg.max_relax);
                    let n = t + solar - cool;
                    n + (skin - n) * relax
                }
                TileLayer::Buried { depth_cells } => {
                    // No solar / night air. Hold heat; ease toward geothermal.
                    let geo = self.geothermal_at_depth(depth_cells);
                    let relax = (cfg.geothermal_relax
                        / (1.0 + props.capacity.max(0.05) * cfg.inertia_scale * 0.35))
                        .clamp(0.001, 0.08);
                    let mut n = t + (geo - t) * relax;
                    // Deepest band gets a small constant flux (mantle leak).
                    if hy <= deepest_hy + 1 {
                        n += cfg.geothermal_flux_c;
                    }
                    n
                }
            };
            self.cells.insert((hx, hy), next);
        }
        self.diffuse(cfg.diffuse_alpha);
    }

    /// Pairwise temperature diffusion. Vertical mix is gentle so night air
    /// cannot drain lakes, but warm bedrock still leaks heat upward.
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
                // Mild vertical conductivity — geothermal path upward.
                let flow = (val - n_val) * alpha * 0.35;
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

fn tile_thermal_props(
    temp: &Temperature,
    world: Option<&World>,
    hx: i32,
    hy: i32,
) -> TileThermal {
    let air = MaterialRegistry::props(MaterialId::Air);
    let tc = temp.tile_cols.max(1);
    let tile_mid_y = hy * tc + tc / 2;
    let Some(world) = world else {
        return TileThermal {
            layer: TileLayer::Air,
            capacity: air.heat_capacity,
            albedo: air.albedo,
        };
    };
    // Rock estimate at tile centre. Anchor the cheap band to both rock
    // and sea level so painted lakes/snow at sea still sit in-scan when
    // the live surface differs (unit fixtures + flat shelves).
    let gx_mid = world.wrap_x(hx * tc + tc / 2);
    let rock_mid = live_surface_at(world, temp.seed, gx_mid, temp.sea_level_y, temp.width_cols);
    let anchor_lo = rock_mid.min(temp.sea_level_y);
    let anchor_hi = rock_mid.max(temp.sea_level_y);
    // Margin covers tall packs / carved relief above the free surface.
    const AIR_MARGIN: i32 = 24;
    const BURIED_MARGIN: i32 = 8;
    if tile_mid_y > anchor_hi + AIR_MARGIN {
        return TileThermal {
            layer: TileLayer::Air,
            capacity: air.heat_capacity,
            albedo: air.albedo,
        };
    }
    if tile_mid_y + tc < anchor_lo - BURIED_MARGIN {
        let bedrock = MaterialRegistry::props(MaterialId::Bedrock);
        let depth = (rock_mid - tile_mid_y) as f32;
        return TileThermal {
            layer: TileLayer::Buried { depth_cells: depth },
            capacity: bedrock.heat_capacity * 1.25,
            albedo: 0.0,
        };
    }
    // Surface band only — not full sky↔bedrock (~320 cells/column before).
    let (bound_lo, bound_hi) = match temp.bounds {
        Some(b) => (b.hy_min * tc - 2, b.hy_max * tc + tc + 2),
        None => (anchor_lo - 8, anchor_hi + 64),
    };
    let y_lo = (anchor_lo - 8).max(bound_lo);
    let y_hi = (anchor_hi + 64).min(bound_hi);
    let mut cap_sum = 0.0;
    let mut alb_sum = 0.0;
    let mut surf_sum = 0.0;
    let mut water_cols = 0.0;
    let mut n = 0.0;
    for lx in 0..tc {
        let gx = world.wrap_x(hx * tc + lx);
        let rock = live_surface_at(world, temp.seed, gx, temp.sea_level_y, temp.width_cols);
        let col_lo = (rock.min(temp.sea_level_y) - 8).max(y_lo);
        let col_hi = (rock.max(temp.sea_level_y) + 64).min(y_hi);
        let (surf_y, cap, albedo, watery) =
            column_surface_thermal(world, gx, col_lo, col_hi, rock, &temp.config);
        cap_sum += cap;
        alb_sum += albedo;
        surf_sum += surf_y as f32;
        if watery {
            water_cols += 1.0;
        }
        n += 1.0;
    }
    if n < 1.0 {
        return TileThermal {
            layer: TileLayer::Air,
            capacity: air.heat_capacity,
            albedo: air.albedo,
        };
    }
    let surf_y = (surf_sum / n).round() as i32;
    let cap = cap_sum / n;
    let albedo = alb_sum / n;
    let watery = water_cols / n >= 0.5;

    if tile_mid_y > surf_y + tc {
        return TileThermal {
            layer: TileLayer::Air,
            capacity: air.heat_capacity,
            albedo: air.albedo,
        };
    }
    if tile_mid_y + tc < surf_y {
        let bedrock = MaterialRegistry::props(MaterialId::Bedrock);
        let depth = (surf_y - tile_mid_y) as f32;
        return TileThermal {
            layer: TileLayer::Buried { depth_cells: depth },
            capacity: bedrock.heat_capacity * 1.25,
            albedo: 0.0,
        };
    }
    TileThermal {
        layer: TileLayer::Surface { watery },
        capacity: cap,
        albedo,
    }
}

/// Scan a column for the surface stack: pack / water / ground.
/// Returns `(surface_y, heat_capacity, albedo, is_watery)`.
fn column_surface_thermal(
    world: &World,
    gx: i32,
    y_lo: i32,
    y_hi: i32,
    fallback_y: i32,
    cfg: &TempConfig,
) -> (i32, f32, f32, bool) {
    let gx = world.wrap_x(gx);
    let mut top_y = fallback_y;
    let mut top_cell: Option<Cell> = None;
    for y in (y_lo..=y_hi).rev() {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        let wet_air = cell.material == MaterialId::Air && !cell.sat.is_empty();
        if cell.material != MaterialId::Air || wet_air {
            top_y = y;
            top_cell = Some(cell);
            break;
        }
    }
    let mut water_like = 0i32;
    for y in (y_lo..=top_y).rev() {
        let Some(cell) = world.get_cell(gx, y) else {
            continue;
        };
        let wet_air = cell.material == MaterialId::Air && !cell.sat.is_empty();
        let frozen = matches!(cell.material, MaterialId::Ice | MaterialId::Snow);
        if wet_air || cell.material == MaterialId::Water || frozen {
            water_like += 1;
        } else {
            break;
        }
    }
    let cell = top_cell.unwrap_or(Cell::solid(MaterialId::Stone));
    let mat = if cell.material == MaterialId::Air && !cell.sat.is_empty() {
        MaterialId::Water
    } else {
        cell.material
    };
    let props = MaterialRegistry::props(mat);
    let stack = (water_like.saturating_sub(1) as f32).max(0.0);
    let cap = props.heat_capacity + stack * cfg.water_stack_cap;
    let watery = water_like > 0
        || matches!(mat, MaterialId::Water | MaterialId::Ice);
    (top_y, cap, props.albedo, watery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::ChunkCoord;
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
        for _ in 0..8 {
            day.step(None, &h, 0);
            night.step(None, &h, DEMO_DAY_TICKS / 2);
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
            clear.step(None, &h_clear, 0);
            cloudy.step(None, &h_cloud, 0);
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

    fn fill_tile_surface(w: &mut World, tile_x0: i32, y_ground: i32, mat: MaterialId, water_h: i32) {
        for x in tile_x0..tile_x0 + 4 {
            for y in (y_ground - 1)..=(y_ground + water_h + 1) {
                w.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
            }
            w.set_cell(x, y_ground, Cell::solid(MaterialId::Stone));
            if water_h > 0 {
                for y in 1..=water_h {
                    w.set_cell(x, y_ground + y, Cell::water());
                }
            } else {
                w.set_cell(x, y_ground + 1, Cell::solid(mat));
            }
        }
    }

    fn fill_buried_rock(w: &mut World, tile_x0: i32, y_ground: i32, depth: i32) {
        for x in tile_x0..tile_x0 + 4 {
            for y in (y_ground - depth)..=(y_ground + 1) {
                w.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
            }
            for y in (y_ground - depth)..y_ground {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
            w.set_cell(x, y_ground, Cell::solid(MaterialId::Stone));
            w.set_cell(x, y_ground + 1, Cell::solid(MaterialId::Sand));
        }
    }

    #[test]
    fn water_column_lags_cold_snap_more_than_dry_sand() {
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let pond_x0: i32 = 4;
        let dry_x0: i32 = 20;
        let mut world = World::new(7);
        fill_tile_surface(&mut world, pond_x0, sea, MaterialId::Water, 5);
        fill_tile_surface(&mut world, dry_x0, sea, MaterialId::Sand, 0);
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
            p.seed,
            p.width_cols,
            sea,
            false,
        );
        for v in t.cells.values_mut() {
            *v = 12.0;
        }
        t.config.base_temp_c = -15.0;
        t.config.diffuse_alpha = 0.0;
        let h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        for i in 0..5 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD);
        }
        let pond_t = t.at_cell(pond_x0 + 1, sea + 3);
        let dry_t = t.at_cell(dry_x0 + 1, sea + 1);
        assert!(
            pond_t > dry_t + 1.5,
            "pond {pond_t:.1}C should stay warmer than dry sand {dry_t:.1}C after a cold snap"
        );
        assert!(
            pond_t > 5.0,
            "lake must hold heat through a short cold snap (got {pond_t:.1})"
        );
    }

    #[test]
    fn buried_bedrock_ignores_night_air_snap() {
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let x0: i32 = 8;
        let mut world = World::new(3);
        fill_buried_rock(&mut world, x0, sea, 24);
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            32,
            p.sky_ceiling_y,
            1,
            32,
            sea,
            false,
        );
        for v in t.cells.values_mut() {
            *v = 20.0;
        }
        t.config.base_temp_c = -20.0;
        t.config.diffuse_alpha = 0.0;
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y);
        // One climate "night" worth of thermal steps.
        for i in 0..8 {
            t.step(Some(&world), &h, DEMO_DAY_TICKS / 2 + i * TEMP_STEP_PERIOD);
        }
        let deep_y = sea - 16;
        let deep_t = t.at_cell(x0 + 1, deep_y);
        let air_y = sea + 20;
        let air_t = t.at_cell(x0 + 1, air_y);
        assert!(
            deep_t > 12.0,
            "buried bedrock must not drop with night air (deep={deep_t:.1})"
        );
        assert!(
            deep_t > air_t + 15.0,
            "deep {deep_t:.1} should stay far warmer than air {air_t:.1}"
        );
    }

    #[test]
    fn geothermal_warms_cold_deep_rock_over_time() {
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let x0: i32 = 8;
        let mut world = World::new(3);
        fill_buried_rock(&mut world, x0, sea, 24);
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            32,
            p.sky_ceiling_y,
            1,
            32,
            sea,
            false,
        );
        for v in t.cells.values_mut() {
            *v = 0.0;
        }
        t.config.diffuse_alpha = 0.0;
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y);
        let deep_y = sea - 16;
        let before = t.at_cell(x0 + 1, deep_y);
        for i in 0..30 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD);
        }
        let after = t.at_cell(x0 + 1, deep_y);
        assert!(
            after > before + 1.0,
            "geothermal should warm deep rock ({before:.1} → {after:.1})"
        );
    }

    #[test]
    fn snow_albedo_slows_daytime_warming_vs_bare_rock() {
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let x0: i32 = 8;
        let mut snow_w = World::new(3);
        let mut rock_w = World::new(3);
        fill_tile_surface(&mut snow_w, x0, sea, MaterialId::Snow, 0);
        fill_tile_surface(&mut rock_w, x0, sea, MaterialId::Stone, 0);

        let mut t_snow = Temperature::with_world_bounds(
            4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y, 1, 32, sea, false,
        );
        let mut t_rock = Temperature::with_world_bounds(
            4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y, 1, 32, sea, false,
        );
        for v in t_snow.cells.values_mut().chain(t_rock.cells.values_mut()) {
            *v = 0.0;
        }
        // Isolate albedo: no skin pull, no day-amp drift — only solar.
        for t in [&mut t_snow, &mut t_rock] {
            t.config.base_temp_c = 0.0;
            t.config.day_amp_c = 0.0;
            t.config.sky_relax = 0.0;
            t.config.min_relax = 0.0;
            t.config.diffuse_alpha = 0.0;
            t.config.solar_heat_c = 0.5;
        }
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y);
        for i in 0..6 {
            t_snow.step(Some(&snow_w), &h, i * TEMP_STEP_PERIOD);
            t_rock.step(Some(&rock_w), &h, i * TEMP_STEP_PERIOD);
        }
        let ts = t_snow.at_cell(x0 + 1, sea + 1);
        let tr = t_rock.at_cell(x0 + 1, sea + 1);
        assert!(
            tr > ts + 0.4,
            "bare rock {tr:.2} should warm faster than snow pack {ts:.2} under sun"
        );
    }
}
