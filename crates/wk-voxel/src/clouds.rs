//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Discrete cloud parcels: humidity coagulates into blobs, wind
//! carries them, and heavy parcels release a downpour back into the
//! cell grid (mass-conservative).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::wind::Wind;
use crate::worldgen::continental_surface_y;

/// Soft cap so cartoon skies stay readable.
pub const MAX_CLOUD_PARCELS: usize = 36;

/// Humidity mass a sky tile needs before it can seed / feed a cloud.
const COAG_MIN_HUM: f32 = 28.0;
/// Fraction of a tile's humidity sucked into a nearby parcel each step.
const COAG_RATE: f32 = 0.12;
/// Max humidity mass transferred into one parcel per tick.
const COAG_MAX_TAKE: f32 = 48.0;
/// Spawn a new parcel when no neighbour is within this many cells.
const SPAWN_RADIUS: f32 = 18.0;
/// Merge parcels closer than this (world cells).
const MERGE_DIST: f32 = 10.0;
/// Mass at which a parcel starts dumping rain.
pub const DOWNPOUR_MASS: f32 = 160.0;
/// Mass drained per downpour tick (split across columns under the cloud).
const DOWNPOUR_DRAIN: f32 = 55.0;
/// Stop dumping once mass falls below this fraction of the threshold.
const DOWNPOUR_STOP_FRAC: f32 = 0.35;
/// Preferred cloud altitude above sea level (cells).
const CLOUD_ALT_ABOVE_SEA: i32 = 28;

/// One wind-blown cloud blob in continuous world space.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CloudParcel {
    pub fx: f32,
    pub fy: f32,
    pub mass: f32,
    /// True while actively dumping rain this storm pulse.
    pub raining: bool,
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
    /// coagulate ← humidity, advect, merge, downpour → world.
    pub fn step(
        &mut self,
        world: &mut World,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
        tick: u64,
    ) {
        self.coagulate(humidity, wind, sea_level_y, sky_ceiling_y);
        self.advect(wind, sea_level_y, sky_ceiling_y);
        self.merge();
        self.downpour(world, wind, tick);
        // Drop empty wisps.
        self.parcels.retain(|p| p.mass > 1.0);
    }

    /// Pull humidity from wet sky tiles into nearby parcels (or spawn).
    fn coagulate(
        &mut self,
        humidity: &mut Humidity,
        wind: &Wind,
        sea_level_y: i32,
        sky_ceiling_y: i32,
    ) {
        let tc = humidity.tile_cols.max(1);
        let sky_hy_min = (sea_level_y + 6).div_euclid(tc);
        let sky_hy_max = (sky_ceiling_y - 2).div_euclid(tc);
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
                    let alt = (sea_level_y + CLOUD_ALT_ABOVE_SEA)
                        .min(sky_ceiling_y - 4)
                        .max(sea_level_y + 8);
                    self.parcels.push(CloudParcel {
                        fx: cx as f32,
                        fy: alt as f32,
                        mass: 0.0,
                        raining: false,
                    });
                    self.parcels.len() - 1
                }
                None => continue,
            };

            // Drain humidity tile.
            if let Some(entry) = humidity.cells.get_mut(&(hx, hy)) {
                *entry -= take;
                if *entry < 1e-3 {
                    humidity.cells.remove(&(hx, hy));
                }
            }
            self.parcels[idx].mass += take;
            // Gently pull parcel toward the feeding tile / wind.
            let p = &mut self.parcels[idx];
            p.fx = p.fx * 0.92 + cx as f32 * 0.08;
            let target_y = cy as f32;
            p.fy = p.fy * 0.95 + target_y * 0.05;
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

    fn advect(&mut self, wind: &Wind, sea_level_y: i32, sky_ceiling_y: i32) {
        let tc = wind.tile_cols.max(1) as f32;
        let vx = wind.climate_vx * tc; // cells / tick
        let y_lo = (sea_level_y + 8) as f32;
        let y_hi = (sky_ceiling_y - 3) as f32;
        let width = wind.width_cols.max(1) as f32;
        for p in &mut self.parcels {
            let hx = (p.fx / tc).floor() as i32;
            let vy = wind.vy_at(hx, 0) * tc;
            p.fx += vx;
            p.fy += vy * 0.5;
            if wind.wrap_x {
                p.fx = p.fx.rem_euclid(width);
            } else {
                p.fx = p.fx.clamp(0.0, width - 1.0);
            }
            p.fy = p.fy.clamp(y_lo, y_hi);
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
            let oro = orographic_boost(wind, p.fx);
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
            // Burst of columns under the cloud — heavier in the centre.
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

fn orographic_boost(wind: &Wind, fx: f32) -> f32 {
    let hx = (fx / wind.tile_cols.max(1) as f32).floor() as i32;
    let ascent = wind.ascent_cells(hx);
    let gx = fx.round() as i32;
    let s = continental_surface_y(wind.seed, gx, wind.sea_level_y, wind.width_cols);
    let tall = s >= wind.sea_level_y + 22;
    let mut boost = 1.0 + (ascent / 40.0).clamp(0.0, 1.0) * 0.8;
    if tall {
        boost += 0.35;
    }
    boost.clamp(1.0, 2.2)
}

/// Drop up to `budget` sat into the first open Air cell at/below `start_y`.
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
    // Slight jitter so the sheet isn't a perfect grid.
    let jx = world.wrap_x(gx + ((tick as i32 + salt * 3) % 3) - 1);
    let mut y = start_y;
    for _ in 0..48 {
        let Some(cell) = world.get_cell(jx, y) else {
            y -= 1;
            continue;
        };
        if cell.material != MaterialId::Air {
            // Try one cell above a solid as splash.
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
    use crate::chunk::ChunkCoord;
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
        let sky_y = p.sea_level_y + CLOUD_ALT_ABOVE_SEA;
        for x in 40..56 {
            h.add(x, sky_y, 80.0);
        }
        let hum_before = h.total_mass();
        let mut clouds = CloudStore::new();
        let mut world = World::new(p.seed);
        for t in 0..30 {
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
        assert!(
            clouds.total_mass() > 0.0,
            "cloud mass should be positive"
        );
        assert!(
            h.total_mass() < hum_before,
            "humidity should drain into clouds"
        );
    }

    #[test]
    fn heavy_cloud_downpours_into_air() {
        let p = WorldgenParams::default();
        let wind = wind_for(&p);
        let mut world = World::new(p.seed);
        world.ensure_chunk(ChunkCoord::new(0, 0));
        let gx = 20;
        let top = 40;
        // Wide Air pad — downpour jitters ±1 column.
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
        });
        let mass_before = clouds.total_mass();
        clouds.downpour(&mut world, &wind, 0);
        let mass_after = clouds.total_mass();
        assert!(mass_after < mass_before, "downpour drains cloud");
        let mut sat = 0i32;
        for x in (gx - 3)..=(gx + 3) {
            for y in 0..=top {
                if let Some(c) = world.get_cell(x, y) {
                    sat += c.sat.0 as i32;
                }
            }
        }
        assert!(
            sat > 0,
            "rain should land in the Air column (cloud mass {mass_before} → {mass_after}, sat={sat})"
        );
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
        });
        let x0 = clouds.parcels[0].fx;
        let mut h = Humidity::new(4);
        let mut world = World::new(1);
        for t in 0..20 {
            clouds.advect(&wind, p.sea_level_y, p.sky_ceiling_y);
            let _ = (t, &mut h, &mut world);
        }
        assert!(
            clouds.parcels[0].fx > x0,
            "prevailing +x wind should carry the parcel"
        );
    }
}
