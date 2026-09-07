//! Sparse buoyant steam — boil free + pore water above ~100 °C (geyser).
//!
//! Steam is **warm void vapour**, not a second liquid. It wants to rise.
//! Humidity stays the coarse **sky** field (rain / H overlay) — steam does
//! not dump into it. Confined pockets charge pressure; pressure drives
//! reverse seepage and rate-capped tube carving toward the surface.
//! Cooling recondenses to water and drops dissolved mineral (sinter).
//!
//! Hard-capped. No world-wide vapour CA. See docs/VOXEL_GEYSER.md.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{
    is_flow_erodible, is_grain, permeability_cell, water_capacity_cell, Cell, Sat,
};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::mineral::{
    carry_with_water, dissolved_at, precipitate_artesian_warm, widen_aperture,
};
use crate::temperature::Temperature;

/// Cadence for boil / rise / escape / recondense.
pub const STEAM_EVERY: u64 = 5;

/// Default boil point (°C).
pub const BOIL_POINT_C: f32 = 100.0;

/// Recondense when cooler than boil by this margin (hysteresis).
pub const RECONDENSE_MARGIN_C: f32 = 5.0;

/// Hard cap on cells that may hold steam.
pub const MAX_STEAM_CELLS: usize = 512;

/// Max sat→steam boiled from free Air per cell per cadence.
pub const BOIL_MAX_PER_CELL: u8 = 48;

/// Max pore sat→steam per solid cell per cadence.
pub const PORE_BOIL_MAX_PER_CELL: u8 = 24;

/// Max steam that may rise one Air cell per cadence.
pub const RISE_MAX_PER_CELL: u8 = 48;

/// Mist left on open wet vents (unconfined only).
pub const SURFACE_STEAM_RESIDUAL: u8 = 24;

/// Min [`steam_pressure_norm`] before escape / reverse seepage fires.
pub const ESCAPE_PRESSURE_MIN: f32 = 0.22;

/// Max roof escapes (burst / widen / reverse push) per cadence.
pub const MAX_ESCAPES_PER_TICK: u8 = 12;

/// How far up we walk to decide "open sky" vs solid roof.
const ROOF_PROBE: i32 = 48;

/// How far down a shaft we sum steam for pressure.
const PRESSURE_DEPTH: i32 = 16;

/// Confined-rise multiplier span at full steam pressure (stacks with geo).
pub const STEAM_PRESSURE_RATE_SPAN: f32 = 0.55;

/// Tab / world-step knobs for sparse buoyant steam.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamConfig {
    pub enabled: bool,
    pub boil_point_c: f32,
    pub boil_max_per_cell: u8,
    pub pore_boil_max_per_cell: u8,
    pub rise_max_per_cell: u8,
    /// Open wet vents only — confined steam always wants to leave.
    pub surface_residual: u8,
    pub max_steam_cells: u16,
    pub period_ticks: u64,
    /// Hot wet rock pores flash to steam (inject into nearby Air).
    pub enable_pore_boil: bool,
    /// High pressure pushes water out / carves upward tubes.
    pub enable_escape: bool,
    pub escape_pressure_min: f32,
    pub max_escapes_per_tick: u8,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            boil_point_c: BOIL_POINT_C,
            boil_max_per_cell: BOIL_MAX_PER_CELL,
            pore_boil_max_per_cell: PORE_BOIL_MAX_PER_CELL,
            rise_max_per_cell: RISE_MAX_PER_CELL,
            surface_residual: SURFACE_STEAM_RESIDUAL,
            max_steam_cells: MAX_STEAM_CELLS as u16,
            period_ticks: STEAM_EVERY,
            enable_pore_boil: true,
            enable_escape: true,
            escape_pressure_min: ESCAPE_PRESSURE_MIN,
            max_escapes_per_tick: MAX_ESCAPES_PER_TICK,
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
pub fn void_is_confined(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    for dy in 1..=ROOF_PROBE {
        match world.get_cell(gx, gy + dy) {
            None => return false,
            Some(c) if c.material == MaterialId::Air => continue,
            Some(_) => return true,
        }
    }
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

fn try_place_steam(world: &mut World, gx: i32, gy: i32, amt: u8, max_cells: usize) -> u8 {
    if amt == 0 {
        return 0;
    }
    if !can_admit_new_steam_cell(world, gx, gy, max_cells) && steam_at(world, gx, gy) == 0 {
        return 0;
    }
    add_steam(world, gx, gy, amt);
    amt
}

/// Prefer injecting boiled steam into Air above / beside the source.
fn inject_steam_near(
    world: &mut World,
    gx: i32,
    gy: i32,
    amt: u8,
    max_cells: usize,
) -> u8 {
    if amt == 0 {
        return 0;
    }
    // Prefer upward, then sides, then source Air cell.
    const DELTAS: [(i32, i32); 6] = [(0, 1), (0, 2), (-1, 1), (1, 1), (-1, 0), (1, 0)];
    for (dx, dy) in DELTAS {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(c) = world.get_cell(tx, ty) else {
            continue;
        };
        if c.material != MaterialId::Air {
            continue;
        }
        if try_place_steam(world, tx, ty, amt, max_cells) > 0 {
            return amt;
        }
    }
    if let Some(c) = world.get_cell(gx, gy) {
        if c.material == MaterialId::Air {
            return try_place_steam(world, gx, gy, amt, max_cells);
        }
    }
    0
}

/// Boil / rise / escape / recondense. Humidity (sky) is untouched.
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

    recondense_cool(world, temp, recondense_below);
    boil_hot_air(world, temp, cfg, max_cells);
    if cfg.enable_pore_boil {
        boil_hot_pores(world, temp, cfg, max_cells);
    }
    // Rise after boil so new vapour leaves the water's seat same cadence.
    rise_steam(world, cfg, max_cells);
    if cfg.enable_escape {
        escape_pressurized(world, temp, cfg, max_cells);
    }
}

fn recondense_cool(world: &mut World, temp: &Temperature, recondense_below: f32) {
    if world.steam.is_empty() {
        return;
    }
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    for (gx, gy) in keys {
        if temp.at_cell(gx, gy) >= recondense_below {
            continue;
        }
        let Some(cell) = world.get_cell(gx, gy) else {
            world.steam.remove(&(gx, gy));
            continue;
        };
        if cell.material != MaterialId::Air {
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
        // Cooling steam/water mix drops dissolved load as sinter.
        let warmth = ((recondense_below - temp.at_cell(gx, gy)) / 40.0).clamp(0.0, 1.0);
        precipitate_artesian_warm(world, gx, gy, warmth);
    }
}

/// Steam rises through Air — confined caves included (piles under the roof).
fn rise_steam(world: &mut World, cfg: &SteamConfig, max_cells: usize) {
    if world.steam.is_empty() {
        return;
    }
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut keys = keys;
    keys.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    for (gx, gy) in keys {
        let here = steam_at(world, gx, gy);
        if here == 0 {
            continue;
        }
        // Residual mist only on open wet vents — confined vapour always climbs.
        let residual = if void_is_confined(world, gx, gy) {
            0
        } else {
            match world.get_cell(gx, gy) {
                Some(c) if c.material == MaterialId::Air && c.sat.0 > 0 => cfg.surface_residual,
                _ => 0,
            }
        };
        let amt = here.saturating_sub(residual).min(cfg.rise_max_per_cell);
        if amt == 0 {
            continue;
        }
        let Some(above) = world.get_cell(gx, gy + 1) else {
            continue;
        };
        if above.material != MaterialId::Air {
            continue;
        }
        if !can_admit_new_steam_cell(world, gx, gy + 1, max_cells) && steam_at(world, gx, gy + 1) == 0
        {
            continue;
        }
        let took = take_steam(world, gx, gy, amt);
        add_steam(world, gx, gy + 1, took);
    }
}

/// Hot free Air water → steam (then rise will lift it).
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
    jobs.sort_by(|a, b| {
        let ca = void_is_confined(world, a.0, a.1);
        let cb = void_is_confined(world, b.0, b.1);
        cb.cmp(&ca).then(b.2.cmp(&a.2))
    });
    for (gx, gy, amt) in jobs {
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
        // Prefer lifting vapour out of the water seat immediately.
        let placed = inject_steam_near(world, gx, gy, take, max_cells);
        if placed == 0 {
            continue;
        }
        let mut next = cell;
        next.sat = Sat(cell.sat.0 - placed);
        world.set_cell(gx, gy, next);
    }
}

fn boil_hot_pores(world: &mut World, temp: &Temperature, cfg: &SteamConfig, max_cells: usize) {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let boil = cfg.boil_point_c;
    let mut jobs: Vec<(i32, i32, u8)> = Vec::new();
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_pores)
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
                if cell.material == MaterialId::Air || cell.sat.0 == 0 {
                    continue;
                }
                if permeability_cell(cell, &world.hydro) == 0 {
                    continue;
                }
                let gx = world.wrap_x(base_gx + lx as i32);
                let gy = base_gy + ly as i32;
                let t_c = temp.at_cell(gx, gy);
                if t_c < boil {
                    continue;
                }
                // Need somewhere for vapour to go.
                if !has_air_neighbor(world, gx, gy) {
                    continue;
                }
                let heat = ((t_c - boil) / 50.0).clamp(0.0, 2.0);
                let cap = ((cfg.pore_boil_max_per_cell as f32) * (1.0 + heat))
                    .round()
                    .clamp(1.0, 255.0) as u8;
                let amt = cell.sat.0.min(cap);
                if amt > 0 {
                    jobs.push((gx, gy, amt));
                }
            }
        }
    }
    jobs.sort_by(|a, b| b.2.cmp(&a.2));
    for (gx, gy, amt) in jobs {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if cell.material == MaterialId::Air || cell.sat.0 == 0 {
            continue;
        }
        let take = amt.min(cell.sat.0);
        let placed = inject_steam_near(world, gx, gy, take, max_cells);
        if placed == 0 {
            continue;
        }
        let before = cell.sat.0;
        let mut next = cell;
        next.sat = Sat(before - placed);
        world.set_cell(gx, gy, next);
        // Dissolved load follows the flashed mass into the steam seat above.
        if let Some((tx, ty)) = nearest_steam_air(world, gx, gy) {
            carry_with_water(world, (gx, gy), (tx, ty), placed, before);
        }
        // Reverse seed: push leftover pore water one step up when possible.
        reverse_push_pore_water(world, gx, gy, placed);
    }
}

fn has_air_neighbor(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(0, 1), (0, 2), (-1, 0), (1, 0), (-1, 1), (1, 1), (0, -1)] {
        if world
            .get_cell(world.wrap_x(gx + dx), gy + dy)
            .is_some_and(|c| c.material == MaterialId::Air)
        {
            return true;
        }
    }
    false
}

fn nearest_steam_air(world: &World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    for (dx, dy) in [(0, 1), (0, 2), (-1, 1), (1, 1), (-1, 0), (1, 0)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        if steam_at(world, tx, ty) > 0 {
            return Some((tx, ty));
        }
    }
    None
}

/// Push pore water upward into permeable rock or Air (seepage in reverse).
fn reverse_push_pore_water(world: &mut World, gx: i32, gy: i32, drive: u8) {
    if drive == 0 {
        return;
    }
    let Some(src) = world.get_cell(gx, gy) else {
        return;
    };
    if src.material == MaterialId::Air || src.sat.0 == 0 {
        return;
    }
    let want = (drive / 2).max(1).min(src.sat.0);
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(dst) = world.get_cell(tx, ty) else {
            continue;
        };
        let cap = water_capacity_cell(dst, &world.hydro);
        if cap == 0 {
            continue;
        }
        let room = cap.saturating_sub(dst.sat.0);
        if room == 0 {
            continue;
        }
        if dst.material != MaterialId::Air && permeability_cell(dst, &world.hydro) == 0 {
            continue;
        }
        let moved = want.min(room);
        let before = src.sat.0;
        let mut s = world.get_cell(gx, gy).unwrap();
        let mut d = dst;
        s.sat = Sat(s.sat.0 - moved);
        d.sat = Sat(d.sat.0 + moved);
        world.set_cell(gx, gy, s);
        world.set_cell(tx, ty, d);
        carry_with_water(world, (gx, gy), (tx, ty), moved, before);
        return;
    }
}

/// Pressurized steam under a roof tries to escape: reverse push, widen, burst.
fn escape_pressurized(
    world: &mut World,
    temp: &Temperature,
    cfg: &SteamConfig,
    max_cells: usize,
) {
    if world.steam.is_empty() {
        return;
    }
    let min_p = cfg.escape_pressure_min.clamp(0.05, 0.95);
    let mut keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    // Top-first: escape from the roof contact, not the pool floor.
    keys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut escapes = 0u8;
    let max_esc = cfg.max_escapes_per_tick.max(1);
    for (gx, gy) in keys {
        if escapes >= max_esc {
            break;
        }
        let steam = steam_at(world, gx, gy);
        if steam < 24 {
            continue;
        }
        let press = steam_pressure_norm(world, gx, gy);
        if press < min_p {
            continue;
        }
        // Only act when something solid blocks the rise.
        let Some(above) = world.get_cell(gx, gy + 1) else {
            continue;
        };
        if above.material == MaterialId::Air {
            continue;
        }
        if above.material == MaterialId::Bedrock {
            continue;
        }

        // 1) Reverse seepage through permeable roof rock.
        if reverse_escape_through_rock(world, gx, gy, steam, press) {
            escapes = escapes.saturating_add(1);
            continue;
        }

        // 2) Widen soluble / abradable competent rock (steam tube).
        if crate::cell::is_competent_rock(above.material) {
            let throughput = (80.0 + press * 175.0) as u8;
            let scale = 0.35 + press * 1.25;
            let opened = widen_aperture(world, gx, gy + 1, throughput, scale, 0x57EA_u64);
            if opened {
                // Roof became Air or loose sediment — move steam in.
                let moved = take_steam(world, gx, gy, steam.min(96));
                if let Some(c) = world.get_cell(gx, gy + 1) {
                    if c.material == MaterialId::Air {
                        try_place_steam(world, gx, gy + 1, moved, max_cells);
                    } else {
                        // Loose grains: push sat up and keep steam below until next widen.
                        add_steam(world, gx, gy, moved);
                        reverse_push_pore_water(world, gx, gy + 1, moved / 2);
                    }
                } else {
                    add_steam(world, gx, gy, moved);
                }
                escapes = escapes.saturating_add(1);
                continue;
            }
        }

        // 3) Burst soft grains under high pressure into a steam tube.
        if press >= min_p + 0.15
            && (is_grain(above.material) || is_flow_erodible(above.material))
        {
            if burst_grain_tube(world, gx, gy, gx, gy + 1, max_cells) {
                escapes = escapes.saturating_add(1);
                // Hot escape outlet can drop mineral as it flash-cools against cooler roof air.
                let t = temp.at_cell(gx, gy + 1);
                if t < cfg.boil_point_c {
                    let warmth = ((cfg.boil_point_c - t) / 60.0).clamp(0.0, 1.0);
                    precipitate_artesian_warm(world, gx, gy + 1, warmth);
                }
            }
        }
    }
}

fn reverse_escape_through_rock(
    world: &mut World,
    gx: i32,
    gy: i32,
    steam: u8,
    press: f32,
) -> bool {
    let Some(roof) = world.get_cell(gx, gy + 1) else {
        return false;
    };
    let perm = permeability_cell(roof, &world.hydro);
    if perm < 8 || roof.sat.0 == 0 {
        return false;
    }
    // Push roof pore water further up / aside; move some steam with it if Air opens.
    let drive = ((steam as f32) * (0.15 + press * 0.5)).round() as u8;
    let drive = drive.max(4);
    reverse_push_pore_water(world, gx, gy + 1, drive);
    // Also try to seep a little steam mass as liquid into the roof pores then out —
    // mass-flat: convert a little steam back to sat in the roof if it has room,
    // then push that sat upward (steam "wanting out" as hot water).
    let roof2 = world.get_cell(gx, gy + 1).unwrap_or(roof);
    let cap = water_capacity_cell(roof2, &world.hydro);
    let room = cap.saturating_sub(roof2.sat.0);
    if room > 0 && steam > 8 {
        let put = room.min(steam / 4).max(1);
        let took = take_steam(world, gx, gy, put);
        let mut r = roof2;
        r.sat = Sat(r.sat.0.saturating_add(took));
        world.set_cell(gx, gy + 1, r);
        reverse_push_pore_water(world, gx, gy + 1, took);
        return true;
    }
    drive > 0
}

fn burst_grain_tube(
    world: &mut World,
    from_x: i32,
    from_y: i32,
    tx: i32,
    ty: i32,
    max_cells: usize,
) -> bool {
    let tx = world.wrap_x(tx);
    let Some(cell) = world.get_cell(tx, ty) else {
        return false;
    };
    if !is_grain(cell.material) && !is_flow_erodible(cell.material) {
        return false;
    }
    // Convert grain to Air tube; keep its pore water as free sat; carry mineral load.
    let load = dissolved_at(world, tx, ty);
    let mut air = Cell::air();
    air.sat = Sat(cell.sat.0);
    world.set_cell(tx, ty, air);
    if load > 0 {
        // Load stays on the new Air cell (same coords).
        let _ = load;
    }
    let moved = take_steam(world, from_x, from_y, steam_at(world, from_x, from_y).min(128));
    try_place_steam(world, tx, ty, moved, max_cells);
    true
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
    use crate::mineral::add_dissolved;
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
    fn cave_steam_rises_to_roof_and_pressurizes() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
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
        // Same cadence: vapour should prefer the roof Air, not sit in the pool.
        assert!(
            steam_at(&w, 4, 3) + steam_at(&w, 5, 3) > 0,
            "confined steam must rise toward the roof"
        );
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 6) == 0 && steam_at(&w, 4, 7) == 0,
            "steam must not pass an intact stone roof in two cadences"
        );
        assert!(
            steam_pressure_norm(&w, 4, 3) > 0.0,
            "roof steam must read as pressure"
        );
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
        w.set_cell(2, 1, {
            let mut a = Cell::air();
            a.sat = Sat(40);
            a
        });
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
        let hot = temp_fill(&w, 110.0);
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
        // Boil injects upward first — residual mist may sit on water or in plume.
        assert!(
            steam_total(&w) > 0,
            "hot wet surface must produce steam"
        );
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 2) + steam_at(&w, 4, 3) + steam_at(&w, 4, 1) > 0,
            "open vent must keep a rising plume"
        );
    }

    #[test]
    fn pore_water_boils_into_adjacent_air() {
        let mut w = World::new(17);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(crate::cell::water_capacity(MaterialId::Sand));
        w.set_cell(4, 1, sand);
        w.set_cell(4, 2, Cell::air());
        for y in 3..10 {
            w.set_cell(4, y, Cell::air());
        }
        let hot = temp_fill(&w, 140.0);
        w.tick = STEAM_EVERY;
        let before = sat_totals(&w).cell_total;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(
            steam_total(&w) > 0,
            "hot wet sand must boil pore water to steam"
        );
        assert!(
            w.get_cell(4, 1).unwrap().sat.0 < crate::cell::water_capacity(MaterialId::Sand),
            "pore boil must consume rock sat"
        );
        assert_eq!(sat_totals(&w).cell_total, before);
    }

    #[test]
    fn pressurized_steam_bursts_sand_roof() {
        let mut w = World::new(19);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Sealed pocket with a sand lid.
        for x in 3..7 {
            for y in 1..5 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 2, Cell::air());
        w.set_cell(5, 2, Cell::air());
        w.set_cell(4, 3, Cell::air());
        w.set_cell(5, 3, Cell::air());
        w.set_cell(4, 4, Cell::solid(MaterialId::Sand));
        w.set_cell(5, 4, Cell::solid(MaterialId::Sand));
        // Cap above sand so it stays confined until burst.
        w.set_cell(4, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
        add_steam(&mut w, 4, 3, 220);
        add_steam(&mut w, 5, 3, 220);
        assert!(void_is_confined(&w, 4, 3));
        let hot = temp_fill(&w, 130.0);
        let cfg = SteamConfig {
            escape_pressure_min: 0.05,
            ..SteamConfig::default()
        };
        // Several cadences: charge + burst.
        for i in 1..12 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &hot, &cfg);
        }
        let sand_gone = w.get_cell(4, 4).unwrap().material == MaterialId::Air
            || w.get_cell(5, 4).unwrap().material == MaterialId::Air
            || steam_at(&w, 4, 4) > 0
            || steam_at(&w, 5, 4) > 0;
        assert!(
            sand_gone,
            "high pressure must open a sand steam tube (4,4)={:?} (5,4)={:?}",
            w.get_cell(4, 4).map(|c| c.material),
            w.get_cell(5, 4).map(|c| c.material)
        );
    }

    #[test]
    fn cool_recondense_drops_dissolved_mineral() {
        let mut w = World::new(23);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(3, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(3, 1, Cell::air());
        add_steam(&mut w, 3, 1, 100);
        add_dissolved(&mut w, 3, 1, 400);
        let before_min = crate::audit::mineral_total(&w);
        let cool = temp_fill(&w, 10.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &cool, &SteamConfig::default());
        assert_eq!(steam_at(&w, 3, 1), 0);
        assert_eq!(crate::audit::mineral_total(&w), before_min);
        // Some load should have left solution into solid/pore occlusion or stay —
        // artesian precip only drops excess over ceiling; with recondensed water
        // the ceiling is low enough that load should fall.
        assert!(
            dissolved_at(&w, 3, 1) < 400
                || w.get_cell(3, 1).unwrap().material == MaterialId::Flowstone,
            "cooling steam/water should shed dissolved load"
        );
    }
}
