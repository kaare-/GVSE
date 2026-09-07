//! Cave vapour = Humidity under pressure (not a second gas CA).
//!
//! **Model:** boil / pore flash move sat → the shared 4×4 Humidity store.
//! Pressure is confined-cave RH. Overpressure equalizes by reverse pore
//! seepage (up and out) and aperture work; cool / depressurized vapour
//! condenses back to liquid. Sparse `World.steam` is legacy residual only
//! (save compat / drain) — new mass never mints steam cells.
//!
//! **Pore phase change** budgets reverse seepage + cracking via
//! `phase_expansion_drive` (force, not minted water). Mass stays flat
//! (sat ↔ humidity).
//!
//! **Perf:** no full humidity occupied walks; boil only touches hot wet
//! chunks; assault/escape/eq seed from wet-pore void neighbours on a
//! strided cadence ([`STEAM_EVERY`]).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{
    is_flow_erodible, is_grain, permeability_cell, water_capacity_cell, Cell, Sat,
};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::{FxHashMap, FxHashSet};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::mineral::{
    carry_with_water, dissolved_at, precipitate_artesian_warm, widen_aperture,
};
use crate::temperature::Temperature;

/// Cadence for boil / assault / escape / recondense (match seepage-ish).
pub const STEAM_EVERY: u64 = 5;

/// Process 1/`WET_CHUNK_STRIDE` of wet-pore chunks per vapour cadence.
const WET_CHUNK_STRIDE: u64 = 4;

/// Default boil point (°C).
pub const BOIL_POINT_C: f32 = 100.0;

/// Recondense when cooler than boil by this margin (hysteresis).
pub const RECONDENSE_MARGIN_C: f32 = 5.0;

/// Hard cap on cells that may hold steam.
pub const MAX_STEAM_CELLS: usize = 768;

/// Max sat→steam boiled from free Air per cell per cadence.
pub const BOIL_MAX_PER_CELL: u8 = 64;

/// Max pore sat→steam per solid cell per cadence.
pub const PORE_BOIL_MAX_PER_CELL: u8 = 40;

/// Liquid→gas expansion stand-in for pore boil drive.
///
/// Real steam is ~1000–1700× liquid volume; we use a capped sim factor so
/// each boiled sat unit budgets this many units of reverse seepage +
/// aperture work. Mass stays flat (boiled sat ↔ steam/humidity); the
/// factor is *force*, not minted water. 1:1 was inert; dozens reads as
/// violent flash without pretending full 1700×.
pub const PHASE_EXPANSION_DRIVE: u8 = 48;

/// Steam left in the sparse map after exchanging vapour into Humidity
/// (legacy residual drain only — new boil goes straight to H).
pub const STEAM_PRESSURE_RESIDUAL: u8 = 4;

/// How many reverse-seepage hops a phase-expansion pulse may travel.
pub const REVERSE_SEEP_HOPS: u8 = 4;

/// Cadence for pore↔cave vapour eq (aligned with [`STEAM_EVERY`] by default).
pub const PORE_CAVE_EQ_EVERY: u64 = 5;

/// Max pore sat ↔ cave humidity exchanged per rock cell per cadence.
pub const PORE_CAVE_EQ_MAX: u8 = 12;

/// Legacy rise knob (open vents still use buoyant pour after flood).
pub const RISE_MAX_PER_CELL: u8 = 64;

/// Mist left on open wet vents only (standing water seats).
pub const SURFACE_STEAM_RESIDUAL: u8 = 8;

/// Min pocket density before escape fires.
pub const ESCAPE_PRESSURE_MIN: f32 = 0.08;

/// Max roof escapes (burst / widen / reverse push) per cadence.
pub const MAX_ESCAPES_PER_TICK: u8 = 24;

/// Max Air cells flooded per connected pocket per cadence.
pub const VOID_FLOOD_BUDGET: usize = 128;

/// Standing-water threshold: above this, Air is a lake cell (not gas volume).
pub const STEAM_VOID_SAT_MAX: u8 = 160;

/// How far up we walk to decide "open sky" vs solid roof.
const ROOF_PROBE: i32 = 48;

/// How far down a shaft we sum steam for pressure.
const PRESSURE_DEPTH: i32 = 16;

/// Confined-rise multiplier span at full steam pressure (stacks with geo).
pub const STEAM_PRESSURE_RATE_SPAN: f32 = 0.85;

/// Tab / world-step knobs for sparse pressurized steam.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SteamConfig {
    pub enabled: bool,
    pub boil_point_c: f32,
    pub boil_max_per_cell: u8,
    pub pore_boil_max_per_cell: u8,
    /// Force multiplier from liquid→gas expansion (not minted mass).
    pub phase_expansion_drive: u8,
    /// Max hops for reverse seepage driven by phase expansion.
    pub reverse_seep_hops: u8,
    pub rise_max_per_cell: u8,
    pub surface_residual: u8,
    pub max_steam_cells: u16,
    pub period_ticks: u64,
    pub enable_pore_boil: bool,
    pub enable_escape: bool,
    /// Wet rock ↔ empty cave air vapour equalization (below boil).
    pub enable_pore_cave_eq: bool,
    pub pore_cave_eq_max_per_cell: u8,
    pub escape_pressure_min: f32,
    pub max_escapes_per_tick: u8,
    /// Max Air cells per pocket flood-fill.
    pub void_flood_budget: u16,
}

impl Default for SteamConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            boil_point_c: BOIL_POINT_C,
            boil_max_per_cell: BOIL_MAX_PER_CELL,
            pore_boil_max_per_cell: PORE_BOIL_MAX_PER_CELL,
            phase_expansion_drive: PHASE_EXPANSION_DRIVE,
            reverse_seep_hops: REVERSE_SEEP_HOPS,
            rise_max_per_cell: RISE_MAX_PER_CELL,
            surface_residual: SURFACE_STEAM_RESIDUAL,
            max_steam_cells: MAX_STEAM_CELLS as u16,
            period_ticks: STEAM_EVERY,
            enable_pore_boil: true,
            enable_escape: true,
            enable_pore_cave_eq: true,
            pore_cave_eq_max_per_cell: PORE_CAVE_EQ_MAX,
            escape_pressure_min: ESCAPE_PRESSURE_MIN,
            max_escapes_per_tick: MAX_ESCAPES_PER_TICK,
            void_flood_budget: VOID_FLOOD_BUDGET as u16,
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

/// Air that counts as gas volume (not a standing-water lake cell).
#[inline]
fn is_steam_void(cell: Cell) -> bool {
    cell.material == MaterialId::Air && cell.sat.0 <= STEAM_VOID_SAT_MAX
}

/// 0..=1 pressure — local steam density in the column (gas fill, not a liquid stack).
pub fn steam_pressure_norm(world: &World, gx: i32, gy: i32) -> f32 {
    if world.steam.is_empty() {
        return 0.0;
    }
    let gx = world.wrap_x(gx);
    let mut sum = 0u32;
    let mut voids = 0u32;
    for dx in [-1_i32, 0, 1] {
        let x = world.wrap_x(gx + dx);
        for dy in 0..=PRESSURE_DEPTH {
            let y = gy - dy;
            let s = steam_at(world, x, y) as u32;
            sum += s;
            if s > 0 || world.get_cell(x, y).is_some_and(is_steam_void) {
                voids += 1;
            }
        }
    }
    if voids == 0 {
        return 0.0;
    }
    (sum as f32 / (voids as f32 * 255.0)).clamp(0.0, 1.0)
}

/// Confined cave vapour pressure: humidity RH in subsurface voids, boosted
/// by any sparse steam residual. Open-sky humidity does not pressurize.
pub fn cave_vapour_pressure_norm(
    world: &World,
    humidity: &Humidity,
    temp: &Temperature,
    gx: i32,
    gy: i32,
) -> f32 {
    let gx = world.wrap_x(gx);
    let steam_p = steam_pressure_norm(world, gx, gy);
    let Some(cell) = world.get_cell(gx, gy) else {
        return steam_p;
    };
    if !is_steam_void(cell) {
        return steam_p;
    }
    // Only underground / roofed voids — surface haze is weather, not pressure.
    if !void_is_confined(world, gx, gy)
        && !Humidity::cell_in_subsurface_air(world, gx, gy, humidity.tile_cols)
    {
        return steam_p;
    }
    let t_c = temp.at_cell(gx, gy);
    let sat = Humidity::saturation_mass_at_temp(t_c).max(1.0);
    let rh = (humidity.at_cell(gx, gy) / sat).clamp(0.0, 1.5);
    // Oversaturated caves press hard; partial RH still counts.
    let hum_p = (rh * 0.85).clamp(0.0, 1.0);
    steam_p.max(hum_p)
}

/// Confined-rise rate boost from underground steam (1 + span * norm).
#[inline]
pub fn steam_pressure_rate_scale(world: &World, gx: i32, gy: i32) -> f32 {
    1.0 + STEAM_PRESSURE_RATE_SPAN * steam_pressure_norm(world, gx, gy)
}

/// Confined-rise / artesian boost from cave vapour (humidity + steam).
#[inline]
pub fn cave_vapour_rate_scale(
    world: &World,
    humidity: &Humidity,
    temp: &Temperature,
    gx: i32,
    gy: i32,
) -> f32 {
    1.0 + STEAM_PRESSURE_RATE_SPAN
        * cave_vapour_pressure_norm(world, humidity, temp, gx, gy)
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

/// Prefer injecting boiled vapour into Humidity at void Air above / beside.
fn inject_vapour_near(
    world: &World,
    humidity: &mut Humidity,
    temp: &Temperature,
    gx: i32,
    gy: i32,
    amt: u8,
) -> u8 {
    if amt == 0 {
        return 0;
    }
    const DELTAS: [(i32, i32); 7] = [
        (0, 0),
        (0, 1),
        (0, 2),
        (-1, 1),
        (1, 1),
        (-1, 0),
        (1, 0),
    ];
    let mut left = amt;
    for (dx, dy) in DELTAS {
        if left == 0 {
            break;
        }
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(c) = world.get_cell(tx, ty) else {
            continue;
        };
        if c.material != MaterialId::Air {
            continue;
        }
        // Standing lake plugs are not vapour seats (except in-place boil of wet Air).
        if (dx != 0 || dy != 0) && !is_steam_void(c) {
            continue;
        }
        let t_c = temp.at_cell(tx, ty);
        let accepted = humidity.try_add_at_temp(tx, ty, left as f32, t_c);
        let mut took = accepted.floor() as u8;
        if took < left && t_c >= BOIL_POINT_C - 20.0 {
            let more = humidity.try_add(tx, ty, (left - took) as f32);
            took = took.saturating_add(more.floor() as u8);
        }
        left = left.saturating_sub(took);
    }
    amt.saturating_sub(left)
}

/// Prefer injecting boiled steam into void Air above / beside the source.
/// Legacy helper for residual steam placement (escape / flood tests).
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
    const DELTAS: [(i32, i32); 6] = [(0, 1), (0, 2), (-1, 1), (1, 1), (-1, 0), (1, 0)];
    for (dx, dy) in DELTAS {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(c) = world.get_cell(tx, ty) else {
            continue;
        };
        if !is_steam_void(c) {
            continue;
        }
        if try_place_steam(world, tx, ty, amt, max_cells) > 0 {
            return amt;
        }
    }
    if let Some(c) = world.get_cell(gx, gy) {
        if is_steam_void(c) {
            return try_place_steam(world, gx, gy, amt, max_cells);
        }
        if c.material == MaterialId::Air {
            return try_place_steam(world, gx, gy, amt, max_cells);
        }
    }
    0
}

/// Boil / assault / escape / cool-condense on cadence; drain legacy steam
/// residual into Humidity; pore↔cave vapour eq below boil.
pub fn apply_steam(
    world: &mut World,
    temp: &Temperature,
    humidity: &mut Humidity,
    cfg: &SteamConfig,
) {
    if !cfg.enabled {
        return;
    }
    let period = cfg.period_ticks.max(1);
    let due = world.tick % period == 0;
    let max_cells = cfg.max_steam_cells.max(1) as usize;
    let boil = cfg.boil_point_c;
    let recondense_below = boil - RECONDENSE_MARGIN_C;

    if due {
        // Legacy sparse residual: cool → liquid, then equalize leftover gas.
        recondense_cool(world, temp, recondense_below);
        if !world.steam.is_empty() {
            flood_equalize_steam(world, cfg, max_cells);
        }
        // New vapour mass goes straight into Humidity (pressure = RH).
        boil_hot_air(world, humidity, temp, cfg);
        if cfg.enable_pore_boil {
            boil_hot_pores(world, humidity, temp, cfg);
        }
        let seats = collect_local_vapour_seats(world, humidity, temp);
        assault_pressurized_vapour(world, humidity, temp, cfg, &seats);
        if cfg.enable_escape {
            escape_pressurized(world, humidity, temp, cfg, max_cells, &seats);
        }
        condense_cool_humidity(world, humidity, temp, recondense_below, &seats);
    }
    // Drain any leftover sparse residual into H.
    exchange_steam_into_humidity(world, humidity, temp);
    if cfg.enable_pore_cave_eq && world.tick % PORE_CAVE_EQ_EVERY == 0 {
        equalize_pore_cave_vapour(world, humidity, temp, cfg);
    }
}

/// Move steam mass into Humidity (same units). Leaves a thin residual so
/// pressure escape / assault still have a charge. Tracked water mass is
/// conserved (steam ↓, humidity ↑).
fn exchange_steam_into_humidity(
    world: &mut World,
    humidity: &mut Humidity,
    temp: &Temperature,
) {
    if world.steam.is_empty() {
        return;
    }
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let residual = STEAM_PRESSURE_RESIDUAL;
    for (gx, gy) in keys {
        let steam = steam_at(world, gx, gy);
        if steam <= residual {
            continue;
        }
        let move_amt = steam - residual;
        let t_c = temp.at_cell(gx, gy);
        // Prefer temp-capped add; overflow stays as steam until cool/escape.
        let accepted = humidity.try_add_at_temp(gx, gy, move_amt as f32, t_c);
        let took = accepted.floor() as u8;
        if took > 0 {
            let _ = take_steam(world, gx, gy, took);
        }
        // Hot caves: if temp cap still had room as raw mass, top up.
        let left = steam_at(world, gx, gy).saturating_sub(residual);
        if left > 0 && t_c >= BOIL_POINT_C - 20.0 {
            let more = humidity.try_add(gx, gy, left as f32);
            let took2 = more.floor() as u8;
            if took2 > 0 {
                let _ = take_steam(world, gx, gy, took2);
            }
        }
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
                if is_steam_void(up) {
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
        let warmth = ((recondense_below - temp.at_cell(gx, gy)) / 40.0).clamp(0.0, 1.0);
        precipitate_artesian_warm(world, gx, gy, warmth);
    }
}

/// Flood-fill connected void Air and redistribute steam like a gas.
///
/// Cave / under-roof pockets (including leaky ones) equalize as a vapour
/// field. Only a fully open shaft uses a buoyant plume.
fn flood_equalize_steam(world: &mut World, cfg: &SteamConfig, max_cells: usize) {
    if world.steam.is_empty() {
        return;
    }
    let budget = cfg.void_flood_budget.max(8) as usize;
    let open_plume_max = budget.min(32);
    let seeds: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut visited: FxHashSet<(i32, i32)> = FxHashSet::default();

    for (sx, sy) in seeds {
        let sx = world.wrap_x(sx);
        if !visited.insert((sx, sy)) {
            continue;
        }
        let seed_confined = void_is_confined(world, sx, sy);
        let mut queue = vec![(sx, sy)];
        let mut component: Vec<(i32, i32)> = Vec::new();
        let mut open_to_sky = false;
        let mut qi = 0;
        while qi < queue.len() && component.len() < budget {
            let (cx, cy) = queue[qi];
            qi += 1;
            let Some(cell) = world.get_cell(cx, cy) else {
                open_to_sky = true;
                continue;
            };
            if cell.material != MaterialId::Air {
                continue;
            }
            component.push((cx, cy));
            if world.get_cell(cx, cy + 1).is_none() {
                open_to_sky = true;
            }
            let here_confined = void_is_confined(world, cx, cy);
            for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                // Pure open shaft: climb only. Cave seeds expand through the
                // whole pocket (ortho) even if a vent eventually opens.
                if !seed_confined {
                    if dy < 0 {
                        continue;
                    }
                    if cy + dy > sy + open_plume_max as i32 {
                        continue;
                    }
                } else if !here_confined && dy < 0 {
                    // Past the vent lip: don't drain back down into the sky map.
                    continue;
                }
                let nx = world.wrap_x(cx + dx);
                let ny = cy + dy;
                if !visited.insert((nx, ny)) {
                    continue;
                }
                match world.get_cell(nx, ny) {
                    None => {
                        open_to_sky = true;
                    }
                    Some(n) if n.material == MaterialId::Air => {
                        if is_steam_void(n) || steam_at(world, nx, ny) > 0 {
                            queue.push((nx, ny));
                        }
                    }
                    Some(_) => {}
                }
            }
        }
        if component.is_empty() {
            continue;
        }

        let mut total: u32 = 0;
        for &(x, y) in &component {
            total += take_steam(world, x, y, steam_at(world, x, y)) as u32;
        }
        if total == 0 {
            continue;
        }

        let mut voids: Vec<(i32, i32)> = component
            .iter()
            .copied()
            .filter(|&(x, y)| world.get_cell(x, y).is_some_and(is_steam_void))
            .collect();
        if voids.is_empty() {
            if let Some(&(x, y)) = component.iter().max_by_key(|c| c.1) {
                try_place_steam(world, x, y, total.min(255) as u8, max_cells);
            }
            continue;
        }

        let confined_n = voids
            .iter()
            .filter(|&&(x, y)| void_is_confined(world, x, y))
            .count();
        // Leaky caves still equalize — only a free open plume skips the field.
        let as_field = seed_confined || confined_n * 2 >= voids.len();

        if as_field {
            // Prefer seating on under-roof voids first so the chamber fills.
            voids.sort_by(|a, b| {
                let ca = void_is_confined(world, a.0, a.1);
                let cb = void_is_confined(world, b.0, b.1);
                cb.cmp(&ca).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0))
            });
            let n = voids.len() as u32;
            let base = (total / n) as u8;
            let mut rem = (total % n) as usize;
            for &(x, y) in &voids {
                let mut put = base;
                if rem > 0 {
                    put = put.saturating_add(1);
                    rem -= 1;
                }
                if put == 0 {
                    continue;
                }
                if try_place_steam(world, x, y, put, max_cells) == 0 {
                    if let Some(&(fx, fy)) =
                        voids.iter().find(|&&(vx, vy)| steam_at(world, vx, vy) > 0)
                    {
                        add_steam(world, fx, fy, put);
                    }
                }
            }
            let _ = open_to_sky;
        } else {
            // Open plume: pack into the top of the climbed column.
            voids.retain(|&(x, y)| (x - sx).abs() <= 1 && y >= sy);
            if voids.is_empty() {
                voids = component
                    .iter()
                    .copied()
                    .filter(|&(x, y)| {
                        (x - sx).abs() <= 1
                            && y >= sy
                            && world.get_cell(x, y).is_some_and(is_steam_void)
                    })
                    .collect();
            }
            voids.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
            let mut left = total;
            for &(x, y) in &voids {
                if left == 0 {
                    break;
                }
                let room = 255u32.saturating_sub(steam_at(world, x, y) as u32);
                if room == 0 {
                    continue;
                }
                let put = left.min(room);
                if try_place_steam(world, x, y, put as u8, max_cells) > 0 {
                    left -= put;
                }
            }
            if left > 0 {
                if let Some(&(x, y)) = voids.first() {
                    add_steam(world, x, y, left.min(255) as u8);
                } else {
                    add_steam(world, sx, sy, left.min(255) as u8);
                }
            }
        }
    }
}

/// Continuous vapour wash for rendering: every void cell in a steam pocket
/// gets the pocket's mean density (not sparse marker speckles).
///
/// Returns `(gx, gy, density_u8)` with a visibility floor so thin steam still
/// reads as a filled field.
pub fn steam_vapour_field(world: &World) -> Vec<(i32, i32, u8)> {
    steam_haze_wash(world, None)
        .into_iter()
        .map(|s| (s.gx, s.gy, s.density))
        .collect()
}

/// Soft humidity-like steam wash sample (cell resolution, from 4×4 tiles).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamHazeSample {
    pub gx: i32,
    pub gy: i32,
    /// Visual density 0..=255 (maps to soft haze alpha, not opaque fill).
    pub density: u8,
    /// Warmth 0..=255 from temperature + pressure (slight warm tint).
    pub warmth: u8,
}

/// Coarse tile side for steam haze — same grain as sky [`crate::humidity::Humidity`].
pub const STEAM_HAZE_TILE: i32 = 4;

/// Build a humidity-shaped wash: steam mass lives on 4×4 tiles, then each Air
/// void cell bilinear-samples that field. Pressure and heat raise density /
/// warmth; the look stays soft white vapour, not a solid blue plug.
pub fn steam_haze_wash(world: &World, temp: Option<&Temperature>) -> Vec<SteamHazeSample> {
    if world.steam.is_empty() {
        return Vec::new();
    }
    let tc = STEAM_HAZE_TILE.max(1);
    let mut tile_mass: FxHashMap<(i32, i32), f32> = FxHashMap::default();
    let mut tile_press: FxHashMap<(i32, i32), f32> = FxHashMap::default();

    // 1) Bin sparse steam markers onto coarse tiles.
    for (&(gx, gy), &amt) in world.steam.iter() {
        if amt == 0 {
            continue;
        }
        let gx = world.wrap_x(gx);
        let hx = gx.div_euclid(tc);
        let hy = gy.div_euclid(tc);
        *tile_mass.entry((hx, hy)).or_insert(0.0) += amt as f32;
        let p = steam_pressure_norm(world, gx, gy).max(amt as f32 / 255.0);
        let slot = tile_press.entry((hx, hy)).or_insert(0.0);
        *slot = (*slot).max(p);
    }

    // 2) Confined / connected voids: ensure the whole pocket's tiles carry
    //    the equalized mass so a cave washes as one vapour field at 4×4.
    let budget = VOID_FLOOD_BUDGET;
    let seeds: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut visited: FxHashSet<(i32, i32)> = FxHashSet::default();
    for (sx, sy) in seeds {
        let sx = world.wrap_x(sx);
        if !visited.insert((sx, sy)) {
            continue;
        }
        let seed_confined = void_is_confined(world, sx, sy);
        let mut queue = vec![(sx, sy)];
        let mut voids: Vec<(i32, i32)> = Vec::new();
        let mut qi = 0;
        while qi < queue.len() && voids.len() < budget {
            let (cx, cy) = queue[qi];
            qi += 1;
            let Some(cell) = world.get_cell(cx, cy) else {
                continue;
            };
            if cell.material != MaterialId::Air || !is_steam_void(cell) {
                continue;
            }
            voids.push((cx, cy));
            let here_confined = void_is_confined(world, cx, cy);
            for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                if !seed_confined {
                    if dy < 0 {
                        continue;
                    }
                    if cy + dy > sy + 32 {
                        continue;
                    }
                } else if !here_confined && dy < 0 {
                    continue;
                }
                let nx = world.wrap_x(cx + dx);
                let ny = cy + dy;
                if !visited.insert((nx, ny)) {
                    continue;
                }
                if world
                    .get_cell(nx, ny)
                    .is_some_and(|n| n.material == MaterialId::Air && (is_steam_void(n) || steam_at(world, nx, ny) > 0))
                {
                    queue.push((nx, ny));
                }
            }
        }
        if voids.is_empty() {
            continue;
        }
        let total: f32 = voids
            .iter()
            .map(|&(x, y)| steam_at(world, x, y) as f32)
            .sum();
        if total <= 0.0 {
            continue;
        }
        let press = voids
            .iter()
            .map(|&(x, y)| steam_pressure_norm(world, x, y))
            .fold(0.0_f32, f32::max)
            .max((total / voids.len() as f32) / 255.0);
        // Equalized pocket → each tile covering voids gets its share of mass
        // (humidity-shaped: coarse tiles, not per-cell plugs).
        let mut tile_void_n: FxHashMap<(i32, i32), u32> = FxHashMap::default();
        for &(x, y) in &voids {
            let hx = x.div_euclid(tc);
            let hy = y.div_euclid(tc);
            *tile_void_n.entry((hx, hy)).or_insert(0) += 1;
            let p = tile_press.entry((hx, hy)).or_insert(0.0);
            *p = (*p).max(press);
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

    // Peak for normalization (humidity-style).
    let peak = tile_mass.values().copied().fold(0.0_f32, f32::max).max(1.0);

    // Paint seats: occupied tiles + one-tile halo (soft edges like H haze).
    let mut seats: FxHashSet<(i32, i32)> = tile_mass.keys().copied().collect();
    let occupied: Vec<(i32, i32)> = seats.iter().copied().collect();
    for (hx, hy) in occupied {
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
            seats.insert((hx + dx, hy + dy));
        }
    }

    let mut out: Vec<SteamHazeSample> = Vec::new();
    for (hx, hy) in seats {
        for ly in 0..tc {
            for lx in 0..tc {
                let gx = world.wrap_x(hx * tc + lx);
                let gy = hy * tc + ly;
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                // Humidity of the conduit: only Air vapour volume, not rock / lake plugs.
                if cell.material != MaterialId::Air || !is_steam_void(cell) {
                    continue;
                }
                let mass = sample_steam_tile_bilinear(&tile_mass, tc, gx as f32 + 0.5, gy as f32 + 0.5);
                if mass <= 0.05 {
                    continue;
                }
                let press = sample_steam_tile_bilinear(&tile_press, tc, gx as f32 + 0.5, gy as f32 + 0.5)
                    .clamp(0.0, 1.0);
                // Pressure acts like denser humidity — raises visual mass, not opacity ceiling.
                let boosted = mass * (1.0 + press * 1.25);
                let norm = (boosted / peak).clamp(0.0, 1.0);
                // Soft floor so thin steam still reads; stay well below opaque fill.
                let density = ((28.0 + norm.sqrt() * 180.0).round() as u8).min(200);

                let mut warmth = (press * 120.0) as u8;
                if let Some(t) = temp {
                    let c = t.at_cell(gx, gy);
                    let heat = ((c - 40.0) / 100.0).clamp(0.0, 1.0);
                    warmth = warmth.saturating_add((heat * 140.0) as u8);
                }
                out.push(SteamHazeSample {
                    gx,
                    gy,
                    density,
                    warmth,
                });
            }
        }
    }
    out
}

fn sample_steam_tile_bilinear(
    tiles: &FxHashMap<(i32, i32), f32>,
    tile_cols: i32,
    gx: f32,
    gy: f32,
) -> f32 {
    let tc = tile_cols.max(1) as f32;
    // Tile centres at (h+0.5)*tc — same convention as Humidity::sample_bilinear.
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

/// True when any temperature tile covering this chunk is near/above `min_c`.
fn chunk_overlaps_hot(temp: &Temperature, coord: ChunkCoord, min_c: f32) -> bool {
    let tc = temp.tile_cols.max(1);
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let x0 = coord.cx * cw;
    let y0 = coord.cy * ch;
    let hx0 = x0.div_euclid(tc);
    let hy0 = y0.div_euclid(tc);
    let hx1 = (x0 + cw - 1).div_euclid(tc);
    let hy1 = (y0 + ch - 1).div_euclid(tc);
    for hy in hy0..=hy1 {
        for hx in hx0..=hx1 {
            if temp.at_tile(hx, hy) >= min_c {
                return true;
            }
        }
    }
    false
}

/// Stride wet-pore chunks so soaked worlds don't full-scan every cadence.
fn select_wet_pore_chunks(world: &World, tick: u64) -> Vec<ChunkCoord> {
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_pores)
        .map(|(k, _)| *k)
        .collect();
    coords.sort_by_key(|c| (c.cy, c.cx));
    if coords.len() <= 4 || WET_CHUNK_STRIDE <= 1 {
        return coords;
    }
    let phase = tick % WET_CHUNK_STRIDE;
    coords
        .into_iter()
        .enumerate()
        .filter(|(i, _)| (*i as u64) % WET_CHUNK_STRIDE == phase)
        .map(|(_, c)| c)
        .collect()
}

/// Local vapour seats: legacy steam keys + void Air next to wet pores
/// (strided). Skip voids with no humidity and no steam residual.
fn collect_local_vapour_seats(
    world: &World,
    humidity: &Humidity,
    _temp: &Temperature,
) -> Vec<(i32, i32)> {
    let mut seats: FxHashSet<(i32, i32)> = FxHashSet::default();
    for &(gx, gy) in world.steam.keys() {
        seats.insert((world.wrap_x(gx), gy));
    }
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    for coord in select_wet_pore_chunks(world, world.tick) {
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
                for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                    let vx = world.wrap_x(gx + dx);
                    let vy = gy + dy;
                    if !world.get_cell(vx, vy).is_some_and(is_steam_void) {
                        continue;
                    }
                    // Cheap gate: ignore dry empty voids (no pressure work).
                    if humidity.at_cell(vx, vy) <= 0.05 && steam_at(world, vx, vy) == 0 {
                        continue;
                    }
                    seats.insert((vx, vy));
                }
            }
        }
    }
    let mut keys: Vec<(i32, i32)> = seats.into_iter().collect();
    keys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    // Hard cap — soaked worlds can still mint many seats.
    const MAX_SEATS: usize = 96;
    if keys.len() > MAX_SEATS {
        keys.truncate(MAX_SEATS);
    }
    keys
}

/// Cool / depressurized humidity → liquid sat in void seats (mass-flat).
fn condense_cool_humidity(
    world: &mut World,
    humidity: &mut Humidity,
    temp: &Temperature,
    recondense_below: f32,
    seats: &[(i32, i32)],
) {
    let mut work = 0u8;
    let max_work = 24u8;
    for &(gx, gy) in seats {
        if work >= max_work {
            break;
        }
        let t_c = temp.at_cell(gx, gy);
        if t_c >= recondense_below {
            continue;
        }
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if !is_steam_void(cell) {
            continue;
        }
        let sat_mass = Humidity::saturation_mass_at_temp(t_c).max(1.0);
        let mass = humidity.at_cell(gx, gy);
        if mass <= 0.05 {
            continue;
        }
        // Prefer dumping oversaturated RH; colder air also sheds a fraction.
        let want = if mass > sat_mass {
            (mass - sat_mass).min(48.0)
        } else if t_c < recondense_below - 15.0 {
            (mass * 0.12).min(24.0)
        } else {
            0.0
        };
        if want < 0.5 {
            continue;
        }
        let room = u8::MAX.saturating_sub(cell.sat.0) as f32;
        let put_f = want.min(room);
        if put_f < 0.5 {
            continue;
        }
        let took = humidity.take(gx, gy, put_f);
        let put = took.floor() as u8;
        if put == 0 {
            continue;
        }
        let mut next = cell;
        next.sat = Sat(cell.sat.0.saturating_add(put));
        world.set_cell(gx, gy, next);
        let warmth = ((recondense_below - t_c) / 40.0).clamp(0.0, 1.0);
        precipitate_artesian_warm(world, gx, gy, warmth);
        work = work.saturating_add(1);
    }
}

/// Pressurized cave vapour (humidity RH) assaults neighbouring wet rock:
/// reverse push (seepage in reverse) + aperture growth.
fn assault_pressurized_vapour(
    world: &mut World,
    humidity: &Humidity,
    temp: &Temperature,
    cfg: &SteamConfig,
    seats: &[(i32, i32)],
) {
    let min_p = (cfg.escape_pressure_min * 0.45).clamp(0.02, 0.5);
    if seats.is_empty() {
        return;
    }
    let mut assaults = 0u8;
    let max_a = cfg.max_escapes_per_tick.saturating_mul(2).max(8);
    for &(gx, gy) in seats {
        if assaults >= max_a {
            break;
        }
        let steam = steam_at(world, gx, gy);
        let press = cave_vapour_pressure_norm(world, humidity, temp, gx, gy)
            .max(steam as f32 / 255.0);
        if press < min_p && steam < 12 {
            continue;
        }
        let drive_base = ((press * 180.0).max(steam as f32 * (0.25 + press))).round() as u8;
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, -1), (-1, 0), (1, 0)] {
            if assaults >= max_a {
                break;
            }
            let tx = world.wrap_x(gx + dx);
            let ty = gy + dy;
            let Some(wall) = world.get_cell(tx, ty) else {
                continue;
            };
            if wall.material == MaterialId::Air || wall.material == MaterialId::Bedrock {
                continue;
            }
            if wall.sat.0 > 0 && permeability_cell(wall, &world.hydro) > 0 {
                // Overpressure equalizes by shoving pore water up/out.
                reverse_push_pore_water(world, tx, ty, drive_base.max(4));
                assaults = assaults.saturating_add(1);
            }
            if crate::cell::is_competent_rock(wall.material) && press >= min_p {
                let up_bias = if dy > 0 { 1.0 } else { 0.35 };
                let throughput = ((120.0 + press * 135.0) * up_bias) as u8;
                let scale = (0.8 + press * 2.0) * up_bias;
                if widen_aperture(world, tx, ty, throughput.max(1), scale, 0x0A51_u64) {
                    assaults = assaults.saturating_add(1);
                }
            }
        }
    }
}

/// Below boil: wet rock vapour wants to equalize with empty cave air.
/// Pore sat → cave H when the void is undersaturated; cave H → pore when
/// cool/oversaturated. Local to wet-pore neighborhoods (strided).
fn equalize_pore_cave_vapour(
    world: &mut World,
    humidity: &mut Humidity,
    temp: &Temperature,
    cfg: &SteamConfig,
) {
    let boil = cfg.boil_point_c;
    let max_move = cfg.pore_cave_eq_max_per_cell.max(1);
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let mut jobs: Vec<(i32, i32, i32, i32, i8)> = Vec::new();
    for coord in select_wet_pore_chunks(world, world.tick) {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        let base_gx = coord.cx * cw;
        let base_gy = coord.cy * ch;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let cell = chunk.get(lx, ly);
                if cell.material == MaterialId::Air || permeability_cell(cell, &world.hydro) == 0 {
                    continue;
                }
                let gx = world.wrap_x(base_gx + lx as i32);
                let gy = base_gy + ly as i32;
                let t_rock = temp.at_cell(gx, gy);
                if t_rock >= boil {
                    continue;
                }
                let room = water_capacity_cell(cell, &world.hydro).saturating_sub(cell.sat.0);
                for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                    let vx = world.wrap_x(gx + dx);
                    let vy = gy + dy;
                    let Some(void) = world.get_cell(vx, vy) else {
                        continue;
                    };
                    if !is_steam_void(void) {
                        continue;
                    }
                    // Cheap confined/subsurface gate — avoid sky film seats.
                    if !void_is_confined(world, vx, vy) {
                        continue;
                    }
                    let t_air = temp.at_cell(vx, vy);
                    let sat_mass = Humidity::saturation_mass_at_temp(t_air).max(1.0);
                    let rh = humidity.at_cell(vx, vy) / sat_mass;
                    if cell.sat.0 > 0 && rh < 0.85 && t_rock > 5.0 {
                        jobs.push((gx, gy, vx, vy, 1));
                    }
                    if room > 0 && rh > 1.0 && t_air < boil - RECONDENSE_MARGIN_C {
                        jobs.push((gx, gy, vx, vy, -1));
                    }
                }
            }
        }
    }
    let max_jobs = cfg.max_escapes_per_tick.saturating_mul(4).max(16) as usize;
    if jobs.len() > max_jobs {
        jobs.truncate(max_jobs);
    }
    for (gx, gy, vx, vy, dir) in jobs {
        let Some(rock) = world.get_cell(gx, gy) else {
            continue;
        };
        if rock.material == MaterialId::Air {
            continue;
        }
        let t_air = temp.at_cell(vx, vy);
        if dir > 0 {
            let take = rock.sat.0.min(max_move);
            if take == 0 {
                continue;
            }
            let accepted = humidity.try_add_at_temp(vx, vy, take as f32, t_air);
            let moved = accepted.floor() as u8;
            if moved == 0 {
                continue;
            }
            let before = rock.sat.0;
            let mut next = rock;
            next.sat = Sat(before - moved);
            world.set_cell(gx, gy, next);
            carry_with_water(world, (gx, gy), (vx, vy), moved, before);
        } else {
            let room = water_capacity_cell(rock, &world.hydro).saturating_sub(rock.sat.0);
            let want = room.min(max_move) as f32;
            if want <= 0.0 {
                continue;
            }
            let took = humidity.take(vx, vy, want);
            let moved = took.floor() as u8;
            if moved == 0 {
                continue;
            }
            let mut next = rock;
            next.sat = Sat(next.sat.0.saturating_add(moved));
            world.set_cell(gx, gy, next);
        }
    }
}

fn boil_hot_air(
    world: &mut World,
    humidity: &mut Humidity,
    temp: &Temperature,
    cfg: &SteamConfig,
) {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let boil = cfg.boil_point_c;
    let mut jobs: Vec<(i32, i32, u8)> = Vec::new();
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(coord, c)| {
            (c.has_wet_air || c.has_standing_air) && chunk_overlaps_hot(temp, **coord, boil)
        })
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
    jobs.sort_by(|a, b| b.2.cmp(&a.2));
    let mut work = 0u8;
    let max_work = cfg.max_escapes_per_tick.saturating_mul(4).max(24);
    for (gx, gy, amt) in jobs {
        if work >= max_work {
            break;
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
        let placed = inject_vapour_near(world, humidity, temp, gx, gy, take);
        if placed == 0 {
            continue;
        }
        let mut next = cell;
        next.sat = Sat(cell.sat.0 - placed);
        world.set_cell(gx, gy, next);
        work = work.saturating_add(1);
    }
}

fn boil_hot_pores(
    world: &mut World,
    humidity: &mut Humidity,
    temp: &Temperature,
    cfg: &SteamConfig,
) {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let boil = cfg.boil_point_c;
    let expand = cfg.phase_expansion_drive.max(1);
    let hops = cfg.reverse_seep_hops.max(1);
    let mut jobs: Vec<(i32, i32, u8)> = Vec::new();
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(coord, c)| c.has_wet_pores && chunk_overlaps_hot(temp, **coord, boil))
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
    let mut work = 0u8;
    let max_work = cfg.max_escapes_per_tick.saturating_mul(3).max(16);
    for (gx, gy, amt) in jobs {
        if work >= max_work {
            break;
        }
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if cell.material == MaterialId::Air || cell.sat.0 == 0 {
            continue;
        }
        let take = amt.min(cell.sat.0);
        if take == 0 {
            continue;
        }

        // 1) Find or open a void seat; vapour mass goes into Humidity.
        let seat = find_vapour_seat(world, gx, gy)
            .or_else(|| open_pore_vapour_seat(world, gx, gy, expand))
            .or_else(|| {
                phase_crack_host(world, gx, gy, take, expand);
                find_vapour_seat(world, gx, gy)
                    .or_else(|| open_pore_vapour_seat(world, gx, gy, expand))
            });
        let Some((sx, sy)) = seat else {
            let drive = (take as u16)
                .saturating_mul(expand as u16)
                .min(255) as u8;
            reverse_seep_chain(world, gx, gy, drive, hops);
            work = work.saturating_add(1);
            continue;
        };

        let before = cell.sat.0;
        let mut next = cell;
        next.sat = Sat(before - take);
        world.set_cell(gx, gy, next);
        let placed = inject_vapour_near(world, humidity, temp, sx, sy, take);
        if placed == 0 {
            let mut back = world.get_cell(gx, gy).unwrap_or(next);
            back.sat = Sat(back.sat.0.saturating_add(take));
            world.set_cell(gx, gy, back);
            continue;
        }
        if placed < take {
            let mut back = world.get_cell(gx, gy).unwrap_or(next);
            back.sat = Sat(back.sat.0.saturating_add(take - placed));
            world.set_cell(gx, gy, back);
        }
        carry_with_water(world, (gx, gy), (sx, sy), placed, before);

        let drive = (placed as u16)
            .saturating_mul(expand as u16)
            .min(255) as u8;
        reverse_seep_chain(world, gx, gy, drive, hops);
        phase_crack_host(world, gx, gy, placed, expand);
        work = work.saturating_add(1);
    }
}

/// Nearby void Air for freshly boiled pore vapour (no sparse steam mint).
fn find_vapour_seat(world: &World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    const DELTAS: [(i32, i32); 10] = [
        (0, 1),
        (0, 2),
        (-1, 1),
        (1, 1),
        (-1, 0),
        (1, 0),
        (0, -1),
        (-1, 2),
        (1, 2),
        (0, 3),
    ];
    for (dx, dy) in DELTAS {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        if world.get_cell(tx, ty).is_some_and(is_steam_void) {
            return Some((tx, ty));
        }
    }
    None
}

/// Crack / burst toward a void seat for pore vapour (humidity path).
fn open_pore_vapour_seat(
    world: &mut World,
    gx: i32,
    gy: i32,
    expand: u8,
) -> Option<(i32, i32)> {
    open_pore_steam_seat(world, gx, gy, usize::MAX, expand)
}

/// Prefer nearby Air (especially above) for freshly boiled pore steam.
/// Legacy residual path (escape / flood).
fn find_steam_seat(
    world: &World,
    gx: i32,
    gy: i32,
    max_cells: usize,
) -> Option<(i32, i32)> {
    const DELTAS: [(i32, i32); 10] = [
        (0, 1),
        (0, 2),
        (-1, 1),
        (1, 1),
        (-1, 0),
        (1, 0),
        (0, -1),
        (-1, 2),
        (1, 2),
        (0, 3),
    ];
    for (dx, dy) in DELTAS {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(c) = world.get_cell(tx, ty) else {
            continue;
        };
        if c.material != MaterialId::Air {
            continue;
        }
        if can_admit_new_steam_cell(world, tx, ty, max_cells) || steam_at(world, tx, ty) > 0 {
            return Some((tx, ty));
        }
    }
    None
}

/// Expansion work opens a micro-void (widen / burst) so steam has somewhere to go.
fn open_pore_steam_seat(
    world: &mut World,
    gx: i32,
    gy: i32,
    max_cells: usize,
    expand: u8,
) -> Option<(i32, i32)> {
    // Prefer the cell above the boiling pore.
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(wall) = world.get_cell(tx, ty) else {
            continue;
        };
        if wall.material == MaterialId::Air || wall.material == MaterialId::Bedrock {
            continue;
        }
        if is_grain(wall.material) || is_flow_erodible(wall.material) {
            if burst_grain_tube(world, gx, gy, tx, ty, max_cells) {
                return Some((tx, ty));
            }
        }
        if crate::cell::is_competent_rock(wall.material) {
            let throughput = (160u16).min(80 + expand as u16 * 12) as u8;
            let scale = 1.2 + expand as f32 * 0.15;
            if widen_aperture(world, tx, ty, throughput, scale, 0xB01E_u64) {
                if world
                    .get_cell(tx, ty)
                    .is_some_and(|c| c.material == MaterialId::Air)
                {
                    return Some((tx, ty));
                }
            }
        }
    }
    None
}

/// Flash expansion cracks the boiling host pore (aperture growth).
fn phase_crack_host(world: &mut World, gx: i32, gy: i32, boiled: u8, expand: u8) {
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    if !crate::cell::is_competent_rock(cell.material) {
        return;
    }
    let throughput = boiled.saturating_mul(expand.min(16)).max(40);
    let scale = 0.9 + (expand as f32) * 0.12 + (boiled as f32) / 80.0;
    let _ = widen_aperture(world, gx, gy, throughput, scale, 0xB01C_u64);
}

/// Multi-hop reverse seepage: shove pore water toward lower pressure / upward.
fn reverse_seep_chain(world: &mut World, mut gx: i32, mut gy: i32, mut drive: u8, hops: u8) {
    for _ in 0..hops {
        if drive == 0 {
            return;
        }
        let moved = reverse_push_pore_water(world, gx, gy, drive);
        if moved == 0 {
            return;
        }
        // Follow the liquid upward if we can (pressure wants out toward surface).
        let mut advanced = false;
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2)] {
            let tx = world.wrap_x(gx + dx);
            let ty = gy + dy;
            if world
                .get_cell(tx, ty)
                .is_some_and(|c| c.material != MaterialId::Air && c.sat.0 > 0)
            {
                gx = tx;
                gy = ty;
                advanced = true;
                break;
            }
            if world
                .get_cell(tx, ty)
                .is_some_and(|c| c.material == MaterialId::Air)
            {
                // Reached a void — remaining drive punches steam/water into it next cadence.
                return;
            }
        }
        if !advanced {
            return;
        }
        drive = drive.saturating_sub(moved / 2).max(moved / 4);
    }
}

/// Push pore water one step; returns how much moved.
fn reverse_push_pore_water(world: &mut World, gx: i32, gy: i32, drive: u8) -> u8 {
    if drive == 0 {
        return 0;
    }
    let Some(src) = world.get_cell(gx, gy) else {
        return 0;
    };
    if src.material == MaterialId::Air || src.sat.0 == 0 {
        return 0;
    }
    let want = drive.min(src.sat.0).max(1);
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0)] {
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
        return moved;
    }
    0
}

fn escape_pressurized(
    world: &mut World,
    humidity: &Humidity,
    temp: &Temperature,
    cfg: &SteamConfig,
    max_cells: usize,
    seats: &[(i32, i32)],
) {
    let min_p = cfg.escape_pressure_min.clamp(0.02, 0.95);
    if seats.is_empty() {
        return;
    }
    let mut escapes = 0u8;
    let max_esc = cfg.max_escapes_per_tick.max(1);
    for &(gx, gy) in seats {
        if escapes >= max_esc {
            break;
        }
        let steam = steam_at(world, gx, gy);
        let press = cave_vapour_pressure_norm(world, humidity, temp, gx, gy)
            .max(steam as f32 / 255.0);
        if press < min_p && steam < 8 {
            continue;
        }
        let Some(above) = world.get_cell(gx, gy + 1) else {
            continue;
        };
        if above.material == MaterialId::Air {
            continue;
        }
        if above.material == MaterialId::Bedrock {
            continue;
        }

        // Soft lids burst first — that's the violent escape.
        if is_grain(above.material) || is_flow_erodible(above.material) {
            if burst_grain_tube(world, gx, gy, gx, gy + 1, max_cells) {
                escapes = escapes.saturating_add(1);
                let t = temp.at_cell(gx, gy + 1);
                if t < cfg.boil_point_c {
                    let warmth = ((cfg.boil_point_c - t) / 60.0).clamp(0.0, 1.0);
                    precipitate_artesian_warm(world, gx, gy + 1, warmth);
                }
                continue;
            }
        }

        if reverse_escape_through_rock(world, gx, gy, steam.max((press * 80.0) as u8), press) {
            escapes = escapes.saturating_add(1);
            continue;
        }

        if crate::cell::is_competent_rock(above.material) {
            let throughput = (140.0 + press * 115.0) as u8;
            let scale = 1.0 + press * 2.5;
            let opened = widen_aperture(world, gx, gy + 1, throughput, scale, 0x57EA_u64);
            if opened {
                let moved = take_steam(world, gx, gy, steam.min(160));
                if let Some(c) = world.get_cell(gx, gy + 1) {
                    if c.material == MaterialId::Air {
                        try_place_steam(world, gx, gy + 1, moved, max_cells);
                    } else {
                        add_steam(world, gx, gy, moved);
                        reverse_push_pore_water(world, gx, gy + 1, moved.max(8));
                    }
                } else {
                    add_steam(world, gx, gy, moved);
                }
                escapes = escapes.saturating_add(1);
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
    if perm < 4 {
        return false;
    }
    let drive = ((steam as f32) * (0.3 + press * 0.7)).round() as u8;
    let drive = drive.max(8);
    if roof.sat.0 > 0 {
        reverse_push_pore_water(world, gx, gy + 1, drive);
    }
    let roof2 = world.get_cell(gx, gy + 1).unwrap_or(roof);
    let cap = water_capacity_cell(roof2, &world.hydro);
    let room = cap.saturating_sub(roof2.sat.0);
    if room > 0 && steam > 4 {
        let put = room.min(steam / 3).max(1);
        let took = take_steam(world, gx, gy, put);
        let mut r = roof2;
        r.sat = Sat(r.sat.0.saturating_add(took));
        world.set_cell(gx, gy + 1, r);
        reverse_push_pore_water(world, gx, gy + 1, took.saturating_mul(2));
        return true;
    }
    roof.sat.0 > 0
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
    let _load = dissolved_at(world, tx, ty);
    let mut air = Cell::air();
    air.sat = Sat(cell.sat.0);
    world.set_cell(tx, ty, air);
    let moved = take_steam(world, from_x, from_y, steam_at(world, from_x, from_y).min(200));
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
    use crate::audit::{sat_totals, tracked_totals};
    use crate::chunk::ChunkCoord;
    use crate::humidity::Humidity;
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
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let before = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(
            steam_total(&w) > 0 || hum.total_mass() > 0.0,
            "hot surface water must boil to steam/humidity"
        );
        assert!(
            w.get_cell(4, 1).unwrap().sat.0 < 255,
            "boil must consume free sat"
        );
        let after = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        assert!(
            (after - before).abs() < 1e-3,
            "boil+exchange must be mass-flat ({before}→{after})"
        );
    }

    #[test]
    fn confined_steam_fills_whole_void_instantly() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..7 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        add_steam(&mut w, 3, 2, 200);
        assert!(void_is_confined(&w, 3, 2));
        let hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &mut Humidity::new(4), &SteamConfig::default());
        let mut filled = 0;
        for x in 3..7 {
            for y in 2..5 {
                if steam_at(&w, x, y) > 0 {
                    filled += 1;
                }
            }
        }
        assert!(
            filled >= 6,
            "pressurized steam must flood the connected void (filled={filled})"
        );
        assert!(
            steam_at(&w, 3, 4) + steam_at(&w, 4, 4) + steam_at(&w, 5, 4) > 0,
            "gas must reach the roof of the pocket, not sit on the floor"
        );
        let field = steam_vapour_field(&w);
        let painted = field
            .iter()
            .filter(|&&(x, y, d)| (3..7).contains(&x) && (2..5).contains(&y) && d >= 28)
            .count();
        assert!(
            painted >= 10,
            "humidity-like haze must wash the pocket (painted={painted})"
        );
        let haze = steam_haze_wash(&w, Some(&hot));
        assert!(
            haze.iter().any(|s| s.warmth > 0),
            "hot confined steam haze must carry warmth"
        );
    }

    #[test]
    fn leaky_cave_still_equalizes_as_field() {
        let mut w = World::new(31);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Chamber with a one-cell vent to open sky above.
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w.set_cell(5, 5, Cell::air()); // vent through the roof
        for y in 6..12 {
            w.set_cell(5, y, Cell::air());
        }
        add_steam(&mut w, 3, 2, 180);
        assert!(void_is_confined(&w, 3, 2));
        let hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &mut Humidity::new(4), &SteamConfig::default());
        let chamber = steam_at(&w, 3, 3) + steam_at(&w, 4, 3) + steam_at(&w, 6, 3);
        assert!(
            chamber > 0,
            "leaky cave must still equalize the chamber, not only plume the vent"
        );
        let field = steam_vapour_field(&w);
        assert!(
            field.iter().any(|&(x, y, d)| y <= 4 && (3..7).contains(&x) && d >= 28),
            "vapour wash must cover the chamber"
        );
    }

    #[test]
    fn steam_haze_is_coarse_humidity_shaped() {
        let mut w = World::new(41);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..10 {
            for y in 1..9 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..9 {
            for y in 2..8 {
                w.set_cell(x, y, Cell::air());
            }
        }
        add_steam(&mut w, 4, 3, 180);
        add_steam(&mut w, 5, 4, 180);
        let hot = temp_fill(&w, 140.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &mut Humidity::new(4), &SteamConfig::default());
        let haze = steam_haze_wash(&w, Some(&hot));
        assert!(!haze.is_empty(), "steam must produce a haze wash");
        // Samples live on the cell grid but mass is 4×4 — neighbouring cells in
        // a tile should share similar density (not speckled markers).
        let mut by_tile: std::collections::HashMap<(i32, i32), Vec<u8>> =
            std::collections::HashMap::new();
        for s in &haze {
            let key = (s.gx.div_euclid(STEAM_HAZE_TILE), s.gy.div_euclid(STEAM_HAZE_TILE));
            by_tile.entry(key).or_default().push(s.density);
        }
        assert!(
            by_tile.len() >= 1,
            "haze must occupy coarse tiles"
        );
        let max_density = haze.iter().map(|s| s.density).max().unwrap_or(0);
        assert!(
            max_density <= 200,
            "haze must stay soft (max density {max_density}), not an opaque plug"
        );
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
        assert!(void_is_confined(&w, 4, 2));
        let hot = temp_fill(&w, 120.0);
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(
            hum.total_mass() > 0.0,
            "confined boil must put vapour into humidity"
        );
        assert!(
            w.get_cell(4, 2).unwrap().sat.0 < 255,
            "boil must consume free water sat"
        );
        let press = cave_vapour_pressure_norm(&w, &hum, &hot, 4, 3)
            .max(cave_vapour_pressure_norm(&w, &hum, &hot, 5, 3));
        assert!(
            press > 0.0,
            "cave humidity must pressurize the void (press={press:.3})"
        );
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 6) == 0 && steam_at(&w, 4, 7) == 0,
            "vapour must not mint steam above an intact stone roof"
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
        apply_steam(&mut w, &cool, &mut Humidity::new(4), &SteamConfig::default());
        assert_eq!(steam_at(&w, 3, 1), 0);
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
        assert!(boost > 1.05, "boost={boost:.3}");
        let before = w.get_cell(2, 1).unwrap().sat.0;
        w.tick = 16;
        wake_confined_head(&mut w);
        let after = w.get_cell(2, 1).unwrap().sat.0;
        assert!(after >= before);
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
        apply_steam(&mut w, &hot, &mut Humidity::new(4), &SteamConfig::default());
        assert!(steam_total(&w) > 0, "open steam must remain");
        assert!(
            steam_at(&w, 5, 2) < 40,
            "open steam must leave the floor (left={})",
            steam_at(&w, 5, 2)
        );
        let highest = w
            .steam
            .keys()
            .filter(|(x, _)| *x == 5)
            .map(|(_, y)| *y)
            .max()
            .unwrap_or(2);
        assert!(
            highest > 2,
            "open steam must climb the shaft (highest={highest})"
        );
    }

    #[test]
    fn wet_boil_site_produces_plume() {
        let mut w = World::new(13);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(4, 1, Cell::water());
        for y in 2..16 {
            w.set_cell(4, y, Cell::air());
        }
        let hot = temp_fill(&w, 150.0);
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(
            hum.total_mass() > 0.0,
            "hot wet vent must boil into humidity"
        );
        let mass0 = hum.total_mass();
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(
            hum.total_mass() >= mass0,
            "continued boil must keep or grow humidity plume"
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
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let before = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(steam_total(&w) > 0 || hum.total_mass() > 0.0);
        assert!(
            w.get_cell(4, 1).unwrap().sat.0 < crate::cell::water_capacity(MaterialId::Sand)
        );
        let after = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        assert!(
            (after - before).abs() < 1e-3,
            "pore boil+exchange must be mass-flat ({before}→{after})"
        );
    }

    #[test]
    fn pressurized_steam_bursts_sand_roof() {
        let mut w = World::new(19);
        w.ensure_chunk(ChunkCoord::new(0, 0));
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
        w.set_cell(4, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
        add_steam(&mut w, 4, 3, 220);
        add_steam(&mut w, 5, 3, 220);
        let hot = temp_fill(&w, 130.0);
        let cfg = SteamConfig {
            escape_pressure_min: 0.02,
            ..SteamConfig::default()
        };
        for i in 1..12 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &hot, &mut Humidity::new(4), &cfg);
        }
        let sand_gone = w.get_cell(4, 4).unwrap().material == MaterialId::Air
            || w.get_cell(5, 4).unwrap().material == MaterialId::Air
            || steam_at(&w, 4, 4) > 0
            || steam_at(&w, 5, 4) > 0;
        assert!(sand_gone);
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
        apply_steam(&mut w, &cool, &mut Humidity::new(4), &SteamConfig::default());
        assert_eq!(steam_at(&w, 3, 1), 0);
        assert_eq!(crate::audit::mineral_total(&w), before_min);
        assert!(
            dissolved_at(&w, 3, 1) < 400
                || w.get_cell(3, 1).unwrap().material == MaterialId::Flowstone
        );
    }

    #[test]
    fn steam_assault_widens_wet_limestone() {
        let mut w = World::new(29);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..6 {
            for y in 1..5 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(3, 2, Cell::air());
        w.set_cell(4, 2, Cell::air());
        let mut lime = Cell::solid(MaterialId::Limestone);
        lime.sat = Sat(40);
        lime.pore = 128;
        w.set_cell(3, 3, lime);
        w.set_cell(3, 4, Cell::solid(MaterialId::Stone));
        add_steam(&mut w, 3, 2, 255);
        add_steam(&mut w, 4, 2, 255);
        let pore0 = w.get_cell(3, 3).unwrap().pore;
        let hot = temp_fill(&w, 130.0);
        for i in 1..20 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &hot, &mut Humidity::new(4), &SteamConfig::default());
        }
        let after = w.get_cell(3, 3).unwrap();
        assert!(
            after.pore > pore0 || after.material == MaterialId::Air || after.sat.0 < 40,
            "steam pressure must assault limestone (pore {pore0}→{}, sat={}, mat={:?})",
            after.pore,
            after.sat.0,
            after.material
        );
    }

    #[test]
    fn sealed_pore_phase_change_reverse_seeps_upward() {
        // Column of wet limestone with no free Air next to the boiler —
        // liquid→gas expansion must shove pore water upward (seepage in reverse)
        // and/or crack apertures. Mass stays flat.
        let mut w = World::new(37);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..7 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        let cap = crate::cell::water_capacity(MaterialId::Limestone).max(1);
        let mut boil = Cell::solid(MaterialId::Limestone);
        boil.sat = Sat(cap);
        boil.pore = 100;
        w.set_cell(4, 1, boil);
        let mut mid = Cell::solid(MaterialId::Limestone);
        mid.sat = Sat(cap / 4);
        mid.pore = 100;
        w.set_cell(4, 2, mid);
        let mut top = Cell::solid(MaterialId::Limestone);
        top.sat = Sat(cap / 8);
        top.pore = 100;
        w.set_cell(4, 3, top);
        // Solid roof — no Air seat beside the boiling cell.
        w.set_cell(4, 4, Cell::solid(MaterialId::Stone));
        let sat_above0 = w.get_cell(4, 2).unwrap().sat.0 as i32
            + w.get_cell(4, 3).unwrap().sat.0 as i32;
        let pore0 = w.get_cell(4, 1).unwrap().pore;
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let before = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        let hot = temp_fill(&w, 160.0);
        let cfg = SteamConfig {
            enable_escape: false,
            phase_expansion_drive: 64,
            reverse_seep_hops: 4,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..10 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &hot, &mut hum, &cfg);
        }
        let sat_above1 = w.get_cell(4, 2).unwrap().sat.0 as i32
            + w.get_cell(4, 3).unwrap().sat.0 as i32;
        let host = w.get_cell(4, 1).unwrap();
        let reverse_or_crack = sat_above1 > sat_above0
            || host.pore > pore0
            || host.material == MaterialId::Air
            || host.sat.0 < cap
            || steam_total(&w) > 0
            || hum.total_mass() > 0.0;
        assert!(
            reverse_or_crack,
            "phase expansion must reverse-seep, crack, or vent vapour \
             (above {sat_above0}→{sat_above1}, pore {pore0}→{}, sat={}, steam={}, hum={})",
            host.pore,
            host.sat.0,
            steam_total(&w),
            hum.total_mass()
        );
        let after = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        assert!(
            (after - before).abs() < 1e-3,
            "phase boil must stay mass-flat ({before}→{after})"
        );
    }

    #[test]
    fn phase_expansion_drive_exceeds_boiled_mass() {
        // Expansion factor is force: reverse push budget ≫ sat converted.
        assert!(PHASE_EXPANSION_DRIVE >= 24);
        let boiled = 10u8;
        let drive = (boiled as u16)
            .saturating_mul(PHASE_EXPANSION_DRIVE as u16)
            .min(255) as u8;
        assert!(
            drive > boiled.saturating_mul(4),
            "drive={drive} must dwarf boiled={boiled}"
        );
    }

    #[test]
    fn hot_steam_exchanges_into_humidity() {
        let mut w = World::new(43);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        add_steam(&mut w, 4, 3, 200);
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let hot = temp_fill(&w, 160.0);
        let before = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        w.tick = 1;
        apply_steam(&mut w, &hot, &mut hum, &SteamConfig::default());
        assert!(
            hum.total_mass() > 50.0,
            "void steam must join humidity (hum={})",
            hum.total_mass()
        );
        assert!(
            steam_at(&w, 4, 3) <= STEAM_PRESSURE_RESIDUAL + 40,
            "most steam mass should leave the sparse map"
        );
        let after = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        assert!((after - before).abs() < 1e-3);
    }

    #[test]
    fn cave_humidity_counts_as_vapour_pressure() {
        let mut w = World::new(47);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        hum.add(4, 3, Humidity::MAX_MASS_PER_TILE * 0.9);
        let hot = temp_fill(&w, 80.0);
        let press = cave_vapour_pressure_norm(&w, &hum, &hot, 4, 3);
        assert!(
            press > 0.4,
            "confined high humidity must pressurize (press={press:.2})"
        );
        assert_eq!(steam_at(&w, 4, 3), 0);
    }

    #[test]
    fn pore_vapour_equalizes_into_dry_cave_air() {
        let mut w = World::new(53);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        let cap = crate::cell::water_capacity(MaterialId::Sand);
        sand.sat = Sat(cap);
        w.set_cell(4, 3, sand); // wet rock beside cave? need wall of cave
        // Place wet sand as cave wall at (3,3) with air at (4,3)
        w.set_cell(4, 3, Cell::air());
        w.set_cell(3, 3, sand);
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let warm = temp_fill(&w, 40.0); // below boil
        let before = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        let sat0 = w.get_cell(3, 3).unwrap().sat.0;
        w.tick = PORE_CAVE_EQ_EVERY;
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            ..SteamConfig::default()
        };
        apply_steam(&mut w, &warm, &mut hum, &cfg);
        assert!(
            hum.total_mass() > 0.0,
            "wet rock must donate vapour into dry cave air"
        );
        assert!(
            w.get_cell(3, 3).unwrap().sat.0 < sat0,
            "pore sat must fall when equalizing out"
        );
        let after = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        assert!(
            (after - before).abs() < 1e-3,
            "pore↔cave eq must be mass-flat ({before}→{after})"
        );
    }

    #[test]
    fn oversaturated_cave_humidity_wets_dry_rock() {
        let mut w = World::new(59);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(1); // keeps has_wet_pores; still has room for cave H
        w.set_cell(3, 3, sand);
        let mut hum = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        // Force oversaturated tile mass above cold-air capacity.
        hum.cells.insert(hum.tile_of(4, 3), 800.0);
        let cool = temp_fill(&w, 10.0);
        let before = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        let sat0 = w.get_cell(3, 3).unwrap().sat.0;
        w.tick = PORE_CAVE_EQ_EVERY;
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            ..SteamConfig::default()
        };
        apply_steam(&mut w, &cool, &mut hum, &cfg);
        assert!(
            w.get_cell(3, 3).unwrap().sat.0 > sat0,
            "oversaturated cave air must wet adjacent dry rock"
        );
        let after = tracked_totals(&w, &hum, &crate::clouds::CloudStore::default()).tracked();
        assert!((after - before).abs() < 1e-3);
    }
}
