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
//!   Sun/radiate ΔT is divided by heat capacity so a lake is a buffer,
//!   not a sun magnet: dry sand/stone skins heat faster than deep water.
//!   Rock / sand / lakes / snow keep inertia so a lake does not slam
//!   from +20 °C to −10 °C in one night.
//! - **Air** does **not** absorb solar or radiate to space. It sits on
//!   the climate lapse **at its own height** and couples to the ground:
//!   warm skin loft, cold skin inversion. Lapse is not `−crest` stamped
//!   onto the whole column (that made a cold cap above every hill).
//!   Tropospheric lapse runs only up to [`TempConfig::tropopause_elev_cells`]
//!   above sea; above that the profile is a weak stratospheric slope
//!   so a taller sky box is extra air, not a colder wet lid.
//!   Wet air has more thermal mass (vapor Cp ~1.9× dry) so it relaxes
//!   slower and a rising plume mixes that heat into the tile above.
//!   No noon/midnight skin snap on the sky.
//! - **Buried** rock ignores solar / sky radiation. Overburden is
//!   cells below the **live** surface (not the seed crest). F3-erasing
//!   a hill drops that depth; the leftover core is not a stamp of the
//!   mountain that is gone. Fill (no world yet) uses sea-datum so the
//!   seed profile is not painted in. Bedrock still adds a uniform
//!   bottom flux; diffusion carries heat upward.
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
    /// Cells above sea where tropospheric lapse stops. `0` = no knee
    /// (linear to the sky box, the old profile). Peaks stay in the
    /// lapse so high land can freeze; free sky above does not keep
    /// cooling just because the ceiling moved.
    #[serde(default = "default_tropopause_elev_cells")]
    pub tropopause_elev_cells: i32,
    /// °C per cell above the tropopause. Default 0 — isothermal lid.
    #[serde(default = "default_strat_lapse_c")]
    pub strat_lapse_c: f32,
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
    /// Radiative leak multiplier on surface water. Near 1 = water
    /// radiates like land; buffering is capacity, not a heat trap.
    pub water_night_cool_scale: f32,
    /// How hard surface capacity damps sun/radiate ΔT (0 = raw °C).
    #[serde(default = "default_force_inertia")]
    pub force_inertia: f32,
    /// Deep-rock geothermal target at the live surface (°C).
    pub geothermal_surface_c: f32,
    /// Extra °C per cell of overburden below the live surface.
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
/// Knee at [`crate::worldgen::TROPOSPHERE_TOP_Y`] when sea is 80
/// (y=1000 ≈ 250 m). Peaks stay in the lapse; the lid sits above that.
fn default_tropopause_elev_cells() -> i32 {
    (crate::worldgen::TROPOSPHERE_TOP_Y - 80).max(1)
}
fn default_strat_lapse_c() -> f32 {
    0.0
}

/// Tropospheric drop plus the weaker slope above the knee.
/// `knee <= 0` is the old linear profile (no tropopause).
fn lapse_drop(elev: f32, knee: f32, tropo_lapse: f32, strato_lapse: f32) -> f32 {
    let e = elev.max(0.0);
    if knee <= 0.0 {
        return tropo_lapse * e;
    }
    tropo_lapse * e.min(knee) + strato_lapse * (e - knee).max(0.0)
}
fn default_force_inertia() -> f32 {
    0.20
}

/// Deep water only counts this many extra cells toward skin capacity.
const WATER_STACK_CAP_CELLS: f32 = 12.0;

impl Default for TempConfig {
    fn default() -> Self {
        Self {
            base_temp_c: 18.0,
            sea_bias_c: -2.0,
            land_day_bump_c: 0.0,
            lapse_c: 0.08,
            tropopause_elev_cells: default_tropopause_elev_cells(),
            strat_lapse_c: default_strat_lapse_c(),
            day_amp_c: 0.0,
            solar_heat_c: 0.55,
            night_cool_c: 0.30,
            cloud_shade: 0.55,
            hum_shade_ref: 80.0,
            sky_relax: 0.12,
            diffuse_alpha: 0.10,
            inertia_scale: 1.6,
            min_relax: 0.003,
            max_relax: 0.28,
            water_stack_cap: 1.4,
            water_night_cool_scale: 0.70,
            force_inertia: default_force_inertia(),
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
    /// Live skin y per humidity-tile column, rebuilt each [`Self::step`].
    #[serde(skip)]
    surf_cache: HashMap<i32, i32>,
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
            surf_cache: HashMap::new(),
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

    /// Cells below sea level. Fill / no-world fallback — not a seed hill.
    pub fn geothermal_depth_at_y(&self, y_cells: i32) -> f32 {
        (self.sea_level_y - y_cells).max(0) as f32
    }

    /// Overburden cells below the live skin. Seed crest is only a hint
    /// for the walk; a deleted hill must drop this depth.
    pub fn geothermal_overburden_cells(
        &self,
        world: Option<&World>,
        hx: i32,
        y_cells: i32,
    ) -> f32 {
        match world {
            Some(_) => {
                let surf = self.column_surface_y_estimate(world, hx);
                (surf - y_cells).max(0) as f32
            }
            None => self.geothermal_depth_at_y(y_cells),
        }
    }

    /// Geothermal target (°C) at `depth_cells` of overburden.
    pub fn geothermal_at_depth(&self, depth_cells: f32) -> f32 {
        let cfg = &self.config;
        cfg.geothermal_surface_c + cfg.geothermal_gradient_c_per_cell * depth_cells.max(0.0)
    }

    /// Geothermal target at world Y using the sea-datum fallback.
    pub fn geothermal_at_y(&self, y_cells: i32) -> f32 {
        self.geothermal_at_depth(self.geothermal_depth_at_y(y_cells))
    }

    /// Drop cached layer/capacity so the next step re-reads the world.
    /// F3 paint must not keep a hill-core “Buried” stamp.
    pub fn invalidate_props(&mut self) {
        self.props_cache.clear();
        self.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
    }

    /// Fill tiles: air/surface from the climate baseline; deep crust
    /// from the sea-datum geothermal (not the seed hill profile).
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
                let mid = hy * tc + tc / 2;
                let depth = self.geothermal_depth_at_y(mid);
                let t0 = if depth > tc as f32 {
                    self.geothermal_at_depth(depth)
                } else {
                    self.climate_at_tile(None, hx, hy)
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
        Self::land_from_surface(self.column_surface_y_estimate(world, hx), self.sea_level_y)
    }

    fn land_from_surface(surf_y: i32, sea_level_y: i32) -> f32 {
        let d = (surf_y - sea_level_y) as f32;
        ((d + 2.0) / 4.0).clamp(0.0, 1.0)
    }

    fn climate_from_land(&self, land: f32, y_cells: i32) -> f32 {
        let cfg = &self.config;
        let elev = (y_cells - self.sea_level_y).max(0) as f32;
        let drop = lapse_drop(
            elev,
            cfg.tropopause_elev_cells as f32,
            cfg.lapse_c,
            cfg.strat_lapse_c,
        );
        cfg.base_temp_c + cfg.sea_bias_c * (1.0 - land) - drop
    }

    fn tile_mid_y(&self, hy: i32) -> i32 {
        let tc = self.tile_cols.max(1);
        hy * tc + tc / 2
    }

    /// Climate-mean skin for surface inertia (no noon/midnight swap).
    pub fn skin_temp(&self, hx: i32, hy: i32, tick: u64) -> f32 {
        self.skin_temp_on(None, hx, hy, tick)
    }

    fn skin_temp_on(&self, world: Option<&World>, hx: i32, hy: i32, tick: u64) -> f32 {
        // `tick` used to drive a `day_amp_c` noon/midnight skin.
        // The diurnal is sun minus radiation now; the ground relaxes
        // toward climate at the **crest**, not a sky-column stamp.
        let _ = tick;
        self.climate_at_height(world, hx, self.tile_mid_y(hy))
    }

    /// Highest humidity-tile row still inside the troposphere, if a
    /// knee is configured. Rise and the climate profile share this.
    pub fn tropopause_max_hy(&self, tile_cols: i32) -> Option<i32> {
        let elev = self.config.tropopause_elev_cells;
        if elev <= 0 {
            return None;
        }
        let y = self.sea_level_y.saturating_add(elev);
        Some((y - 1).div_euclid(tile_cols.max(1)))
    }

    /// Elevation / sea-land climate with **no** day/night swap.
    ///
    /// Lapse follows the sample height up to the tropopause, then the
    /// weaker stratospheric slope. Stamping `−lapse × crest` onto every
    /// air tile in the column was leftover column-skin climate: a cold
    /// cap above every hill.
    fn climate_at_height(&self, world: Option<&World>, hx: i32, y_cells: i32) -> f32 {
        self.climate_from_land(self.land_factor(world, hx), y_cells)
    }

    fn climate_at_tile(&self, world: Option<&World>, hx: i32, hy: i32) -> f32 {
        self.climate_at_height(world, hx, self.tile_mid_y(hy))
    }

    /// Ground-crest climate (mountain tops are colder). Not for air.
    fn climate_baseline(&self, world: Option<&World>, hx: i32) -> f32 {
        let surf = self.column_surface_y_estimate(world, hx);
        self.climate_at_height(world, hx, surf)
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
        // One live-surface walk per column per step — a tall sky used
        // to call this twice per air tile (couple + climate).
        let mut surf_by_hx: HashMap<i32, i32> = HashMap::new();
        let mut land_by_hx: HashMap<i32, f32> = HashMap::new();
        for &(hx, _) in &keys {
            if surf_by_hx.contains_key(&hx) {
                continue;
            }
            let surf = self.column_surface_y_estimate(world, hx);
            surf_by_hx.insert(hx, surf);
            land_by_hx.insert(hx, Self::land_from_surface(surf, self.sea_level_y));
        }
        self.surf_cache.clone_from(&surf_by_hx);
        for (hx, hy) in keys {
            let props = self
                .props_cache
                .get(&(hx, hy))
                .copied()
                .unwrap_or_else(|| tile_thermal_props(self, world, hx, hy));
            let t = self.at_tile(hx, hy);
            let surf = *surf_by_hx.get(&hx).unwrap_or(&self.sea_level_y);
            let land = *land_by_hx.get(&hx).unwrap_or(&1.0);
            let wind_k = || {
                let (lvx, lvy) = wind
                    .map(|w| w.vector_at(world, hx, hy))
                    .unwrap_or((0.0, 0.0));
                (lvx.abs().max(lvy.abs()) / 0.14).clamp(0.0, 1.5)
                    * cfg.wind_mix.clamp(0.0, 1.0)
            };
            let next = match props.layer {
                TileLayer::Air => {
                    let tc = self.tile_cols.max(1);
                    let mid = hy * tc + tc / 2;
                    let height_above = (mid - surf).max(0);
                    let band = cfg.near_surface_tiles.max(1) * tc;
                    // Sun and radiation hit the ground, not this tile.
                    // Air only warms or cools by sitting on that skin —
                    // that lapse is the draft that lofts humidity.
                    // Cooking the column with solar (even a 25 % aloft
                    // leak) flattened the pipe and left the sky equalised.
                    let climate = self.climate_from_land(land, mid);
                    // Far sky already on the lapse: skip couple / humidity
                    // lookup. A 1000-cell column is mostly this case.
                    if height_above > band && (t - climate).abs() < 0.04 {
                        t
                    } else {
                        let wind_k = wind_k();
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
                        let cap =
                            humid_air_capacity_scale(humidity, hx, hy, t, cfg.humid_heat_scale);
                        let relax = (cfg.sky_relax * 0.40 / cap).clamp(cfg.min_relax, 0.08);
                        t + (target - t) * relax
                    }
                }
                TileLayer::Surface { watery } => {
                    let wind_k = wind_k();
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
                    let skin = self.climate_from_land(land, surf);
                    let relax = (cfg.sky_relax
                        / (1.0 + props.capacity.max(0.05) * cfg.inertia_scale))
                        .clamp(cfg.min_relax, cfg.max_relax);
                    // Capacity damps the °C kick. Without this, water's
                    // low albedo + old 0.15 leak added raw heat every
                    // step while sand could net-cool at noon.
                    let force = solar - radiate;
                    let damp = 1.0
                        + props.capacity.max(0.05)
                            * cfg.inertia_scale
                            * cfg.force_inertia.clamp(0.0, 2.0);
                    let n = t + force / damp.max(1.0);
                    n + (skin - n) * relax
                }
                TileLayer::Buried { .. } => {
                    // Overburden from the live surface, every step.
                    // Cached depth would keep a deleted hill hot.
                    let geo = self.geothermal_at_depth(self.geothermal_overburden_cells(
                        world,
                        hx,
                        self.tile_mid_y(hy),
                    ));
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
    ///
    /// Only tiles in [`crate::wind::Wind::field`] (occupied humidity +
    /// halo). Mixing the whole sky box with uniform climate wind was a
    /// 1000-cell no-op that cloned every tile.
    pub(crate) fn advect_air(&mut self, world: Option<&World>, wind: &crate::wind::Wind) {
        let mix = self.config.wind_mix.clamp(0.0, 1.0);
        if mix < 1e-4 || self.cells.is_empty() || wind.field.is_empty() {
            return;
        }
        let mut snap: HashMap<(i32, i32), f32> = HashMap::new();
        snap.reserve(wind.field.len().saturating_mul(3));
        for &(hx, hy) in wind.field.keys() {
            snap.entry((hx, hy)).or_insert_with(|| self.at_tile(hx, hy));
            snap.entry((hx, hy + 1)).or_insert_with(|| self.at_tile(hx, hy + 1));
            snap.entry((hx, hy - 1)).or_insert_with(|| self.at_tile(hx, hy - 1));
            if let Some(sx) = self.wrap_hx(hx + 1) {
                snap.entry((sx, hy)).or_insert_with(|| self.at_tile(sx, hy));
            }
            if let Some(sx) = self.wrap_hx(hx - 1) {
                snap.entry((sx, hy)).or_insert_with(|| self.at_tile(sx, hy));
            }
        }
        for &(hx, hy) in wind.field.keys() {
            let t = *snap.get(&(hx, hy)).unwrap_or(&self.config.base_temp_c);
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
        let free_sky = |hx: i32, hy: i32| -> bool {
            let surf = self
                .surf_cache
                .get(&hx)
                .copied()
                .unwrap_or(self.sea_level_y);
            hy * self.tile_cols.max(1) + self.tile_cols.max(1) / 2 > surf + 16
        };
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&base);
            let here_sky = free_sky(hx, hy);
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) && nx != hx {
                    let n_val = *snap.get(&(nx, hy)).unwrap_or(&base);
                    // Same-height free sky is already on the lapse.
                    if here_sky && free_sky(nx, hy) && (val - n_val).abs() < 0.35 {
                        // skip
                    } else {
                        let flow = (val - n_val) * alpha;
                        if flow.abs() >= 1e-9 {
                            *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                            *deltas.entry((nx, hy)).or_insert(0.0) += flow;
                        }
                    }
                }
            }
            let n_key = (hx, hy + 1);
            if self.accepts(n_key.0, n_key.1) {
                if here_sky && free_sky(n_key.0, n_key.1) {
                    continue;
                }
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
        let depth = (rock_mid - tile_mid_y).max(0) as f32;
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
        let depth = (surf_y - tile_mid_y).max(0) as f32;
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
    let stack = (water_like.saturating_sub(1) as f32)
        .max(0.0)
        .min(WATER_STACK_CAP_CELLS);
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
        t.config.lapse_c = 0.0;
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
        t.config.force_inertia = 0.0;
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
    fn noon_sand_warms_faster_than_deep_water() {
        // Defaults used to net-cool sand at noon while a dark, 0.15×-radiating
        // ocean stacked raw °C. Land skins should lead; water lags via capacity.
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let pond_x0: i32 = 4;
        let dry_x0: i32 = 20;
        let mut world = World::new(7);
        fill_tile_surface(&mut world, pond_x0, sea, MaterialId::Water, 16);
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
        t.config.diffuse_alpha = 0.0;
        t.config.sea_bias_c = 0.0;
        t.config.lapse_c = 0.0;
        t.fill_initial(0);
        for v in t.cells.values_mut() {
            *v = 12.0;
        }
        t.rebuild_row_means();
        let h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        for i in 0..10 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let pond_t = t.at_cell(pond_x0 + 1, sea + 8);
        let dry_t = t.at_cell(dry_x0 + 1, sea + 1);
        assert!(
            dry_t > pond_t + 0.8,
            "sand skin {dry_t:.1}C should heat faster than deep water {pond_t:.1}C at noon"
        );
        assert!(
            dry_t > 12.0 + 0.4,
            "dry sand must net-heat at noon (got {dry_t:.1})"
        );
    }

    #[test]
    fn air_above_a_hill_is_not_stamped_to_mountaintop_climate() {
        // Residue: every air tile in a tall column relaxed toward
        // `base − lapse × crest`, so the sky over a hill was a cold cap
        // while the same height over the sea stayed mild.
        let p = WorldgenParams::default();
        let sea = p.sea_level_y;
        let hill_x0: i32 = 4;
        let low_x0: i32 = 24;
        let hill_h = 48;
        let mut world = World::new(7);
        fill_tile_surface(&mut world, hill_x0, sea, MaterialId::Stone, 0);
        fill_tile_surface(&mut world, low_x0, sea, MaterialId::Stone, 0);
        for x in hill_x0..hill_x0 + 4 {
            for y in (sea + 1)..=(sea + hill_h) {
                world.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
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
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.sea_bias_c = 0.0;
        t.fill_initial(0);
        let h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        for i in 0..12 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let sample_y = sea + hill_h + 16;
        let hill_air = t.at_cell(hill_x0 + 1, sample_y);
        let low_air = t.at_cell(low_x0 + 1, sample_y);
        let old_stamp_gap = t.config.lapse_c * hill_h as f32;
        assert!(
            (hill_air - low_air).abs() < 1.5,
            "same-height air over a hill {hill_air:.1}C must match low land {low_air:.1}C"
        );
        assert!(
            old_stamp_gap > 2.5,
            "fixture: a 48-cell hill must have been a >2.5C column stamp"
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
    fn geothermal_is_the_same_at_the_same_height() {
        // Old fill used the seed crest as depth. A mountain column was
        // stamped hotter than the same Y under the sea — a hotspot you
        // could still see after F3-erasing the hill.
        let p = WorldgenParams::default();
        let mut t = Temperature::with_world_bounds(
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
        t.fill_initial(0);
        let tc = t.tile_cols.max(1);
        let mut hi_hx = 0;
        let mut lo_hx = 0;
        let mut hi = i32::MIN;
        let mut lo = i32::MAX;
        for hx in 0..(p.width_cols / tc) {
            let gx = hx * tc + tc / 2;
            let s = crate::worldgen::continental_surface_y(
                p.seed,
                gx,
                p.sea_level_y,
                p.width_cols,
            );
            if s > hi {
                hi = s;
                hi_hx = hx;
            }
            if s < lo {
                lo = s;
                lo_hx = hx;
            }
        }
        assert!(
            hi > lo + 16,
            "need seed relief so a crest-depth stamp would disagree (hi={hi} lo={lo})"
        );
        let hy_deep = (p.sea_level_y / 2).div_euclid(tc);
        let a = t.at_tile(hi_hx, hy_deep);
        let b = t.at_tile(lo_hx, hy_deep);
        assert!(
            (a - b).abs() < 0.05,
            "same world-Y must share geothermal, not follow the seed hill ({a:.2} vs {b:.2})"
        );
        assert!(
            hi > p.sea_level_y + 24,
            "need a seed hill above sea (crest={hi})"
        );
        // Just above sea under the mountain: old code treated this as
        // tens of cells of overburden (the leftover hotspot).
        let hy_core = (p.sea_level_y + tc).div_euclid(tc);
        let painted = t.at_tile(hi_hx, hy_core);
        let climate = t.climate_at_tile(None, hi_hx, hy_core);
        let old_overburden = t.geothermal_at_depth((hi - t.tile_mid_y(hy_core)) as f32);
        assert!(
            (painted - climate).abs() < 3.0,
            "hill core above sea is climate, not a painted hotspot \
             ({painted:.1} vs climate {climate:.1})"
        );
        assert!(
            painted < old_overburden - 4.0,
            "must not stamp crest-depth geothermal into the hill \
             ({painted:.1} vs old overburden {old_overburden:.1})"
        );
    }

    #[test]
    fn overburden_geothermal_follows_the_live_surface() {
        let sea: i32 = 80;
        let crest: i32 = 140;
        let bed: i32 = 40;
        let probe_y: i32 = 20;
        let mut w = World::new(3);
        for y in 0..=crest {
            w.ensure_chunk(ChunkCoord::new(
                0,
                y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
            ));
        }
        for x in 0..8 {
            for y in 0..=crest {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 16, 160, 1, 16, sea, false);
        t.config.diffuse_alpha = 0.0;
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.inertia_scale = 0.0;
        t.config.geothermal_relax = 0.20;
        t.fill_initial(0);
        let hill_depth = t.geothermal_overburden_cells(Some(&w), 0, probe_y);
        let hill_geo = t.geothermal_at_depth(hill_depth);
        assert!(
            (hill_depth - (crest - probe_y) as f32).abs() < 4.0,
            "intact hill overburden must read the live crest (depth={hill_depth})"
        );
        for v in t.cells.values_mut() {
            *v = 18.0;
        }
        t.rebuild_row_means();
        t.invalidate_props();
        let h = Humidity::with_world_bounds(4, 0, 0, 16, 160);
        for i in 0..24 {
            t.step(Some(&w), &h, i * TEMP_STEP_PERIOD, None);
        }
        let with_hill = t.at_cell(2, probe_y);
        assert!(
            with_hill > 22.0,
            "live overburden under the hill should warm the core ({with_hill:.1}, target {hill_geo:.1})"
        );

        for x in 0..8 {
            for y in (bed + 1)..=crest {
                w.set_cell(x, y, Cell::air());
            }
        }
        t.invalidate_props();
        let cut_depth = t.geothermal_overburden_cells(Some(&w), 0, probe_y);
        let cut_geo = t.geothermal_at_depth(cut_depth);
        assert!(
            cut_depth < hill_depth * 0.5,
            "erasing the hill must drop live overburden ({cut_depth} vs {hill_depth})"
        );
        for i in 24..48 {
            t.step(Some(&w), &h, i * TEMP_STEP_PERIOD, None);
        }
        let after_cut = t.at_cell(2, probe_y);
        assert!(
            after_cut + 2.0 < with_hill,
            "core must follow the updated surface ({with_hill:.1} → {after_cut:.1}, target {cut_geo:.1})"
        );
        assert!(
            (after_cut - cut_geo).abs() < (after_cut - hill_geo).abs(),
            "closer to the new live overburden {cut_geo:.1} than the deleted crest {hill_geo:.1} \
             (got {after_cut:.1})"
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
            t.config.force_inertia = 0.0;
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

    #[test]
    fn tropopause_knee_stops_the_lapse_so_a_tall_sky_is_not_colder() {
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 400, 1, 32, 80, false);
        t.config.base_temp_c = 18.0;
        t.config.sea_bias_c = 0.0;
        t.config.lapse_c = 0.08;
        t.config.tropopause_elev_cells = 160;
        t.config.strat_lapse_c = 0.0;
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.near_surface_couple = 0.0;
        let mid_tropo = t.climate_at_height(None, 0, 80 + 80);
        let at_knee = t.climate_at_height(None, 0, 80 + 160);
        let above = t.climate_at_height(None, 0, 80 + 280);
        let linear_lid = 18.0 - 0.08 * 280.0;
        assert!(
            (mid_tropo - (18.0 - 0.08 * 80.0)).abs() < 0.2,
            "below the knee the lapse is still linear ({mid_tropo:.1})"
        );
        assert!(
            (at_knee - above).abs() < 0.2,
            "isothermal lid: knee {at_knee:.1} vs far sky {above:.1}"
        );
        assert!(
            above > linear_lid + 8.0,
            "tall sky must not keep the old linear drop (above={above:.1} linear={linear_lid:.1})"
        );
    }
}
