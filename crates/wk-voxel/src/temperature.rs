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

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry};

use crate::cell::Cell;
use crate::climate::{day_night_factor_cfg, ClimateConfig};
use crate::fasthash::FxHashMap;
use crate::grid::World;
use crate::humidity::{Humidity, TileBounds};
use crate::worldgen::{
    continental_surface_y, live_surface_at, live_surface_y, LIVE_SURFACE_SEARCH,
};

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
/// Far-sky / deep-crust margins used by [`tile_thermal_props`] and the
/// column-anchor refresh. A painted pack or carved relief can sit this
/// far above the seed rock; below this, only the surface band scans.
const PROPS_AIR_MARGIN: i32 = 24;
const PROPS_BURIED_MARGIN: i32 = 8;

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
    /// Coarse free-water ↔ underlying rock heat exchange per thermal step
    /// (0 = off). Capacity-weighted mix toward local equilibrium.
    #[serde(default = "default_water_rock_couple")]
    pub water_rock_couple: f32,
    /// Bias free-water vertical exchange by ΔT (0 = off). Warm prefers
    /// rise (confined); warm-over-cold throttles gravity fall.
    #[serde(default = "default_water_convect_bias")]
    pub water_convect_bias: f32,
    /// Coarse open-water ↔ near-surface air heat exchange per thermal
    /// step (0 = off). Cold air cools the water skin; warm water warms air.
    #[serde(default = "default_air_water_skin_couple")]
    pub air_water_skin_couple: f32,
    /// Lateral heat drift on near-surface free water along local wind
    /// (0 = off). Imparts surface current energy without a CFD solver.
    #[serde(default = "default_water_wind_drift")]
    pub water_wind_drift: f32,
    /// Wet porous solids ↔ neighbour rock tiles per thermal step (0 = off).
    /// Scaled by mean pore wetness × diffusivity (reduced vs free-water couple).
    #[serde(default = "default_pore_water_couple")]
    pub pore_water_couple: f32,
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
fn default_water_rock_couple() -> f32 {
    0.12
}
fn default_water_convect_bias() -> f32 {
    0.35
}
fn default_air_water_skin_couple() -> f32 {
    0.18
}
fn default_water_wind_drift() -> f32 {
    0.28
}
fn default_pore_water_couple() -> f32 {
    0.06
}

/// Reference material κ so pair scales sit near 1 for typical rock/water.
const REF_THERMAL_DIFFUSIVITY: f32 = 0.0015;

/// Harmonic-mean κ relative to [`REF_THERMAL_DIFFUSIVITY`], clamped.
fn pair_diff_scale(ka: f32, kb: f32) -> f32 {
    let ka = ka.max(1e-6);
    let kb = kb.max(1e-6);
    let harm = 2.0 * ka * kb / (ka + kb);
    (harm / REF_THERMAL_DIFFUSIVITY).clamp(0.35, 2.0)
}

/// Diffuse gate between free-water and rock/air tiles.
///
/// Geothermal paints horizontal isotherms into Buried rock. Un-gated
/// diffuse then copies those bands into an adjacent lake (same-Y cliff
/// face ↔ water), so cutting a hill "reveals" the lake's stratification.
/// Bed heat still enters via a weak vertical path + water↔rock couple.
fn diffuse_free_water_gate(fw_a: f32, fw_b: f32, vertical: bool) -> f32 {
    let a = fw_a >= 0.5;
    let b = fw_b >= 0.5;
    if a == b {
        return 1.0;
    }
    if vertical {
        0.10
    } else {
        0.02
    }
}

/// ΔT reference (°C) for water convection bias scales.
const WATER_CONVECT_DT_REF: f32 = 20.0;

/// Confined-rise rate scale: warm donor under cooler destination → &gt;1.
pub fn water_convect_rise_scale(
    temp: &Temperature,
    donor_gx: i32,
    donor_gy: i32,
    dst_gx: i32,
    dst_gy: i32,
) -> f32 {
    let bias = temp.config.water_convect_bias.clamp(0.0, 1.0);
    if bias < 1e-5 {
        return 1.0;
    }
    let dt = temp.at_cell(donor_gx, donor_gy) - temp.at_cell(dst_gx, dst_gy);
    (1.0 + bias * (dt / WATER_CONVECT_DT_REF).clamp(-1.0, 1.0)).clamp(0.35, 1.65)
}

/// Gravity fall scale for Air→Air: warm-over-cold throttles dump (stable).
pub fn water_convect_fall_scale(
    temp: &Temperature,
    above_gx: i32,
    above_gy: i32,
    below_gx: i32,
    below_gy: i32,
) -> f32 {
    let bias = temp.config.water_convect_bias.clamp(0.0, 1.0);
    if bias < 1e-5 {
        return 1.0;
    }
    let dt = temp.at_cell(above_gx, above_gy) - temp.at_cell(below_gx, below_gy);
    (1.0 - 0.75 * bias * (dt / WATER_CONVECT_DT_REF).clamp(0.0, 1.0)).clamp(0.25, 1.0)
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
            water_rock_couple: default_water_rock_couple(),
            water_convect_bias: default_water_convect_bias(),
            air_water_skin_couple: default_air_water_skin_couple(),
            water_wind_drift: default_water_wind_drift(),
            pore_water_couple: default_pore_water_couple(),
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
    /// Material `thermal_diffusivity` (game-tuned); weights tile diffusion.
    diffusivity: f32,
    /// Mean `sat / capacity` over porous solids in the tile (0..1).
    pore_wet: f32,
    /// Fraction of tile cells that are free standing water (0..1).
    free_water: f32,
}

impl Default for TileThermal {
    fn default() -> Self {
        Self {
            layer: TileLayer::Air,
            capacity: 1.0,
            albedo: 0.0,
            diffusivity: REF_THERMAL_DIFFUSIVITY,
            pore_wet: 0.0,
            free_water: 0.0,
        }
    }
}

/// Sparse (but usually dense-filled) temperature field in °C.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Temperature {
    pub tile_cols: i32,
    /// Runtime Fx dual — couple / diffuse / wind-mix poke this map on
    /// sparse paths; SipHash was leftover after the humidity Fx cut.
    /// Serde still writes a plain map (same postcard shape). Dense
    /// ticks already prefer [`Self::slab`].
    pub cells: FxHashMap<(i32, i32), f32>,
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
    props_cache: FxHashMap<(i32, i32), TileThermal>,
    #[serde(skip)]
    props_cache_age: u32,
    /// Per-row mean °C, rebuilt at the end of [`Self::step`]. Convection
    /// reads this instead of scanning the whole tile row every humidity tick.
    #[serde(skip)]
    row_mean: FxHashMap<i32, f32>,
    /// Live skin y per humidity-tile column, rebuilt each [`Self::step`].
    #[serde(skip)]
    surf_cache: FxHashMap<i32, i32>,
    /// Dense °C slab for a filled sky box (serde-skip). Packed from
    /// [`Self::cells`] at the start of each dense step so tests that
    /// mutate the map stay correct. Couple writes here; diffuse reuses
    /// it instead of walking the HashMap a second time.
    #[serde(skip)]
    slab: Vec<f32>,
    #[serde(skip)]
    slab_deltas: Vec<f32>,
    /// Live skin y indexed by `hx - bounds.hx_min` when the box is dense.
    #[serde(skip)]
    surf_col: Vec<i32>,
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
            cells: FxHashMap::default(),
            bounds: Some(TileBounds::from_world_cells(tile_cols, x0, y0, x1, y1)),
            wrap_x,
            seed,
            width_cols: width_cols.max(1),
            sea_level_y,
            config: TempConfig::default(),
            climate: ClimateConfig::default(),
            props_cache: FxHashMap::default(),
            props_cache_age: TEMP_PROPS_REFRESH_STEPS,
            row_mean: FxHashMap::default(),
            surf_cache: FxHashMap::default(),
            slab: Vec::new(),
            slab_deltas: Vec::new(),
            surf_col: Vec::new(),
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
        let mut acc: FxHashMap<i32, (f32, u32)> = FxHashMap::default();
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

    fn is_dense_filled(&self) -> bool {
        match self.bounds {
            Some(b) => {
                let cap = b.tile_capacity();
                cap > 0 && self.cells.len().saturating_mul(2) >= cap
            }
            None => false,
        }
    }

    fn pack_slab(&mut self, b: TileBounds) {
        let (w, h) = b.dims();
        let n = w.saturating_mul(h);
        let base = self.config.base_temp_c;
        self.slab.clear();
        self.slab.resize(n, base);
        for (&(hx, hy), &v) in &self.cells {
            if b.contains(hx, hy) {
                self.slab[b.index(w, hx, hy)] = v;
            }
        }
        if self.slab_deltas.len() != n {
            self.slab_deltas.resize(n, 0.0);
        }
    }

    fn rebuild_row_means_from_slab(&mut self, b: TileBounds) {
        let (w, h) = b.dims();
        if w == 0 || h == 0 || self.slab.len() != w * h {
            self.rebuild_row_means();
            return;
        }
        self.row_mean.clear();
        for hy in b.hy_min..=b.hy_max {
            let row = (hy - b.hy_min) as usize * w;
            let mut sum = 0.0f32;
            for dx in 0..w {
                sum += self.slab[row + dx];
            }
            self.row_mean.insert(hy, sum / w.max(1) as f32);
        }
    }

    fn column_rock_anchors(&self, world: &World, hx: i32) -> (i32, i32, i32) {
        let tc = self.tile_cols.max(1);
        let gx_mid = world.wrap_x(hx * tc + tc / 2);
        let rock_mid = live_surface_at(world, self.seed, gx_mid, self.sea_level_y, self.width_cols);
        (
            rock_mid,
            rock_mid.min(self.sea_level_y),
            rock_mid.max(self.sea_level_y),
        )
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

    /// Neighbour tile in +x / −x, wrapping horizontally on ring maps.
    pub fn wrap_tile_x(&self, hx: i32) -> Option<i32> {
        self.wrap_hx(hx)
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

    /// Packed-slab read when the last dense step filled the box.
    /// Prefer this for overlays — [`Self::at_tile`] only hits the sparse map
    /// and can miss live lake °C that still lives in the slab.
    /// Ring worlds wrap `hx` so seam-adjacent overlay reads hit real tiles.
    pub fn at_tile_packed(&self, hx: i32, hy: i32) -> f32 {
        let hx = self.wrap_hx(hx).unwrap_or(hx);
        if let Some(b) = self.bounds {
            let cap = b.tile_capacity();
            if cap > 0 && self.slab.len() == cap && b.contains(hx, hy) {
                let (w, _) = b.dims();
                return self.slab[b.index(w, hx, hy)];
            }
        }
        self.at_tile(hx, hy)
    }

    pub fn at_cell(&self, gx: i32, gy: i32) -> f32 {
        let (hx, hy) = self.tile_of(gx, gy);
        self.at_tile(hx, hy)
    }

    /// Write a tile temperature, keeping the dense slab in sync when present.
    pub fn set_tile_c(&mut self, hx: i32, hy: i32, celsius: f32) {
        let hx = self.wrap_hx(hx).unwrap_or(hx);
        if !celsius.is_finite() {
            return;
        }
        self.cells.insert((hx, hy), celsius);
        if let Some(b) = self.bounds {
            let cap = b.tile_capacity();
            if cap > 0 && self.slab.len() == cap && b.contains(hx, hy) {
                let (w, _) = b.dims();
                self.slab[b.index(w, hx, hy)] = celsius;
            }
        }
    }

    /// Nudge the tile covering `(gx, gy)` toward `target_c` by `mix` (0..=1).
    pub fn deposit_heat_toward(&mut self, gx: i32, gy: i32, target_c: f32, mix: f32) {
        if !target_c.is_finite() {
            return;
        }
        let mix = mix.clamp(0.0, 1.0);
        if mix < 1e-5 {
            return;
        }
        let (hx, hy) = self.tile_of(gx, gy);
        let before = self.at_tile_packed(hx, hy);
        let after = before + (target_c - before) * mix;
        self.set_tile_c(hx, hy, after);
    }

    /// Carry heat with a liquid/vapour mass move between cells (tile-coarse).
    ///
    /// Destination mixes toward the source tile; source cools slightly so the
    /// channel warms as hot water/vapour is shoved through colder rock.
    pub fn advect_with_mass(
        &mut self,
        from_gx: i32,
        from_gy: i32,
        to_gx: i32,
        to_gy: i32,
        moved: u8,
    ) {
        if moved == 0 {
            return;
        }
        let (fhx, fhy) = self.tile_of(from_gx, from_gy);
        let (thx, thy) = self.tile_of(to_gx, to_gy);
        if fhx == thx && fhy == thy {
            return;
        }
        let src = self.at_tile_packed(fhx, fhy);
        let dest = self.at_tile_packed(thx, thy);
        // One cell move is a small fraction of a tile's water, but channels
        // should warm over repeated pulses — bias the mix upward for feel.
        let mix = ((moved as f32) / 40.0).clamp(0.0, 0.55);
        if mix < 1e-4 {
            return;
        }
        self.set_tile_c(thx, thy, dest + (src - dest) * mix);
        self.set_tile_c(fhx, fhy, src + (dest - src) * (mix * 0.35));
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

    /// Overburden cells of **solid rock** below the live rock surface.
    /// Standing water is not crust — counting the waterline as cover made
    /// deep lakes a static hot-bottom geothermal paint.
    pub fn geothermal_overburden_cells(&self, world: Option<&World>, hx: i32, y_cells: i32) -> f32 {
        match world {
            Some(_) => {
                let rock = self.column_rock_y_estimate(world, hx);
                (rock - y_cells).max(0) as f32
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
        let Some(world) = world else {
            let air = air_thermal();
            for &k in keys {
                self.props_cache.insert(k, air);
            }
            self.props_cache_age = 0;
            return;
        };
        // One seed-rock walk per column. Far-sky / deep-crust tiles
        // share that anchor — calling `live_surface_at` per tall-sky
        // tile was leftover world scan on 1000-cell columns.
        let mut anchors: FxHashMap<i32, (i32, i32, i32)> = FxHashMap::default();
        let tc = self.tile_cols.max(1);
        for &(hx, hy) in keys {
            let (rock_mid, alo, ahi) = *anchors
                .entry(hx)
                .or_insert_with(|| self.column_rock_anchors(world, hx));
            let props = match props_early_from_anchor(hy, tc, rock_mid, alo, ahi) {
                Some(mut p) => {
                    // Early buried skips the surface-band scan — still detect
                    // flooded shafts / deep lakes so they are not rock-geo.
                    if matches!(p.layer, TileLayer::Buried { .. }) {
                        let fw = tile_free_water_frac(world, hx, hy, tc);
                        if fw >= 0.5 {
                            let water = MaterialRegistry::props(MaterialId::Water);
                            p.free_water = fw;
                            p.capacity = water.heat_capacity
                                * (1.0 + self.config.water_stack_cap * fw);
                            p.diffusivity = water.thermal_diffusivity;
                            p.albedo = water.albedo;
                        }
                    }
                    p
                }
                None => tile_thermal_props(self, Some(world), hx, hy),
            };
            self.props_cache.insert((hx, hy), props);
        }
        self.props_cache_age = 0;
    }

    /// Live rock crest (ignores standing water). Geothermal overburden.
    fn column_rock_y_estimate(&self, world: Option<&World>, hx: i32) -> i32 {
        let tc = self.tile_cols.max(1);
        let gx = hx * tc + tc / 2;
        let hint = continental_surface_y(self.seed, gx, self.sea_level_y, self.width_cols);
        match world {
            Some(w) => live_surface_y(w, gx, hint, LIVE_SURFACE_SEARCH),
            None => hint,
        }
    }

    /// Skin the air sits on (rock + standing water). Climate / couples.
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
    #[allow(dead_code)] // retained for climate diagnostics / future HUD
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
        let dense = self.is_dense_filled();
        let bounds = self.bounds;
        // Lowest world-y tile band (= deepest underground).
        let deepest_hy = match bounds {
            Some(b) if dense && self.cells.len() == b.tile_capacity() => b.hy_min,
            _ => keys.iter().map(|&(_, hy)| hy).min().unwrap_or(i32::MAX),
        };
        if self.props_cache_age >= TEMP_PROPS_REFRESH_STEPS || self.props_cache.len() != keys.len()
        {
            self.refresh_props_cache(world, &keys);
        }
        self.props_cache_age = self.props_cache_age.saturating_add(1);
        // One live-surface walk per column per step — a tall sky used
        // to call this twice per air tile (couple + climate).
        let mut surf_by_hx: FxHashMap<i32, i32> = FxHashMap::default();
        let mut land_by_hx: FxHashMap<i32, f32> = FxHashMap::default();
        if dense {
            if let Some(b) = bounds {
                let cols = (b.hx_max - b.hx_min + 1).max(0) as usize;
                self.surf_col.clear();
                self.surf_col.resize(cols, self.sea_level_y);
                for hx in b.hx_min..=b.hx_max {
                    let surf = self.column_surface_y_estimate(world, hx);
                    surf_by_hx.insert(hx, surf);
                    land_by_hx.insert(hx, Self::land_from_surface(surf, self.sea_level_y));
                    self.surf_col[(hx - b.hx_min) as usize] = surf;
                }
                self.pack_slab(b);
            }
        } else {
            for &(hx, _) in &keys {
                if surf_by_hx.contains_key(&hx) {
                    continue;
                }
                let surf = self.column_surface_y_estimate(world, hx);
                surf_by_hx.insert(hx, surf);
                land_by_hx.insert(hx, Self::land_from_surface(surf, self.sea_level_y));
            }
        }
        self.surf_cache.clone_from(&surf_by_hx);
        let slab_w = bounds.map(TileBounds::dims).map(|(w, _)| w).unwrap_or(0);
        for (hx, hy) in keys {
            let props = self
                .props_cache
                .get(&(hx, hy))
                .copied()
                .unwrap_or_else(|| tile_thermal_props(self, world, hx, hy));
            let t = if dense {
                if let Some(b) = bounds {
                    if b.contains(hx, hy) && self.slab.len() == b.tile_capacity() {
                        self.slab[b.index(slab_w, hx, hy)]
                    } else {
                        self.at_tile(hx, hy)
                    }
                } else {
                    self.at_tile(hx, hy)
                }
            } else {
                self.at_tile(hx, hy)
            };
            let surf = *surf_by_hx.get(&hx).unwrap_or(&self.sea_level_y);
            let land = *land_by_hx.get(&hx).unwrap_or(&1.0);
            let wind_k = || {
                let (lvx, lvy) = wind
                    .map(|w| w.vector_at(world, hx, hy))
                    .unwrap_or((0.0, 0.0));
                (lvx.abs().max(lvy.abs()) / 0.14).clamp(0.0, 1.5) * cfg.wind_mix.clamp(0.0, 1.0)
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
                            let surf_t = if dense {
                                if let Some(b) = bounds {
                                    if b.contains(hx, surf_hy)
                                        && self.slab.len() == b.tile_capacity()
                                    {
                                        self.slab[b.index(slab_w, hx, surf_hy)]
                                    } else {
                                        self.at_tile(hx, surf_hy)
                                    }
                                } else {
                                    self.at_tile(hx, surf_hy)
                                }
                            } else {
                                self.at_tile(hx, surf_hy)
                            };
                            let falloff = 1.0 - (height_above as f32 / band as f32).clamp(0.0, 1.0);
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
                    // Free-water / ice column is not rock — skip geothermal.
                    // Geothermal on bed ice left warm packs inside cold lakes.
                    let icy = world
                        .map(|w| tile_ice_frac(w, hx, hy, self.tile_cols.max(1)) >= 0.5)
                        .unwrap_or(false);
                    if props.free_water >= 0.5 || icy {
                        t
                    } else {
                        // Overburden from the live rock surface, every step.
                        // Cached depth would keep a deleted hill hot.
                        let geo = self.geothermal_at_depth(self.geothermal_overburden_cells(
                            world,
                            hx,
                            self.tile_mid_y(hy),
                        ));
                        let k_lag = (0.7
                            + 0.3 * (props.diffusivity / REF_THERMAL_DIFFUSIVITY))
                            .clamp(0.55, 1.4);
                        let relax = (cfg.geothermal_relax * k_lag
                            / (1.0 + props.capacity.max(0.05) * cfg.inertia_scale * 0.35))
                            .clamp(0.001, 0.08);
                        let mut n = t + (geo - t) * relax;
                        // Deepest band gets a small constant flux (mantle leak).
                        if hy <= deepest_hy + 1 {
                            n += cfg.geothermal_flux_c;
                        }
                        n
                    }
                }
            };
            // Far sky already on the lapse keeps `next == t`. Rewriting
            // every tile was leftover hasher on a tall box.
            if (next - t).abs() >= 1e-5 {
                self.cells.insert((hx, hy), next);
                if dense {
                    if let Some(b) = bounds {
                        if b.contains(hx, hy) && self.slab.len() == b.tile_capacity() {
                            self.slab[b.index(slab_w, hx, hy)] = next;
                        }
                    }
                }
            }
        }
        // Coarse free-water ↔ rock / air skin exchange before diffuse.
        self.couple_water_rock(dense, bounds, slab_w);
        self.couple_air_water_skin(dense, bounds, slab_w);
        self.couple_pore_water(dense, bounds, slab_w);
        self.couple_free_water_buoyancy(dense, bounds, slab_w);
        self.couple_free_water_wind_drift(world, wind, dense, bounds, slab_w);
        let alpha = (cfg.diffuse_alpha * (1.0 + 0.5 * climate_k)).clamp(0.0, 0.25);
        if dense {
            if let Some(b) = bounds {
                if self.slab.len() == b.tile_capacity() {
                    self.diffuse_slab(alpha, b);
                } else {
                    self.diffuse(alpha);
                }
            } else {
                self.diffuse(alpha);
            }
        } else {
            self.diffuse(alpha);
        }
        if let Some(w) = wind {
            self.advect_air(world, w);
        }
        if dense {
            if let Some(b) = bounds {
                if self.cells.len() == b.tile_capacity() && self.slab.len() == b.tile_capacity() {
                    self.rebuild_row_means_from_slab(b);
                } else {
                    self.rebuild_row_means();
                }
            } else {
                self.rebuild_row_means();
            }
        } else {
            self.rebuild_row_means();
        }
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
        if mix < 1e-4 || self.cells.is_empty() || wind.field_is_empty() {
            return;
        }
        let mut snap: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        snap.reserve(wind.field_len().saturating_mul(3));
        wind.for_each_field(|(hx, hy), _| {
            snap.entry((hx, hy)).or_insert_with(|| self.at_tile(hx, hy));
            snap.entry((hx, hy + 1))
                .or_insert_with(|| self.at_tile(hx, hy + 1));
            snap.entry((hx, hy - 1))
                .or_insert_with(|| self.at_tile(hx, hy - 1));
            if let Some(sx) = self.wrap_hx(hx + 1) {
                snap.entry((sx, hy)).or_insert_with(|| self.at_tile(sx, hy));
            }
            if let Some(sx) = self.wrap_hx(hx - 1) {
                snap.entry((sx, hy)).or_insert_with(|| self.at_tile(sx, hy));
            }
        });
        let mut seats: Vec<(i32, i32)> = Vec::with_capacity(wind.field_len());
        wind.for_each_field(|(hx, hy), _| seats.push((hx, hy)));
        for &(hx, hy) in &seats {
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

    /// Coarse free-water ↔ underlying rock heat couple (T1).
    ///
    /// Watery surface tiles mix toward capacity-weighted equilibrium with
    /// the tile below (buried or dry surface). °C only — cell mass unchanged.
    fn couple_water_rock(&mut self, dense: bool, bounds: Option<TileBounds>, slab_w: usize) {
        let rate = self.config.water_rock_couple.clamp(0.0, 0.5);
        if rate < 1e-5 || self.props_cache.is_empty() {
            return;
        }
        let watery: Vec<(i32, i32)> = self
            .props_cache
            .iter()
            .filter(|(_, p)| {
                matches!(p.layer, TileLayer::Surface { watery: true }) || p.free_water >= 0.5
            })
            .map(|(&k, _)| k)
            .collect();
        if watery.is_empty() {
            return;
        }
        for (hx, hy) in watery {
            let Some(water_props) = self.props_cache.get(&(hx, hy)).copied() else {
                continue;
            };
            let rock_hy = hy - 1;
            let Some(rock_props) = self.props_cache.get(&(hx, rock_hy)).copied() else {
                continue;
            };
            if rock_props.free_water >= 0.5 {
                continue;
            }
            match rock_props.layer {
                TileLayer::Buried { .. } | TileLayer::Surface { watery: false } => {}
                _ => continue,
            }
            let tw = if dense {
                if let Some(b) = bounds {
                    if b.contains(hx, hy) && self.slab.len() == b.tile_capacity() {
                        self.slab[b.index(slab_w, hx, hy)]
                    } else {
                        self.at_tile(hx, hy)
                    }
                } else {
                    self.at_tile(hx, hy)
                }
            } else {
                self.at_tile(hx, hy)
            };
            let tr = if dense {
                if let Some(b) = bounds {
                    if b.contains(hx, rock_hy) && self.slab.len() == b.tile_capacity() {
                        self.slab[b.index(slab_w, hx, rock_hy)]
                    } else {
                        self.at_tile(hx, rock_hy)
                    }
                } else {
                    self.at_tile(hx, rock_hy)
                }
            } else {
                self.at_tile(hx, rock_hy)
            };
            let cw = water_props.capacity.max(0.05);
            let cr = rock_props.capacity.max(0.05);
            let a = (rate
                * pair_diff_scale(water_props.diffusivity, rock_props.diffusivity))
            .clamp(0.0, 1.0);
            if a < 1e-5 {
                continue;
            }
            let teq = (tw * cw + tr * cr) / (cw + cr);
            let nw = tw + (teq - tw) * a;
            let nr = tr + (teq - tr) * a;
            if (nw - tw).abs() >= 1e-5 {
                self.cells.insert((hx, hy), nw);
                if dense {
                    if let Some(b) = bounds {
                        if b.contains(hx, hy) && self.slab.len() == b.tile_capacity() {
                            self.slab[b.index(slab_w, hx, hy)] = nw;
                        }
                    }
                }
            }
            if (nr - tr).abs() >= 1e-5 {
                self.cells.insert((hx, rock_hy), nr);
                if dense {
                    if let Some(b) = bounds {
                        if b.contains(hx, rock_hy) && self.slab.len() == b.tile_capacity() {
                            self.slab[b.index(slab_w, hx, rock_hy)] = nr;
                        }
                    }
                }
            }
        }
    }

    /// Coarse open-water ↔ near-surface air heat couple (T3).
    ///
    /// Watery surface tiles mix with the air tile above. Cold air cools
    /// the water skin; warm water warms a thin air band. Complements the
    /// existing one-way air→ground near-surface couple.
    fn couple_air_water_skin(&mut self, dense: bool, bounds: Option<TileBounds>, slab_w: usize) {
        let rate = self.config.air_water_skin_couple.clamp(0.0, 0.5);
        if rate < 1e-5 || self.props_cache.is_empty() {
            return;
        }
        let watery: Vec<(i32, i32)> = self
            .props_cache
            .iter()
            .filter(|(_, p)| matches!(p.layer, TileLayer::Surface { watery: true }))
            .map(|(&k, _)| k)
            .collect();
        if watery.is_empty() {
            return;
        }
        for (hx, hy) in watery {
            let Some(water_props) = self.props_cache.get(&(hx, hy)).copied() else {
                continue;
            };
            // Surface-band tiles just above open water are still `Surface`
            // (wide mid-y window). Walk up a few tiles to the first Air.
            let mut air_hy = None;
            let mut air_props = None;
            for d in 1..=4 {
                let try_hy = hy + d;
                let Some(p) = self.props_cache.get(&(hx, try_hy)).copied() else {
                    continue;
                };
                if matches!(p.layer, TileLayer::Air) {
                    air_hy = Some(try_hy);
                    air_props = Some(p);
                    break;
                }
            }
            let (Some(air_hy), Some(air_props)) = (air_hy, air_props) else {
                continue;
            };
            let tw = self.read_tile_temp(hx, hy, dense, bounds, slab_w);
            let ta = self.read_tile_temp(hx, air_hy, dense, bounds, slab_w);
            let cw = water_props.capacity.max(0.05);
            // Air capacity is small — keep a floor so water does not dump all heat in one step.
            let ca = air_props.capacity.max(0.15);
            // Night quench: warm lake under cold air couples harder so the
            // skin cools fast enough to overturn (cold sinks / warm rises).
            let quench = if tw > ta + 2.5 {
                (1.0 + 0.55 * ((tw - ta - 2.5) / 12.0).clamp(0.0, 1.0)).min(1.65)
            } else {
                1.0
            };
            let a = (rate
                * quench
                * pair_diff_scale(water_props.diffusivity, air_props.diffusivity))
            .clamp(0.0, 1.0);
            if a < 1e-5 {
                continue;
            }
            let teq = (tw * cw + ta * ca) / (cw + ca);
            let nw = tw + (teq - tw) * a;
            let na = ta + (teq - ta) * a;
            self.write_tile_temp(hx, hy, tw, nw, dense, bounds, slab_w);
            self.write_tile_temp(hx, air_hy, ta, na, dense, bounds, slab_w);
        }
    }

    /// Coarse wet-pore ↔ neighbour rock heat couple (T4).
    ///
    /// Tiles with mean pore wetness exchange with the rock/surface tile
    /// above (one directed edge per vertical pair). Rate scales with
    /// `pore_water_couple × max(wet) × diffusivity`. Free-water surfaces
    /// stay on the T1 path.
    fn couple_pore_water(&mut self, dense: bool, bounds: Option<TileBounds>, slab_w: usize) {
        let rate = self.config.pore_water_couple.clamp(0.0, 0.5);
        if rate < 1e-5 || self.props_cache.is_empty() {
            return;
        }
        let keys: Vec<(i32, i32)> = self.props_cache.keys().copied().collect();
        for (hx, hy) in keys {
            let Some(lo) = self.props_cache.get(&(hx, hy)).copied() else {
                continue;
            };
            if !Self::pore_couple_host(lo.layer) {
                continue;
            }
            let above_hy = hy + 1;
            let Some(hi) = self.props_cache.get(&(hx, above_hy)).copied() else {
                continue;
            };
            if !Self::pore_couple_host(hi.layer) {
                continue;
            }
            let wet = lo.pore_wet.max(hi.pore_wet);
            if wet < 1e-3 {
                continue;
            }
            let t0 = self.read_tile_temp(hx, hy, dense, bounds, slab_w);
            let t1 = self.read_tile_temp(hx, above_hy, dense, bounds, slab_w);
            let c0 = lo.capacity.max(0.05);
            let c1 = hi.capacity.max(0.05);
            let a = (rate * wet * pair_diff_scale(lo.diffusivity, hi.diffusivity)).clamp(0.0, 1.0);
            if a < 1e-5 {
                continue;
            }
            let teq = (t0 * c0 + t1 * c1) / (c0 + c1);
            let n0 = t0 + (teq - t0) * a;
            let n1 = t1 + (teq - t1) * a;
            self.write_tile_temp(hx, hy, t0, n0, dense, bounds, slab_w);
            self.write_tile_temp(hx, above_hy, t1, n1, dense, bounds, slab_w);
        }
    }

    /// Open free-water buoyancy heat mix (lake / shaft columns).
    ///
    /// T2 flow bias only touches confined rise + gravity into empty Air —
    /// a full lake never moves cells, and tile °C is not carried with sat.
    /// When warm sits under colder free water (unstable), mix heat upward
    /// here — the cold anomaly sinks as the warm anomaly rises.
    fn couple_free_water_buoyancy(
        &mut self,
        dense: bool,
        bounds: Option<TileBounds>,
        slab_w: usize,
    ) {
        let bias = self.config.water_convect_bias.clamp(0.0, 1.0);
        if bias < 1e-5 || self.props_cache.is_empty() {
            return;
        }
        let keys: Vec<(i32, i32)> = self.props_cache.keys().copied().collect();
        for (hx, hy) in keys {
            let Some(lo) = self.props_cache.get(&(hx, hy)).copied() else {
                continue;
            };
            if lo.free_water < 0.5 {
                continue;
            }
            let above_hy = hy + 1;
            let Some(hi) = self.props_cache.get(&(hx, above_hy)).copied() else {
                continue;
            };
            if hi.free_water < 0.5 {
                continue;
            }
            let t0 = self.read_tile_temp(hx, hy, dense, bounds, slab_w);
            let t1 = self.read_tile_temp(hx, above_hy, dense, bounds, slab_w);
            let dt = t0 - t1; // >0 ⇒ warm below cold (unstable)
            if dt <= 0.35 {
                continue;
            }
            let strength = bias * (dt / WATER_CONVECT_DT_REF).clamp(0.0, 1.5);
            let a = (0.85 * strength * pair_diff_scale(lo.diffusivity, hi.diffusivity))
                .clamp(0.0, 0.95);
            if a < 1e-5 {
                continue;
            }
            let c0 = lo.capacity.max(0.05);
            let c1 = hi.capacity.max(0.05);
            let teq = (t0 * c0 + t1 * c1) / (c0 + c1);
            // Directed: push the warm anomaly up and the cold anomaly down
            // a touch harder than a pure capacity mix (return flow).
            let n0 = t0 + (teq - t0) * a * 1.05;
            let n1 = t1 + (teq - t1) * a * 1.05;
            self.write_tile_temp(hx, hy, t0, n0, dense, bounds, slab_w);
            self.write_tile_temp(hx, above_hy, t1, n1, dense, bounds, slab_w);
        }
    }

    /// Near-surface free-water heat drift along local wind (lake skin).
    ///
    /// Open lakes do not move cells, so wind cannot push sat. Advect tile
    /// °C downwind on the top free-water band so surface currents and
    /// day/night skin gradients can close a horizontal loop.
    fn couple_free_water_wind_drift(
        &mut self,
        world: Option<&World>,
        wind: Option<&crate::wind::Wind>,
        dense: bool,
        bounds: Option<TileBounds>,
        slab_w: usize,
    ) {
        let rate = self.config.water_wind_drift.clamp(0.0, 1.0);
        let Some(wind) = wind else {
            return;
        };
        if rate < 1e-5 || self.props_cache.is_empty() {
            return;
        }
        let tc = self.tile_cols.max(1);
        let keys: Vec<(i32, i32)> = self.props_cache.keys().copied().collect();
        let mut moves: Vec<(i32, i32, i32, f32)> = Vec::new();
        for (hx, hy) in keys {
            let Some(props) = self.props_cache.get(&(hx, hy)).copied() else {
                continue;
            };
            let watery_surface = matches!(props.layer, TileLayer::Surface { watery: true });
            let fw = if props.free_water >= 0.5 {
                props.free_water
            } else if watery_surface {
                world
                    .map(|w| tile_free_water_frac(w, hx, hy, tc))
                    .unwrap_or(0.0)
            } else {
                props.free_water
            };
            if fw < 0.5 {
                continue;
            }
            let surf = *self.surf_cache.get(&hx).unwrap_or(&self.sea_level_y);
            let mid = hy * tc + tc / 2;
            // Top few free-water tiles under the skin.
            if mid + tc < surf - 3 * tc {
                continue;
            }
            let (mut vx, _) = wind.vector_at(world, hx, hy);
            if vx.abs() < 0.008 {
                vx = wind.climate_vx;
            }
            if vx.abs() < 0.008 {
                continue;
            }
            let dir = if vx >= 0.0 { 1 } else { -1 };
            let Some(up_hx) = self.wrap_hx(hx - dir) else {
                continue;
            };
            let Some(up) = self.props_cache.get(&(up_hx, hy)).copied() else {
                continue;
            };
            let up_fw = if up.free_water >= 0.5 {
                up.free_water
            } else if matches!(up.layer, TileLayer::Surface { watery: true }) {
                world
                    .map(|w| tile_free_water_frac(w, up_hx, hy, tc))
                    .unwrap_or(0.0)
            } else {
                up.free_water
            };
            if up_fw < 0.5 {
                continue;
            }
            let depth_falloff = if mid + tc / 2 >= surf { 1.0 } else { 0.55 };
            let a = ((vx.abs() / 0.05) * 0.22 * rate * depth_falloff).clamp(0.0, 0.55);
            if a < 1e-5 {
                continue;
            }
            moves.push((up_hx, hx, hy, a));
        }
        if moves.is_empty() {
            return;
        }
        // Snapshot sources so a chain does not see already-updated temps.
        let mut snap: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        for &(up_hx, hx, hy, _) in &moves {
            snap.entry((up_hx, hy))
                .or_insert_with(|| self.read_tile_temp(up_hx, hy, dense, bounds, slab_w));
            snap.entry((hx, hy))
                .or_insert_with(|| self.read_tile_temp(hx, hy, dense, bounds, slab_w));
        }
        for (up_hx, hx, hy, a) in moves {
            let t_up = *snap.get(&(up_hx, hy)).unwrap_or(&self.config.base_temp_c);
            let t0 = *snap.get(&(hx, hy)).unwrap_or(&self.config.base_temp_c);
            let n = t0 + (t_up - t0) * a;
            self.write_tile_temp(hx, hy, t0, n, dense, bounds, slab_w);
        }
    }

    #[inline]
    fn pore_couple_host(layer: TileLayer) -> bool {
        match layer {
            TileLayer::Buried { .. } => true,
            TileLayer::Surface { watery: false } => true,
            TileLayer::Surface { watery: true } | TileLayer::Air => false,
        }
    }

    #[inline]
    fn read_tile_temp(
        &self,
        hx: i32,
        hy: i32,
        dense: bool,
        bounds: Option<TileBounds>,
        slab_w: usize,
    ) -> f32 {
        if dense {
            if let Some(b) = bounds {
                if b.contains(hx, hy) && self.slab.len() == b.tile_capacity() {
                    return self.slab[b.index(slab_w, hx, hy)];
                }
            }
        }
        self.at_tile(hx, hy)
    }

    #[inline]
    fn write_tile_temp(
        &mut self,
        hx: i32,
        hy: i32,
        before: f32,
        after: f32,
        dense: bool,
        bounds: Option<TileBounds>,
        slab_w: usize,
    ) {
        if (after - before).abs() < 1e-5 {
            return;
        }
        self.cells.insert((hx, hy), after);
        if dense {
            if let Some(b) = bounds {
                if b.contains(hx, hy) && self.slab.len() == b.tile_capacity() {
                    self.slab[b.index(slab_w, hx, hy)] = after;
                }
            }
        }
    }

    /// Pairwise temperature diffusion. Vertical mix is gentle so night air
    /// cannot drain lakes, but warm bedrock still leaks heat upward.
    /// Pair conductivity is weighted by material `thermal_diffusivity`.
    pub fn diffuse(&mut self, alpha: f32) {
        let alpha = alpha.clamp(0.0, 0.25);
        if alpha == 0.0 || self.cells.is_empty() {
            return;
        }
        // A filled sky box is a dense rectangle. Pair walks on a slab,
        // write back only deltas — HashMap snap + insert of every tile
        // was leftover on 1000-cell columns already on the lapse.
        if let Some(b) = self.bounds {
            if self.is_dense_filled() {
                self.diffuse_dense(alpha, b);
                return;
            }
        }
        self.diffuse_sparse(alpha);
    }

    fn diffuse_sparse(&mut self, alpha: f32) {
        // Snapshot so mid-walk inserts do not disturb the read set.
        // Keys are unique — sort+dedup was leftover; +x/+y visits keep
        // pairs commutative.
        let snap: FxHashMap<(i32, i32), f32> = self.cells.iter().map(|(&k, &v)| (k, v)).collect();
        let sources: Vec<(i32, i32)> = snap.keys().copied().collect();
        let mut deltas: FxHashMap<(i32, i32), f32> = FxHashMap::default();
        let base = self.config.base_temp_c;
        let free_sky = |hx: i32, hy: i32| -> bool { self.tile_is_free_sky(hx, hy) };
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&base);
            let here_sky = free_sky(hx, hy);
            let k_here = self
                .props_cache
                .get(&(hx, hy))
                .map(|p| p.diffusivity)
                .unwrap_or(REF_THERMAL_DIFFUSIVITY);
            let fw_here = self
                .props_cache
                .get(&(hx, hy))
                .map(|p| p.free_water)
                .unwrap_or(0.0);
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) && nx != hx {
                    let n_val = *snap.get(&(nx, hy)).unwrap_or(&base);
                    // Same-height free sky is already on the lapse.
                    if here_sky && free_sky(nx, hy) && (val - n_val).abs() < 0.35 {
                        // skip
                    } else {
                        let props_n = self.props_cache.get(&(nx, hy));
                        let k_n = props_n
                            .map(|p| p.diffusivity)
                            .unwrap_or(REF_THERMAL_DIFFUSIVITY);
                        let fw_n = props_n.map(|p| p.free_water).unwrap_or(0.0);
                        let gate = diffuse_free_water_gate(fw_here, fw_n, false);
                        let flow =
                            (val - n_val) * alpha * pair_diff_scale(k_here, k_n) * gate;
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
                let props_n = self.props_cache.get(&n_key);
                let k_n = props_n
                    .map(|p| p.diffusivity)
                    .unwrap_or(REF_THERMAL_DIFFUSIVITY);
                let fw_n = props_n.map(|p| p.free_water).unwrap_or(0.0);
                let gate = diffuse_free_water_gate(fw_here, fw_n, true);
                // Mild vertical conductivity — geothermal path upward.
                // Free-water↔free-water gets a buoyancy-friendly boost.
                let vert = if fw_here >= 0.5 && fw_n >= 0.5 {
                    0.55
                } else {
                    0.35
                };
                let flow =
                    (val - n_val) * alpha * vert * pair_diff_scale(k_here, k_n) * gate;
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

    fn tile_is_free_sky(&self, hx: i32, hy: i32) -> bool {
        let surf = if let Some(b) = self.bounds {
            let col = (hx - b.hx_min) as usize;
            if col < self.surf_col.len() {
                self.surf_col[col]
            } else {
                self.surf_cache
                    .get(&hx)
                    .copied()
                    .unwrap_or(self.sea_level_y)
            }
        } else {
            self.surf_cache
                .get(&hx)
                .copied()
                .unwrap_or(self.sea_level_y)
        };
        hy * self.tile_cols.max(1) + self.tile_cols.max(1) / 2 > surf + 16
    }

    fn diffuse_dense(&mut self, alpha: f32, b: TileBounds) {
        self.pack_slab(b);
        self.diffuse_slab(alpha, b);
    }

    /// Pair stencil on [`Self::slab`]. Caller packed (or couple already
    /// wrote) the slab. Write back only deltas.
    fn diffuse_slab(&mut self, alpha: f32, b: TileBounds) {
        let (w, h) = b.dims();
        let n = w.saturating_mul(h);
        if w == 0 || h == 0 || self.slab.len() != n {
            return;
        }
        if self.slab_deltas.len() != n {
            self.slab_deltas.resize(n, 0.0);
        }
        self.slab_deltas.fill(0.0);
        let base = self.config.base_temp_c;
        let mut any = false;
        for hy in b.hy_min..=b.hy_max {
            for hx in b.hx_min..=b.hx_max {
                let i = b.index(w, hx, hy);
                let val = self.slab[i];
                let here_sky = self.tile_is_free_sky(hx, hy);
                let k_here = self
                    .props_cache
                    .get(&(hx, hy))
                    .map(|p| p.diffusivity)
                    .unwrap_or(REF_THERMAL_DIFFUSIVITY);
                let fw_here = self
                    .props_cache
                    .get(&(hx, hy))
                    .map(|p| p.free_water)
                    .unwrap_or(0.0);
                if let Some(nx) = self.wrap_hx(hx + 1) {
                    if b.contains(nx, hy) && nx != hx {
                        let ni = b.index(w, nx, hy);
                        let n_val = self.slab[ni];
                        if here_sky && self.tile_is_free_sky(nx, hy) && (val - n_val).abs() < 0.35 {
                            // skip
                        } else {
                            let props_n = self.props_cache.get(&(nx, hy));
                            let k_n = props_n
                                .map(|p| p.diffusivity)
                                .unwrap_or(REF_THERMAL_DIFFUSIVITY);
                            let fw_n = props_n.map(|p| p.free_water).unwrap_or(0.0);
                            let gate = diffuse_free_water_gate(fw_here, fw_n, false);
                            let flow =
                                (val - n_val) * alpha * pair_diff_scale(k_here, k_n) * gate;
                            if flow.abs() >= 1e-9 {
                                self.slab_deltas[i] -= flow;
                                self.slab_deltas[ni] += flow;
                                any = true;
                            }
                        }
                    }
                }
                let n_hy = hy + 1;
                if b.contains(hx, n_hy) {
                    if here_sky && self.tile_is_free_sky(hx, n_hy) {
                        continue;
                    }
                    let ni = b.index(w, hx, n_hy);
                    let n_val = self.slab[ni];
                    let props_n = self.props_cache.get(&(hx, n_hy));
                    let k_n = props_n
                        .map(|p| p.diffusivity)
                        .unwrap_or(REF_THERMAL_DIFFUSIVITY);
                    let fw_n = props_n.map(|p| p.free_water).unwrap_or(0.0);
                    let gate = diffuse_free_water_gate(fw_here, fw_n, true);
                    let vert = if fw_here >= 0.5 && fw_n >= 0.5 {
                        0.55
                    } else {
                        0.35
                    };
                    let flow =
                        (val - n_val) * alpha * vert * pair_diff_scale(k_here, k_n) * gate;
                    if flow.abs() >= 1e-9 {
                        self.slab_deltas[i] -= flow;
                        self.slab_deltas[ni] += flow;
                        any = true;
                    }
                }
            }
        }
        if !any {
            return;
        }
        for hy in b.hy_min..=b.hy_max {
            for hx in b.hx_min..=b.hx_max {
                let i = b.index(w, hx, hy);
                let d = self.slab_deltas[i];
                if d.abs() >= 1e-9 {
                    self.slab[i] += d;
                    *self.cells.entry((hx, hy)).or_insert(base) += d;
                }
            }
        }
    }
}

/// Vapor heat-capacity scale for an air tile (1 = dry).
///
/// Water vapor's specific heat is about 1.9× dry air. `scale` is the
/// Tab knob; 1.0 reaches that ratio at saturation.
fn humid_air_capacity_scale(humidity: &Humidity, hx: i32, hy: i32, temp_c: f32, scale: f32) -> f32 {
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

fn air_thermal() -> TileThermal {
    let air = MaterialRegistry::props(MaterialId::Air);
    TileThermal {
        layer: TileLayer::Air,
        capacity: air.heat_capacity,
        albedo: air.albedo,
        diffusivity: air.thermal_diffusivity,
        pore_wet: 0.0,
        free_water: 0.0,
    }
}

fn buried_thermal(depth_cells: f32) -> TileThermal {
    let bedrock = MaterialRegistry::props(MaterialId::Bedrock);
    TileThermal {
        layer: TileLayer::Buried { depth_cells },
        capacity: bedrock.heat_capacity * 1.25,
        albedo: 0.0,
        diffusivity: bedrock.thermal_diffusivity,
        pore_wet: 0.0,
        free_water: 0.0,
    }
}

/// Fraction of tile cells that are free standing water (Water or full wet Air).
fn tile_free_water_frac(world: &World, hx: i32, hy: i32, tile_cols: i32) -> f32 {
    let tc = tile_cols.max(1);
    let x0 = hx * tc;
    let y0 = hy * tc;
    let mut wet = 0.0f32;
    let n = (tc * tc) as f32;
    for ly in 0..tc {
        for lx in 0..tc {
            let gx = world.wrap_x(x0 + lx);
            let gy = y0 + ly;
            let Some(cell) = world.get_cell(gx, gy) else {
                continue;
            };
            if cell.material == MaterialId::Water
                || (cell.material == MaterialId::Air && cell.sat.0 >= 200)
            {
                wet += 1.0;
            }
        }
    }
    (wet / n.max(1.0)).clamp(0.0, 1.0)
}

/// Fraction of a tile that is Ice/Snow (lake pack / bed ice).
fn tile_ice_frac(world: &World, hx: i32, hy: i32, tile_cols: i32) -> f32 {
    let tc = tile_cols.max(1);
    let x0 = hx * tc;
    let y0 = hy * tc;
    let mut icy = 0.0f32;
    let n = (tc * tc) as f32;
    for ly in 0..tc {
        for lx in 0..tc {
            let gx = world.wrap_x(x0 + lx);
            let gy = y0 + ly;
            let Some(cell) = world.get_cell(gx, gy) else {
                continue;
            };
            if matches!(cell.material, MaterialId::Ice | MaterialId::Snow) {
                icy += 1.0;
            }
        }
    }
    (icy / n.max(1.0)).clamp(0.0, 1.0)
}

/// Mean pore wetness (`sat / capacity`) over porous solids in a tile.
fn tile_pore_wet_frac(world: &World, hx: i32, hy: i32, tile_cols: i32) -> f32 {
    let tc = tile_cols.max(1);
    let x0 = hx * tc;
    let y0 = hy * tc;
    let mut wet_sum = 0.0f32;
    let mut n = 0.0f32;
    for ly in 0..tc {
        for lx in 0..tc {
            let gx = world.wrap_x(x0 + lx);
            let gy = y0 + ly;
            let Some(cell) = world.get_cell(gx, gy) else {
                continue;
            };
            // Free water / air / ice are not pore-host media (T1 / T3 cover those).
            if matches!(
                cell.material,
                MaterialId::Air | MaterialId::Water | MaterialId::Ice | MaterialId::Snow
            ) {
                continue;
            }
            let cap = crate::cell::water_capacity(cell.material);
            if cap == 0 {
                continue;
            }
            wet_sum += (cell.sat.0 as f32) / (cap as f32);
            n += 1.0;
        }
    }
    if n < 1.0 {
        0.0
    } else {
        (wet_sum / n).clamp(0.0, 1.0)
    }
}

/// Same early-out as [`tile_thermal_props`]: far-sky Air and deep
/// crust Buried. `None` means the surface band still needs a scan.
fn props_early_from_anchor(
    hy: i32,
    tile_cols: i32,
    rock_mid: i32,
    anchor_lo: i32,
    anchor_hi: i32,
) -> Option<TileThermal> {
    let tc = tile_cols.max(1);
    let tile_mid_y = hy * tc + tc / 2;
    if tile_mid_y > anchor_hi + PROPS_AIR_MARGIN {
        return Some(air_thermal());
    }
    if tile_mid_y + tc < anchor_lo - PROPS_BURIED_MARGIN {
        let depth = (rock_mid - tile_mid_y).max(0) as f32;
        return Some(buried_thermal(depth));
    }
    None
}

fn tile_thermal_props(temp: &Temperature, world: Option<&World>, hx: i32, hy: i32) -> TileThermal {
    let tc = temp.tile_cols.max(1);
    let tile_mid_y = hy * tc + tc / 2;
    let Some(world) = world else {
        return air_thermal();
    };
    // Rock estimate at tile centre. Anchor the cheap band to both rock
    // and sea level so painted lakes/snow at sea still sit in-scan when
    // the live surface differs (unit fixtures + flat shelves).
    let (rock_mid, anchor_lo, anchor_hi) = temp.column_rock_anchors(world, hx);
    if let Some(early) = props_early_from_anchor(hy, tc, rock_mid, anchor_lo, anchor_hi) {
        return early;
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
    let mut diff_sum = 0.0;
    let mut surf_sum = 0.0;
    let mut water_cols = 0.0;
    let mut n = 0.0;
    for lx in 0..tc {
        let gx = world.wrap_x(hx * tc + lx);
        let rock = live_surface_at(world, temp.seed, gx, temp.sea_level_y, temp.width_cols);
        let col_lo = (rock.min(temp.sea_level_y) - 8).max(y_lo);
        let col_hi = (rock.max(temp.sea_level_y) + 64).min(y_hi);
        let (surf_y, cap, albedo, watery, diff) =
            column_surface_thermal(world, gx, col_lo, col_hi, rock, &temp.config);
        cap_sum += cap;
        alb_sum += albedo;
        diff_sum += diff;
        surf_sum += surf_y as f32;
        if watery {
            water_cols += 1.0;
        }
        n += 1.0;
    }
    if n < 1.0 {
        return air_thermal();
    }
    let surf_y = (surf_sum / n).round() as i32;
    let cap = cap_sum / n;
    let albedo = alb_sum / n;
    let diffusivity = diff_sum / n;
    let watery = water_cols / n >= 0.5;

    if tile_mid_y > surf_y + tc {
        return air_thermal();
    }
    if tile_mid_y + tc < surf_y {
        let depth = (surf_y - tile_mid_y).max(0) as f32;
        let free_water = tile_free_water_frac(world, hx, hy, tc);
        let mut props = buried_thermal(depth);
        props.free_water = free_water;
        if free_water >= 0.5 {
            // Deep lake / flooded shaft — water thermal mass, not rock geo.
            let water = MaterialRegistry::props(MaterialId::Water);
            props.capacity = water.heat_capacity
                * (1.0 + temp.config.water_stack_cap * free_water);
            props.diffusivity = water.thermal_diffusivity;
            props.albedo = water.albedo;
        } else {
            props.pore_wet = tile_pore_wet_frac(world, hx, hy, tc);
            props.capacity *= 1.0 + 0.35 * props.pore_wet;
        }
        return props;
    }
    let pore_wet = tile_pore_wet_frac(world, hx, hy, tc);
    // Always scan this tile's cells — column `watery` is true for the
    // whole surface band above a lake, but empty air tiles must not
    // count as free-water for buoyancy.
    let free_water = tile_free_water_frac(world, hx, hy, tc);
    TileThermal {
        layer: TileLayer::Surface { watery },
        capacity: cap * (1.0 + 0.25 * pore_wet),
        albedo,
        diffusivity,
        pore_wet,
        free_water,
    }
}

/// Scan a column for the surface stack: pack / water / ground.
/// Returns `(surface_y, heat_capacity, albedo, is_watery, diffusivity)`.
fn column_surface_thermal(
    world: &World,
    gx: i32,
    y_lo: i32,
    y_hi: i32,
    fallback_y: i32,
    cfg: &TempConfig,
) -> (i32, f32, f32, bool, f32) {
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
    let watery = water_like > 0 || matches!(mat, MaterialId::Water | MaterialId::Ice);
    (
        top_y,
        cap,
        props.albedo,
        watery,
        props.thermal_diffusivity.max(1e-6),
    )
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
        let mut h =
            Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, p.width_cols, p.sky_ceiling_y);
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

    fn fill_tile_surface(
        w: &mut World,
        tile_x0: i32,
        y_ground: i32,
        mat: MaterialId,
        water_h: i32,
    ) {
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
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, p.width_cols, p.sky_ceiling_y);
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
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, p.width_cols, p.sky_ceiling_y);
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
        let h = Humidity::with_world_bounds(4, 0, p.bedrock_floor_y, p.width_cols, p.sky_ceiling_y);
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
        assert!(h.at_tile(2, 5) > 0.0, "vapour still has to actually lift");
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
            let s = crate::worldgen::continental_surface_y(p.seed, gx, p.sea_level_y, p.width_cols);
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
    fn lake_does_not_inherit_cliff_geothermal_bands() {
        // Same-Y rock↔water diffuse used to copy geo isotherms into the lake
        // so a cut hill "revealed" the water's stratification.
        let sea: i32 = 40;
        let bed: i32 = 8;
        let crest: i32 = 40;
        let mut world = World::new(5);
        for y in 0..=crest + 4 {
            world.ensure_chunk(ChunkCoord::new(
                0,
                y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
            ));
            world.ensure_chunk(ChunkCoord::new(
                1,
                y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
            ));
        }
        // Lake columns x=0..7, cliff x=8..15.
        for x in 0..8 {
            for y in 0..=bed {
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in (bed + 1)..=sea {
                world.set_cell(x, y, Cell::water());
            }
        }
        for x in 8..16 {
            for y in 0..=crest {
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 80, 1, 32, sea, false);
        t.fill_initial(0);
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.water_rock_couple = 0.0;
        t.config.air_water_skin_couple = 0.0;
        t.config.pore_water_couple = 0.0;
        t.config.water_convect_bias = 0.0;
        t.config.diffuse_alpha = 0.20;
        t.config.geothermal_relax = 0.25;
        t.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 80);
        // Mid-column sample: lake tile vs cliff tile at the same hy.
        let tc = t.tile_cols.max(1);
        let sample_y = (bed + sea) / 2;
        let hy = sample_y.div_euclid(tc);
        let lake_hx = 1i32; // x~4
        let cliff_hx = 3i32; // x~12
        for i in 0..20 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let lake_t = t.at_tile(lake_hx, hy);
        let cliff_t = t.at_tile(cliff_hx, hy);
        assert!(
            (cliff_t - lake_t).abs() > 4.0,
            "lake must not lock to cliff geo isotherm (lake={lake_t:.1} cliff={cliff_t:.1} hy={hy})"
        );
    }

    #[test]
    fn lake_water_column_is_not_rock_overburden() {
        // Standing water must not count as crust cover — otherwise deep
        // lakes paint a static hot-bottom geothermal profile.
        let sea: i32 = 80;
        let bed: i32 = 20;
        let water_top: i32 = 80;
        let probe_y: i32 = 16;
        let mut w = World::new(3);
        for y in 0..=water_top + 4 {
            w.ensure_chunk(ChunkCoord::new(
                0,
                y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
            ));
        }
        for x in 0..8 {
            for y in 0..=bed {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in (bed + 1)..=water_top {
                w.set_cell(x, y, Cell::water());
            }
        }
        let t = Temperature::with_world_bounds(4, 0, 0, 16, 160, 1, 16, sea, false);
        let rock_depth = t.geothermal_overburden_cells(Some(&w), 0, probe_y);
        let skin_depth = (water_top - probe_y) as f32;
        assert!(
            (rock_depth - (bed - probe_y) as f32).abs() < 4.0,
            "overburden must stop at the rock bed (got {rock_depth}, bed-rel {})",
            (bed - probe_y) as f32
        );
        assert!(
            rock_depth < skin_depth * 0.35,
            "water column must not inflate geothermal depth ({rock_depth} vs skin {skin_depth})"
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
        let mut t_rock = Temperature::with_world_bounds(
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

    #[test]
    fn dense_diffuse_matches_sparse_on_a_filled_box() {
        // Same pair stencil; the slab is leftover hasher, not new physics.
        let mut dense = Temperature::with_world_bounds(4, 0, 0, 16, 32, 1, 16, 8, false);
        dense.fill_initial(0);
        dense.cells.insert((1, 3), 40.0);
        dense.surf_cache.insert(1, 8);
        let mut sparse = dense.clone();
        dense.diffuse_dense(0.1, dense.bounds.expect("bounds"));
        sparse.diffuse_sparse(0.1);
        for (k, &a) in &dense.cells {
            let b = sparse.cells.get(k).copied().unwrap_or(f32::NAN);
            assert!(
                (a - b).abs() < 1e-5,
                "dense/sparse mismatch at {k:?}: {a} vs {b}"
            );
        }
        assert_eq!(dense.cells.len(), sparse.cells.len());
    }

    #[test]
    fn column_anchor_skips_match_per_tile_props_on_far_sky_and_deep_crust() {
        // Same Air / Buried early-out; one seed-rock walk per column
        // is leftover scan, not a new climate.
        let (world, mut t, _h, sea) = grounded_scene();
        let (rock, alo, ahi) = t.column_rock_anchors(&world, 0);
        let tc = t.tile_cols.max(1);
        // Compact scene ceiling is y=64 → hy_max=15. Mid of hy=12 is 50,
        // which is above sea+AIR_MARGIN (40).
        let far_hy = ((sea + PROPS_AIR_MARGIN + tc) / tc) + 2;
        let deep_hy = 0;
        assert!(
            far_hy <= t.bounds.expect("bounds").hy_max,
            "far_hy {far_hy} must sit in the compact box"
        );
        let far_early =
            props_early_from_anchor(far_hy, tc, rock, alo, ahi).expect("far sky should early-out");
        let far_full = tile_thermal_props(&t, Some(&world), 0, far_hy);
        let deep_full = tile_thermal_props(&t, Some(&world), 0, deep_hy);
        assert!(matches!(far_early.layer, TileLayer::Air));
        assert_eq!(far_early.layer, far_full.layer);
        t.refresh_props_cache(Some(&world), &[(0, far_hy), (0, deep_hy)]);
        assert_eq!(
            t.props_cache.get(&(0, far_hy)).map(|p| p.layer),
            Some(far_full.layer)
        );
        assert_eq!(
            t.props_cache.get(&(0, deep_hy)).map(|p| p.layer),
            Some(deep_full.layer)
        );
    }

    #[test]
    fn material_diffusivity_is_scanned_into_tile_props() {
        let (world, mut t, _h, sea) = grounded_scene();
        let tc = t.tile_cols.max(1);
        let surf_hy = (sea + tc / 2).div_euclid(tc);
        t.refresh_props_cache(Some(&world), &[(0, surf_hy), (0, 0)]);
        let surf = t.props_cache.get(&(0, surf_hy)).expect("surface props");
        let buried = t.props_cache.get(&(0, 0)).expect("buried props");
        assert!(
            surf.diffusivity > 0.0 && buried.diffusivity > 0.0,
            "κ must be scanned (surf={}, buried={})",
            surf.diffusivity,
            buried.diffusivity
        );
        assert!(
            (pair_diff_scale(0.004, 0.004) - pair_diff_scale(0.001, 0.001)).abs() > 0.2,
            "air-like κ pairs must conduct faster than rock-like pairs"
        );
    }

    #[test]
    fn cold_pond_cools_hot_rock_underneath() {
        // T1 acceptance: watery surface exchanges with buried/dry rock below.
        let sea = 16;
        let x0 = 4;
        let mut world = World::new(11);
        fill_tile_surface(&mut world, x0, sea, MaterialId::Water, 3);
        fill_buried_rock(&mut world, x0, sea, 12);
        // Standing water on stone (fill_tile_surface already set stone bed).
        for x in x0..x0 + 4 {
            for y in 1..=3 {
                world.set_cell(x, sea + y, Cell::water());
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t.fill_initial(0);
        let tc = t.tile_cols.max(1);
        let water_hy = ((sea + 2) / tc).max(1);
        let rock_hy = water_hy - 1;
        let hx = x0.div_euclid(tc);
        for v in t.cells.values_mut() {
            *v = 20.0;
        }
        t.cells.insert((hx, water_hy), 5.0);
        t.cells.insert((hx, rock_hy), 80.0);
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.geothermal_relax = 0.0;
        t.config.geothermal_flux_c = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.water_rock_couple = 0.35;
        t.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        let rock0 = t.at_tile(hx, rock_hy);
        let water0 = t.at_tile(hx, water_hy);
        for i in 0..6 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let rock1 = t.at_tile(hx, rock_hy);
        let water1 = t.at_tile(hx, water_hy);
        assert!(
            rock1 < rock0 - 2.0,
            "cold pond must cool hot rock ({rock0:.1} → {rock1:.1})"
        );
        assert!(
            water1 > water0 + 2.0,
            "hot rock must warm the pond ({water0:.1} → {water1:.1})"
        );
    }

    #[test]
    fn cold_air_cools_warm_water_skin() {
        // T3 acceptance: open water under cold air loses heat vs insulated control.
        let sea = 16;
        let x0 = 4;
        let mut world = World::new(11);
        fill_tile_surface(&mut world, x0, sea, MaterialId::Water, 3);
        fill_buried_rock(&mut world, x0, sea, 12);
        for x in x0..x0 + 4 {
            for y in 1..=3 {
                world.set_cell(x, sea + y, Cell::water());
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t.fill_initial(0);
        let tc = t.tile_cols.max(1);
        let water_hy = ((sea + 2) / tc).max(1);
        let hx = x0.div_euclid(tc);
        for v in t.cells.values_mut() {
            *v = 20.0;
        }
        t.cells.insert((hx, water_hy), 35.0);
        // Cold free-air band above the surface window (hy+1 may still be Surface).
        for d in 1..=4 {
            t.cells.insert((hx, water_hy + d), 0.0);
        }
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.geothermal_relax = 0.0;
        t.config.geothermal_flux_c = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.water_rock_couple = 0.0;
        t.config.air_water_skin_couple = 0.4;
        t.config.water_convect_bias = 0.0;
        t.config.pore_water_couple = 0.0;
        t.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        let water0 = t.at_tile(hx, water_hy);
        for i in 0..6 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let water1 = t.at_tile(hx, water_hy);
        assert!(
            water1 < water0 - 1.5,
            "cold air must cool warm water skin ({water0:.1} → {water1:.1})"
        );
        // At least one free-air tile above should have warmed.
        let air_warmed = (1..=4).any(|d| t.at_tile(hx, water_hy + d) > 0.5);
        assert!(
            air_warmed,
            "warm water must warm the free-air band above the skin"
        );

        // Control: couple off → water stays warm.
        let mut t_off = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t_off.fill_initial(0);
        for v in t_off.cells.values_mut() {
            *v = 20.0;
        }
        t_off.cells.insert((hx, water_hy), 35.0);
        for d in 1..=4 {
            t_off.cells.insert((hx, water_hy + d), 0.0);
        }
        t_off.config = t.config.clone();
        t_off.config.air_water_skin_couple = 0.0;
        t_off.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        for i in 0..6 {
            t_off.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let water_off = t_off.at_tile(hx, water_hy);
        assert!(
            (water_off - 35.0).abs() < 0.5,
            "insulated control must keep warm skin ({water_off:.1})"
        );
    }

    #[test]
    fn cold_wet_pores_cool_hot_rock_neighbour() {
        // T4 acceptance: wet sand above hot dry rock exchanges at pore rate.
        let sea = 16;
        let x0 = 4;
        let mut world = World::new(11);
        fill_tile_surface(&mut world, x0, sea, MaterialId::Sand, 0);
        fill_buried_rock(&mut world, x0, sea, 12);
        let tc = 4;
        let wet_hy = ((sea - 2) / tc).max(1); // just under surface → buried band
        let rock_hy = wet_hy - 1;
        let y_wet0 = wet_hy * tc;
        let y_rock0 = rock_hy * tc;
        for x in x0..x0 + 4 {
            for y in y_wet0..y_wet0 + tc {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = crate::cell::Sat::FULL;
                sand.pore = 200;
                world.set_cell(x, y, sand);
            }
            for y in y_rock0..y_rock0 + tc {
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t.fill_initial(0);
        let hx = x0.div_euclid(tc);
        for v in t.cells.values_mut() {
            *v = 20.0;
        }
        t.cells.insert((hx, wet_hy), 5.0);
        t.cells.insert((hx, rock_hy), 80.0);
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.geothermal_relax = 0.0;
        t.config.geothermal_flux_c = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.water_rock_couple = 0.0;
        t.config.air_water_skin_couple = 0.0;
        t.config.pore_water_couple = 0.4;
        t.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        let rock0 = t.at_tile(hx, rock_hy);
        let wet0 = t.at_tile(hx, wet_hy);
        for i in 0..8 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let rock1 = t.at_tile(hx, rock_hy);
        let wet1 = t.at_tile(hx, wet_hy);

        // Control: pore couple off (geo floor can still drift slightly).
        let mut t_off = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t_off.fill_initial(0);
        for v in t_off.cells.values_mut() {
            *v = 20.0;
        }
        t_off.cells.insert((hx, wet_hy), 5.0);
        t_off.cells.insert((hx, rock_hy), 80.0);
        t_off.config = t.config.clone();
        t_off.config.pore_water_couple = 0.0;
        t_off.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        for i in 0..8 {
            t_off.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let rock_off = t_off.at_tile(hx, rock_hy);
        let wet_off = t_off.at_tile(hx, wet_hy);
        assert!(
            rock1 < rock_off - 1.5,
            "wet pores must cool hot rock more than control (on={rock1:.1} off={rock_off:.1}; start={rock0:.1})"
        );
        assert!(
            wet1 > wet_off + 1.5,
            "hot rock must warm wet pores more than control (on={wet1:.1} off={wet_off:.1}; start={wet0:.1})"
        );
    }

    #[test]
    fn unstable_lake_column_mixes_heat_upward() {
        // Open-lake convection: warm bottom under cold top must mix when
        // water_convect_bias > 0 (full columns never move cells).
        let sea = 24;
        let x0 = 4;
        let mut world = World::new(11);
        fill_tile_surface(&mut world, x0, sea, MaterialId::Water, 0);
        fill_buried_rock(&mut world, x0, sea, 20);
        // Tall free-water column from y=4..sea.
        for x in x0..x0 + 4 {
            for y in 4..=sea {
                world.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
                world.set_cell(x, y, Cell::water());
            }
        }
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t.fill_initial(0);
        let tc = t.tile_cols.max(1);
        let bot_hy = 8i32 / tc; // y~8–11
        let top_hy = 20i32 / tc; // y~20–23, still under sea=24 surface
        let hx = x0.div_euclid(tc);
        assert!(
            top_hy > bot_hy,
            "fixture: need stacked free-water tiles ({bot_hy}..{top_hy})"
        );
        for v in t.cells.values_mut() {
            *v = 10.0;
        }
        t.cells.insert((hx, bot_hy), 40.0);
        t.cells.insert((hx, top_hy), 2.0);
        // Seed intermediates cold so buoyancy must climb the stack.
        for hy in (bot_hy + 1)..top_hy {
            t.cells.insert((hx, hy), 5.0);
        }
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.geothermal_relax = 0.0;
        t.config.geothermal_flux_c = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.water_rock_couple = 0.0;
        t.config.air_water_skin_couple = 0.0;
        t.config.pore_water_couple = 0.0;
        t.config.water_convect_bias = 1.0;
        t.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        for i in 0..10 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let bot1 = t.at_tile(hx, bot_hy);
        let top1 = t.at_tile(hx, top_hy);

        let mut t_off = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t_off.fill_initial(0);
        for v in t_off.cells.values_mut() {
            *v = 10.0;
        }
        t_off.cells.insert((hx, bot_hy), 40.0);
        t_off.cells.insert((hx, top_hy), 2.0);
        for hy in (bot_hy + 1)..top_hy {
            t_off.cells.insert((hx, hy), 5.0);
        }
        t_off.config = t.config.clone();
        t_off.config.water_convect_bias = 0.0;
        t_off.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        for i in 0..10 {
            t_off.step(Some(&world), &h, i * TEMP_STEP_PERIOD, None);
        }
        let bot_off = t_off.at_tile(hx, bot_hy);
        let top_off = t_off.at_tile(hx, top_hy);
        let gap_on = bot1 - top1;
        let gap_off = bot_off - top_off;
        assert!(
            gap_on < gap_off - 5.0,
            "unstable lake must shrink ΔT vs bias-off (on={gap_on:.1} off={gap_off:.1}; bot={bot1:.1}/{bot_off:.1} top={top1:.1}/{top_off:.1})"
        );
        assert!(
            top1 > top_off + 2.0,
            "heat must reach the cold lake top (on={top1:.1} off={top_off:.1})"
        );
    }

    #[test]
    fn lake_skin_wind_drifts_heat_downwind() {
        use crate::wind::Wind;

        let sea = 24;
        let mut world = World::new(11);
        // Two adjacent free-water tiles at hy=5 (y 20..23), air above.
        for x in 0i32..12 {
            for y in 0i32..=20 {
                world.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
            }
            for y in 0i32..=16 {
                world.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 17i32..=23 {
                world.set_cell(x, y, Cell::water());
            }
        }
        // Sparse field — only the skin row we care about.
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, sea, false);
        t.cells.clear();
        let skin_hy = 5;
        t.cells.insert((1, skin_hy), 30.0);
        t.cells.insert((2, skin_hy), 10.0);
        t.cells.insert((1, skin_hy - 1), 10.0);
        t.cells.insert((2, skin_hy - 1), 10.0);
        t.config.solar_heat_c = 0.0;
        t.config.night_cool_c = 0.0;
        t.config.diffuse_alpha = 0.0;
        t.config.sky_relax = 0.0;
        t.config.min_relax = 0.0;
        t.config.geothermal_relax = 0.0;
        t.config.geothermal_flux_c = 0.0;
        t.config.near_surface_couple = 0.0;
        t.config.water_rock_couple = 0.0;
        t.config.air_water_skin_couple = 0.0;
        t.config.pore_water_couple = 0.0;
        t.config.water_convect_bias = 0.0;
        t.config.water_wind_drift = 1.0;
        t.props_cache_age = TEMP_PROPS_REFRESH_STEPS;
        let h = Humidity::with_world_bounds(4, 0, 0, 32, 64);
        let mut wind = Wind::climate(4, 0.20, 11, 32, sea, 0, 64, false);
        wind.variance = 0.0;
        let down0 = t.at_tile(2, skin_hy);
        for i in 0..6 {
            t.step(Some(&world), &h, i * TEMP_STEP_PERIOD, Some(&wind));
        }
        let down1 = t.at_tile(2, skin_hy);
        assert!(
            down1 > down0 + 1.0,
            "downwind skin must warm from wind drift ({down0:.1} → {down1:.1})"
        );
    }

    #[test]
    fn water_convect_scales_prefer_warm_up_and_throttle_warm_over_cold() {
        let mut t = Temperature::with_world_bounds(4, 0, 0, 32, 64, 1, 32, 16, false);
        t.fill_initial(0);
        t.config.water_convect_bias = 1.0;
        // Tile (0,0) warm, (0,4) cold — cell coords map through tile_cols=4.
        for v in t.cells.values_mut() {
            *v = 10.0;
        }
        t.cells.insert((0, 0), 40.0); // low tile warm
        t.cells.insert((0, 4), 0.0); // high tile cold
        let rise_warm_below = water_convect_rise_scale(&t, 1, 1, 1, 17);
        let rise_cold_below = water_convect_rise_scale(&t, 1, 17, 1, 1);
        assert!(
            rise_warm_below > 1.0 && rise_cold_below < 1.0,
            "warm donor should boost rise ({rise_warm_below:.2}), cold donor throttle ({rise_cold_below:.2})"
        );
        let fall_stable = water_convect_fall_scale(&t, 1, 17, 1, 1); // cold above? wait 17 is hy=4 cold, 1 is hy=0 warm — cold above warm is unstable
        // Warm above cold: put warm at high cell
        t.cells.insert((0, 4), 40.0);
        t.cells.insert((0, 0), 0.0);
        let fall_warm_over_cold = water_convect_fall_scale(&t, 1, 17, 1, 1);
        assert!(
            fall_warm_over_cold < 1.0,
            "warm-over-cold must throttle gravity fall ({fall_warm_over_cold:.2})"
        );
        let _ = fall_stable;
    }
}
