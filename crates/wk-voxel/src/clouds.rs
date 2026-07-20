//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Discrete cloud parcels: humidity rises, then coagulates into blobs;
//! wind carries them; they ride above the terrain (no clipping through
//! mountains or ice/water columns) and dump rain sooner when scraping ridges.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::grid::World;
use crate::humidity::Humidity;
use crate::phase::{deposit_precip_on_surface, PhaseConfig};
use crate::temperature::Temperature;
use crate::wind::Wind;
use crate::worldgen::continental_surface_y;

/// Soft cap so cartoon skies stay readable (default for [`CloudConfig`]).
pub const MAX_CLOUD_PARCELS: usize = 36;
/// Default mass at which a parcel starts dumping rain.
pub const DOWNPOUR_MASS: f32 = 200.0;

/// Live-tunable cloud / coagulation / downpour knobs.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CloudConfig {
    pub max_parcels: usize,
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
    pub buoyant_rise: f32,
}

impl Default for CloudConfig {
    fn default() -> Self {
        Self {
            max_parcels: MAX_CLOUD_PARCELS,
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

/// Population of coagulated clouds.
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

    /// Full atmosphere step for clouds:
    /// rise is done on humidity first; then coagulate, advect+collide,
    /// merge, downpour.
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

    /// Like [`Self::step`], but cold columns receive **snow** when
    /// `temp` / `phase` are provided.
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
        // Let ocean vapor climb into the cloud deck before clumping.
        let tc = humidity.tile_cols.max(1);
        let deck_hy = (sea_level_y + cfg.cloud_alt_above_sea).div_euclid(tc);
        humidity.buoyant_rise(cfg.buoyant_rise, deck_hy);

        self.coagulate(humidity, wind, sea_level_y, sky_ceiling_y, cfg);
        self.advect_and_collide(world, wind, sea_level_y, sky_ceiling_y, cfg);
        self.merge(cfg);
        self.downpour(world, wind, tick, cfg, temp, phase);
        for p in &mut self.parcels {
            p.smooth_visuals();
        }
        self.parcels.retain(|p| p.mass > 1.0);
    }

    /// Pull humidity only from risen sky tiles into parcels.
    fn coagulate(
        &mut self,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
        cfg: &CloudConfig,
    ) {
        let tc = humidity.tile_cols.max(1);
        let sky_hy_min = (sea_level_y + cfg.coag_min_above_sea).div_euclid(tc);
        let sky_hy_max = (sky_ceiling_y - 2).div_euclid(tc);
        let preferred_alt = (sea_level_y + cfg.cloud_alt_above_sea)
            .min(sky_ceiling_y - 4)
            .max(sea_level_y + cfg.coag_min_above_sea) as f32;
        let keys: Vec<(i32, i32)> = humidity.cells.keys().copied().collect();
        for (hx, hy) in keys {
            if hy < sky_hy_min || hy > sky_hy_max {
                continue;
            }
            let mass = humidity.at_tile(hx, hy);
            if mass < cfg.coag_min_hum {
                continue;
            }
            let cx = hx * tc + tc / 2;
            let cy = hy * tc + tc / 2;
            let take = (mass * cfg.coag_rate).min(cfg.coag_max_take).min(mass);
            if take <= 0.0 {
                continue;
            }

            let idx = self.nearest_parcel(cx as f32, cy as f32, cfg.spawn_radius);
            let idx = match idx {
                Some(i) => i,
                None if self.parcels.len() < cfg.max_parcels.max(1) => {
                    let seed = parcel_shape_seed(cx, cy);
                    // Spread spawn altitudes so the sky isn't one flat deck.
                    let elev_jitter = ((seed >> 8) & 31) as f32 - 8.0; // -8..+23
                    let fy = (cy as f32).max(preferred_alt) + elev_jitter;
                    self.parcels.push(CloudParcel {
                        fx: cx as f32,
                        fy,
                        mass: 0.0,
                        raining: false,
                        on_ridge: false,
                        shape_seed: seed,
                        cruise_fy: fy,
                        vis_mass: 0.0,
                        deform: 0.0,
                    });
                    self.parcels.len() - 1
                }
                None => continue,
            };

            if let Some(entry) = humidity.cells.get_mut(&(hx, hy)) {
                *entry -= take;
                if *entry < 1e-3 {
                    humidity.cells.remove(&(hx, hy));
                }
            }
            let p = &mut self.parcels[idx];
            p.mass += take;
            // Do NOT pull parcels horizontally toward vapor sources.
            // Only ease y up toward vapor when below the parcel's cruise.
            let target_y = (cy as f32)
                .max(preferred_alt * 0.85)
                .max(p.cruise_fy * 0.9);
            if target_y + 1.0 >= p.fy && p.fy + 2.0 < p.cruise_fy.max(target_y) {
                p.fy = p.fy * 0.985 + target_y * 0.015;
            }
            let _ = wind;
        }
    }

    fn nearest_parcel(&self, x: f32, y: f32, max_dist: f32) -> Option<usize> {
        let mut best = None;
        let mut best_d = max_dist;
        for (i, p) in self.parcels.iter().enumerate() {
            let dx = p.fx - x;
            let dy = p.fy - y;
            let d = (dx * dx + dy * dy).sqrt();
            if d < best_d {
                best_d = d;
                best = Some(i);
            }
        }
        best
    }

    /// Wind drift, then soft collision with the land / ice / water column
    /// top so clouds crest peaks and ride above lake lids (not through them).
    fn advect_and_collide(
        &mut self,
        world: &World,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
        cfg: &CloudConfig,
    ) {
        let tc = wind.tile_cols.max(1) as f32;
        let vx = wind.climate_vx * tc * cfg.parcel_wind_scale;
        let y_lo = (sea_level_y + cfg.coag_min_above_sea) as f32;
        let y_hi = (sky_ceiling_y - 3) as f32;
        let width = wind.width_cols.max(1) as f32;
        let wind_sign = if wind.climate_vx >= 0.0 { 1.0 } else { -1.0 };
        let deck = preferred_deck(sea_level_y, sky_ceiling_y, cfg);
        for p in &mut self.parcels {
            p.on_ridge = false;
            if p.cruise_fy <= 0.0 {
                p.cruise_fy = p.fy.max(deck);
            }
            let hx = (p.fx / tc).floor() as i32;
            let vy = wind.vy_at(hx, 0) * tc * cfg.parcel_wind_scale;
            p.fx += vx;
            p.fy += vy * 0.35;
            if wind.wrap_x {
                p.fx = p.fx.rem_euclid(width);
            } else {
                p.fx = p.fx.clamp(0.0, width - 1.0);
            }

            let r = p.radius();
            // Soft sample under centre + leading edge (windward).
            let sample_x = p.fx + wind_sign * r * 0.35;
            let floor = cloud_floor_y(world, wind, p.fx)
                .max(cloud_floor_y(world, wind, sample_x));
            let land = floor > sea_level_y as f32 + 1.0;
            let min_fy = if land {
                floor + cfg.ridge_clearance + (r * 0.12).min(3.5)
            } else {
                y_lo.max(sea_level_y as f32 + cfg.cloud_alt_above_sea as f32 * 0.45)
            };

            if p.fy < min_fy {
                let lift = min_fy - p.fy;
                // Soft deform: ease up instead of hard snapping.
                let blend = (0.25 + lift * 0.04).clamp(0.18, 0.55);
                p.fy = p.fy * (1.0 - blend) + min_fy * blend;
                p.deform = (p.deform + lift * 0.08).clamp(0.0, 1.0);
                if land {
                    p.on_ridge = true;
                    // Keep the higher path after the crest.
                    p.cruise_fy = p.cruise_fy.max(min_fy);
                    p.fx += wind_sign * (0.15 + lift * 0.025).min(0.55);
                    if wind.wrap_x {
                        p.fx = p.fx.rem_euclid(width);
                    }
                }
            }

            // Hold cruise altitude (slow ease up if below; never yank down
            // to the free-air deck after a ridge lift).
            if p.fy + 0.5 < p.cruise_fy {
                p.fy = p.fy * 0.96 + p.cruise_fy * 0.04;
            }
            // Over open ocean only, very slowly forget extreme cruise.
            if !land && p.cruise_fy > deck + 6.0 {
                p.cruise_fy = p.cruise_fy * 0.9985 + deck * 0.0015;
            }

            p.fy = p.fy.clamp(y_lo.min(min_fy), y_hi);
            p.cruise_fy = p.cruise_fy.clamp(y_lo, y_hi);
        }
    }

    fn merge(&mut self, cfg: &CloudConfig) {
        let mut i = 0;
        while i < self.parcels.len() {
            let mut j = i + 1;
            while j < self.parcels.len() {
                let dx = self.parcels[i].fx - self.parcels[j].fx;
                let dy = self.parcels[i].fy - self.parcels[j].fy;
                if (dx * dx + dy * dy).sqrt() < cfg.merge_dist {
                    let other = self.parcels.swap_remove(j);
                    let a = &mut self.parcels[i];
                    let total = a.mass + other.mass;
                    if total > 0.0 {
                        a.fx = (a.fx * a.mass + other.fx * other.mass) / total;
                        a.fy = (a.fy * a.mass + other.fy * other.mass) / total;
                    }
                    let other_mass = other.mass;
                    let keep_other_shape = other_mass > a.mass;
                    a.mass = total;
                    a.vis_mass = (a.vis_mass + other.vis_mass) * 0.5;
                    a.raining = a.raining || other.raining;
                    a.on_ridge = a.on_ridge || other.on_ridge;
                    a.cruise_fy = a.cruise_fy.max(other.cruise_fy);
                    a.deform = a.deform.max(other.deform);
                    // Keep the heavier parcel's silhouette.
                    if keep_other_shape {
                        a.shape_seed = other.shape_seed;
                    }
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    /// Heavy parcels dump sat into Air cells beneath them.
    fn downpour(
        &mut self,
        world: &mut World,
        wind: &Wind,
        tick: u64,
        cfg: &CloudConfig,
        temp: Option<&Temperature>,
        phase: Option<&PhaseConfig>,
    ) {
        for p in &mut self.parcels {
            let mut oro = orographic_boost(wind, p.fx);
            if p.on_ridge {
                // Gentle orographic nudge — not a dump-to-death on peaks.
                oro = (oro + 0.25).min(1.6);
            }
            let trigger = cfg.downpour_mass / oro;
            if !p.raining {
                if p.mass >= trigger {
                    p.raining = true;
                } else {
                    continue;
                }
            }
            if p.mass < cfg.downpour_mass * cfg.downpour_stop_frac {
                p.raining = false;
                continue;
            }

            let drain = (cfg.downpour_drain * oro).min(p.mass);
            let radius = p.radius();
            // Wider footprint when snowing so flakes seed slopes, not only
            // the ridge column under the parcel centre.
            let snowing = temp.is_some() && phase.is_some();
            let cols = if snowing {
                ((radius * 2.2) as i32).clamp(4, 18)
            } else {
                ((radius * 1.25) as i32).clamp(2, 10)
            };
            let mut remaining = drain;
            // Fractional column shares are << one Snow cell (255). Cold
            // columns retry a full cell from parcel mass — never soak.
            let snow_cell = phase
                .map(|ph| ph.min_budget_to_snow.max(u8::MAX as f32))
                .unwrap_or(0.0);
            let mut snow_cells_this_tick = 0u8;
            let snow_cell_cap = if snowing { 5 } else { 2 };
            for k in 0..cols {
                if remaining <= 0.0 || p.mass <= 0.0 {
                    break;
                }
                let t = if cols == 1 {
                    0.0
                } else {
                    k as f32 / (cols - 1) as f32 * 2.0 - 1.0
                };
                let span = if snowing { radius * 1.35 } else { radius * 0.85 };
                let gx = world.wrap_x((p.fx + t * span).round() as i32);
                let share = (drain / cols as f32) * (1.15 - 0.3 * t.abs());
                let mut dropped = deposit_rain_column(
                    world,
                    gx,
                    p.fy.round() as i32,
                    share.min(remaining),
                    tick,
                    k,
                    temp,
                    phase,
                );
                if dropped <= 0.0
                    && snow_cell > 0.0
                    && snow_cells_this_tick < snow_cell_cap
                    && p.mass >= snow_cell
                {
                    dropped = deposit_rain_column(
                        world,
                        gx,
                        p.fy.round() as i32,
                        snow_cell,
                        tick,
                        k,
                        temp,
                        phase,
                    );
                    if dropped > 0.0 {
                        snow_cells_this_tick = snow_cells_this_tick.saturating_add(1);
                    }
                }
                let pay = dropped.min(p.mass);
                if pay <= 0.0 {
                    continue;
                }
                p.mass -= pay;
                // Snow-cell retries may exceed this tick's drain slice.
                remaining = (remaining - pay.min(remaining)).max(0.0);
            }
            if p.mass < 1.0 {
                p.mass = 0.0;
                p.raining = false;
            }
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

/// Occupied column top (rock / ice / snow / standing water) for cloud
/// collision and precip drawing — top-down so streaks stop on the true
/// surface instead of punching through slopes.
pub fn cloud_floor_y(world: &World, wind: &Wind, fx: f32) -> f32 {
    let rock = surface_y(wind, fx);
    let gx = world.wrap_x(fx.round() as i32);
    let rock_i = rock as i32;
    let y_hi = rock_i + 64;
    let y_lo = rock_i - 12;
    for y in (y_lo..=y_hi).rev() {
        match world.get_cell(gx, y) {
            Some(c) if c.material != MaterialId::Air => {
                return (y as f32).max(rock);
            }
            Some(c) if !c.sat.is_empty() => {
                return (y as f32).max(rock);
            }
            _ => {}
        }
    }
    rock
}

fn preferred_deck(sea_level_y: i32, sky_ceiling_y: i32, cfg: &CloudConfig) -> f32 {
    (sea_level_y + cfg.cloud_alt_above_sea)
        .min(sky_ceiling_y - 4)
        .max(sea_level_y + cfg.coag_min_above_sea) as f32
}

fn parcel_shape_seed(cx: i32, cy: i32) -> u32 {
    let mut h = (cx as u32).wrapping_mul(0x9E37_79B9)
        ^ (cy as u32).wrapping_mul(0x85EB_CA6B);
    h ^= h >> 16;
    h = h.wrapping_mul(0xC2B2_AE3D);
    h ^= h >> 13;
    h
}

fn orographic_boost(wind: &Wind, fx: f32) -> f32 {
    let hx = (fx / wind.tile_cols.max(1) as f32).floor() as i32;
    let ascent = wind.ascent_cells(hx);
    let s = surface_y(wind, fx);
    let tall = s >= wind.sea_level_y as f32 + 22.0;
    let mut boost = 1.0 + (ascent / 55.0).clamp(0.0, 1.0) * 0.45;
    if tall {
        boost += 0.15;
    }
    boost.clamp(1.0, 1.55)
}

fn deposit_rain_column(
    world: &mut World,
    gx: i32,
    start_y: i32,
    budget: f32,
    tick: u64,
    salt: i32,
    temp: Option<&Temperature>,
    phase: Option<&PhaseConfig>,
) -> f32 {
    let jx = world.wrap_x(gx + ((tick as i32 + salt * 3) % 3) - 1);
    deposit_precip_on_surface(world, jx, start_y, budget, temp, phase)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
    use crate::worldgen::WorldgenParams;
    use wk_material::MaterialId;

    fn wind_for(p: &WorldgenParams) -> Wind {
        Wind::climate(
            4,
            0.1,
            p.seed,
            p.width_cols,
            p.sea_level_y,
            p.bedrock_floor_y,
            p.sky_ceiling_y,
            true,
        )
    }

    #[test]
    fn humidity_coagulates_into_parcels() {
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
        // Seed vapor low (ocean film), then steps should lift + coagulate.
        let film_y = p.sea_level_y + 2;
        for x in 40..56 {
            h.add(x, film_y, 120.0);
        }
        let hum_before = h.total_mass();
        let mut clouds = CloudStore::new();
        let mut world = World::new(p.seed);
        let cfg = CloudConfig::default();
        for t in 0..200 {
            clouds.step(
                &mut world,
                &mut h,
                &wind,
                p.sea_level_y,
                p.sky_ceiling_y,
                t,
                &cfg,
            );
        }
        assert!(!clouds.is_empty(), "should form at least one parcel");
        assert!(clouds.total_mass() > 0.0);
        assert!(h.total_mass() < hum_before);
        // Parcels should sit well above the sea film.
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
    fn ridge_collision_lifts_parcel_above_surface() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        // Find a tall inland column.
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
        let mut clouds = CloudStore::new();
        let cfg = CloudConfig::default();
        clouds.parcels.push(CloudParcel {
            fx: peak_x as f32,
            fy: surface - 5.0, // intentionally inside the mountain
            mass: 80.0,
            raining: false,
            on_ridge: false,
            shape_seed: 1,
            cruise_fy: surface - 5.0,
            vis_mass: 80.0,
            deform: 0.0,
        });
        // Soft blend needs a few ticks to clear the ridge floor.
        let world = World::new(p.seed);
        for _ in 0..8 {
            clouds.advect_and_collide(&world, &wind, p.sea_level_y, p.sky_ceiling_y, &cfg);
        }
        let c = &clouds.parcels[0];
        let min_clear = surface + cfg.ridge_clearance * 0.5;
        assert!(
            c.fy > min_clear,
            "cloud fy={} must sit above surface+clearance {}",
            c.fy,
            min_clear
        );
        assert!(
            c.cruise_fy >= min_clear,
            "cruise altitude should stick after ridge lift (cruise={})",
            c.cruise_fy
        );
    }

    #[test]
    fn heavy_cloud_downpours_into_air() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut world = World::new(p.seed);
        let gx: i32 = 20;
        let top: i32 = 40;
        let floor: i32 = 5;
        // Ensure chunks covering cloud → ground.
        for y in [floor, top] {
            world.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        for x in (gx - 3)..=(gx + 3) {
            world.set_cell(x, floor, Cell::solid(MaterialId::Stone));
            for y in (floor + 1)..=top {
                world.set_cell(x, y, Cell::air());
            }
        }
        let mut clouds = CloudStore::new();
        clouds.parcels.push(CloudParcel {
            fx: gx as f32,
            fy: top as f32,
            mass: DOWNPOUR_MASS * 1.5,
            raining: false,
            on_ridge: false,
            shape_seed: 2,
            cruise_fy: top as f32,
            vis_mass: DOWNPOUR_MASS * 1.5,
            deform: 0.0,
        });
        let mass_before = clouds.total_mass();
        clouds.downpour(&mut world, &wind, 0, &CloudConfig::default(), None, None);
        assert!(clouds.total_mass() < mass_before);
        // Rain should land just above the stone floor, not hang at cloud height.
        let landed = world.get_cell(gx, floor + 1).map(|c| c.sat.0).unwrap_or(0);
        assert!(
            landed > 0,
            "downpour should deposit on the ground (got sat={landed})"
        );
        let high = world.get_cell(gx, top).map(|c| c.sat.0).unwrap_or(0);
        assert_eq!(high, 0, "rain must not hang in the sky at cloud height");
    }

    #[test]
    fn wind_moves_parcels() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut clouds = CloudStore::new();
        let cfg = CloudConfig::default();
        let fy = (p.sea_level_y + cfg.cloud_alt_above_sea) as f32;
        clouds.parcels.push(CloudParcel {
            fx: 50.0,
            fy,
            mass: 40.0,
            raining: false,
            on_ridge: false,
            shape_seed: 3,
            cruise_fy: fy,
            vis_mass: 40.0,
            deform: 0.0,
        });
        let x0 = clouds.parcels[0].fx;
        let world = World::new(p.seed);
        for _ in 0..80 {
            clouds.advect_and_collide(&world, &wind, p.sea_level_y, p.sky_ceiling_y, &cfg);
        }
        assert!(
            clouds.parcels[0].fx > x0,
            "parcels should drift with prevailing wind"
        );
    }
}
