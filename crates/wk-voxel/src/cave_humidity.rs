//! Sealed-cave ambient humidity — sparse mini field, not sky weather.
//!
//! **Store:** [`World::cave_humidity`] is a capped sparse map of moist Air
//! mass inside **roofed / side-closed** voids. Same sat mass units as free
//! water. It is **not** sky [`Humidity`] and **not** pressurized
//! [`World::steam`].
//!
//! **Why:** Open caves / overhangs share sky Humidity (T5). Sealed voids under
//! rock need a second mini field for ordinary closed-cave air — any ambient
//! moisture, not a pressure or high-RH prerequisite — without minting geyser
//! steam or expanding the weather walk underground.
//!
//! **Motor:** sealed-film evaporation deposits here. On cadence:
//! - seats that open to sky hand mass to weather Humidity (or drip liquid);
//! - surplus above the Magnus cell capacity at local T recondenses into Air
//!   sat (mass-flat). Diffusion inside sealed voids is still later.
//! Never writes the rain lottery.
//!
//! **Look:** [`cave_humidity_haze_wash`] builds the same 4×4 soft white vapour
//! grain as sky Humidity / steam haze — painted under the `H` overlay.

use crate::cell::{water_capacity_cell, Cell, Sat};
use crate::fasthash::{FxHashMap, FxHashSet};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::temperature::Temperature;
use wk_material::MaterialId;

/// Hard cap — sealed moist air is local, not world-filling.
pub const MAX_CAVE_HUMIDITY_CELLS: usize = 4096;

/// Same cadence as steam — sparse seats, not every tick.
pub const CAVE_HUMIDITY_EVERY: u64 = 5;

/// Units at `(gx, gy)`, or 0.
#[inline]
pub fn cave_humidity_at(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    world.cave_humidity.get(&(gx, gy)).copied().unwrap_or(0)
}

/// Total sealed-cave humidity mass (sat units).
pub fn cave_humidity_total(world: &World) -> u64 {
    world.cave_humidity.values().map(|&v| v as u64).sum()
}

/// Add up to `want` units into a sealed-cave seat. Returns accepted mass.
///
/// Refuses new seats once [`MAX_CAVE_HUMIDITY_CELLS`] is reached (existing
/// seats may still fill toward 255).
pub fn try_add_cave_humidity(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    if want == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let key = (gx, gy);
    let cur = world.cave_humidity.get(&key).copied().unwrap_or(0);
    let room = 255u8.saturating_sub(cur);
    if room == 0 {
        return 0;
    }
    if cur == 0 && world.cave_humidity.len() >= MAX_CAVE_HUMIDITY_CELLS {
        return 0;
    }
    let take = want.min(room);
    world.cave_humidity.insert(key, cur + take);
    take
}

/// Remove up to `want` units. Returns how much was taken.
pub fn take_cave_humidity(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    if want == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let key = (gx, gy);
    let cur = world.cave_humidity.get(&key).copied().unwrap_or(0);
    if cur == 0 {
        return 0;
    }
    let take = want.min(cur);
    let left = cur - take;
    if left == 0 {
        world.cave_humidity.remove(&key);
    } else {
        world.cave_humidity.insert(key, left);
    }
    take
}

/// Debug / tests: replace the sparse map.
pub fn replace_cave_humidity(world: &mut World, map: FxHashMap<(i32, i32), u8>) {
    world.cave_humidity = map;
}

/// Soft white wash sample — same grain as sky Humidity / [`crate::steam::SteamHazeSample`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaveHumidityHazeSample {
    pub gx: i32,
    pub gy: i32,
    /// Visual density 0..=255 (maps to soft haze alpha, not opaque fill).
    pub density: u8,
}

/// Same 4×4 tile grain as sky Humidity and steam haze.
pub const CAVE_HUMIDITY_HAZE_TILE: i32 = 4;


/// Free-water / standing pool — vapour wash must stop at the surface.
#[inline]
fn is_free_water_seat(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if cell.material != MaterialId::Air {
        return true;
    }
    // Same free-water read as sky wind/haze: near-full Air sat is a pool cell.
    if cell.sat.0 >= 200 {
        return true;
    }
    crate::rules::is_standing_water(world, gx, gy)
}

/// Air volume that can hold cave humidity vapour (above the waterline).
#[inline]
fn is_cave_humidity_void(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    cell.material == MaterialId::Air && !is_free_water_seat(world, gx, gy, cell)
}

/// Build a sky-Humidity-shaped wash over sealed cave air.
///
/// Mass bins onto coarse tiles, then each sealed Air cell bilinear-samples
/// the field. No pressure/heat tint — ambient cave vapour reads as the same
/// soft white wash as the `H` overlay.
pub fn cave_humidity_haze_wash(world: &World) -> Vec<CaveHumidityHazeSample> {
    if world.cave_humidity.is_empty() {
        return Vec::new();
    }
    let tc = CAVE_HUMIDITY_HAZE_TILE.max(1);
    let mut tile_mass: FxHashMap<(i32, i32), f32> = FxHashMap::default();

    for (&(gx, gy), &amt) in world.cave_humidity.iter() {
        if amt == 0 {
            continue;
        }
        let gx = world.wrap_x(gx);
        let hx = gx.div_euclid(tc);
        let hy = gy.div_euclid(tc);
        *tile_mass.entry((hx, hy)).or_insert(0.0) += amt as f32;
    }

    // Spread equalized mass across connected sealed Air so a pocket washes
    // as one vapour field (same idea as steam haze, without pressure).
    let budget = 96usize;
    let seeds: Vec<(i32, i32)> = world.cave_humidity.keys().copied().collect();
    let mut visited: FxHashSet<(i32, i32)> = FxHashSet::default();
    for (sx, sy) in seeds {
        let sx = world.wrap_x(sx);
        if !visited.insert((sx, sy)) {
            continue;
        }
        if crate::steam::air_void_open_to_sky(world, sx, sy) {
            continue;
        }
        let mut queue = vec![(sx, sy)];
        let mut voids: Vec<(i32, i32)> = Vec::new();
        let mut qi = 0;
        while qi < queue.len() && voids.len() < budget {
            let (cx, cy) = queue[qi];
            qi += 1;
            let Some(cell) = world.get_cell(cx, cy) else {
                continue;
            };
            if !is_cave_humidity_void(world, cx, cy, cell) {
                continue;
            }
            if crate::steam::air_void_open_to_sky(world, cx, cy) {
                continue;
            }
            voids.push((cx, cy));
            for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                let nx = world.wrap_x(cx + dx);
                let ny = cy + dy;
                if !visited.insert((nx, ny)) {
                    continue;
                }
                if world.get_cell(nx, ny).is_some_and(|n| is_cave_humidity_void(world, nx, ny, n)) {
                    queue.push((nx, ny));
                }
            }
        }
        if voids.is_empty() {
            continue;
        }
        let total: f32 = voids
            .iter()
            .map(|&(x, y)| cave_humidity_at(world, x, y) as f32)
            .sum();
        if total <= 0.0 {
            continue;
        }
        let mut tile_void_n: FxHashMap<(i32, i32), u32> = FxHashMap::default();
        for &(x, y) in &voids {
            let hx = x.div_euclid(tc);
            let hy = y.div_euclid(tc);
            *tile_void_n.entry((hx, hy)).or_insert(0) += 1;
        }
        let n_voids = voids.len() as f32;
        for ((hx, hy), n) in tile_void_n {
            let share = total * (n as f32 / n_voids);
            let m = tile_mass.entry((hx, hy)).or_insert(0.0);
            *m = (*m).max(share);
        }
    }

    if tile_mass.is_empty() {
        return Vec::new();
    }

    let peak = tile_mass.values().copied().fold(0.0_f32, f32::max).max(1.0);

    let mut seats: FxHashSet<(i32, i32)> = tile_mass.keys().copied().collect();
    let occupied: Vec<(i32, i32)> = seats.iter().copied().collect();
    for (hx, hy) in occupied {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            seats.insert((hx + dx, hy + dy));
        }
    }

    let mut out: Vec<CaveHumidityHazeSample> = Vec::new();
    for (hx, hy) in seats {
        for ly in 0..tc {
            for lx in 0..tc {
                let gx = world.wrap_x(hx * tc + lx);
                let gy = hy * tc + ly;
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                if !is_cave_humidity_void(world, gx, gy, cell) {
                    continue;
                }
                // Open shafts belong to sky H — don't double-paint.
                if crate::steam::air_void_open_to_sky(world, gx, gy) {
                    continue;
                }
                let mass =
                    sample_cave_tile_bilinear(&tile_mass, tc, gx as f32 + 0.5, gy as f32 + 0.5);
                if mass <= 0.05 {
                    continue;
                }
                let norm = (mass / peak).clamp(0.0, 1.0);
                // Soft floor like steam/H — stay well below opaque fill.
                let density = ((28.0 + norm.sqrt() * 180.0).round() as u8).min(200);
                out.push(CaveHumidityHazeSample { gx, gy, density });
            }
        }
    }
    out
}

fn sample_cave_tile_bilinear(
    tiles: &FxHashMap<(i32, i32), f32>,
    tile_cols: i32,
    gx: f32,
    gy: f32,
) -> f32 {
    let tc = tile_cols.max(1) as f32;
    let fx = gx / tc - 0.5;
    let fy = gy / tc - 0.5;
    let x0 = fx.floor() as i32;
    let y0 = fy.floor() as i32;
    let tx = fx - x0 as f32;
    let ty = fy - y0 as f32;
    let v00 = tiles.get(&(x0, y0)).copied().unwrap_or(0.0);
    let v10 = tiles.get(&(x0 + 1, y0)).copied().unwrap_or(0.0);
    let v01 = tiles.get(&(x0, y0 + 1)).copied().unwrap_or(0.0);
    let v11 = tiles.get(&(x0 + 1, y0 + 1)).copied().unwrap_or(0.0);
    let a = v00 + (v10 - v00) * tx;
    let b = v01 + (v11 - v01) * tx;
    a + (b - a) * ty
}

/// Cadenced motor: open-vent handoff + cool surplus → Air sat.
pub fn apply_cave_humidity(
    world: &mut World,
    temp: &Temperature,
    humidity: &mut Humidity,
) {
    if world.cave_humidity.is_empty() {
        return;
    }
    if world.tick % CAVE_HUMIDITY_EVERY.max(1) != 0 {
        return;
    }
    let keys: Vec<(i32, i32)> = world.cave_humidity.keys().copied().collect();
    for (gx, gy) in keys {
        let Some(cell) = world.get_cell(gx, gy) else {
            world.cave_humidity.remove(&(gx, gy));
            continue;
        };
        if cell.material != MaterialId::Air {
            world.cave_humidity.remove(&(gx, gy));
            continue;
        }
        // Flooded pool seats: vapour stops at the waterline — drip into sat.
        if is_free_water_seat(world, gx, gy, cell) {
            let hum = cave_humidity_at(world, gx, gy);
            if hum > 0 {
                let took = take_cave_humidity(world, gx, gy, hum);
                let _ = drip_into_air(world, gx, gy, took);
            } else {
                world.cave_humidity.remove(&(gx, gy));
            }
            continue;
        }
        let hum = cave_humidity_at(world, gx, gy);
        if hum == 0 {
            world.cave_humidity.remove(&(gx, gy));
            continue;
        }

        // Side vent / skylight opened → same store as free sky.
        if crate::steam::air_void_open_to_sky(world, gx, gy) {
            let took = take_cave_humidity(world, gx, gy, hum);
            let t_c = temp.at_cell(gx, gy);
            let accepted = humidity
                .try_add_at_temp(gx, gy, took as f32, t_c)
                .round()
                .clamp(0.0, 255.0) as u8;
            let left = took.saturating_sub(accepted);
            if left > 0 {
                let _ = drip_into_air(world, gx, gy, left);
            }
            continue;
        }

        // Capacity from the same Magnus curve as sky H, scaled to a cell.
        let cap = Humidity::saturation_cell_sat_at_temp(temp.at_cell(gx, gy))
            .floor()
            .clamp(0.0, 255.0) as u8;
        if hum <= cap {
            continue;
        }
        let surplus = hum - cap;
        let put = drip_into_air(world, gx, gy, surplus);
        if put > 0 {
            let _ = take_cave_humidity(world, gx, gy, put);
        }
    }
}

/// Mass-flat: move vapour units into Air sat room. Returns placed amount.
fn drip_into_air(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    if want == 0 {
        return 0;
    }
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    if cell.material != MaterialId::Air {
        return 0;
    }
    let cap = water_capacity_cell(cell, &world.hydro);
    let room = cap.saturating_sub(cell.sat.0);
    let put = want.min(room);
    if put == 0 {
        return 0;
    }
    world.set_cell(
        gx,
        gy,
        Cell {
            sat: Sat(cell.sat.0.saturating_add(put)),
            ..cell
        },
    );
    put
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use crate::grid::World;

    fn hot_fill(world: &World, c: f32) -> Temperature {
        let mut t = Temperature::with_world_bounds(4, 0, 0, 64, 64, world.seed.0, 64, 20, false);
        for v in t.cells.values_mut() {
            *v = c;
        }
        t
    }

    fn sealed_pocket(w: &mut World) {
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..=5 {
            for y in 1..=3 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut air = Cell::air();
        air.sat = Sat(0);
        w.set_cell(4, 2, air);
    }

    #[test]
    fn try_add_fills_and_caps_per_cell() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        assert_eq!(try_add_cave_humidity(&mut w, 2, 3, 200), 200);
        assert_eq!(cave_humidity_at(&w, 2, 3), 200);
        assert_eq!(try_add_cave_humidity(&mut w, 2, 3, 100), 55);
        assert_eq!(cave_humidity_at(&w, 2, 3), 255);
        assert_eq!(cave_humidity_total(&w), 255);
    }

    #[test]
    fn cool_surplus_recondenses_into_air_sat_mass_flat() {
        let mut w = World::new(1);
        sealed_pocket(&mut w);
        assert!(!crate::steam::air_void_open_to_sky(&w, 4, 2));
        let _ = try_add_cave_humidity(&mut w, 4, 2, 200);
        let before = cave_humidity_total(&w)
            + w.get_cell(4, 2).map(|c| c.sat.0 as u64).unwrap_or(0);
        let cool = hot_fill(&w, 0.0);
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        w.tick = CAVE_HUMIDITY_EVERY;
        apply_cave_humidity(&mut w, &cool, &mut h);
        let after = cave_humidity_total(&w)
            + w.get_cell(4, 2).map(|c| c.sat.0 as u64).unwrap_or(0);
        assert_eq!(before, after, "recondense is mass-flat");
        let cap = Humidity::saturation_cell_sat_at_temp(0.0).floor() as u8;
        assert!(
            cave_humidity_at(&w, 4, 2) <= cap,
            "surplus above cool capacity must drip"
        );
        assert!(
            w.get_cell(4, 2).unwrap().sat.0 > 0,
            "dripped into Air sat"
        );
        assert_eq!(h.total_mass(), 0.0, "sealed seat does not touch sky H");
    }

    #[test]
    fn open_vent_hands_off_to_sky_humidity() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for y in 0..32 {
            w.set_cell(4, y, Cell::air());
        }
        assert!(crate::steam::air_void_open_to_sky(&w, 4, 2));
        let _ = try_add_cave_humidity(&mut w, 4, 2, 80);
        let warm = hot_fill(&w, 25.0);
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        w.tick = CAVE_HUMIDITY_EVERY;
        apply_cave_humidity(&mut w, &warm, &mut h);
        assert_eq!(cave_humidity_at(&w, 4, 2), 0, "open seats leave cave_humidity");
        assert!(h.total_mass() > 0.0, "mass moves into sky Humidity");
    }

    #[test]
    fn warm_capacity_holds_without_drip() {
        let mut w = World::new(1);
        sealed_pocket(&mut w);
        let _ = try_add_cave_humidity(&mut w, 4, 2, 40);
        let warm = hot_fill(&w, 30.0);
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        w.tick = CAVE_HUMIDITY_EVERY;
        apply_cave_humidity(&mut w, &warm, &mut h);
        assert_eq!(cave_humidity_at(&w, 4, 2), 40);
        assert_eq!(w.get_cell(4, 2).unwrap().sat.0, 0);
    }

        #[test]
    fn sealed_pocket_washes_soft_white_haze() {
        let mut w = World::new(1);
        sealed_pocket(&mut w);
        let _ = try_add_cave_humidity(&mut w, 4, 2, 180);
        let wash = cave_humidity_haze_wash(&w);
        assert!(
            wash.iter().any(|s| s.gx == 4 && s.gy == 2 && s.density > 0),
            "sealed moist air must paint a haze seat"
        );
        assert!(
            wash.iter().all(|s| s.density <= 200),
            "haze stays soft, never opaque fill"
        );
    }

    #[test]
    fn haze_stops_at_cave_pool_surface() {
        let mut w = World::new(1);
        sealed_pocket(&mut w);
        // Tall sealed shaft: stone shell, full pool on floor, dry air above.
        for x in 3..=5 {
            for y in 1..=4 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut pool = Cell::air();
        pool.sat = Sat(255);
        w.set_cell(4, 2, pool);
        let mut air = Cell::air();
        air.sat = Sat(0);
        w.set_cell(4, 3, air);
        let _ = try_add_cave_humidity(&mut w, 4, 3, 180);
        let _ = try_add_cave_humidity(&mut w, 4, 2, 180);
        let wash = cave_humidity_haze_wash(&w);
        assert!(
            wash.iter().any(|s| s.gx == 4 && s.gy == 3 && s.density > 0),
            "vapour above the pool must still wash"
        );
        assert!(
            wash.iter().all(|s| !(s.gx == 4 && s.gy == 2)),
            "haze must stop at the water surface (no wash in the pool)"
        );
    }
}
