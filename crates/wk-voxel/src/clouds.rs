//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Atmosphere step helper and save-compat parcel dump.
//!
//! Atmospheric water lives on [`Humidity`] tiles. Rain is condensation /
//! dew from that field. Cartoon N banks are gone: this crate only lifts
//! vapour and returns leftover save-file parcel mass to humidity.
//! [`cloud_floor_y`] still clips the H haze against terrain.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::grid::World;
use crate::humidity::Humidity;
use crate::phase::PhaseConfig;
use crate::temperature::Temperature;
use crate::wind::Wind;
use crate::worldgen::{airborne_loose_at, live_surface_at};

/// Save-compat ceiling (parcels no longer store water).
pub const MAX_CLOUD_PARCELS: usize = 36;
/// Default wetness scale kept for old presets / shade callers.
pub const DOWNPOUR_MASS: f32 = 200.0;
/// Save-compat ceiling (parcels no longer store water).
pub const MAX_CLOUD_TOTAL_MASS: f32 = 80_000.0;
/// Save-compat per-parcel ceiling.
pub const MAX_CLOUD_PARCEL_MASS: f32 = 8_000.0;

/// Live-tunable knobs. Only [`Self::buoyant_rise`] is used by physics;
/// the rest stay so old presets and saves still deserialize.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CloudConfig {
    pub max_parcels: usize,
    #[serde(default = "default_max_total_mass")]
    pub max_total_mass: f32,
    #[serde(default = "default_max_parcel_mass")]
    pub max_parcel_mass: f32,
    pub coag_min_hum: f32,
    pub coag_rate: f32,
    pub coag_max_take: f32,
    pub spawn_radius: f32,
    pub merge_dist: f32,
    pub downpour_mass: f32,
    pub downpour_drain: f32,
    pub downpour_stop_frac: f32,
    pub cloud_alt_above_sea: i32,
    pub coag_min_above_sea: i32,
    pub ridge_clearance: f32,
    pub parcel_wind_scale: f32,
    /// Fraction of each tile that convects one row up per tick.
    pub buoyant_rise: f32,
    #[serde(default = "default_snow_footprint_mult")]
    pub snow_footprint_mult: f32,
    #[serde(default = "default_rain_footprint_mult")]
    pub rain_footprint_mult: f32,
    #[serde(default = "default_snow_span_mult")]
    pub snow_span_mult: f32,
    #[serde(default = "default_rain_span_mult")]
    pub rain_span_mult: f32,
    #[serde(default = "default_snow_cells_per_tick")]
    pub snow_cells_per_tick: u8,
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
            buoyant_rise: 0.14,
            snow_footprint_mult: default_snow_footprint_mult(),
            rain_footprint_mult: default_rain_footprint_mult(),
            snow_span_mult: default_snow_span_mult(),
            rain_span_mult: default_rain_span_mult(),
            snow_cells_per_tick: default_snow_cells_per_tick(),
            rain_cells_per_tick: default_rain_cells_per_tick(),
        }
    }
}

/// Leftover save-compat blob. Live weather does not spawn or draw these.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudParcel {
    pub fx: f32,
    pub fy: f32,
    pub mass: f32,
    #[serde(default)]
    pub raining: bool,
    #[serde(default)]
    pub on_ridge: bool,
    #[serde(default)]
    pub shape_seed: u32,
    #[serde(default)]
    pub cruise_fy: f32,
    #[serde(default)]
    pub vis_mass: f32,
    #[serde(default)]
    pub deform: f32,
}

/// Save-compat holder. Parcels are dumped into humidity on the next step
/// and never rebuilt.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CloudStore {
    #[serde(default)]
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

    pub fn visual_mass(&self) -> f32 {
        0.0
    }

    /// Dump leftover save-file parcel mass into humidity and lift vapor.
    /// Does **not** rain and does **not** draw clouds.
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

    /// Like [`Self::step`]. `temp` scales thermal rise and lets a wet
    /// plume carry heat upward. `phase` is unused (rain is condensation).
    pub fn step_with_precip(
        &mut self,
        world: &mut World,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
        tick: u64,
        cfg: &CloudConfig,
        mut temp: Option<&mut Temperature>,
        phase: Option<&PhaseConfig>,
    ) {
        let _ = (sea_level_y, phase, world, wind);
        self.release_parcels_into_humidity(humidity);
        // Rise until the sky box, not `sea + cloud_alt`. That deck cap
        // sat on mountain ridges (surface hy ≥ deck) and turned every
        // thermal into a fog film. Unstable lapse already stops the lift.
        //
        // Every other tick: the walk is the same, the loft rate is close
        // enough, and doing it every physics tick was an FPS sink once
        // vapour occupied more than a fog film.
        if tick % 2 == 0 {
            let max_hy = humidity
                .bounds
                .map(|b| b.hy_max)
                .unwrap_or_else(|| sky_ceiling_y.div_euclid(humidity.tile_cols.max(1)));
            humidity.buoyant_rise_thermal(cfg.buoyant_rise, max_hy, temp.as_deref_mut());
        }
        self.parcels.clear();
    }

    /// Move leftover save-file blob mass back onto humidity tiles.
    pub fn release_parcels_into_humidity(&mut self, humidity: &mut Humidity) {
        for p in std::mem::take(&mut self.parcels) {
            if p.mass > 1e-3 {
                humidity.add(p.fx.round() as i32, p.fy.round() as i32, p.mass);
            }
        }
    }
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
    // Damp air is **not** a floor: rain falls straight through haze.
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

/// Occupied column top (rock / ice / snow / standing water) for
/// humidity haze clip.
///
/// Starts from the worldgen surface band, then **climbs** while the
/// column is still solid or wet so a player tower above
/// `surface + 64` still bumps haze instead of letting it pass through.
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
        w.variance = 0.0;
        w
    }

    #[test]
    fn thermal_rise_lifts_ridge_fog_without_a_sea_deck_cap() {
        let mut world = World::new(7);
        world.ensure_chunk(ChunkCoord::new(0, 0));
        for y in 0..=36 {
            world.set_cell(6, y, Cell::solid(MaterialId::Stone));
        }
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 256);
        h.add(6, 38, 200.0);
        let mut wind = Wind::climate(4, 0.0, 7, 64, 0, 0, 256, false);
        wind.variance = 0.0;
        let mut clouds = CloudStore::new();
        let cfg = CloudConfig {
            cloud_alt_above_sea: 8,
            buoyant_rise: 0.30,
            coag_min_hum: 20.0,
            max_parcels: 4,
            ..CloudConfig::default()
        };
        clouds.step_with_precip(&mut world, &mut h, &wind, 0, 256, 2, &cfg, None, None);
        assert!(
            h.at_tile(1, 10) > 20.0,
            "ridge fog must climb toward the sky box, not park at sea+cloud_alt (hy10={})",
            h.at_tile(1, 10)
        );
        assert!(clouds.is_empty(), "N banks must not come back");
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
        clouds.step(
            &mut world,
            &mut h,
            &wind,
            p.sea_level_y,
            p.sky_ceiling_y,
            1,
            &CloudConfig::default(),
        );
        assert!(
            (h.total_mass() - 4.0).abs() < 1e-3,
            "old parcel mass should land in humidity, got {}",
            h.total_mass()
        );
        assert!(clouds.is_empty());
    }

    #[test]
    fn step_does_not_remint_humidity() {
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
        let cfg = CloudConfig::default();
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
        assert!(clouds.is_empty());
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
    fn damp_air_is_not_a_cloud_floor() {
        let mut w = World::new(5);
        w.ensure_chunk(crate::chunk::ChunkCoord::new(0, 0));
        let wind = Wind::climate(4, 0.05, 5u64, 64, 20, 0, 320, false);
        let gx = 8;
        for y in 0..=6 {
            w.set_cell(gx, y, Cell::solid(MaterialId::Stone));
        }
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

        for y in 7..=12 {
            w.set_cell(gx, y, Cell::water());
        }
        let wet_floor = cloud_floor_y(&w, &wind, gx as f32);
        assert!(
            wet_floor >= 12.0,
            "standing water should raise the floor (got {wet_floor})"
        );
    }
}
