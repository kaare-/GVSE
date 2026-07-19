//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Discrete cloud parcels: humidity rises, then coagulates into blobs;
//! wind carries them; they ride above the terrain (no clipping through
//! mountains) and dump rain sooner when scraping ridges.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::wind::Wind;
use crate::worldgen::continental_surface_y;

/// Soft cap so cartoon skies stay readable.
pub const MAX_CLOUD_PARCELS: usize = 36;

/// Humidity mass a risen tile needs before it can seed / feed a cloud.
const COAG_MIN_HUM: f32 = 28.0;
/// Fraction of a tile's humidity sucked into a nearby parcel each step.
const COAG_RATE: f32 = 0.12;
/// Max humidity mass transferred into one parcel per tick.
const COAG_MAX_TAKE: f32 = 48.0;
/// Spawn a new parcel when no neighbour is within this many cells.
const SPAWN_RADIUS: f32 = 18.0;
/// Merge parcels closer than this (world cells).
const MERGE_DIST: f32 = 10.0;
/// Mass at which a parcel starts dumping rain (lower on ridges).
pub const DOWNPOUR_MASS: f32 = 160.0;
/// Mass drained per downpour tick (split across columns under the cloud).
const DOWNPOUR_DRAIN: f32 = 55.0;
/// Stop dumping once mass falls below this fraction of the threshold.
const DOWNPOUR_STOP_FRAC: f32 = 0.35;
/// Preferred free-air cloud altitude above sea (cells).
const CLOUD_ALT_ABOVE_SEA: i32 = 32;
/// Vapor must rise at least this far above sea before coagulating.
const COAG_MIN_ABOVE_SEA: i32 = 18;
/// Clearance of cloud centre above solid surface (plus radius factor).
const RIDGE_CLEARANCE: f32 = 3.0;

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
}

impl CloudParcel {
    /// Visual / rain footprint radius in world cells.
    pub fn radius(&self) -> f32 {
        (6.0 + (self.mass / 40.0).sqrt() * 3.5).clamp(6.0, 22.0)
    }

    /// 0..1 wetness for drawing (relative to downpour threshold).
    pub fn wetness(&self) -> f32 {
        (self.mass / DOWNPOUR_MASS).clamp(0.0, 1.5) / 1.5
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
    ) {
        // Let ocean vapor climb into the cloud deck before clumping.
        let tc = humidity.tile_cols.max(1);
        let deck_hy = (sea_level_y + CLOUD_ALT_ABOVE_SEA).div_euclid(tc);
        humidity.buoyant_rise(0.08, deck_hy);

        self.coagulate(humidity, wind, sea_level_y, sky_ceiling_y);
        self.advect_and_collide(wind, sea_level_y, sky_ceiling_y);
        self.merge();
        self.downpour(world, wind, tick);
        self.parcels.retain(|p| p.mass > 1.0);
    }

    /// Pull humidity only from risen sky tiles into parcels.
    fn coagulate(
        &mut self,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
    ) {
        let tc = humidity.tile_cols.max(1);
        let sky_hy_min = (sea_level_y + COAG_MIN_ABOVE_SEA).div_euclid(tc);
        let sky_hy_max = (sky_ceiling_y - 2).div_euclid(tc);
        let preferred_alt = (sea_level_y + CLOUD_ALT_ABOVE_SEA)
            .min(sky_ceiling_y - 4)
            .max(sea_level_y + COAG_MIN_ABOVE_SEA) as f32;
        let keys: Vec<(i32, i32)> = humidity.cells.keys().copied().collect();
        for (hx, hy) in keys {
            if hy < sky_hy_min || hy > sky_hy_max {
                continue;
            }
            let mass = humidity.at_tile(hx, hy);
            if mass < COAG_MIN_HUM {
                continue;
            }
            let cx = hx * tc + tc / 2;
            let cy = hy * tc + tc / 2;
            let take = (mass * COAG_RATE).min(COAG_MAX_TAKE).min(mass);
            if take <= 0.0 {
                continue;
            }

            let idx = self.nearest_parcel(cx as f32, cy as f32, SPAWN_RADIUS);
            let idx = match idx {
                Some(i) => i,
                None if self.parcels.len() < MAX_CLOUD_PARCELS => {
                    self.parcels.push(CloudParcel {
                        fx: cx as f32,
                        // Spawn at the risen vapor altitude, not the sea film.
                        fy: (cy as f32).max(preferred_alt),
                        mass: 0.0,
                        raining: false,
                        on_ridge: false,
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
            // Horizontal drift toward the feeding column only — never
            // drag the parcel back down to the sea surface.
            p.fx = p.fx * 0.92 + cx as f32 * 0.08;
            let target_y = (cy as f32).max(preferred_alt * 0.85);
            if target_y + 1.0 >= p.fy {
                p.fy = p.fy * 0.97 + target_y * 0.03;
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

    /// Wind drift, then soft collision with the land surface so clouds
    /// ride ridge tops instead of clipping through the mountain body.
    fn advect_and_collide(&mut self, wind: &Wind, sea_level_y: i32, sky_ceiling_y: i32) {
        let tc = wind.tile_cols.max(1) as f32;
        let vx = wind.climate_vx * tc;
        let y_lo = (sea_level_y + COAG_MIN_ABOVE_SEA) as f32;
        let y_hi = (sky_ceiling_y - 3) as f32;
        let width = wind.width_cols.max(1) as f32;
        for p in &mut self.parcels {
            p.on_ridge = false;
            let hx = (p.fx / tc).floor() as i32;
            let vy = wind.vy_at(hx, 0) * tc;
            p.fx += vx;
            p.fy += vy * 0.5;
            if wind.wrap_x {
                p.fx = p.fx.rem_euclid(width);
            } else {
                p.fx = p.fx.clamp(0.0, width - 1.0);
            }

            // Sample surface under the parcel centre and near edges.
            let r = p.radius();
            let mut floor = surface_y(wind, p.fx);
            floor = floor.max(surface_y(wind, p.fx - r * 0.5));
            floor = floor.max(surface_y(wind, p.fx + r * 0.5));
            // Over ocean, keep a free-air deck; over land, ride the top.
            let land = floor > sea_level_y as f32 + 1.0;
            let min_fy = if land {
                floor + RIDGE_CLEARANCE + r * 0.25
            } else {
                y_lo.max(sea_level_y as f32 + CLOUD_ALT_ABOVE_SEA as f32 * 0.55)
            };
            if p.fy < min_fy {
                p.fy = min_fy;
                if land {
                    p.on_ridge = true;
                }
            }
            p.fy = p.fy.clamp(y_lo.min(min_fy), y_hi);
        }
    }

    fn merge(&mut self) {
        let mut i = 0;
        while i < self.parcels.len() {
            let mut j = i + 1;
            while j < self.parcels.len() {
                let dx = self.parcels[i].fx - self.parcels[j].fx;
                let dy = self.parcels[i].fy - self.parcels[j].fy;
                if (dx * dx + dy * dy).sqrt() < MERGE_DIST {
                    let other = self.parcels.swap_remove(j);
                    let a = &mut self.parcels[i];
                    let total = a.mass + other.mass;
                    if total > 0.0 {
                        a.fx = (a.fx * a.mass + other.fx * other.mass) / total;
                        a.fy = (a.fy * a.mass + other.fy * other.mass) / total;
                    }
                    a.mass = total;
                    a.raining = a.raining || other.raining;
                    a.on_ridge = a.on_ridge || other.on_ridge;
                } else {
                    j += 1;
                }
            }
            i += 1;
        }
    }

    /// Heavy parcels dump sat into Air cells beneath them.
    fn downpour(&mut self, world: &mut World, wind: &Wind, tick: u64) {
        for p in &mut self.parcels {
            let mut oro = orographic_boost(wind, p.fx);
            if p.on_ridge {
                // Scraping a peak — dump sooner / harder.
                oro = (oro + 0.85).min(2.8);
            }
            let trigger = DOWNPOUR_MASS / oro;
            if !p.raining {
                if p.mass >= trigger {
                    p.raining = true;
                } else {
                    continue;
                }
            }
            if p.mass < DOWNPOUR_MASS * DOWNPOUR_STOP_FRAC {
                p.raining = false;
                continue;
            }

            let drain = (DOWNPOUR_DRAIN * oro).min(p.mass);
            let radius = p.radius();
            let cols = ((radius * 1.6) as i32).clamp(3, 16);
            let mut remaining = drain;
            for k in 0..cols {
                if remaining <= 0.0 {
                    break;
                }
                let t = if cols == 1 {
                    0.0
                } else {
                    k as f32 / (cols - 1) as f32 * 2.0 - 1.0
                };
                let gx = world.wrap_x((p.fx + t * radius * 0.85).round() as i32);
                let share = (drain / cols as f32) * (1.15 - 0.3 * t.abs());
                let dropped = deposit_rain_column(world, gx, p.fy.round() as i32, share, tick, k);
                remaining -= dropped;
                p.mass -= dropped;
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

fn orographic_boost(wind: &Wind, fx: f32) -> f32 {
    let hx = (fx / wind.tile_cols.max(1) as f32).floor() as i32;
    let ascent = wind.ascent_cells(hx);
    let s = surface_y(wind, fx);
    let tall = s >= wind.sea_level_y as f32 + 22.0;
    let mut boost = 1.0 + (ascent / 40.0).clamp(0.0, 1.0) * 0.8;
    if tall {
        boost += 0.35;
    }
    boost.clamp(1.0, 2.2)
}

fn deposit_rain_column(
    world: &mut World,
    gx: i32,
    start_y: i32,
    budget: f32,
    tick: u64,
    salt: i32,
) -> f32 {
    if budget <= 0.0 {
        return 0.0;
    }
    let jx = world.wrap_x(gx + ((tick as i32 + salt * 3) % 3) - 1);
    let mut y = start_y;
    for _ in 0..48 {
        let Some(cell) = world.get_cell(jx, y) else {
            y -= 1;
            continue;
        };
        if cell.material != MaterialId::Air {
            if let Some(above) = world.get_cell(jx, y + 1) {
                if above.material == MaterialId::Air {
                    return fill_sat(world, jx, y + 1, above, budget);
                }
            }
            return 0.0;
        }
        if !cell.sat.is_full() {
            return fill_sat(world, jx, y, cell, budget);
        }
        y -= 1;
    }
    0.0
}

fn fill_sat(world: &mut World, gx: i32, gy: i32, cell: Cell, budget: f32) -> f32 {
    let free = u8::MAX as f32 - cell.sat.0 as f32;
    let transfer = budget.min(free).max(0.0);
    let u = transfer.round() as i32;
    if u <= 0 {
        return 0.0;
    }
    let new_sat = (cell.sat.0 as i32 + u).clamp(0, u8::MAX as i32) as u8;
    world.set_cell(
        gx,
        gy,
        Cell {
            sat: Sat(new_sat),
            ..cell
        },
    );
    u as f32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
    use crate::worldgen::WorldgenParams;

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
        for t in 0..80 {
            clouds.step(
                &mut world,
                &mut h,
                &wind,
                p.sea_level_y,
                p.sky_ceiling_y,
                t,
            );
        }
        assert!(!clouds.is_empty(), "should form at least one parcel");
        assert!(clouds.total_mass() > 0.0);
        assert!(h.total_mass() < hum_before);
        // Parcels should sit well above the sea film.
        for pcloud in &clouds.parcels {
            assert!(
                pcloud.fy > p.sea_level_y as f32 + COAG_MIN_ABOVE_SEA as f32 * 0.5,
                "cloud fy={} too low (sea={})",
                pcloud.fy,
                p.sea_level_y
            );
        }
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
        clouds.parcels.push(CloudParcel {
            fx: peak_x as f32,
            fy: surface - 5.0, // intentionally inside the mountain
            mass: 80.0,
            raining: false,
            on_ridge: false,
        });
        clouds.advect_and_collide(&wind, p.sea_level_y, p.sky_ceiling_y);
        let c = &clouds.parcels[0];
        assert!(c.on_ridge, "should register ridge contact");
        assert!(
            c.fy > surface,
            "cloud fy={} must sit above surface {}",
            c.fy,
            surface
        );
    }

    #[test]
    fn heavy_cloud_downpours_into_air() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut world = World::new(p.seed);
        let gx: i32 = 20;
        let top: i32 = 40;
        let cc = ChunkCoord::new(
            gx.div_euclid(CHUNK_CELLS_W as i32),
            top.div_euclid(CHUNK_CELLS_H as i32),
        );
        world.ensure_chunk(cc);
        for x in (gx - 3)..=(gx + 3) {
            for y in 0..=top {
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
        });
        let mass_before = clouds.total_mass();
        clouds.downpour(&mut world, &wind, 0);
        assert!(clouds.total_mass() < mass_before);
        let mut sat = 0i32;
        for x in (gx - 3)..=(gx + 3) {
            for y in 0..=top {
                if let Some(c) = world.get_cell(x, y) {
                    sat += c.sat.0 as i32;
                }
            }
        }
        assert!(sat > 0);
    }

    #[test]
    fn wind_moves_parcels() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut clouds = CloudStore::new();
        clouds.parcels.push(CloudParcel {
            fx: 50.0,
            fy: (p.sea_level_y + CLOUD_ALT_ABOVE_SEA) as f32,
            mass: 40.0,
            raining: false,
            on_ridge: false,
        });
        let x0 = clouds.parcels[0].fx;
        for _ in 0..20 {
            clouds.advect_and_collide(&wind, p.sea_level_y, p.sky_ceiling_y);
        }
        assert!(clouds.parcels[0].fx > x0);
    }
}
