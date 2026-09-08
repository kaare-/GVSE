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

use crate::cell::{water_capacity_cell, Cell, Sat};
use crate::fasthash::FxHashMap;
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
}
