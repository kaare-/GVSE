//! Sparse conduit steam — boil free water above ~100 °C (geyser P3).
//!
//! Humidity stays the coarse sky field. Steam here is **void/Air-only**
//! markers: boil takes [`Cell::sat`] from Air into `World.steam`, rise /
//! recondense move it back. Confined caves (solid roof overhead) keep
//! steam and pressurize confined rise — that is the underground boil
//! motor. Open surface steam rises and recondenses when cool.
//!
//! Hard-capped. No world-wide vapour CA. See docs/VOXEL_GEYSER.md § P3.
//! Play visibility lives in the app (mist draw + Tab / inspector).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::temperature::Temperature;

/// Cadence for boil / rise / recondense (default [`SteamConfig::period_ticks`]).
pub const STEAM_EVERY: u64 = 5;

/// Default boil point (°C). Free Air sat at or above this may flash to steam.
pub const BOIL_POINT_C: f32 = 100.0;

/// Recondense when cooler than boil by this margin (hysteresis).
pub const RECONDENSE_MARGIN_C: f32 = 5.0;

/// Hard cap on cells that may hold steam. Hundreds, not world-wide.
pub const MAX_STEAM_CELLS: usize = 512;

/// Max sat→steam units boiled in one cell per cadence tick (play default).
pub const BOIL_MAX_PER_CELL: u8 = 48;

/// Max steam units that may rise one cell per cadence tick.
pub const RISE_MAX_PER_CELL: u8 = 16;

/// Steam left at a still-wet boil site so open vents keep a mist plume.
pub const SURFACE_STEAM_RESIDUAL: u8 = 40;

/// How far up we walk to decide "open sky" vs solid roof.
const ROOF_PROBE: i32 = 48;

/// How far down a shaft we sum steam for pressure.
const PRESSURE_DEPTH: i32 = 16;

/// Confined-rise multiplier span at full steam pressure (stacks with geo).
pub const STEAM_PRESSURE_RATE_SPAN: f32 = 0.55;

/// Tab / world-step knobs for sparse conduit steam.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct SteamConfig {
    /// Master switch (Tab → Climate → Steam).
    pub enabled: bool,
    /// Free Air sat at/above this (°C) may boil to steam.
    pub boil_point_c: f32,
    /// Max sat→steam per cell per cadence tick.
    pub boil_max_per_cell: u8,
    /// Max steam that may rise one cell per cadence tick (open vents).
    pub rise_max_per_cell: u8,
    /// Leave this much steam on still-wet open cells so plumes stay visible.
    pub surface_residual: u8,
    /// Hard cap on cells that may hold steam.
    pub max_steam_cells: u16,
    /// Cadence: run when `world.tick % period_ticks == 0`.
    pub period_ticks: u64,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            boil_point_c: BOIL_POINT_C,
            boil_max_per_cell: BOIL_MAX_PER_CELL,
            rise_max_per_cell: RISE_MAX_PER_CELL,
            surface_residual: SURFACE_STEAM_RESIDUAL,
            max_steam_cells: MAX_STEAM_CELLS as u16,
            period_ticks: STEAM_EVERY,
        }
    }
}

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

fn can_admit_new_steam_cell(world: &World, gx: i32, gy: i32, max_cells: usize) -> bool {
    if world.steam.contains_key(&(world.wrap_x(gx), gy)) {
        return true;
    }
    world.steam.len() < max_cells
}

/// Boil / rise / recondense free-water steam.
///
/// Only walks wet-Air chunks (boil) and existing steam keys (rise /
/// recondense). Humidity is untouched.
pub fn apply_steam(world: &mut World, temp: &Temperature, cfg: &SteamConfig) {
    if !cfg.enabled {
        return;
    }
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let boil = cfg.boil_point_c;
    let recondense_below = boil - RECONDENSE_MARGIN_C;
    let max_cells = cfg.max_steam_cells.max(1) as usize;

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
                        if can_admit_new_steam_cell(world, gx, gy + 1, max_cells)
                            || steam_at(world, gx, gy + 1) > 0
                        {
                            add_steam(world, gx, gy + 1, moved);
                        } else {
                            add_steam(world, gx, gy, moved);
                        }
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
    // Wet boil sites keep a residual so open vents still show a plume.
    if !world.steam.is_empty() {
        let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
        // Bottom-up so a bubble can climb multiple cells across cadences.
        let mut keys = keys;
        keys.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
        for (gx, gy) in keys {
            if void_is_confined(world, gx, gy) {
                continue; // pressurized pocket stays put
            }
            let here = steam_at(world, gx, gy);
            if here == 0 {
                continue;
            }
            let residual = match world.get_cell(gx, gy) {
                Some(c) if c.material == MaterialId::Air && c.sat.0 > 0 => cfg.surface_residual,
                _ => 0,
            };
            let amt = here.saturating_sub(residual).min(cfg.rise_max_per_cell);
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
            if !can_admit_new_steam_cell(world, gx, gy + 1, max_cells)
                && steam_at(world, gx, gy + 1) == 0
            {
                continue;
            }
            let took = take_steam(world, gx, gy, amt);
            add_steam(world, gx, gy + 1, took);
        }
    }

    // 3) Boil hot free water (Air sat) → steam.
    boil_hot_air(world, temp, cfg, max_cells);
}

fn boil_hot_air(world: &mut World, temp: &Temperature, cfg: &SteamConfig, max_cells: usize) {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let boil = cfg.boil_point_c;
    let prefer_confined = world.steam.len() + 32 >= max_cells;
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
                let t_c = temp.at_cell(gx, gy);
                if t_c < boil {
                    continue;
                }
                let confined = void_is_confined(world, gx, gy);
                if prefer_confined && !confined && steam_at(world, gx, gy) == 0 {
                    continue;
                }
                if !can_admit_new_steam_cell(world, gx, gy, max_cells) {
                    continue;
                }
                // Hotter → faster flash (caps at 3× boil_max).
                let heat = ((t_c - boil) / 50.0).clamp(0.0, 2.0);
                let cap = ((cfg.boil_max_per_cell as f32) * (1.0 + heat))
                    .round()
                    .clamp(1.0, 255.0) as u8;
                let boil_amt = cell.sat.0.min(cap);
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
        if !can_admit_new_steam_cell(world, gx, gy, max_cells) {
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
        apply_steam(&mut w, &hot, &SteamConfig::default());
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
        apply_steam(&mut w, &hot, &SteamConfig::default());
        let steam = steam_at(&w, 4, 2) + steam_at(&w, 4, 3);
        assert!(steam > 0, "cave water must boil");
        // Another cadence: confined steam must not rise out through rock.
        w.tick = STEAM_EVERY * 2;
        let before = steam_total(&w);
        apply_steam(&mut w, &hot, &SteamConfig::default());
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
        apply_steam(&mut w, &cool, &SteamConfig::default());
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
        apply_steam(&mut w, &hot, &SteamConfig::default());
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

    #[test]
    fn wet_boil_site_keeps_surface_mist() {
        let mut w = World::new(13);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(4, 1, Cell::water());
        for y in 2..16 {
            w.set_cell(4, y, Cell::air());
        }
        let hot = temp_fill(&w, 150.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 1) >= SURFACE_STEAM_RESIDUAL.min(40),
            "hot wet surface must keep a mist residual (steam={})",
            steam_at(&w, 4, 1)
        );
        // Second cadence: residual must still sit on the wet cell while plume rises.
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 1) > 0,
            "wet boil site must not fully vent every cadence"
        );
        assert!(
            steam_total(&w) > steam_at(&w, 4, 1) as i64,
            "excess steam should rise into the plume"
        );
    }
}
