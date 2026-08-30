//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cloud **visuals** derived from the humidity field.
//!
//! Atmospheric water lives on [`Humidity`] tiles. Rain is condensation /
//! dew from that field. [`CloudStore`] parcels are a bounded display
//! echo (N banks, shade, streaks) rebuilt each step from the wettest
//! sky tiles — not a second water store and not a rain engine.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::grid::World;
use crate::humidity::Humidity;
use crate::phase::PhaseConfig;
use crate::temperature::Temperature;
use crate::wind::Wind;
use crate::worldgen::{airborne_loose_at, live_surface_at};

/// Soft cap so cartoon skies stay readable (default for [`CloudConfig`]).
pub const MAX_CLOUD_PARCELS: usize = 36;
/// Default visual wetness scale (draw / shade, not a rain trigger).
pub const DOWNPOUR_MASS: f32 = 200.0;
/// Save-compat ceiling (parcels no longer store water).
pub const MAX_CLOUD_TOTAL_MASS: f32 = 80_000.0;
/// Save-compat per-parcel ceiling.
pub const MAX_CLOUD_PARCEL_MASS: f32 = 8_000.0;

/// Live-tunable visual-cloud knobs.
///
/// Rain is condensation / dew from [`Humidity`]. Parcel fields that used
/// to drive coagulate/downpour are kept for save compatibility and for
/// draw wetness (`downpour_mass`).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CloudConfig {
    pub max_parcels: usize,
    /// Unused by physics (save compat).
    #[serde(default = "default_max_total_mass")]
    pub max_total_mass: f32,
    /// Unused by physics (save compat).
    #[serde(default = "default_max_parcel_mass")]
    pub max_parcel_mass: f32,
    /// Skip sky tiles drier than this when picking visual echoes.
    pub coag_min_hum: f32,
    /// Unused by physics (save compat).
    pub coag_rate: f32,
    /// Unused by physics (save compat).
    pub coag_max_take: f32,
    /// Unused by physics (save compat).
    pub spawn_radius: f32,
    /// Unused by physics (save compat).
    pub merge_dist: f32,
    /// Draw / shade wetness scale (not a rain trigger).
    pub downpour_mass: f32,
    /// Unused by physics (save compat).
    pub downpour_drain: f32,
    /// Unused by physics (save compat).
    pub downpour_stop_frac: f32,
    pub cloud_alt_above_sea: i32,
    pub coag_min_above_sea: i32,
    pub ridge_clearance: f32,
    pub parcel_wind_scale: f32,
    pub buoyant_rise: f32,
    /// Unused by physics (save compat).
    #[serde(default = "default_snow_footprint_mult")]
    pub snow_footprint_mult: f32,
    /// Column fan width multiplier for liquid rain. Default `1.25`.
    #[serde(default = "default_rain_footprint_mult")]
    pub rain_footprint_mult: f32,
    /// Landing span multiplier (× parcel radius) when snowing. Default `1.35`.
    #[serde(default = "default_snow_span_mult")]
    pub snow_span_mult: f32,
    /// Landing span multiplier for liquid rain. Default `0.85`.
    #[serde(default = "default_rain_span_mult")]
    pub rain_span_mult: f32,
    /// Max full Snow cells seated per parcel per tick (retry path).
    #[serde(default = "default_snow_cells_per_tick")]
    pub snow_cells_per_tick: u8,
    /// Max full-cell precip retries per parcel per tick when raining liquid.
    #[serde(default = "default_rain_cells_per_tick")]
    pub rain_cells_per_tick: u8,
}

fn default_max_total_mass() -> f32 {
    MAX_CLOUD_TOTAL_MASS
}
fn default_max_parcel_mass() -> f32 {
    MAX_CLOUD_PARCEL_MASS
}
fn default_snow_footprint_mult() -> f32 {
    2.2
}
fn default_rain_footprint_mult() -> f32 {
    1.25
}
fn default_snow_span_mult() -> f32 {
    1.35
}
fn default_rain_span_mult() -> f32 {
    0.85
}
fn default_snow_cells_per_tick() -> u8 {
    5
}
fn default_rain_cells_per_tick() -> u8 {
    2
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            max_parcels: MAX_CLOUD_PARCELS,
            max_total_mass: default_max_total_mass(),
            max_parcel_mass: default_max_parcel_mass(),
            coag_min_hum: 36.0,
            coag_rate: 0.04,
            coag_max_take: 14.0,
            spawn_radius: 22.0,
            merge_dist: 12.0,
            downpour_mass: DOWNPOUR_MASS,
            downpour_drain: 28.0,
            downpour_stop_frac: 0.40,
            cloud_alt_above_sea: 40,
            coag_min_above_sea: 18,
            ridge_clearance: 12.0,
            parcel_wind_scale: 0.28,
            buoyant_rise: 0.08,
            snow_footprint_mult: default_snow_footprint_mult(),
            rain_footprint_mult: default_rain_footprint_mult(),
            snow_span_mult: default_snow_span_mult(),
            rain_span_mult: default_rain_span_mult(),
            snow_cells_per_tick: default_snow_cells_per_tick(),
            rain_cells_per_tick: default_rain_cells_per_tick(),
        }
    }
}

/// One wind-blown cloud blob in continuous world space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudParcel {
    pub fx: f32,
    pub fy: f32,
    pub mass: f32,
    /// True while actively dumping rain this storm pulse.
    pub raining: bool,
    /// True this tick after gently colliding with a ridge / peak.
    #[serde(default)]
    pub on_ridge: bool,
    /// Stable shape RNG seed (set at spawn; survives merges via keep-left).
    #[serde(default)]
    pub shape_seed: u32,
    /// Cruise altitude after orographic lift — parcels keep this path
    /// instead of dropping straight back to the free-air deck.
    #[serde(default)]
    pub cruise_fy: f32,
    /// EMA of mass for drawing so size/shade don't pulse every tick.
    #[serde(default)]
    pub vis_mass: f32,
    /// 0..1 how hard the parcel is currently pressing a ridge (draw squash).
    #[serde(default)]
    pub deform: f32,
}

impl CloudParcel {
    /// Visual / rain footprint radius in world cells (uses smoothed mass).
    pub fn radius(&self) -> f32 {
        let m = if self.vis_mass > 1.0 {
            self.vis_mass
        } else {
            self.mass
        };
        // Mostly stable size; mass only nudges gently.
        let base = 8.0 + ((self.shape_seed % 7) as f32) * 0.55;
        (base + (m / 90.0).sqrt() * 2.2).clamp(7.0, 20.0)
    }

    /// 0..1 wetness for drawing (relative to downpour threshold).
    pub fn wetness(&self) -> f32 {
        self.wetness_with(DOWNPOUR_MASS)
    }

    pub fn wetness_with(&self, downpour_mass: f32) -> f32 {
        let m = if self.vis_mass > 1.0 {
            self.vis_mass
        } else {
            self.mass
        };
        (m / downpour_mass.max(1.0)).clamp(0.0, 1.5) / 1.5
    }

    /// Smooth visual mass toward physics mass (call once per tick).
    pub fn smooth_visuals(&mut self) {
        if self.vis_mass <= 0.0 {
            self.vis_mass = self.mass;
        } else {
            self.vis_mass = self.vis_mass * 0.94 + self.mass * 0.06;
        }
        self.deform *= 0.85;
    }
}

/// Bounded visual echo of wet humidity tiles.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudStore {
    pub parcels: Vec<CloudParcel>,
}

impl CloudStore {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn len(&self) -> usize {
        self.parcels.len()
    }

    pub fn is_empty(&self) -> bool {
        self.parcels.is_empty()
    }

    pub fn total_mass(&self) -> f32 {
        self.parcels.iter().map(|p| p.mass).sum()
    }

    /// Display mass for HUD / N banks. Visual echoes keep this in
    /// `vis_mass` so leftover `mass` is only real water from old saves.
    pub fn visual_mass(&self) -> f32 {
        self.parcels
            .iter()
            .map(|p| if p.vis_mass > 0.0 { p.vis_mass } else { p.mass })
            .sum()
    }

    /// Atmosphere step: return any leftover parcel mass to humidity,
    /// lift vapor, rebuild a small visual echo. Does **not** rain.
    pub fn step(
        &mut self,
        world: &mut World,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
        tick: u64,
        cfg: &CloudConfig,
    ) {
        self.step_with_precip(
            world,
            humidity,
            wind,
            sea_level_y,
            sky_ceiling_y,
            tick,
            cfg,
            None,
            None,
        );
    }

    /// Like [`Self::step`]. `temp` scales thermal rise and visual wetness;
    /// `phase` is unused (rain is condensation).
    pub fn step_with_precip(
        &mut self,
        world: &mut World,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
        tick: u64,
        cfg: &CloudConfig,
        temp: Option<&Temperature>,
        phase: Option<&PhaseConfig>,
    ) {
        let _ = (sky_ceiling_y, tick, phase);
        self.release_parcels_into_humidity(humidity);
        // Rise toward the lifting condensation level — not a hard shelf at
        // `sea + cloud_alt`. That shelf emptied the water–air interface and
        // pinned rain to one altitude (playtest ~y225 when alt was large).
        humidity.buoyant_rise_weather(
            cfg.buoyant_rise,
            sea_level_y,
            cfg.cloud_alt_above_sea,
            temp,
        );
        self.rebuild_visuals_from_humidity(humidity, world, wind, sea_level_y, cfg, temp);
    }

    /// Move blob mass back onto humidity tiles (old saves / leftover coag).
    ///
    /// Only parcels that actually **hold mass** are released. Current parcels are
    /// pure visuals with `mass == 0.0`, and taking the whole list would clear the
    /// deck every tick — which is what parcel persistence needs not to happen, and
    /// what silently made an earlier attempt at it a no-op.
    pub fn release_parcels_into_humidity(&mut self, humidity: &mut Humidity) {
        if self.parcels.is_empty() {
            return;
        }
        let mut carried: Vec<CloudParcel> = Vec::new();
        for p in std::mem::take(&mut self.parcels) {
            if p.mass > 1e-3 {
                humidity.add(p.fx.round() as i32, p.fy.round() as i32, p.mass);
            } else {
                carried.push(p);
            }
        }
        self.parcels = carried;
    }

    /// Advance the visual cloud deck: drift, grow, lift, dissipate, spawn.
    ///
    /// Parcels **persist across ticks**. They used to be cleared and rebuilt
    /// from whatever the wettest tiles happened to be, which is why the deck
    /// jittered: a parcel had no identity, so it could not drift, build or
    /// clear, only blink to a new position. `vis_mass` was already documented as
    /// an EMA "so size/shade don't pulse every tick" and never got to be one,
    /// because the parcel holding it was replaced every tick.
    ///
    /// Parcels stay pure visuals — `mass` is always 0.0. They are an echo of the
    /// humidity field, never a second store of it; giving them real mass would
    /// let `release_parcels_into_humidity` pour a copy back into the sky and
    /// mint vapour.
    pub fn rebuild_visuals_from_humidity(
        &mut self,
        humidity: &Humidity,
        world: &World,
        wind: &Wind,
        sea_level_y: i32,
        cfg: &CloudConfig,
        temp: Option<&Temperature>,
    ) {
        let tc = humidity.tile_cols.max(1);
        let cap = cfg.max_parcels.max(1);
        let width = wind.width_cols.max(1);
        let sat_at = |hx: i32, hy: i32| -> f32 {
            temp.map(|t| Humidity::saturation_mass_at_temp(t.at_tile(hx, hy)))
                .unwrap_or(Humidity::MAX_MASS_PER_TILE)
                .max(1.0)
        };

        // --- advance what is already up there ---
        for p in &mut self.parcels {
            // Drift downwind. This is what makes a deck move across the sky
            // instead of re-materialising each tick.
            p.fx += wind.climate_vx * CLOUD_DRIFT_SCALE;
            if p.fx < 0.0 {
                p.fx += width as f32;
            } else if p.fx >= width as f32 {
                p.fx -= width as f32;
            }
            let hx = (p.fx.round() as i32).div_euclid(tc);
            let hy = (p.fy.round() as i32).div_euclid(tc);
            let local = humidity.at_tile(hx, hy);
            // Grow in moist air, thin out in dry air. The lag *is* the inertia.
            p.vis_mass += (local - p.vis_mass) * CLOUD_MASS_RELAX;

            let sat = sat_at(hx, hy);
            let target = condensation_level(local, sat, sea_level_y, cfg);
            p.cruise_fy += (target - p.cruise_fy) * CLOUD_LIFT_RELAX;
            let floor = cloud_floor_y(world, wind, p.fx);
            let lifted = p.cruise_fy.max(floor + cfg.ridge_clearance);
            p.on_ridge = lifted > p.cruise_fy + 1.0;
            p.deform = if p.on_ridge { 0.25 } else { 0.0 };
            p.fy = lifted;
            p.raining = (p.vis_mass / sat).clamp(0.0, 1.5) / 1.5 >= 0.42;
        }
        // Dissipated: too thin to be a cloud any more.
        let keep = cfg.coag_min_hum * CLOUD_DISSIPATE_FRAC;
        self.parcels.retain(|p| p.vis_mass >= keep);

        // --- spawn into bands that have humidity but no cloud ---
        if self.parcels.len() >= cap {
            return;
        }
        let sky_hy_min = (sea_level_y + cfg.coag_min_above_sea).div_euclid(tc);
        let mut hits: Vec<(f32, i32, i32)> = humidity
            .cells
            .iter()
            .filter_map(|(&(hx, hy), &mass)| {
                if hy < sky_hy_min || mass < cfg.coag_min_hum {
                    return None;
                }
                Some((mass, hx, hy))
            })
            .collect();
        if hits.is_empty() {
            return;
        }
        hits.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let hits = pick_spread_across_x(&hits, cap, width, tc);
        // Do not stack a second parcel on a band that already has one.
        let band_of = |fx: f32| -> usize {
            (((fx.max(0.0) as i64) * cap as i64) / width as i64).min(cap as i64 - 1) as usize
        };
        let mut occupied: Vec<bool> = vec![false; cap];
        for p in &self.parcels {
            occupied[band_of(p.fx)] = true;
        }
        for (mass, hx, hy) in hits {
            if self.parcels.len() >= cap {
                break;
            }
            let cx = hx * tc + tc / 2;
            let cy = hy * tc + tc / 2;
            if occupied[band_of(cx as f32)] {
                continue;
            }
            occupied[band_of(cx as f32)] = true;
            let sat = sat_at(hx, hy);
            let cruise = condensation_level(mass, sat, sea_level_y, cfg);
            let floor = cloud_floor_y(world, wind, cx as f32);
            let fy = cruise.max(floor + cfg.ridge_clearance);
            // Spawn at the local mass rather than ramping from zero, so a single
            // call still produces a usable deck.
            self.parcels.push(CloudParcel {
                fx: cx as f32,
                fy,
                mass: 0.0,
                raining: (mass / sat).clamp(0.0, 1.5) / 1.5 >= 0.42,
                on_ridge: fy > cruise + 1.0,
                shape_seed: parcel_shape_seed(cx, cy),
                cruise_fy: cruise,
                vis_mass: mass,
                deform: if fy > cruise + 1.0 { 0.25 } else { 0.0 },
            });
        }
    }
}

/// One band of cloud, derived straight from the humidity field.
///
/// The end state for cloud drawing: a cloud is an *observation about the field*,
/// not an object that has to be placed, persisted, drifted, lifted and
/// dissipated. Each of those five has already been a bug, and all five stop
/// existing here because the field already carries position, density and motion
/// (`Humidity::advect` moves it).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CloudSample {
    /// World x of the band centre.
    pub fx: f32,
    /// Condensation level for this band's air.
    pub fy: f32,
    /// Saturation ratio, 0..1.5 — how cloud-like this air is.
    pub density: f32,
    /// Wet enough to be precipitating.
    pub raining: bool,
}

impl CloudStore {
    /// Derive the visible deck from `(humidity, temperature)` with no stored
    /// state at all.
    ///
    /// Banded across world x on purpose. Selecting the globally wettest tiles
    /// clusters every cloud into a fraction of the map, and — because the winner
    /// of a global top-N changes discontinuously — it also jitters. Banding is
    /// what makes the derivation spatially stable, which is the precondition for
    /// dropping persistence.
    pub fn deck_from_field(
        humidity: &Humidity,
        world: &World,
        wind: &Wind,
        sea_level_y: i32,
        cfg: &CloudConfig,
        temp: Option<&Temperature>,
    ) -> Vec<CloudSample> {
        let tc = humidity.tile_cols.max(1);
        let cap = cfg.max_parcels.max(1);
        let width = wind.width_cols.max(1);
        let sky_hy_min = (sea_level_y + cfg.coag_min_above_sea).div_euclid(tc);
        let mut hits: Vec<(f32, i32, i32)> = humidity
            .cells
            .iter()
            .filter_map(|(&(hx, hy), &mass)| {
                if hy < sky_hy_min || mass < cfg.coag_min_hum {
                    return None;
                }
                Some((mass, hx, hy))
            })
            .collect();
        if hits.is_empty() {
            return Vec::new();
        }
        hits.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let hits = pick_spread_across_x(&hits, cap, width, tc);
        hits.into_iter()
            .map(|(mass, hx, hy)| {
                let sat = temp
                    .map(|t| Humidity::saturation_mass_at_temp(t.at_tile(hx, hy)))
                    .unwrap_or(Humidity::MAX_MASS_PER_TILE)
                    .max(1.0);
                let cx = (hx * tc + tc / 2) as f32;
                let cruise = condensation_level(mass, sat, sea_level_y, cfg);
                let floor = cloud_floor_y(world, wind, cx);
                let density = (mass / sat).clamp(0.0, 1.5);
                CloudSample {
                    fx: cx,
                    fy: cruise.max(floor + cfg.ridge_clearance),
                    density,
                    raining: density / 1.5 >= 0.42,
                }
            })
            .collect()
    }
}

/// Height at which rising air condenses, in world cells.
///
/// See [`Humidity::lifting_condensation_y`] — shared with buoyant rise so the
/// visual deck and the vapour field climb to the same weather-dependent base.
fn condensation_level(mass: f32, sat: f32, sea_level_y: i32, cfg: &CloudConfig) -> f32 {
    // Recover an equivalent temperature from sat so the shared helper agrees
    // with callers that already computed saturation mass.
    let temp_c = sat_to_temp_c(sat);
    Humidity::lifting_condensation_y(mass, temp_c, sea_level_y, cfg.cloud_alt_above_sea)
}

/// Inverse of the cheap Clausius scale in [`Humidity::saturation_mass_at_temp`].
fn sat_to_temp_c(sat: f32) -> f32 {
    let scale = (sat / Humidity::MAX_MASS_PER_TILE).clamp(0.16, 1.55);
    scale * 26.0 - 8.0
}

/// Keep the wettest candidate in each of `cap` bands across the world, so a
/// fixed parcel budget buys coverage rather than a single dense clump.
///
/// `hits` must already be sorted by mass descending: the first candidate seen
/// for a band wins it, which makes the choice deterministic and keeps the
/// "wettest tile becomes the cloud" behaviour within each band.
///
/// Falls back to filling spare capacity with the next-wettest leftovers, so a
/// world whose humidity really is confined to a few bands still draws `cap`
/// parcels rather than going sparse.
fn pick_spread_across_x(
    hits: &[(f32, i32, i32)],
    cap: usize,
    width_cols: i32,
    tile_cols: i32,
) -> Vec<(f32, i32, i32)> {
    if width_cols <= 0 || hits.len() <= cap {
        return hits.iter().take(cap).copied().collect();
    }
    let mut taken: Vec<Option<(f32, i32, i32)>> = vec![None; cap];
    let mut leftovers: Vec<(f32, i32, i32)> = Vec::new();
    for &(mass, hx, hy) in hits {
        // `hx` is a tile index, so it has to be scaled to world columns first —
        // banding on the raw tile index would only ever reach a fraction of the
        // bands and quietly reintroduce the clumping this exists to fix.
        let world_x = hx.max(0) as i64 * tile_cols.max(1) as i64;
        let band = ((world_x * cap as i64) / width_cols.max(1) as i64) as usize;
        let band = band.min(cap - 1);
        if taken[band].is_none() {
            taken[band] = Some((mass, hx, hy));
        } else {
            leftovers.push((mass, hx, hy));
        }
    }
    let mut out: Vec<(f32, i32, i32)> = taken.into_iter().flatten().collect();
    let spare = cap.saturating_sub(out.len());
    out.extend(leftovers.into_iter().take(spare));
    out
}

fn surface_y(world: &World, wind: &Wind, fx: f32) -> f32 {
    live_surface_at(
        world,
        wind.seed,
        fx.round() as i32,
        wind.sea_level_y,
        wind.width_cols,
    ) as f32
}

/// How far above worldgen surface we always scan. Taller player /
/// editor stacks continue the walk while the column stays occupied.
const CLOUD_FLOOR_SCAN_ABOVE: i32 = 64;

fn occupies_cloud_floor(world: &World, gx: i32, y: i32, c: crate::cell::Cell) -> bool {
    if airborne_loose_at(world, gx, y, c) {
        return false;
    }
    if c.material != MaterialId::Air {
        return true;
    }
    // Damp air is **not** a floor: rain falls straight through haze, and clouds
    // are not held up by it either.
    //
    // Counting any trace of moisture meant that in a humid sky the floor climbed
    // to wherever the topmost damp cell was — right up under the deck. Rain
    // streaks clip against this, so `draw_falling_rain` found no vertical room
    // and skipped every drop, which is why rain stayed invisible even after its
    // size floor was fixed. It also let haze shove clouds around instead of
    // terrain doing it.
    //
    // Same threshold the terrain renderer uses to tell a puddle from
    // atmospheric film.
    c.sat.0 > crate::GRAIN_REPOSE_HAZE_MAX
}

/// Inclusive top cell of the wind/humidity bounds (sky ceiling − 1).
fn sky_top_cell(wind: &Wind) -> i32 {
    match wind.bounds {
        Some(b) => (b.hy_max + 1) * wind.tile_cols.max(1) - 1,
        None => 512,
    }
}

/// Occupied column top (rock / ice / snow / standing water) for cloud
/// collision, humidity haze clip, and precip drawing.
///
/// Starts from the worldgen surface band, then **climbs** while the
/// column is still solid or wet so a player tower above
/// `surface + 64` (the old hard cap — ~y 263 on inland hills) still
/// bumps humidity and clouds instead of letting them pass through.
/// Wind speed multiplier for cloud drift, in cells per tick.
///
/// Clouds ride the climate wind faster than the vapour field advects: a deck
/// that moved at exactly the humidity's pace looked static, because the humidity
/// under it moved with it.
const CLOUD_DRIFT_SCALE: f32 = 6.0;

/// How fast a parcel's drawn size follows the humidity it is sitting in.
///
/// This lag is the inertia. Low enough that a cloud builds and thins over tens
/// of ticks instead of snapping to whatever the field says this frame, which is
/// what made the old deck jitter.
const CLOUD_MASS_RELAX: f32 = 0.04;

/// How fast a parcel climbs or sinks toward its condensation level.
const CLOUD_LIFT_RELAX: f32 = 0.02;

/// Fraction of `coag_min_hum` a parcel may thin to before it dissipates.
///
/// Below the spawn threshold, so a cloud that drifts into slightly drier air
/// fades rather than popping out of existence the moment it crosses the line.
const CLOUD_DISSIPATE_FRAC: f32 = 0.55;

pub fn cloud_floor_y(world: &World, wind: &Wind, fx: f32) -> f32 {
    let rock = surface_y(world, wind, fx);
    let gx = world.wrap_x(fx.round() as i32);
    let rock_i = rock as i32;
    let sky = sky_top_cell(wind).max(rock_i);
    let mut y_hi = (rock_i + CLOUD_FLOOR_SCAN_ABOVE).clamp(rock_i, sky);
    while y_hi < sky {
        match world.get_cell(gx, y_hi + 1) {
            Some(c) if occupies_cloud_floor(world, gx, y_hi + 1, c) => y_hi += 1,
            _ => break,
        }
    }
    let y_lo = rock_i - 12;
    for y in (y_lo..=y_hi).rev() {
        match world.get_cell(gx, y) {
            Some(c) if occupies_cloud_floor(world, gx, y, c) => {
                return (y as f32).max(rock);
            }
            _ => {}
        }
    }
    rock
}

fn parcel_shape_seed(cx: i32, cy: i32) -> u32 {
    let mut h = (cx as u32).wrapping_mul(0x9E37_79B9)
        ^ (cy as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 13;
    h
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
    use crate::worldgen::{continental_surface_y, WorldgenParams};
    use wk_material::MaterialId;

    fn wind_for(p: &WorldgenParams) -> Wind {
        let mut w = Wind::climate(
            4,
            0.1,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        );
        // Cloud unit tests want a steady prevailing push.
        w.variance = 0.0;
        w
    }

    #[test]
    fn humidity_rebuilds_visual_echo() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        let sky_y = p.sea_level_y + 40;
        for x in 40..56 {
            h.add(x, sky_y, 80.0);
        }
        let hum_before = h.total_mass();
        let mut clouds = CloudStore::new();
        let mut world = World::new(p.seed);
        let cfg = CloudConfig {
            coag_min_hum: 20.0,
            max_parcels: 16,
            ..CloudConfig::default()
        };
        clouds.step(
            &mut world,
            &mut h,
            &wind,
            p.sea_level_y,
            p.sky_ceiling_y,
            1,
            &cfg,
        );
        assert!(!clouds.is_empty(), "wet sky tiles should spawn a visual echo");
        assert!(clouds.visual_mass() > 0.0);
        assert!(
            clouds.total_mass() < 1e-3,
            "echo parcels must not hold water or the next tick remints it"
        );
        assert!(
            (h.total_mass() - hum_before).abs() < 1e-3,
            "visual rebuild must not steal humidity (was {hum_before} now {})",
            h.total_mass()
        );
        for pcloud in &clouds.parcels {
            assert!(
                pcloud.fy > p.sea_level_y as f32 + cfg.coag_min_above_sea as f32 * 0.5,
                "cloud fy={} too low (sea={})",
                pcloud.fy,
                p.sea_level_y
            );
        }
    }

    #[test]
    fn leftover_parcels_return_to_humidity() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        let mut clouds = CloudStore::new();
        let fy = (p.sea_level_y + 40) as f32;
        clouds.parcels.push(CloudParcel {
            fx: 48.0,
            fy,
            mass: 4.0,
            raining: false,
            on_ridge: false,
            shape_seed: 1,
            cruise_fy: fy,
            vis_mass: 4.0,
            deform: 0.0,
        });
        let mut world = World::new(p.seed);
        let cfg = CloudConfig {
            coag_min_hum: 10_000.0,
            max_parcels: 8,
            ..CloudConfig::default()
        };
        clouds.step(
            &mut world,
            &mut h,
            &wind,
            p.sea_level_y,
            p.sky_ceiling_y,
            1,
            &cfg,
        );
        assert!(
            (h.total_mass() - 4.0).abs() < 1e-3,
            "old parcel mass should land in humidity, got {}",
            h.total_mass()
        );
    }

    #[test]
    fn visual_echo_does_not_remint_humidity() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        let sky_y = p.sea_level_y + 40;
        for x in 40..56 {
            h.add(x, sky_y, 80.0);
        }
        let mut clouds = CloudStore::new();
        let mut world = World::new(p.seed);
        let cfg = CloudConfig {
            coag_min_hum: 20.0,
            max_parcels: 16,
            ..CloudConfig::default()
        };
        clouds.step(
            &mut world,
            &mut h,
            &wind,
            p.sea_level_y,
            p.sky_ceiling_y,
            1,
            &cfg,
        );
        let after_one = h.total_mass();
        clouds.step(
            &mut world,
            &mut h,
            &wind,
            p.sea_level_y,
            p.sky_ceiling_y,
            2,
            &cfg,
        );
        assert!(
            (h.total_mass() - after_one).abs() < 1e-3,
            "second step reminted humidity ({} → {})",
            after_one,
            h.total_mass()
        );
    }

    #[test]
    fn ice_lid_raises_cloud_floor_above_rock() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut world = World::new(p.seed);
        let gx = 20i32;
        let rock = continental_surface_y(p.seed, gx, p.sea_level_y, p.width_cols);
        let ice_top = rock + 8;
        for y in [rock, ice_top] {
            world.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        for y in (rock + 1)..ice_top {
            world.set_cell(gx, y, Cell::water());
        }
        world.set_cell(gx, ice_top, Cell::solid(MaterialId::Ice));
        let floor = cloud_floor_y(&world, &wind, gx as f32);
        assert!(
            floor >= ice_top as f32,
            "cloud floor {floor} must clear ice lid at {ice_top} (rock was {rock})"
        );
    }

    #[test]
    fn airborne_snow_does_not_raise_the_cloud_floor() {
        // occupies_cloud_floor treated every non-Air as a floor, so a
        // flake pulled the deck up to itself. Seated ice above still
        // counts (see ice_lid_raises_cloud_floor_above_rock).
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut world = World::new(p.seed);
        let gx = 20i32;
        let rock = continental_surface_y(p.seed, gx, p.sea_level_y, p.width_cols);
        for y in [rock, rock + 24] {
            world.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        world.set_cell(gx, rock, Cell::solid(MaterialId::Stone));
        world.set_cell(gx, rock + 24, Cell::solid(MaterialId::Snow));
        let floor = cloud_floor_y(&world, &wind, gx as f32);
        assert!(
            (floor - rock as f32).abs() < 2.0,
            "cloud floor {floor} must stay on the stone ({rock}), not the flake"
        );
    }

    #[test]
    fn tall_editor_tower_raises_cloud_floor_above_worldgen_band() {
        // Humidity / clouds used to stop scanning 64 cells above the
        // generated surface, so a player pillar (y > ~263 inland)
        // let the haze pass through the rock.
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut world = World::new(p.seed);
        let gx = 20i32;
        let rock = continental_surface_y(p.seed, gx, p.sea_level_y, p.width_cols);
        let top = rock + 80;
        assert!(
            top > rock + 64,
            "fixture must stick above the old +64 scan cap"
        );
        for y in rock..=top {
            world.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
            world.set_cell(gx, y, Cell::solid(MaterialId::Stone));
        }
        let floor = cloud_floor_y(&world, &wind, gx as f32);
        assert!(
            floor >= top as f32,
            "cloud/humidity floor {floor} must sit on the tower top {top} (worldgen rock {rock})"
        );
    }

    #[test]
    fn cloud_floor_drops_when_the_hill_erodes() {
        // surface_y used to return the seed profile, and cloud_floor_y then
        // did `found.max(rock)`. Eroding a mountain left the floor sitting
        // on the stale peak — clouds hovering over a hole.
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut gx = None;
        let mut hint = 0;
        for x in 0..p.width_cols {
            let s = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols);
            if s >= p.sea_level_y + 22 {
                gx = Some(x);
                hint = s;
                break;
            }
        }
        let gx = gx.expect("need a mountain column");
        let mut world = World::new(p.seed);
        for y in p.sea_level_y..=hint {
            world.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
            world.set_cell(gx, y, Cell::solid(MaterialId::Stone));
        }
        let before = cloud_floor_y(&world, &wind, gx as f32);
        assert!(
            (before - hint as f32).abs() < 2.0,
            "stacked hill should sit at the seed profile ({hint}), got {before}"
        );

        for y in (p.sea_level_y + 1)..=hint {
            world.set_cell(gx, y, Cell::air());
        }
        world.set_cell(gx, p.sea_level_y, Cell::solid(MaterialId::Stone));
        let after = cloud_floor_y(&world, &wind, gx as f32);
        assert!(
            after < before - 8.0,
            "eroded hill must drop the cloud floor ({before} → {after}), \
             not sit on the stale profile {hint}"
        );
    }

    #[test]
    fn ridge_rebuild_lifts_echo_above_surface() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut peak_x = None;
        for x in 0..p.width_cols {
            let s = continental_surface_y(p.seed, x, p.sea_level_y, p.width_cols);
            if s >= p.sea_level_y + 30 {
                peak_x = Some(x);
                break;
            }
        }
        let peak_x = peak_x.expect("need a mountain column");
        let surface = continental_surface_y(p.seed, peak_x, p.sea_level_y, p.width_cols) as f32;
        let mut h = Humidity::with_world_bounds(
            4,
            0,
            p.bedrock_floor_y,
            p.width_cols,
            p.sky_ceiling_y,
        );
        h.wrap_x = true;
        h.add(peak_x, (surface as i32) + 2, 80.0);
        let mut clouds = CloudStore::new();
        let cfg = CloudConfig {
            coag_min_hum: 10.0,
            ..CloudConfig::default()
        };
        let world = World::new(p.seed);
        clouds.rebuild_visuals_from_humidity(&h, &world, &wind, p.sea_level_y, &cfg, None);
        assert!(!clouds.is_empty());
        let c = &clouds.parcels[0];
        let min_clear = surface + cfg.ridge_clearance * 0.5;
        assert!(
            c.fy > min_clear,
            "cloud fy={} must sit above surface+clearance {}",
            c.fy,
            min_clear
        );
    }

    #[test]
    fn clouds_spread_across_the_world_instead_of_clumping() {
        // Taking the globally wettest tiles put every cloud in the world inside
        // 17% of the map on the demo world: 716 tiles were eligible, 36 were
        // drawn, and wet tiles cluster. The other 83% of the sky never showed a
        // drop even though condensation was raining on it, which is what "rain
        // looks broken" actually was.
        let cap = 8;
        let width_cols = 1024;
        let tile_cols = 4;
        // Candidates heavily weighted toward one band: the wettest 6 are all in
        // tiles 30..36 (world x 120..144), with drier ones spread out.
        let mut hits: Vec<(f32, i32, i32)> = Vec::new();
        for i in 0..6 {
            hits.push((1000.0 - i as f32, 30 + i, 40));
        }
        for b in 1..8 {
            hits.push((100.0, b * 32, 40));
        }
        hits.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

        let picked = pick_spread_across_x(&hits, cap, width_cols, tile_cols);
        let bands: std::collections::HashSet<usize> = picked
            .iter()
            .map(|&(_, hx, _)| {
                ((hx as i64 * tile_cols as i64 * cap as i64) / width_cols as i64) as usize
            })
            .collect();
        assert!(
            bands.len() >= 6,
            "picks should cover most bands, got {} distinct of {cap}",
            bands.len()
        );
        assert_eq!(picked.len(), cap, "the parcel budget should still be spent");
    }

    #[test]
    fn a_world_whose_weather_really_is_local_still_fills_the_budget() {
        // Spreading must not make a genuinely localised storm draw fewer clouds:
        // spare bands fall back to the next-wettest leftovers.
        let cap = 8;
        let hits: Vec<(f32, i32, i32)> = (0..20).map(|i| (500.0 - i as f32, 40 + i, 40)).collect();
        let picked = pick_spread_across_x(&hits, cap, 1024, 4);
        assert_eq!(picked.len(), cap);
    }

    /// Flat world with a steady wind and one moist sky tile.
    fn drift_scene() -> (World, Wind, Humidity, CloudConfig, i32) {
        let width = 256;
        let sea = 40;
        let mut world = World::new(3);
        world.ensure_chunk(crate::chunk::ChunkCoord::new(0, 0));
        let wind = Wind::climate(4, 0.05, 3u64, width, sea, 0, 320, true);
        let mut humidity = Humidity::with_world_bounds(4, 0, 0, width, 320);
        humidity.wrap_x = true;
        let cfg = CloudConfig::default();
        // Wet band across the whole sky so a drifting parcel stays fed.
        let hy = (sea + cfg.coag_min_above_sea + 8) / 4;
        for hx in 0..(width / 4) {
            humidity.cells.insert((hx, hy), cfg.coag_min_hum * 6.0);
        }
        (world, wind, humidity, cfg, sea)
    }

    #[test]
    fn a_cloud_persists_and_drifts_instead_of_blinking() {
        // The deck used to be cleared and rebuilt every tick, so a parcel had no
        // identity: it could not drift, build or clear, only jump to wherever
        // humidity currently peaked. That rebuild is the "clouds jiggling about
        // too fast" the playtest reported.
        let (world, wind, humidity, cfg, sea) = drift_scene();
        let mut store = CloudStore::default();
        store.rebuild_visuals_from_humidity(&humidity, &world, &wind, sea, &cfg, None);
        assert!(!store.parcels.is_empty(), "should seed a deck");
        let seed_before = store.parcels[0].shape_seed;
        let x_before = store.parcels[0].fx;

        for _ in 0..8 {
            store.rebuild_visuals_from_humidity(&humidity, &world, &wind, sea, &cfg, None);
        }
        // Same parcel, moved — not a fresh one at a new spot.
        let same = store.parcels.iter().find(|p| p.shape_seed == seed_before);
        let same = same.expect("the parcel should survive, not be replaced");
        assert!(
            (same.fx - x_before).abs() > 0.5,
            "a persisting cloud should drift downwind (was {x_before}, now {})",
            same.fx
        );
    }

    #[test]
    fn a_cloud_that_drifts_into_dry_air_thins_out_gradually() {
        // Dissipation has to be gradual, or clouds pop in and out as they cross
        // the spawn threshold. `vis_mass` is an EMA, which only works because the
        // parcel holding it now survives between ticks.
        let (world, wind, mut humidity, cfg, sea) = drift_scene();
        let mut store = CloudStore::default();
        store.rebuild_visuals_from_humidity(&humidity, &world, &wind, sea, &cfg, None);
        let full = store.parcels[0].vis_mass;
        // The sky dries out completely.
        humidity.cells.clear();
        store.rebuild_visuals_from_humidity(&humidity, &world, &wind, sea, &cfg, None);
        let after_one = store.parcels.first().map(|p| p.vis_mass).unwrap_or(0.0);
        assert!(
            after_one < full && after_one > full * 0.5,
            "one tick of dry air should thin the cloud, not delete it \
             ({full} -> {after_one})"
        );
        // ...and it does eventually go.
        for _ in 0..400 {
            store.rebuild_visuals_from_humidity(&humidity, &world, &wind, sea, &cfg, None);
        }
        assert!(
            store.parcels.is_empty(),
            "a cloud in permanently dry air should dissipate"
        );
    }

    #[test]
    fn dry_air_condenses_higher_than_moist_air() {
        // Cloud base tracks dewpoint depression: dry or warm air has to climb
        // further before it reaches its dewpoint. This is what makes deck height
        // respond to weather rather than sitting pinned above the terrain.
        let cfg = CloudConfig::default();
        let sat = 400.0;
        let moist = condensation_level(sat * 0.95, sat, 80, &cfg);
        let middling = condensation_level(sat * 0.5, sat, 80, &cfg);
        let dry = condensation_level(sat * 0.1, sat, 80, &cfg);
        assert!(
            moist < middling && middling < dry,
            "cloud base should rise as air dries ({moist} < {middling} < {dry})"
        );
    }

    #[test]
    fn damp_air_is_not_a_cloud_floor() {
        // A humid sky used to raise the "floor" to wherever the topmost damp cell
        // was, because any non-empty sat counted. Rain streaks clip against this,
        // so in a saturated world there was no vertical room between deck and
        // floor and every drop was skipped — rain stayed invisible no matter how
        // large the drops were drawn.
        let mut w = World::new(5);
        w.ensure_chunk(crate::chunk::ChunkCoord::new(0, 0));
        let wind = Wind::climate(4, 0.05, 5u64, 64, 20, 0, 320, false);
        let gx = 8;
        // Solid ground low down...
        for y in 0..=6 {
            w.set_cell(gx, y, Cell::solid(MaterialId::Stone));
        }
        // ...and a tall column of merely damp air above it.
        for y in 7..=60 {
            let mut haze = Cell::air();
            haze.sat = crate::cell::Sat(crate::GRAIN_REPOSE_HAZE_MAX);
            w.set_cell(gx, y, haze);
        }
        let floor = cloud_floor_y(&w, &wind, gx as f32);
        assert!(
            floor < 20.0,
            "haze must not act as a floor (got {floor}, damp air reached y=60)"
        );

        // Standing water, though, genuinely is a surface.
        for y in 7..=12 {
            w.set_cell(gx, y, Cell::water());
        }
        let wet_floor = cloud_floor_y(&w, &wind, gx as f32);
        assert!(
            wet_floor >= 12.0,
            "standing water should raise the floor (got {wet_floor})"
        );
    }

    #[test]
    fn the_field_derived_deck_moves_smoothly_when_the_field_advects() {
        // The precondition for deleting parcel persistence: the derivation has to
        // be *spatially stable*. A global top-N by mass changes winner
        // discontinuously, which is what made the old deck jitter; banding by
        // world x is what fixes it. If this test goes, the jitter comes back.
        let (world, wind, mut humidity, cfg, sea) = drift_scene();
        let before = CloudStore::deck_from_field(&humidity, &world, &wind, sea, &cfg, None);
        assert!(!before.is_empty(), "a moist sky should derive a deck");

        // Nudge the field along and re-derive from scratch.
        for _ in 0..4 {
            humidity.advect(0.5, 0.0);
        }
        let after = CloudStore::deck_from_field(&humidity, &world, &wind, sea, &cfg, None);
        assert_eq!(
            before.len(),
            after.len(),
            "advecting the field should not change how many clouds there are"
        );
        // No sample should have leapt across the map.
        let worst = before
            .iter()
            .zip(after.iter())
            .map(|(a, b)| (a.fx - b.fx).abs())
            .fold(0.0f32, f32::max);
        assert!(
            worst < 32.0,
            "the deck should follow the field, not jump ({worst} cells of movement)"
        );
    }

    #[test]
    fn a_dry_sky_derives_no_clouds() {
        let (world, wind, mut humidity, cfg, sea) = drift_scene();
        humidity.cells.clear();
        let deck = CloudStore::deck_from_field(&humidity, &world, &wind, sea, &cfg, None);
        assert!(deck.is_empty(), "no vapour, no cloud");
    }
}
