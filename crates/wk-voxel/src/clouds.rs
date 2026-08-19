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
use crate::worldgen::continental_surface_y;

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
        let tc = humidity.tile_cols.max(1);
        let deck_hy = (sea_level_y + cfg.cloud_alt_above_sea).div_euclid(tc);
        humidity.buoyant_rise_thermal(cfg.buoyant_rise, deck_hy, temp);
        self.rebuild_visuals_from_humidity(humidity, world, wind, sea_level_y, cfg, temp);
    }

    /// Move blob mass back onto humidity tiles (old saves / leftover coag).
    pub fn release_parcels_into_humidity(&mut self, humidity: &mut Humidity) {
        if self.parcels.is_empty() {
            return;
        }
        let leftover = std::mem::take(&mut self.parcels);
        for p in leftover {
            if p.mass > 1e-3 {
                humidity.add(p.fx.round() as i32, p.fy.round() as i32, p.mass);
            }
        }
    }

    /// Bounded N-bank / shade / streak echoes from the wettest sky tiles.
    pub fn rebuild_visuals_from_humidity(
        &mut self,
        humidity: &Humidity,
        world: &World,
        wind: &Wind,
        sea_level_y: i32,
        cfg: &CloudConfig,
        temp: Option<&Temperature>,
    ) {
        self.parcels.clear();
        let tc = humidity.tile_cols.max(1);
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
        let cap = cfg.max_parcels.max(1);
        for &(mass, hx, hy) in hits.iter().take(cap) {
            let sat = temp
                .map(|t| Humidity::saturation_mass_at_temp(t.at_tile(hx, hy)))
                .unwrap_or(Humidity::MAX_MASS_PER_TILE);
            let wet = (mass / sat.max(1.0)).clamp(0.0, 1.5) / 1.5;
            let cx = hx * tc + tc / 2;
            let cy = hy * tc + tc / 2;
            let seed = parcel_shape_seed(cx, cy);
            let mut fy = cy as f32;
            let floor = cloud_floor_y(world, wind, cx as f32);
            fy = fy.max(floor + cfg.ridge_clearance);
            // `mass` stays 0 — the next tick's leftover-release would
            // otherwise pour this humidity copy back into the sky
            // (mint vapor → condensation mints rain).
            self.parcels.push(CloudParcel {
                fx: cx as f32,
                fy,
                mass: 0.0,
                raining: wet >= 0.42,
                on_ridge: fy > cy as f32 + 1.0,
                shape_seed: seed,
                cruise_fy: fy,
                vis_mass: mass,
                deform: if fy > cy as f32 + 1.0 { 0.25 } else { 0.0 },
            });
        }
    }
}

fn surface_y(wind: &Wind, fx: f32) -> f32 {
    continental_surface_y(
        wind.seed,
        fx.round() as i32,
        wind.sea_level_y,
        wind.width_cols,
    ) as f32
}

/// How far above worldgen surface we always scan. Taller player /
/// editor stacks continue the walk while the column stays occupied.
const CLOUD_FLOOR_SCAN_ABOVE: i32 = 64;

fn occupies_cloud_floor(c: crate::cell::Cell) -> bool {
    c.material != MaterialId::Air || !c.sat.is_empty()
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
pub fn cloud_floor_y(world: &World, wind: &Wind, fx: f32) -> f32 {
    let rock = surface_y(wind, fx);
    let gx = world.wrap_x(fx.round() as i32);
    let rock_i = rock as i32;
    let sky = sky_top_cell(wind).max(rock_i);
    let mut y_hi = (rock_i + CLOUD_FLOOR_SCAN_ABOVE).clamp(rock_i, sky);
    while y_hi < sky {
        match world.get_cell(gx, y_hi + 1) {
            Some(c) if occupies_cloud_floor(c) => y_hi += 1,
            _ => break,
        }
    }
    let y_lo = rock_i - 12;
    for y in (y_lo..=y_hi).rev() {
        match world.get_cell(gx, y) {
            Some(c) if occupies_cloud_floor(c) => {
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
    use crate::worldgen::WorldgenParams;
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
}
