//! Sparse conduit steam — boil free water above ~100 °C (geyser P3).
//!
//! Humidity stays the coarse sky field. Steam here is **void/Air-only**
//! markers: boil takes [`Cell::sat`] from Air into `World.steam`, rise /
//! recondense move it back. Confined caves (solid roof overhead) keep
//! steam and pressurize confined rise — that is the underground boil
//! motor. Open surface steam rises and recondenses when cool.
//!
//! Hard-capped. No world-wide vapour CA. See docs/VOXEL_GEYSER.md § P3.

use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::temperature::Temperature;

/// Cadence for boil / rise / recondense.
pub const STEAM_EVERY: u64 = 5;

/// Default boil point (°C). Free Air sat at or above this may flash to steam.
pub const BOIL_POINT_C: f32 = 100.0;

/// Recondense when cooler than boil by this margin (hysteresis).
pub const RECONDENSE_MARGIN_C: f32 = 5.0;

/// Hard cap on cells that may hold steam. Hundreds, not world-wide.
pub const MAX_STEAM_CELLS: usize = 512;

/// Max sat→steam units boiled in one cell per cadence tick.
pub const BOIL_MAX_PER_CELL: u8 = 24;

/// Max steam units that may rise one cell per cadence tick.
pub const RISE_MAX_PER_CELL: u8 = 32;

/// How far up we walk to decide "open sky" vs solid roof.
const ROOF_PROBE: i32 = 48;

/// How far down a shaft we sum steam for pressure.
const PRESSURE_DEPTH: i32 = 16;

/// Confined-rise multiplier span at full steam pressure (stacks with geo).
pub const STEAM_PRESSURE_RATE_SPAN: f32 = 0.55;

/// Steam units at `(gx, gy)`, or 0.
#[inline]
pub fn steam_at(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    world.steam.get(&(gx, gy)).copied().unwrap_or(0)
}

#[inline]
pub fn add_steam(world: &mut World, gx: i32, gy: i32, amount: u8) {
    if amount == 0 {
        return;
    }
    let gx = world.wrap_x(gx);
    let slot = world.steam.entry((gx, gy)).or_insert(0);
    *slot = slot.saturating_add(amount);
}

/// Remove up to `want`, returning what was taken.
pub fn take_steam(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    if want == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let Some(slot) = world.steam.get_mut(&(gx, gy)) else {
        return 0;
    };
    let took = (*slot).min(want);
    *slot -= took;
    if *slot == 0 {
        world.steam.remove(&(gx, gy));
    }
    took
}

/// True when this Air void sits under a solid roof (cave / conduit).
///
/// Surface ponds and open shafts that reach missing sky return `false`.
pub fn void_is_confined(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    for dy in 1..=ROOF_PROBE {
        match world.get_cell(gx, gy + dy) {
            None => return false,
            Some(c) if c.material == MaterialId::Air => continue,
            Some(_) => return true,
        }
    }
    // Tall open column — treat as vented.
    false
}

/// 0..=1 pressure from steam stacked in this column (and ortho neighbours).
pub fn steam_pressure_norm(world: &World, gx: i32, gy: i32) -> f32 {
    if world.steam.is_empty() {
        return 0.0;
    }
    let gx = world.wrap_x(gx);
    let mut sum = 0u32;
    for dx in [-1_i32, 0, 1] {
        let x = world.wrap_x(gx + dx);
        for dy in 0..=PRESSURE_DEPTH {
            sum += steam_at(world, x, gy - dy) as u32;
        }
    }
    (sum as f32 / (255.0 * 6.0)).clamp(0.0, 1.0)
}

/// Confined-rise rate boost from underground steam (1 + span * norm).
#[inline]
pub fn steam_pressure_rate_scale(world: &World, gx: i32, gy: i32) -> f32 {
    1.0 + STEAM_PRESSURE_RATE_SPAN * steam_pressure_norm(world, gx, gy)
}

fn can_admit_new_steam_cell(world: &World, gx: i32, gy: i32) -> bool {
    if world.steam.contains_key(&(world.wrap_x(gx), gy)) {
        return true;
    }
    world.steam.len() < MAX_STEAM_CELLS
}

/// Boil / rise / recondense free-water steam.
///
/// Only walks wet-Air chunks (boil) and existing steam keys (rise /
/// recondense). Humidity is untouched.
pub fn apply_steam(world: &mut World, temp: &Temperature, boil_point_c: f32) {
    if world.tick % STEAM_EVERY != 0 {
        return;
    }
    let boil = boil_point_c;
    let recondense_below = boil - RECONDENSE_MARGIN_C;

    // 1) Recondense cool steam → Air sat (mass-flat).
    if !world.steam.is_empty() {
        let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
        for (gx, gy) in keys {
            if temp.at_cell(gx, gy) >= recondense_below {
                continue;
            }
            let Some(cell) = world.get_cell(gx, gy) else {
                // Orphan steam with no cell — drop (should be rare).
                world.steam.remove(&(gx, gy));
                continue;
            };
            if cell.material != MaterialId::Air {
                // Host became solid; push steam up into Air if possible.
                if let Some(up) = world.get_cell(gx, gy + 1) {
                    if up.material == MaterialId::Air {
                        let moved = take_steam(world, gx, gy, steam_at(world, gx, gy));
                        add_steam(world, gx, gy + 1, moved);
                    }
                }
                continue;
            }
            let steam = steam_at(world, gx, gy);
            if steam == 0 {
                continue;
            }
            let room = u8::MAX.saturating_sub(cell.sat.0);
            let put = steam.min(room);
            if put == 0 {
                continue;
            }
            let took = take_steam(world, gx, gy, put);
            let mut next = cell;
            next.sat = Sat(cell.sat.0.saturating_add(took));
            world.set_cell(gx, gy, next);
        }
    }

    // 2) Rise open (unconfined) steam one cell when the cell above is Air.
    if !world.steam.is_empty() {
        let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
        // Bottom-up so a bubble can climb multiple cells across cadences.
        let mut keys = keys;
        keys.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        for (gx, gy) in keys {
            if void_is_confined(world, gx, gy) {
                continue; // pressurized pocket stays put
            }
            let amt = steam_at(world, gx, gy).min(RISE_MAX_PER_CELL);
            if amt == 0 {
                continue;
            }
            let Some(above) = world.get_cell(gx, gy + 1) else {
                // Open top of world — vent by recondensing in place next cool
                // pass; do not delete mass here.
                continue;
            };
            if above.material != MaterialId::Air {
                continue;
            }
            if !can_admit_new_steam_cell(world, gx, gy + 1) && steam_at(world, gx, gy + 1) == 0 {
                continue;
            }
            let took = take_steam(world, gx, gy, amt);
            add_steam(world, gx, gy + 1, took);
        }
    }

    // 3) Boil hot free water (Air sat) → steam.
    boil_hot_air(world, temp, boil);
}

fn boil_hot_air(world: &mut World, temp: &Temperature, boil: f32) {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let prefer_confined = world.steam.len() + 32 >= MAX_STEAM_CELLS;
    let mut jobs: Vec<(i32, i32, u8)> = Vec::new();
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air || c.has_standing_air)
        .map(|(k, _)| *k)
        .collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        let base_gx = coord.cx * cw;
        let base_gy = coord.cy * ch;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if cell.material != MaterialId::Air || cell.sat.0 == 0 {
                    continue;
                }
                let gx = world.wrap_x(base_gx + lx as i32);
                let gy = base_gy + ly as i32;
                if temp.at_cell(gx, gy) < boil {
                    continue;
                }
                let confined = void_is_confined(world, gx, gy);
                if prefer_confined && !confined && steam_at(world, gx, gy) == 0 {
                    continue;
                }
                if !can_admit_new_steam_cell(world, gx, gy) {
                    continue;
                }
                let boil_amt = cell.sat.0.min(BOIL_MAX_PER_CELL);
                if boil_amt > 0 {
                    jobs.push((gx, gy, boil_amt));
                }
            }
        }
    }
    // Prefer confined boils when sorting near the cap.
    jobs.sort_by(|a, b| {
        let ca = void_is_confined(world, a.0, a.1);
        let cb = void_is_confined(world, b.0, b.1);
        cb.cmp(&ca).then(b.2.cmp(&a.2))
    });
    for (gx, gy, amt) in jobs {
        if !can_admit_new_steam_cell(world, gx, gy) {
            continue;
        }
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if cell.material != MaterialId::Air || cell.sat.0 == 0 {
            continue;
        }
        let take = amt.min(cell.sat.0);
        if take == 0 {
            continue;
        }
        let mut next = cell;
        next.sat = Sat(cell.sat.0 - take);
        world.set_cell(gx, gy, next);
        add_steam(world, gx, gy, take);
    }
}

/// Total steam units (same mass units as sat).
pub fn steam_total(world: &World) -> i64 {
    world.steam.values().map(|&v| v as i64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::sat_totals;
    use crate::chunk::ChunkCoord;
    use crate::rules::wake_confined_head;

    fn temp_fill(world: &World, celsius: f32) -> Temperature {
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            0,
            64,
            64,
            world.seed.0,
            64,
            20,
            false,
        );
        for v in t.cells.values_mut() {
            *v = celsius;
        }
        t
    }

    #[test]
    fn surface_water_boils_above_100c() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(4, 1, Cell::water());
        // Open sky above → unconfined.
        for y in 2..20 {
            w.set_cell(4, y, Cell::air());
        }
        let hot = temp_fill(&w, 110.0);
        w.tick = STEAM_EVERY;
        let before = sat_totals(&w).cell_total;
        apply_steam(&mut w, &hot, BOIL_POINT_C);
        assert!(steam_total(&w) > 0, "hot surface water must boil to steam");
        assert!(
            w.get_cell(4, 1).unwrap().sat.0 < 255,
            "boil must consume free sat"
        );
        assert_eq!(sat_totals(&w).cell_total, before, "boil must be mass-flat");
    }

    #[test]
    fn cave_steam_stays_and_pressurizes() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Sealed pocket: bedrock box with wet Air inside.
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 2, Cell::water());
        w.set_cell(4, 3, Cell::air());
        w.set_cell(5, 2, Cell::air());
        w.set_cell(5, 3, Cell::air());
        assert!(void_is_confined(&w, 4, 2), "pocket under stone is confined");
        let hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, BOIL_POINT_C);
        let steam = steam_at(&w, 4, 2) + steam_at(&w, 4, 3);
        assert!(steam > 0, "cave water must boil");
        // Another cadence: confined steam must not rise out through rock.
        w.tick = STEAM_EVERY * 2;
        let before = steam_total(&w);
        apply_steam(&mut w, &hot, BOIL_POINT_C);
        assert!(
            steam_at(&w, 4, 6) == 0 && steam_at(&w, 4, 7) == 0,
            "steam must not pass the stone roof"
        );
        assert!(
            steam_pressure_norm(&w, 4, 2) > 0.0,
            "confined steam must read as pressure"
        );
        let _ = before;
    }

    #[test]
    fn cool_steam_recondenses_mass_flat() {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(3, 0, Cell::solid(MaterialId::Bedrock));
        let mut air = Cell::air();
        air.sat = Sat(0);
        w.set_cell(3, 1, air);
        add_steam(&mut w, 3, 1, 80);
        let before = sat_totals(&w).cell_total;
        let cool = temp_fill(&w, 20.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &cool, BOIL_POINT_C);
        assert_eq!(steam_at(&w, 3, 1), 0, "cool steam must recondense");
        assert_eq!(w.get_cell(3, 1).unwrap().sat.0, 80);
        assert_eq!(sat_totals(&w).cell_total, before);
    }

    #[test]
    fn confined_steam_boosts_well_rise() {
        // Mini artesian: saturated sand under bedrock, well at x=2, steam in aquifer void.
        let mut w = World::new(11);
        for cx in 0..2 {
            w.ensure_chunk(ChunkCoord::new(cx, 0));
        }
        let cap = crate::cell::water_capacity(MaterialId::Sand);
        for x in 0..40 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(cap);
            w.set_cell(x, 1, sand);
            w.set_cell(x, 2, Cell::solid(MaterialId::Bedrock));
            for y in 3..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 30..38 {
            for y in 3..8 {
                w.set_cell(x, y, Cell::water());
            }
        }
        w.set_cell(2, 2, Cell::air());
        let mut film = Cell::air();
        film.sat = Sat(20);
        w.set_cell(2, 1, {
            // Open well into the sand row: replace sand with wet air pipe bottom.
            let mut a = Cell::air();
            a.sat = Sat(40);
            a
        });
        // Put steam under the confined sand path via a side pocket.
        w.set_cell(3, 1, Cell::air());
        add_steam(&mut w, 3, 1, 200);
        let boost = steam_pressure_rate_scale(&w, 2, 1);
        assert!(
            boost > 1.05,
            "steam beside the shaft must boost confined rate (boost={boost:.3})"
        );
        let before = w.get_cell(2, 1).unwrap().sat.0;
        w.tick = 16;
        wake_confined_head(&mut w);
        let after = w.get_cell(2, 1).unwrap().sat.0;
        assert!(
            after >= before,
            "pressurized steam path must not block rise (before={before} after={after})"
        );
    }

    #[test]
    fn open_steam_rises_toward_sky() {
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..12 {
            w.set_cell(5, y, Cell::air());
        }
        add_steam(&mut w, 5, 2, 40);
        assert!(!void_is_confined(&w, 5, 2));
        let hot = temp_fill(&w, 110.0); // keep from recondensing
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, BOIL_POINT_C);
        assert!(
            steam_at(&w, 5, 2) < 40,
            "open steam should leave the lower cell (left={})",
            steam_at(&w, 5, 2)
        );
        assert!(
            steam_at(&w, 5, 3) > 0,
            "open steam must rise into the cell above"
        );
    }
}
