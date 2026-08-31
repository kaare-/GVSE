//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Coarse **thermal field** (°C) on the same 4×4 tile grid as humidity /
//! wind.
//!
//! - **Surface** takes the sun and always radiates. Night is just the
//!   sun being off — there is no separate night-cool pulse and no
//!   noon/midnight skin swing. Humidity in the column above reflects
//!   incoming sun (daytime shade) and blankets outgoing radiation.
//!   Rock / sand / lakes / snow keep inertia so a lake does not slam
//!   from +20 °C to −10 °C in one night.
//! - **Air** does **not** absorb solar or radiate to space. It sits on
//!   the climate lapse and couples to the ground: warm skin loft, cold
//!   skin inversion. That is the draft that carries humidity up to
//!   condense. Wet air has more thermal mass (vapor Cp ~1.9× dry) so
//!   it relaxes slower and a rising plume mixes that heat into the
//!   tile above. No noon/midnight skin snap on the sky.
//! - **Buried** rock ignores solar / sky radiation; it relaxes toward a
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
    /// **Retired.** Land used to get an extra noon bump on the climate
    /// skin. The diurnal now comes from sun minus radiation. Kept so
    /// saves still deserialize.
    pub land_day_bump_c: f32,
    pub lapse_c: f32,
    /// **Retired.** Used to snap the surface toward a noon/midnight
    /// skin. The swing is now sun strength vs radiation rate. Kept so
    /// saves still deserialize; the stepper ignores it.
    pub day_amp_c: f32,
    /// Heat added to the ground per thermal step at full sun.
    pub solar_heat_c: f32,
    /// Heat the ground radiates per thermal step (day and night).
    /// Night is this leak with the sun off, not a second forcing.
    pub night_cool_c: f32,
    /// How hard column humidity reflects incoming sun (0..1).
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
    /// Radiative leak multiplier on surface water (<< rock/air).
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
    /// How hard near-surface air tracks the ground (0..1).
    #[serde(default = "default_near_surface_couple")]
    pub near_surface_couple: f32,
    /// Air tiles (in tile units) that couple to the surface.
    #[serde(default = "default_near_surface_tiles")]
    pub near_surface_tiles: i32,
    /// Outgoing radiation held back by wet air (0..1). Day and night.
    #[serde(default = "default_hum_night_blanket")]
    pub hum_night_blanket: f32,
    /// Wind chill / couple scale on the thermal step (0..1).
    #[serde(default = "default_wind_mix")]
    pub wind_mix: f32,
    /// How much vapor raises air's heat capacity and how hard a
    /// rising plume carries that heat (0 = dry air only).
    #[serde(default = "default_humid_heat_scale")]
    pub humid_heat_scale: f32,
}

fn default_near_surface_couple() -> f32 {
    0.55
}
fn default_near_surface_tiles() -> i32 {
    3
}
fn default_hum_night_blanket() -> f32 {
    0.55
}
fn default_wind_mix() -> f32 {
    0.60
}
fn default_humid_heat_scale() -> f32 {
    1.0
}

impl Default for TempConfig {
    fn default() -> Self {
        Self {
            base_temp_c: 18.0,
            sea_bias_c: -2.0,
            land_day_bump_c: 0.0,
            lapse_c: 0.08,
            day_amp_c: 0.0,
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
            near_surface_couple: default_near_surface_couple(),
            near_surface_tiles: default_near_surface_tiles(),
            hum_night_blanket: default_hum_night_blanket(),
            wind_mix: default_wind_mix(),
            humid_heat_scale: default_humid_heat_scale(),
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
    /// Per-row mean °C, rebuilt at the end of [`Self::step`]. Convection
    /// reads this instead of scanning the whole tile row every humidity tick.
    #[serde(skip)]
    row_mean: HashMap<i32, f32>,
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
            row_mean: HashMap::new(),
        };
        t.fill_initial(0);
        t
    }

    /// Horizontal mean at tile row `hy` (world-wide, not occupancy-weighted).
    ///
    /// Cache miss returns [`TempConfig::base_temp_c`] — do not scan the
    /// field here; that was the humidity-rise FPS cliff.
    pub fn row_mean_at(&self, hy: i32) -> f32 {
        self.row_mean
            .get(&hy)
            .copied()
            .unwrap_or(self.config.base_temp_c)
    }

    pub fn rebuild_row_means(&mut self) {
        self.row_mean.clear();
        if self.cells.is_empty() {
            return;
        }
        let mut acc: HashMap<i32, (f32, u32)> = HashMap::new();
        for (&(_, hy), &v) in &self.cells {
            let e = acc.entry(hy).or_insert((0.0, 0));
            e.0 += v;
            e.1 += 1;
        }
        self.row_mean = acc
            .into_iter()
            .map(|(hy, (sum, n))| (hy, sum / n.max(1) as f32))
            .collect();
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

    /// Fill tiles: air/surface from the climate baseline; buried from geothermal.
    pub fn fill_initial(&mut self, tick: u64) {
        let _ = tick;
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
                    self.climate_baseline(None, hx)
                };
                self.cells.insert((hx, hy), t0);
            }
        }
        self.rebuild_row_means();
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
            Some(w) => {
                let rock = live_surface_y(w, gx, hint, LIVE_SURFACE_SEARCH);
                crate::worldgen::live_skin_y(w, gx, rock)
            }
            None => hint,
        }
    }

    fn land_factor(&self, world: Option<&World>, hx: i32) -> f32 {
        // Skin height vs sea — waterline, not the excavated bed.
        // A pond at sea is half-sea climate, not a −2 °C hole with two coasts.
        let s = self.column_surface_y_estimate(world, hx);
        let d = (s - self.sea_level_y) as f32;
        ((d + 2.0) / 4.0).clamp(0.0, 1.0)
    }

    fn elev_cells(&self, world: Option<&World>, hx: i32) -> f32 {
        (self.column_surface_y_estimate(world, hx) - self.sea_level_y).max(0) as f32
    }

    /// Climate-mean skin for surface inertia (no noon/midnight swap).
    pub fn skin_temp(&self, hx: i32, hy: i32, tick: u64) -> f32 {
        self.skin_temp_on(None, hx, hy, tick)
    }

    fn skin_temp_on(&self, world: Option<&World>, hx: i32, hy: i32, tick: u64) -> f32 {
        // `tick` / `hy` used to drive a `day_amp_c` noon/midnight skin.
        // The diurnal is sun minus radiation now; this is just the
        // elevation / sea-land climate the surface relaxes toward.
        let _ = (hy, tick);
        self.climate_baseline(world, hx)
    }

    /// Elevation / sea-land climate with **no** day/night swap.
    ///
    /// Air sits on this lapse and takes heat from the ground, not from
    /// a noon/midnight skin (`day_amp_c` is retired).
    fn climate_baseline(&self, world: Option<&World>, hx: i32) -> f32 {
        let cfg = &self.config;
        let land = self.land_factor(world, hx);
        let elev = self.elev_cells(world, hx);
        cfg.base_temp_c + cfg.sea_bias_c * (1.0 - land) - cfg.lapse_c * elev
    }

    /// One thermal step: layered forcing + inertia + diffusion.
    ///
    /// `world` supplies surface materials. Pass `None` only for air-only tests.
    /// `wind` is optional local speed for radiate chill / couple (read from
    /// the rebuilt field; cheap if empty).
    pub fn step(
        &mut self,
        world: Option<&World>,
        humidity: &Humidity,
        tick: u64,
        wind: Option<&crate::wind::Wind>,
    ) {
        if self.cells.is_empty() {
            self.fill_initial(tick);
        }
        let dn = day_night_factor_cfg(tick, &self.climate);
        let cfg = self.config;
        let climate_k = wind
            .map(|w| (w.climate_vx.abs() / 0.14).clamp(0.0, 1.5) * cfg.wind_mix.clamp(0.0, 1.0))
            .unwrap_or(0.0);
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
            let (lvx, lvy) = wind
                .map(|w| w.vector_at(world, hx, hy))
                .unwrap_or((0.0, 0.0));
            let wind_k = (lvx.abs().max(lvy.abs()) / 0.14).clamp(0.0, 1.5)
                * cfg.wind_mix.clamp(0.0, 1.0);
            let next = match props.layer {
                TileLayer::Air => {
                    let tc = self.tile_cols.max(1);
                    let surf = self.column_surface_y_estimate(world, hx);
                    let mid = hy * tc + tc / 2;
                    let height_above = (mid - surf).max(0);
                    let band = cfg.near_surface_tiles.max(1) * tc;
                    // Sun and radiation hit the ground, not this tile.
                    // Air only warms or cools by sitting on that skin —
                    // that lapse is the draft that lofts humidity.
                    // Cooking the column with solar (even a 25 % aloft
                    // leak) flattened the pipe and left the sky equalised.
                    let climate = self.climate_baseline(world, hx);
                    let mut target = climate;
                    if height_above <= band && cfg.near_surface_couple > 0.0 {
                        let surf_hy = surf.div_euclid(tc);
                        let surf_t = self.at_tile(hx, surf_hy);
                        let falloff = 1.0
                            - (height_above as f32 / band as f32).clamp(0.0, 1.0);
                        let couple = ((cfg.near_surface_couple + 0.25 * wind_k) * falloff)
                            .clamp(0.0, 0.90);
                        target = climate * (1.0 - couple) + surf_t * couple;
                    }
                    // Slow leak so ground-heated plumes persist.
                    // Wet air has more thermal mass (vapor Cp ~1.9× dry).
                    let cap = humid_air_capacity_scale(humidity, hx, hy, t, cfg.humid_heat_scale);
                    let relax = (cfg.sky_relax * 0.40 / cap).clamp(cfg.min_relax, 0.08);
                    t + (target - t) * relax
                }
                TileLayer::Surface { watery } => {
                    let shade = humidity_column_shade(humidity, hx, hy, &cfg);
                    // Night is the sun being off — no extra night pulse.
                    let sun = dn.max(0.0);
                    let solar = cfg.solar_heat_c
                        * sun
                        * (1.0 - cfg.cloud_shade * shade)
                        * (1.0 - props.albedo.clamp(0.0, 0.95));
                    let water_scale = if watery {
                        cfg.water_night_cool_scale
                    } else {
                        1.0
                    };
                    // Wet air blankets the leak day and night (greenhouse).
                    let blanket = (cfg.hum_night_blanket * shade).clamp(0.0, 0.9);
                    let cool_scale = water_scale
                        * (1.0 - blanket)
                        * (1.0 + 0.45 * wind_k * (1.0 - blanket * 0.5));
                    let radiate = cfg.night_cool_c * cool_scale;
                    let skin = self.skin_temp_on(world, hx, hy, tick);
                    let relax = (cfg.sky_relax
                        / (1.0 + props.capacity.max(0.05) * cfg.inertia_scale))
                        .clamp(cfg.min_relax, cfg.max_relax);
                    let n = t + solar - radiate;
                    n + (skin - n) * relax
                }
                TileLayer::Buried { depth_cells } => {
                    // No solar / sky radiation. Hold heat; ease toward geothermal.
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
        let alpha = (cfg.diffuse_alpha * (1.0 + 0.5 * climate_k)).clamp(0.0, 0.25);
        self.diffuse(alpha);
        if let Some(w) = wind {
            self.advect_air(world, w);
        }
        self.rebuild_row_means();
    }

    /// Upwind mix of **air** tiles along the local wind. Period-20 only.
    /// Buried / surface heat stays put — wind should not drain a lake or
    /// the geothermal profile.
    pub(crate) fn advect_air(&mut self, world: Option<&World>, wind: &crate::wind::Wind) {
        let mix = self.config.wind_mix.clamp(0.0, 1.0);
        if mix < 1e-4 || self.cells.is_empty() {
            return;
        }
        let snap = self.cells.clone();
        for (&(hx, hy), &t) in &snap {
            match self.props_cache.get(&(hx, hy)).map(|p| p.layer) {
                Some(TileLayer::Buried { .. }) | Some(TileLayer::Surface { .. }) => continue,
                _ => {}
            }
            let (vx, vy) = wind.vector_at(world, hx, hy);
            // 0.05 tiles/tick is the Tab default; treat that as a real
            // mix, not a 2% nudge that the overlay cannot see.
            let ax = ((vx.abs() / 0.05) * 0.16 * mix).clamp(0.0, 0.50);
            let ay = ((vy.abs() / 0.05) * 0.10 * mix).clamp(0.0, 0.35);
            if ax < 1e-5 && ay < 1e-5 {
                continue;
            }
            let mut n = t;
            if ax > 1e-5 {
                let src = if vx > 0.0 { hx - 1 } else { hx + 1 };
                if let Some(sx) = self.wrap_hx(src) {
                    if self.accepts(sx, hy) {
                        let up = *snap.get(&(sx, hy)).unwrap_or(&t);
                        n = n * (1.0 - ax) + up * ax;
                    }
                }
            }
            if ay > 1e-5 {
                let src_hy = if vy > 0.0 { hy - 1 } else { hy + 1 };
                if self.accepts(hx, src_hy) {
                    let up = *snap.get(&(hx, src_hy)).unwrap_or(&t);
                    n = n * (1.0 - ay) + up * ay;
                }
            }
            self.cells.insert((hx, hy), n);
        }
    }

    /// Mix source-tile air heat into the tile above after vapor rose.
    ///
    /// A dry lift barely moves T. A wet plume carries the warmth it
    /// loaded at the ground — that is the heat capacity of humid air
    /// doing work, not a second solar term on the sky.
    pub(crate) fn lift_heat_with_vapor(&mut self, lifts: &[(i32, i32, f32)]) {
        let carry = self.config.humid_heat_scale.clamp(0.0, 1.5);
        if carry < 1e-4 || lifts.is_empty() {
            return;
        }
        // Snapshot sources so a hy → hy+1 → hy+2 chain in one pass
        // does not use an already-warmed dest as the next src.
        let mut moves: Vec<(i32, i32, f32, f32)> = Vec::with_capacity(lifts.len());
        for &(hx, hy, frac) in lifts {
            if frac < 1e-5 {
                continue;
            }
            let dest_hy = hy + 1;
            if !self.accepts(hx, dest_hy) {
                continue;
            }
            if matches!(
                self.props_cache.get(&(hx, dest_hy)).map(|p| p.layer),
                Some(TileLayer::Buried { .. }) | Some(TileLayer::Surface { .. })
            ) {
                continue;
            }
            moves.push((hx, dest_hy, self.at_tile(hx, hy), frac));
        }
        for (hx, dest_hy, src, frac) in moves {
            let dest = self.at_tile(hx, dest_hy);
            let mix = (frac * (0.55 + 0.45 * carry)).clamp(0.0, 0.40);
            if mix < 1e-5 {
                continue;
            }
            self.cells.insert((hx, dest_hy), dest + (src - dest) * mix);
        }
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

/// Vapor heat-capacity scale for an air tile (1 = dry).
///
/// Water vapor's specific heat is about 1.9× dry air. `scale` is the
/// Tab knob; 1.0 reaches that ratio at saturation.
fn humid_air_capacity_scale(
    humidity: &Humidity,
    hx: i32,
    hy: i32,
    temp_c: f32,
    scale: f32,
) -> f32 {
    if scale <= 1e-4 {
        return 1.0;
    }
    let sat = Humidity::saturation_mass_at_temp(temp_c).max(1.0);
    let wet = (humidity.at_tile(hx, hy) / sat).clamp(0.0, 1.2);
    1.0 + 0.85 * scale.clamp(0.0, 1.5) * wet
}

/// Peak vapour in the column above a surface tile.
///
/// That mass reflects incoming sun before it hits the ground, and
/// blankets outgoing radiation. A lofted deck must count — scanning
/// only the surface seat missed the cloud that actually shades.
fn humidity_column_shade(humidity: &Humidity, hx: i32, hy: i32, cfg: &TempConfig) -> f32 {
    let hy_top = humidity
        .bounds
        .map(|b| b.hy_max.min(hy + Humidity::VAPOR_COLUMN_TILES))
        .unwrap_or(hy + Humidity::VAPOR_COLUMN_TILES);
    let mut peak = 0.0f32;
    let mut y = hy;
    while y <= hy_top {
        peak = peak.max(humidity.at_tile(hx, y));
        y += 1;
    }
    (peak / cfg.hum_shade_ref.max(1.0)).clamp(0.0, 1.0)
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

    /// Compact column with a real ground so sun/night hit the skin.
    fn grounded_scene() -> (World, Temperature, Humidity, i32) {
        let sea = 16;
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..32 {
            for y in 0..=sea {
                w.set_cell(
                    x,
                    y,
                    Cell::solid(if y == sea {
                        MaterialId::Stone
                    } else {
                        MaterialId::Bedrock
                    }),
                );
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t.fill_initial(0);
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        (w, t, h, sea)
    }

    fn mean_at_hy(t: &Temperature, hy: i32) -> f32 {
        let Some(b) = t.bounds else {
            return t.mean();
        };
        let mut sum = 0.0;
        let mut n = 0.0;
        for hx in b.hx_min..=b.hx_max {
            sum += t.at_tile(hx, hy);
            n += 1.0;
        }
        if n > 0.0 {
            sum / n
        } else {
            t.mean()
        }
    }

    fn mean_near_surface(t: &Temperature, sea: i32) -> f32 {
        let tc = t.tile_cols.max(1);
        mean_at_hy(t, (sea + tc / 2).div_euclid(tc))
    }

    fn mean_first_air(t: &Temperature, sea: i32) -> f32 {
        let tc = t.tile_cols.max(1);
        mean_at_hy(t, sea.div_euclid(tc) + 1)
    }

    fn mean_aloft(t: &Temperature) -> f32 {
        let Some(b) = t.bounds else {
            return t.mean();
        };
        mean_at_hy(t, b.hy_max)
    }

    #[test]
    fn noon_ground_warmer_than_midnight_after_steps() {
        let (w_day, mut day, h, sea) = grounded_scene();
        let (w_night, mut night, _, _) = grounded_scene();
        for _ in 0..10 {
            day.step(Some(&w_day), &h, 0, None);
            night.step(Some(&w_night), &h, DEMO_DAY_TICKS / 2, None);
        }
        let day_skin = mean_near_surface(&day, sea);
        let night_skin = mean_near_surface(&night, sea);
        assert!(
            day_skin > night_skin + 1.0,
            "noon ground {:.1} should beat midnight {:.1}",
            day_skin,
            night_skin
        );
    }

    #[test]
    fn clouds_shade_daytime_heating() {
        let (w_clear, mut clear, h_clear, sea) = grounded_scene();
        let (w_cloud, mut cloudy, mut h_cloud, _) = grounded_scene();
        if let Some(b) = h_cloud.bounds {
            for hy in b.hy_min..=b.hy_max {
                for hx in b.hx_min..=b.hx_max {
                    h_cloud
                        .cells
                        .insert((hx, hy), TempConfig::default().hum_shade_ref * 2.0);
                }
            }
        }
        // Isolate reflection: radiation is a separate humidity job
        // (blanket). At noon those two used to be invisible because
        // the leak only ran at night.
        for t in [&mut clear, &mut cloudy] {
            t.config.night_cool_c = 0.0;
            t.config.hum_night_blanket = 0.0;
            t.config.diffuse_alpha = 0.0;
        }
        for _ in 0..8 {
            clear.step(Some(&w_clear), &h_clear, 0, None);
            cloudy.step(Some(&w_cloud), &h_cloud, 0, None);
        }
        let clear_skin = mean_near_surface(&clear, sea);
        let cloud_skin = mean_near_surface(&cloudy, sea);
        assert!(
            clear_skin > cloud_skin + 0.3,
            "clear ground {:.1} should warm more than cloudy {:.1}",
            clear_skin,
            cloud_skin
        );
    }

    #[test]
    fn wet_air_blankets_radiation() {
        let (w_dry, mut dry, h_dry, sea) = grounded_scene();
        let (w_wet, mut wet, mut h_wet, _) = grounded_scene();
        if let Some(b) = h_wet.bounds {
            for hy in b.hy_min..=b.hy_max {
                for hx in b.hx_min..=b.hx_max {
                    h_wet.cells.insert((hx, hy), 400.0);
                }
            }
        }
        for t in [&mut dry, &mut wet] {
            t.config.hum_night_blanket = 0.85;
            t.config.solar_heat_c = 0.0;
            t.config.diffuse_alpha = 0.0;
        }
        for _ in 0..10 {
            dry.step(Some(&w_dry), &h_dry, DEMO_DAY_TICKS / 2, None);
            wet.step(Some(&w_wet), &h_wet, DEMO_DAY_TICKS / 2, None);
        }
        let dry_skin = mean_near_surface(&dry, sea);
        let wet_skin = mean_near_surface(&wet, sea);
        assert!(
            wet_skin > dry_skin + 0.25,
            "humid ground {:.1} should stay warmer than dry {:.1} under the same leak",
            wet_skin,
            dry_skin
        );
    }

    #[test]
    fn lofted_humidity_shades_the_ground() {
        // A deck above the surface must cut the sun — not only vapour
        // sitting on the skin tile.
        let (w_clear, mut clear, h_clear, sea) = grounded_scene();
        let (w_cloud, mut cloudy, mut h_cloud, _) = grounded_scene();
        let tc = cloudy.tile_cols.max(1);
        let surf_hy = (sea + tc / 2).div_euclid(tc);
        if let Some(b) = h_cloud.bounds {
            for hx in b.hx_min..=b.hx_max {
                h_cloud
                    .cells
                    .insert((hx, surf_hy + 4), TempConfig::default().hum_shade_ref * 2.0);
            }
        }
        for t in [&mut clear, &mut cloudy] {
            t.config.night_cool_c = 0.0;
            t.config.diffuse_alpha = 0.0;
            t.config.day_amp_c = 0.0;
        }
        for _ in 0..8 {
            clear.step(Some(&w_clear), &h_clear, 0, None);
            cloudy.step(Some(&w_cloud), &h_cloud, 0, None);
        }
        let clear_skin = mean_near_surface(&clear, sea);
        let cloud_skin = mean_near_surface(&cloudy, sea);
        assert!(
            clear_skin > cloud_skin + 0.3,
            "lofted vapour must shade the ground (clear={clear_skin:.1} cloudy={cloud_skin:.1})"
        );
    }

    #[test]
    fn radiation_cools_the_ground_at_noon_when_the_sun_is_off() {
        // Night is lack of sun, not a second pulse. The leak still
        // runs at tick 0 if solar_heat_c is zero.
        let (w, mut t, h, sea) = grounded_scene();
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.50;
        t.config.diffuse_alpha = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.day_amp_c = 0.0;
        t.fill_initial(0);
        for v in t.cells.values_mut() {
            *v = 18.0;
        }
        t.rebuild_row_means();
        let before = mean_near_surface(&t, sea);
        for _ in 0..8 {
            t.step(Some(&w), &h, 0, None);
        }
        let after = mean_near_surface(&t, sea);
        assert!(
            after < before - 0.8,
            "ground must radiate at noon if the sun knob is off ({before:.1} → {after:.1})"
        );
    }

    #[test]
    fn air_does_not_snap_to_the_noon_skin() {
        let (mut t, h) = demo_temp();
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.day_amp_c = 10.0;
        t.config.diffuse_alpha = 0.0;
        t.fill_initial(0);
        for v in t.cells.values_mut() {
            *v = 18.0;
        }
        for _ in 0..12 {
            t.step(None, &h, 0, None);
        }
        assert!(
            (t.mean() - 18.0).abs() < 1.5,
            "with sun/radiate off, air must stay near the climate baseline, \
             not climb toward a retired noon skin 18+10 (mean={:.1})",
            t.mean()
        );
    }

    #[test]
    fn sun_heats_the_ground_which_warms_near_air() {
        let (w, mut t, h, sea) = grounded_scene();
        t.config.day_amp_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.solar_heat_c = 0.50;
        t.config.near_surface_couple = 0.70;
        t.config.sea_bias_c = 0.0;
        t.config.lapse_c = 0.0;
        t.fill_initial(0);
        for v in t.cells.values_mut() {
            *v = 18.0;
        }
        for _ in 0..16 {
            t.step(Some(&w), &h, 0, None);
        }
        let skin = mean_near_surface(&t, sea);
        let air = mean_first_air(&t, sea);
        let aloft = mean_aloft(&t);
        assert!(
            skin > 18.0 + 0.8,
            "sun should accumulate in the ground (skin={skin:.1})"
        );
        assert!(
            air > aloft + 0.2,
            "ground-heated air must sit warmer than the sky (air={air:.1} aloft={aloft:.1})"
        );
    }

    #[test]
    fn noon_skin_is_warmer_than_aloft_so_the_column_can_draft() {
        let (w, mut t, h, sea) = grounded_scene();
        t.config.diffuse_alpha = 0.0;
        for _ in 0..20 {
            t.step(Some(&w), &h, 0, None);
        }
        let air = mean_first_air(&t, sea);
        let aloft = mean_aloft(&t);
        assert!(
            air > aloft + 0.5,
            "warm ground under colder air is the draft pipe (air={air:.1} aloft={aloft:.1})"
        );
    }

    #[test]
    fn wind_advects_air_heat_downwind() {
        let (mut t, h) = demo_temp();
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.day_amp_c = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.wind_mix = 1.0;
        t.fill_initial(0);
        let b = t.bounds.expect("demo bounds");
        let hy = (b.hy_min + b.hy_max) / 2;
        let mid = (b.hx_min + b.hx_max) / 2;
        for hx in b.hx_min..=b.hx_max {
            t.cells.insert((hx, hy), if hx < mid { 28.0 } else { 2.0 });
        }
        let p = WorldgenParams::default();
        let mut wind = crate::wind::Wind::climate(
            4,
            0.0,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        for hx in b.hx_min..=b.hx_max {
            wind.field.insert((hx, hy), (0.80, 0.0));
        }
        let before = t.at_tile(mid, hy);
        t.step(None, &h, 0, Some(&wind));
        let after = t.at_tile(mid, hy);
        assert!(
            after > before + 1.5,
            "downwind air at hx={mid} should warm ({before:.1} → {after:.1})"
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
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
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
            t.step(
                Some(&world),
                &h,
                DEMO_DAY_TICKS / 2 + i * TEMP_STEP_PERIOD,
                None,
            );
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
            deep_t > air_t + 10.0,
            "deep {deep_t:.1} should stay far warmer than night air {air_t:.1}"
        );
    }

    #[test]
    fn wet_air_holds_heat_longer_than_dry() {
        // world=None → every tile is Air. Climate wants 10 °C; start at 30.
        let mut dry = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, 16, false);
        dry.config.base_temp_c = 10.0;
        dry.config.lapse_c = 0.0;
        dry.config.sea_bias_c = 0.0;
        dry.config.near_surface_couple = 0.0;
        dry.config.diffuse_alpha = 0.0;
        dry.config.humid_heat_scale = 1.0;
        dry.config.sky_relax = 0.12;
        for v in dry.cells.values_mut() {
            *v = 30.0;
        }
        let mut wet = dry.clone();
        let empty = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        let mut humid = empty.clone();
        let sat = Humidity::saturation_mass_at_temp(30.0);
        // hx=2, hy=8
        humid.add(8, 32, sat);
        dry.step(None, &empty, 0, None);
        wet.step(None, &humid, 0, None);
        let dry_t = dry.at_tile(2, 8);
        let wet_t = wet.at_tile(2, 8);
        assert!(
            wet_t > dry_t + 0.15,
            "saturated air must cool slower than dry ({wet_t:.2} vs {dry_t:.2})"
        );
        assert!(
            humid_air_capacity_scale(&humid, 2, 8, 30.0, 1.0) > 1.7,
            "near-sat vapor should approach ~1.85× dry capacity"
        );
    }

    #[test]
    fn rising_humid_air_warms_the_tile_above() {
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, 16, false);
        t.config.humid_heat_scale = 1.0;
        for ((_, hy), v) in t.cells.iter_mut() {
            *v = if *hy <= 4 { 28.0 } else { 6.0 };
        }
        t.rebuild_row_means();
        let mut dry = t.clone();
        dry.config.humid_heat_scale = 0.0;
        let mut h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        // hx=2, hy=4
        h.add(8, 16, 400.0);
        let mut h_dry = h.clone();
        let before = t.at_tile(2, 5);
        h.buoyant_rise_thermal(0.35, 20, Some(&mut t));
        h_dry.buoyant_rise_thermal(0.35, 20, Some(&mut dry));
        let after = t.at_tile(2, 5);
        let dry_after = dry.at_tile(2, 5);
        assert!(
            after > before + 2.0,
            "a wet plume must pull source heat into the tile above ({before:.2} → {after:.2})"
        );
        assert!(
            (dry_after - before).abs() < 0.05,
            "humid_heat_scale=0 must not mix heat on rise ({dry_after:.2})"
        );
        assert!(
            h.at_tile(2, 5) > 0.0,
            "vapour still has to actually lift"
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
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
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
            t.config.night_cool_c = 0.0;
        }
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, 32, p.sky_ceiling_y);
        for i in 0..6 {
            t_snow.step(Some(&snow_w), &h, i * TEMP_STEP_PERIOD, None);
            t_rock.step(Some(&rock_w), &h, i * TEMP_STEP_PERIOD, None);
        }
        let ts = t_snow.at_cell(x0 + 1, sea + 1);
        let tr = t_rock.at_cell(x0 + 1, sea + 1);
        assert!(
            tr > ts + 0.4,
            "bare rock {tr:.2} should warm faster than snow pack {ts:.2} under sun"
        );
    }
}
