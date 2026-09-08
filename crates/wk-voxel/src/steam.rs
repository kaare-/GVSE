//! Pressurized **cavity humidity** — sparse sealed / semi-sealed void store (P3).
//!
//! **Store:** `World.steam` is the wire name for capped sparse **cavity humidity
//! under pressure** in roofed / side-closed voids. It is **not** sky humidity.
//! Cool sealed ambient still uses [`crate::cave_humidity`]; this map is the
//! over-capacity / flash / pressure mode of closed-cavity moisture — not a
//! separate "steam gas" species.
//!
//! **Open vs sealed:**
//! - Unroofed hot free water is ordinary **accelerated evaporation** into
//!   sky [`Humidity`] — owned by [`crate::rules::evap`], not this module.
//! - Open caves / overhangs share sky Humidity (T5 continuity).
//! - Roofed / confined free water may flash into this store (pressure /
//!   geyser motor). Ambient moist cave air under rock uses `cave_humidity`.
//!
//! Sealed cavity humidity **never** dumps into the rain lottery.
//!
//! **Motor:** boil roofed free + pore water above ~100 °C → sparse cavity
//! humidity. Liquid→gas expansion budgets reverse pore seepage + aperture
//! work. Flowing hot water **carries heat** into cooler rock so channels warm
//! over time. Existing cavity humidity keeps pushing pore water by density
//! even through rock below boil. Confined pockets flood-equalize; overpressure
//! assaults wet rock as **high-aperture conduits** (no instant Air pipes) and
//! only bursts soft lids that open to free atmosphere. Cool → liquid + sinter.
//! Buried grain lenses under competent rock are not treated as sand pillars.
//!
//! See docs/VOXEL_GEYSER.md + VOXEL_THERMAL.md.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{
    is_flow_erodible, is_grain, permeability_cell, water_capacity_cell, Cell, Sat,
};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::{FxHashMap, FxHashSet};
use crate::grid::World;
use crate::mineral::{
    add_dissolved, carry_with_water, dissolved_at, emit_from_dissolved_rock,
    is_soluble_rock, precipitate_artesian_warm, precipitate_at, precipitate_dry_cell,
    widen_aperture, MINERAL_PER_CELL,
};
use crate::sediment::{add_suspended, is_suspendable, SEDIMENT_PER_CELL};
use crate::temperature::Temperature;

/// Cadence for boil / flood / assault / recondense (FPS: not every tick).
pub const STEAM_EVERY: u64 = 5;

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
/// Real steam is ~1000–1700× liquid volume; we use a capped sim factor so each
/// boiled sat unit budgets this many units of reverse seepage + aperture
/// work. Mass stays flat (boiled sat ↔ sparse vapour); the factor is *force*,
/// not minted water. Low values feel inert; dozens read as flash without
/// pretending full 1700×. Heat above boil further scales the pulse
/// ([`phase_heat_drive_scale`]) — a significant spike, still not Clausius
/// full expansion.
pub const PHASE_EXPANSION_DRIVE: u8 = 48;

/// Heat multiplier on phase-expansion force above boil.
///
/// 1.0 at the boil point; climbs toward ~3× by boil+80 °C. Hot groundwater
/// therefore pushes harder as it superheats, without minting mass.
#[inline]
pub fn phase_heat_drive_scale(temp_c: f32, boil_c: f32) -> f32 {
    if !temp_c.is_finite() || temp_c <= boil_c {
        return 1.0;
    }
    let over = ((temp_c - boil_c) / 40.0).clamp(0.0, 2.0);
    1.0 + over
}

/// Reverse-seepage + crack budget for a boiled pore pulse (force, not mass).
#[inline]
pub fn expansion_drive_units(boiled: u8, expand: u8, temp_c: f32, boil_c: f32) -> u8 {
    if boiled == 0 {
        return 0;
    }
    let heat = phase_heat_drive_scale(temp_c, boil_c);
    let raw = (boiled as f32) * (expand.max(1) as f32) * heat;
    raw.round().clamp(1.0, 255.0) as u8
}

/// How many reverse-seepage hops a phase-expansion pulse may travel.
pub const REVERSE_SEEP_HOPS: u8 = 10;

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
pub fn add_steam(world: &mut World, gx: i32, gy: i32, amount: u8) -> u8 {
    if amount == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let slot = world.steam.entry((gx, gy)).or_insert(0);
    let before = *slot;
    *slot = before.saturating_add(amount);
    *slot - before
}

/// Alias: pressurized cavity humidity mass at a cell (`World.steam` wire).
#[inline]
pub fn cavity_humidity_at(world: &World, gx: i32, gy: i32) -> u8 {
    steam_at(world, gx, gy)
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

/// Sky-connected Air (open shaft / cave vent) — weather Humidity may sit here.
///
/// Cheap path: upward probe ([`void_is_confined`]). If roofed, a short Air BFS
/// looks for any neighbour that opens to sky (side entrance / skylight offset).
/// Sealed pockets return false so they stay off the rain lottery.
pub fn air_void_open_to_sky(world: &World, gx: i32, gy: i32) -> bool {
    const BFS_BUDGET: usize = 96;
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return true;
    };
    if cell.material != MaterialId::Air {
        return false;
    }
    if !void_is_confined(world, gx, gy) {
        return true;
    }
    let mut q: Vec<(i32, i32)> = vec![(gx, gy)];
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert((gx, gy));
    let mut steps = 0usize;
    while let Some((x, y)) = q.pop() {
        steps += 1;
        if steps > BFS_BUDGET {
            return false;
        }
        for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
            let nx = world.wrap_x(x + dx);
            let ny = y + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            match world.get_cell(nx, ny) {
                None => return true,
                Some(c) if c.material == MaterialId::Air => {
                    if !void_is_confined(world, nx, ny) {
                        return true;
                    }
                    q.push((nx, ny));
                }
                Some(_) => {}
            }
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

/// Confined-rise rate boost from underground steam (1 + span * norm).
#[inline]
pub fn steam_pressure_rate_scale(world: &World, gx: i32, gy: i32) -> f32 {
    1.0 + STEAM_PRESSURE_RATE_SPAN * steam_pressure_norm(world, gx, gy)
}

/// What kind of pressure [`cell_pressure_norm`] is reporting.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CellPressureKind {
    /// Sparse pressurized cavity humidity (wire: `World.steam`).
    Cavity,
    /// Hot wet rock / pore water — flash drive + nearby cavity influence.
    PoreFlash,
    /// Negligible.
    None,
}

/// Unified 0..=1 pressure readout for inspector + overlay.
///
/// - **Air / steam seats:** column cavity fill ([`steam_pressure_norm`]).
/// - **Hot saturated rock:** pore flash drive from wetness × superheat above
///   boil, blended with nearby cavity pressure so conduits read as gradients
///   without inventing a continuum PDE.
pub fn cell_pressure_norm(world: &World, gx: i32, gy: i32, temp_c: f32) -> (f32, CellPressureKind) {
    let steam = steam_at(world, gx, gy);
    let cavity = steam_pressure_norm(world, gx, gy);
    let Some(cell) = world.get_cell(gx, gy) else {
        return if cavity > 0.02 {
            (cavity, CellPressureKind::Cavity)
        } else {
            (0.0, CellPressureKind::None)
        };
    };

    if cell.material == MaterialId::Air || steam > 0 {
        let local = (steam as f32 / 255.0).max(cavity);
        return if local > 0.02 || (cell.material == MaterialId::Air && cell.sat.0 > 0 && temp_c >= 95.0)
        {
            (local.clamp(0.0, 1.0), CellPressureKind::Cavity)
        } else {
            (0.0, CellPressureKind::None)
        };
    }

    let cap = water_capacity_cell(cell, &world.hydro);
    let wet = if cap == 0 {
        0.0
    } else {
        (cell.sat.0 as f32 / cap as f32).clamp(0.0, 1.0)
    };
    if wet < 0.05 && cavity < 0.02 {
        return (0.0, CellPressureKind::None);
    }
    let boil = BOIL_POINT_C;
    let heat = if !temp_c.is_finite() {
        0.0
    } else if temp_c >= boil {
        // 0 at boil → 1 by boil+80 °C (matches phase-heat drive span).
        ((temp_c - boil) / 80.0).clamp(0.0, 1.0)
    } else if temp_c >= boil - 15.0 {
        // Soft approach so near-boil wet rock isn't invisible.
        (((temp_c - (boil - 15.0)) / 15.0).clamp(0.0, 1.0)) * 0.2
    } else {
        0.0
    };
    let pore_flash = wet * heat;
    // Nearby cavity humidity still pushes through wet rock below/around boil.
    let blended = (pore_flash * 0.9 + cavity * (0.35 + 0.65 * wet)).clamp(0.0, 1.0);
    if blended < 0.02 {
        return (0.0, CellPressureKind::None);
    }
    let kind = if pore_flash >= cavity * 0.5 && pore_flash > 0.02 {
        CellPressureKind::PoreFlash
    } else if cavity > 0.02 {
        CellPressureKind::Cavity
    } else {
        CellPressureKind::PoreFlash
    };
    (blended, kind)
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
    let room = 255u8.saturating_sub(steam_at(world, gx, gy));
    let put = amt.min(room);
    if put == 0 {
        return 0;
    }
    add_steam(world, gx, gy, put)
}

/// Spill `amount` across seats without truncating a multi-cell total to 255.
fn spill_steam_across(
    world: &mut World,
    seats: &[(i32, i32)],
    mut amount: u32,
    max_cells: usize,
) -> u32 {
    if amount == 0 || seats.is_empty() {
        return amount;
    }
    for _ in 0..4 {
        if amount == 0 {
            break;
        }
        let mut progress = false;
        for &(x, y) in seats {
            if amount == 0 {
                break;
            }
            let room = 255u32.saturating_sub(steam_at(world, x, y) as u32);
            if room == 0 {
                continue;
            }
            let put = amount.min(room).min(255) as u8;
            let placed = try_place_steam(world, x, y, put, max_cells);
            if placed > 0 {
                amount -= placed as u32;
                progress = true;
            }
        }
        if !progress {
            break;
        }
    }
    amount
}

/// Prefer injecting boiled steam into void Air above / beside the source.
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
        let placed = try_place_steam(world, tx, ty, amt, max_cells);
        if placed > 0 {
            return placed;
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

/// Boil / flood / assault / escape / recondense.
///
/// **Hard rule:** sealed / roofed vapour never dumps into the rain lottery.
/// Unroofed hot free water is left for accelerated evap → sky Humidity.
pub fn apply_steam(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
) {
    if !cfg.enabled {
        return;
    }
    let period = cfg.period_ticks.max(1);
    let due = world.tick % period == 0;
    if !due {
        return;
    }
    let max_cells = cfg.max_steam_cells.max(1) as usize;
    let boil = cfg.boil_point_c;
    let recondense_below = boil - RECONDENSE_MARGIN_C;

    recondense_cool(world, temp, recondense_below);
    boil_hot_air(world, temp, cfg, max_cells);
    if cfg.enable_pore_boil {
        boil_hot_pores(world, temp, cfg, max_cells);
    }
    // Flood + assault only on cadence (every-tick flood crushed FPS).
    if !world.steam.is_empty() {
        flood_equalize_steam(world, cfg, max_cells);
        // Density-driven push + heat deposit continue past the boil isotherm.
        transmit_cavity_pressure(world, temp, cfg);
        assault_steam_walls(world, temp, cfg);
        if cfg.enable_escape {
            escape_pressurized(world, temp, cfg, max_cells);
            if !world.steam.is_empty() {
                flood_equalize_steam(world, cfg, max_cells);
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
            // Unloaded / missing cell: keep mass; do not destroy vapour.
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
            let seats: Vec<(i32, i32)> = component.clone();
            let _ = spill_steam_across(world, &seats, total, max_cells);
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
                let placed = try_place_steam(world, x, y, put, max_cells);
                if placed < put {
                    let _ = spill_steam_across(
                        world,
                        &voids,
                        (put - placed) as u32,
                        max_cells,
                    );
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
                let placed = try_place_steam(world, x, y, put as u8, max_cells) as u32;
                left -= placed;
            }
            if left > 0 {
                let seats = if voids.is_empty() {
                    vec![(sx, sy)]
                } else {
                    voids.clone()
                };
                let _ = spill_steam_across(world, &seats, left, max_cells);
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

/// Steam pressure assaults neighbouring wet rock: reverse push + fast widen.
/// Prefers the roof (up) so energy goes into escape tubes, not sideways leaks.
/// Cavity humidity density keeps shoving pore water and depositing heat
/// past the boil isotherm — pressure does not die the moment rock is <100 °C.
fn transmit_cavity_pressure(world: &mut World, temp: &mut Temperature, cfg: &SteamConfig) {
    if world.steam.is_empty() {
        return;
    }
    let boil = cfg.boil_point_c;
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut work = 0u8;
    let max_work = cfg.max_escapes_per_tick.saturating_mul(2).max(12);
    for (gx, gy) in keys {
        if work >= max_work {
            break;
        }
        let dens = steam_at(world, gx, gy);
        if dens < 10 {
            continue;
        }
        let press = steam_pressure_norm(world, gx, gy).max(dens as f32 / 255.0);
        let target = boil + press * 35.0;
        let mix = (dens as f32 / 255.0) * 0.18;
        temp.deposit_heat_toward(gx, gy, target, mix);
        for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0), (-1, 1), (1, 1)] {
            let tx = world.wrap_x(gx + dx);
            let ty = gy + dy;
            temp.deposit_heat_toward(tx, ty, target, mix * 0.65);
            let Some(wall) = world.get_cell(tx, ty) else {
                continue;
            };
            if wall.material == MaterialId::Air || wall.material == MaterialId::Bedrock {
                continue;
            }
            if wall.sat.0 == 0 || permeability_cell(wall, &world.hydro) == 0 {
                continue;
            }
            let drive = ((dens as f32) * (0.2 + press) * 0.85).round() as u8;
            if drive >= 4 {
                let moved = reverse_push_pore_water(world, temp, tx, ty, drive);
                if moved > 0 {
                    work = work.saturating_add(1);
                }
            }
        }
    }
}

fn assault_steam_walls(world: &mut World, temp: &mut Temperature, cfg: &SteamConfig) {
    if world.steam.is_empty() {
        return;
    }
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut assaults = 0u8;
    let max_a = cfg.max_escapes_per_tick.saturating_mul(2).max(8);
    for (gx, gy) in keys {
        if assaults >= max_a {
            break;
        }
        let steam = steam_at(world, gx, gy);
        if steam < 12 {
            continue;
        }
        let press = steam_pressure_norm(world, gx, gy).max(steam as f32 / 255.0);
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
                let drive = ((steam as f32) * (0.25 + press)).round() as u8;
                reverse_push_pore_water(world, temp, tx, ty, drive.max(4));
                assaults = assaults.saturating_add(1);
            }
            // Prefer pore conduits over minting Air columns. Assault never
            // dissolves the last cement step (`mint_void = false`).
            if crate::cell::is_competent_rock(wall.material)
                && press >= cfg.escape_pressure_min * 0.5
            {
                let up_bias = if dy > 0 { 0.85 } else { 0.4 };
                let throughput = ((80.0 + press * 90.0) * up_bias) as u8;
                let scale = (1.6 + press * 2.8) * up_bias;
                if widen_aperture(
                    world,
                    tx,
                    ty,
                    throughput.max(1),
                    scale,
                    0x0A51_u64,
                    false,
                ) {
                    assaults = assaults.saturating_add(1);
                } else if world
                    .get_cell(tx, ty)
                    .is_some_and(|c| c.pore > wall.pore)
                {
                    assaults = assaults.saturating_add(1);
                }
            }
        }
    }
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

fn boil_hot_air(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    max_cells: usize,
) {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let boil = cfg.boil_point_c;
    let prefer_confined = world.steam.len() + 32 >= max_cells;
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
                let confined = void_is_confined(world, gx, gy);
                // Open seats belong to accelerated evap → sky Humidity.
                // Steam is sealed-flash only. Near cap, prefer confined seats.
                if !confined {
                    continue;
                }
                let _ = prefer_confined; // reserved if we later prioritize seats
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
        // Unroofed seats are filtered at collect time; belt-and-braces.
        if !void_is_confined(world, gx, gy) {
            continue;
        }
        let placed = inject_steam_near(world, gx, gy, take, max_cells);
        if placed == 0 {
            continue;
        }
        let mut next = cell;
        next.sat = Sat(cell.sat.0 - placed);
        world.set_cell(gx, gy, next);
        if next.sat.0 == 0 {
            precipitate_dry_cell(world, gx, gy);
        } else {
            let _ = precipitate_at(world, gx, gy);
        }
        let src_t = temp.at_cell(gx, gy).max(cfg.boil_point_c);
        temp.deposit_heat_toward(gx, gy, src_t, 0.2);
        for (dx, dy) in [(0, 1), (0, 2), (-1, 1), (1, 1)] {
            temp.deposit_heat_toward(world.wrap_x(gx + dx), gy + dy, src_t, 0.12);
        }
    }
}

fn boil_hot_pores(world: &mut World, temp: &mut Temperature, cfg: &SteamConfig, max_cells: usize) {
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

        // 1) Find or open a seat for the vapour mass (mass-flat).
        let seat = find_steam_seat(world, gx, gy, max_cells)
            .or_else(|| open_pore_steam_seat(world, gx, gy, max_cells, expand))
            .or_else(|| {
                // Fully sealed impermeable neighbourhood: spend expansion on
                // aperture growth, then retry for a newly opened seat.
                phase_crack_host(world, gx, gy, take, expand);
                find_steam_seat(world, gx, gy, max_cells)
                    .or_else(|| open_pore_steam_seat(world, gx, gy, max_cells, expand))
            });
        let Some((sx, sy)) = seat else {
            // Still no vapour seat: expansion still shoves remaining pore water
            // along least-resistance seepage so the tube keeps growing.
            let t_c = temp.at_cell(gx, gy);
            let drive = expansion_drive_units(take, expand, t_c, boil);
            reverse_seep_chain(world, temp, gx, gy, drive, hops);
            work = work.saturating_add(1);
            continue;
        };

        let before = cell.sat.0;
        let placed = try_place_steam(world, sx, sy, take, max_cells);
        if placed == 0 {
            // No vapour room: expansion still shoves remaining pore water.
            let t_c = temp.at_cell(gx, gy);
            let drive = expansion_drive_units(take, expand, t_c, boil);
            reverse_seep_chain(world, temp, gx, gy, drive, hops);
            phase_crack_host(world, gx, gy, take, expand);
            work = work.saturating_add(1);
            continue;
        }
        let mut next = cell;
        next.sat = Sat(before - placed);
        world.set_cell(gx, gy, next);
        carry_with_water(world, (gx, gy), (sx, sy), placed, before);
        if world.get_cell(gx, gy).is_some_and(|c| c.sat.0 == 0) {
            precipitate_dry_cell(world, gx, gy);
        } else {
            let _ = precipitate_at(world, gx, gy);
        }
        // Flash vapour / hot liquid carry heat into the seat and channel.
        temp.advect_with_mass(gx, gy, sx, sy, placed);
        temp.deposit_heat_toward(sx, sy, temp.at_cell(gx, gy).max(boil), 0.35);

        // Phase-change pressure: shove after flash. Chain hops into wet
        // neighbours if the host was emptied by the boil take.
        let t_c = temp.at_cell(gx, gy);
        let drive = expansion_drive_units(placed, expand, t_c, boil);
        reverse_seep_chain(world, temp, gx, gy, drive, hops);
        // Host cell itself also widens under flash expansion.
        phase_crack_host(world, gx, gy, placed, expand);
        work = work.saturating_add(1);
    }
}

/// Prefer nearby Air (especially above) for freshly boiled pore steam.
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

/// Expansion work widens wet competent neighbours into conduits. Never bursts
/// buried grains into Air seats — that was the cheap sand-pipe look.
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
        if wall.material == MaterialId::Air {
            if can_admit_new_steam_cell(world, tx, ty, max_cells) || steam_at(world, tx, ty) > 0
            {
                return Some((tx, ty));
            }
            continue;
        }
        if wall.material == MaterialId::Bedrock {
            continue;
        }
        // Soft neighbours: reverse-push only. Burst is reserved for true
        // atmosphere soft lids in `escape_pressurized`.
        if is_grain(wall.material) || is_flow_erodible(wall.material) {
            continue;
        }
        if crate::cell::is_competent_rock(wall.material) {
            let throughput = (140u16).min(48 + expand as u16 * 8) as u8;
            let scale = 2.0 + expand as f32 * 0.08;
            let _ = widen_aperture(world, tx, ty, throughput, scale, 0xB01E_u64, false);
        }
    }
    // After conduit work, only seat into Air that already exists nearby.
    find_steam_seat(world, gx, gy, max_cells)
}

/// Flash expansion cracks the boiling host pore (aperture growth).
fn phase_crack_host(world: &mut World, gx: i32, gy: i32, boiled: u8, expand: u8) {
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    if !crate::cell::is_competent_rock(cell.material) {
        return;
    }
    let throughput = boiled.saturating_mul(expand.min(16)).max(48);
    let scale = 1.2 + (expand as f32) * 0.06 + (boiled as f32) / 120.0;
    let _ = widen_aperture(world, gx, gy, throughput, scale, 0xB01C_u64, false);
}

/// Multi-hop reverse seepage: shove pore water along the easiest wet path.
fn reverse_seep_chain(
    world: &mut World,
    temp: &mut Temperature,
    mut gx: i32,
    mut gy: i32,
    mut drive: u8,
    hops: u8,
) {
    for _ in 0..hops {
        if drive == 0 {
            return;
        }
        let (moved, dest) = reverse_push_pore_water_to(world, temp, gx, gy, drive);
        if moved == 0 {
            // Host may have flashed dry after boil — keep the pulse moving
            // through a wet neighbour so conduits still lengthen.
            let mut hopped = false;
            for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, 2)] {
                let nx = world.wrap_x(gx + dx);
                let ny = gy + dy;
                let Some(c) = world.get_cell(nx, ny) else {
                    continue;
                };
                if c.material == MaterialId::Air || c.sat.0 == 0 {
                    continue;
                }
                if permeability_cell(c, &world.hydro) == 0 {
                    continue;
                }
                gx = nx;
                gy = ny;
                hopped = true;
                break;
            }
            if !hopped {
                return;
            }
            continue;
        }
        let Some((nx, ny)) = dest else {
            return;
        };
        if world
            .get_cell(nx, ny)
            .is_some_and(|c| c.material == MaterialId::Air)
        {
            // Reached a void — remaining drive vents next cadence.
            return;
        }
        gx = nx;
        gy = ny;
        drive = drive.saturating_sub(moved / 2).max(moved / 4);
    }
}

/// Push pore water one step along the path of least resistance.
///
/// Candidates are scored by permeability + room, with a mild upward bias
/// (pressure wants out). Venting into Air drops dissolved load as a warm
/// spring deposit. Returns how much moved.
fn reverse_push_pore_water(world: &mut World, temp: &mut Temperature, gx: i32, gy: i32, drive: u8) -> u8 {
    reverse_push_pore_water_to(world, temp, gx, gy, drive).0
}

fn reverse_push_pore_water_to(world: &mut World, temp: &mut Temperature, gx: i32, gy: i32, drive: u8) -> (u8, Option<(i32, i32)>) {
    if drive == 0 {
        return (0, None);
    }
    let Some(src) = world.get_cell(gx, gy) else {
        return (0, None);
    };
    if src.material == MaterialId::Air || src.sat.0 == 0 {
        return (0, None);
    }
    let want = drive.min(src.sat.0).max(1);
    let mut best: Option<(i32, i32, i32, Cell)> = None; // score, tx, ty, dst
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
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
        let perm = if dst.material == MaterialId::Air {
            // Open void = free escape path (warm vent seat).
            255
        } else {
            let p = permeability_cell(dst, &world.hydro);
            if p == 0 {
                continue;
            }
            p
        };
        // Least resistance: high perm + room, mild upward preference.
        let score = (perm as i32) * 8 + (room as i32) * 2 + dy.max(0) * 40;
        if best.is_none_or(|(s, _, _, _)| score > s) {
            best = Some((score, tx, ty, dst));
        }
    }
    let Some((_, tx, ty, dst)) = best else {
        return (0, None);
    };
    let cap = water_capacity_cell(dst, &world.hydro);
    let room = cap.saturating_sub(dst.sat.0);
    let moved = want.min(room);
    if moved == 0 {
        return (0, None);
    }
    let before = src.sat.0;
    let mut s = world.get_cell(gx, gy).unwrap();
    let mut d = dst;
    s.sat = Sat(s.sat.0 - moved);
    d.sat = Sat(d.sat.0 + moved);
    world.set_cell(gx, gy, s);
    world.set_cell(tx, ty, d);
    carry_with_water(world, (gx, gy), (tx, ty), moved, before);
    temp.advect_with_mass(gx, gy, tx, ty, moved);
    // Self-amplifying steam conduit: pressurized throughput widens rock along
    // the reverse-seep path (mint_void=false — high-aperture rock, not Air pipes).
    if d.material != MaterialId::Air && crate::cell::is_competent_rock(d.material) {
        let thr = moved.max(12);
        let _ = widen_aperture(world, tx, ty, thr, 3.2, 0x5EEF_u64, false);
    }
    if crate::cell::is_competent_rock(s.material) {
        let thr = moved.max(8);
        let _ = widen_aperture(world, gx, gy, thr, 1.6, 0x5EE0_u64, false);
    }
    if d.material == MaterialId::Air {
        // Warm vent: depressurising spring drops dissolved minerals.
        let warmth = steam_pressure_norm(world, gx, gy)
            .max(drive as f32 / 255.0)
            .clamp(0.35, 1.0);
        precipitate_artesian_warm(world, tx, ty, warmth);
        // Second pass — vent mounds were crawling under the old step cap.
        precipitate_artesian_warm(world, tx, ty, warmth);
    } else if dissolved_at(world, tx, ty) > 0 && moved >= 8 {
        // Supersaturated pressurized hops shed a little load into the conduit wall.
        let warmth = (drive as f32 / 255.0).clamp(0.15, 0.7);
        precipitate_artesian_warm(world, tx, ty, warmth);
    }
    (moved, Some((tx, ty)))
}

fn escape_pressurized(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    max_cells: usize,
) {
    if world.steam.is_empty() {
        return;
    }
    let min_p = cfg.escape_pressure_min.clamp(0.02, 0.95);
    let mut keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    keys.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
    let mut escapes = 0u8;
    let max_esc = cfg.max_escapes_per_tick.max(1);
    for (gx, gy) in keys {
        if escapes >= max_esc {
            break;
        }
        let steam = steam_at(world, gx, gy);
        if steam < 8 {
            continue;
        }
        let press = steam_pressure_norm(world, gx, gy).max(steam as f32 / 255.0);
        if press < min_p {
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

        // Soft lids burst only when they open to free atmosphere — buried
        // sand lenses under competent rock must not become rising air pipes.
        if is_grain(above.material) || is_flow_erodible(above.material) {
            if soft_lid_opens_to_atmosphere(world, gx, gy + 1)
                && burst_grain_tube(world, gx, gy, gx, gy + 1, max_cells)
            {
                escapes = escapes.saturating_add(1);
                let t = temp.at_cell(gx, gy + 1);
                if t < cfg.boil_point_c {
                    let warmth = ((cfg.boil_point_c - t) / 60.0).clamp(0.0, 1.0);
                    precipitate_artesian_warm(world, gx, gy + 1, warmth);
                }
                continue;
            }
        }

        if reverse_escape_through_rock(world, temp, gx, gy, steam, press) {
            escapes = escapes.saturating_add(1);
            continue;
        }

        if crate::cell::is_competent_rock(above.material) {
            let throughput = (90.0 + press * 100.0) as u8;
            let scale = 2.2 + press * 3.5;
            // Pore conduit only — do not mint Air roofs as sand/void pipes.
            let _ = widen_aperture(
                world,
                gx,
                gy + 1,
                throughput,
                scale,
                0x57EA_u64,
                false,
            );
            let pore_grew = world
                .get_cell(gx, gy + 1)
                .is_some_and(|c| c.pore > above.pore);
            if pore_grew {
                let drive = ((steam as f32) * (0.2 + press * 0.5))
                    .round()
                    .clamp(4.0, 96.0) as u8;
                reverse_push_pore_water(world, temp, gx, gy + 1, drive);
                escapes = escapes.saturating_add(1);
            }
        }
    }
}

fn reverse_escape_through_rock(
    world: &mut World,
    temp: &mut Temperature,
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
        reverse_push_pore_water(world, temp, gx, gy + 1, drive);
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
        reverse_push_pore_water(world, temp, gx, gy + 1, took.saturating_mul(2));
        return true;
    }
    roof.sat.0 > 0
}

fn soft_lid_opens_to_atmosphere(world: &World, lx: i32, ly: i32) -> bool {
    // Soft column must reach free air (no steam) without crossing competent rock.
    let mut y = ly;
    for _ in 0..20 {
        y += 1;
        let Some(c) = world.get_cell(lx, y) else {
            return true;
        };
        if c.material == MaterialId::Bedrock {
            return false;
        }
        if crate::cell::is_competent_rock(c.material) {
            return false;
        }
        if c.material == MaterialId::Air {
            if steam_at(world, lx, y) == 0 {
                return true;
            }
            continue;
        }
        if is_grain(c.material) || is_flow_erodible(c.material) {
            continue;
        }
        return false;
    }
    false
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
    // Take vapour first; debris must land *outside* the pressure chamber.
    let moved = take_steam(world, from_x, from_y, steam_at(world, from_x, from_y).min(200));
    if !bank_burst_solid(world, from_x, from_y, tx, ty, cell) {
        if moved > 0 {
            add_steam(world, from_x, from_y, moved);
        }
        return false;
    }
    // Dissolved load stays with the water that remains in the opened tube.
    let mut air = Cell::air();
    air.sat = Sat(cell.sat.0);
    world.set_cell(tx, ty, air);
    let placed = try_place_steam(world, tx, ty, moved, max_cells);
    if placed < moved {
        add_steam(world, from_x, from_y, moved - placed);
    }
    true
}

/// Keep burst solids on a ledger: suspend clay, relocate bedload grains, or
/// dissolve carbonate debris into the conduit water. Never delete rock.
/// Never dump debris into the sealed pressure chamber — that looked like a
/// sand pillar collapsing into the void.
fn bank_burst_solid(
    world: &mut World,
    from_x: i32,
    from_y: i32,
    tx: i32,
    ty: i32,
    was: Cell,
) -> bool {
    if is_suspendable(was.material) {
        add_suspended(world, tx, ty, SEDIMENT_PER_CELL);
        return true;
    }
    let ox = (tx - from_x).signum();
    let oy = (ty - from_y).signum().max(1); // prefer ejecta above the lid
    let deltas = [
        (0, 1),
        (0, 2),
        (0, 3),
        (0, 4),
        (-1, 2),
        (1, 2),
        (-1, 3),
        (1, 3),
        (ox, oy),
        (-1, 1),
        (1, 1),
        (-2, 2),
        (2, 2),
        (ox, 0),
        (-1, 0),
        (1, 0),
        (0, oy.saturating_mul(2)),
    ];
    for (dx, dy) in deltas {
        if dx == 0 && dy == 0 {
            continue;
        }
        let nx = world.wrap_x(tx + dx);
        let ny = ty + dy;
        if nx == from_x && ny == from_y {
            continue;
        }
        if try_place_burst_debris(world, nx, ny, was.material, tx, ty, from_x, from_y) {
            return true;
        }
    }
    if is_soluble_rock(was.material) {
        emit_from_dissolved_rock(world, tx, ty, was);
        return true;
    }
    // LooseLimestone is outside solubility props but is still carbonate.
    if was.material == MaterialId::LooseLimestone {
        add_dissolved(world, tx, ty, MINERAL_PER_CELL);
        return true;
    }
    // No exterior bedload seat: fluidize grains into suspended load rather
    // than dumping them into the chamber (collapsing sand pillar).
    if is_grain(was.material) || is_flow_erodible(was.material) {
        add_suspended(world, tx, ty, SEDIMENT_PER_CELL);
        return true;
    }
    false
}

fn try_place_burst_debris(
    world: &mut World,
    nx: i32,
    ny: i32,
    material: MaterialId,
    tube_x: i32,
    tube_y: i32,
    chamber_x: i32,
    chamber_y: i32,
) -> bool {
    let nx = world.wrap_x(nx);
    if nx == chamber_x && ny == chamber_y {
        return false;
    }
    let Some(dst) = world.get_cell(nx, ny) else {
        return false;
    };
    if dst.material != MaterialId::Air {
        return false;
    }
    if steam_at(world, nx, ny) > 0 {
        return false;
    }
    let mut grain = Cell::solid(material);
    let cap = water_capacity_cell(grain, &world.hydro);
    let soak = dst.sat.0.min(cap);
    grain.sat = Sat(soak);
    let leftover = dst.sat.0.saturating_sub(soak);
    world.set_cell(nx, ny, grain);
    if leftover > 0 {
        if let Some(tube) = world.get_cell(tube_x, tube_y) {
            if tube.material == MaterialId::Air {
                let room = u8::MAX.saturating_sub(tube.sat.0);
                let put = leftover.min(room);
                let mut next = tube;
                next.sat = Sat(tube.sat.0.saturating_add(put));
                world.set_cell(tube_x, tube_y, next);
            }
        }
    }
    true
}

/// Total steam units (same mass units as sat).
pub fn steam_total(world: &World) -> i64 {
    world.steam.values().map(|&v| v as i64).sum()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::humidity::Humidity;
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
    fn open_surface_water_is_not_minted_as_steam() {
        // Unroofed hot free water is not steam's job — accelerated evap owns it.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(4, 1, Cell::water());
        for y in 2..20 {
            w.set_cell(4, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 110.0);
        w.tick = STEAM_EVERY;
        let before = sat_totals(&w).cell_total;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert_eq!(steam_total(&w), 0, "open sky must not mint sparse steam");
        assert_eq!(
            sat_totals(&w).cell_total,
            before,
            "steam pass must leave open water for accelerated evap"
        );
    }

    #[test]
    fn roofed_cave_water_boils_into_steam() {
        // Sealed / roofed void keeps underground humidity on World.steam.
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
        w.set_cell(4, 2, Cell::water());
        assert!(void_is_confined(&w, 4, 2));
        let mut hot = temp_fill(&w, 110.0);
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let h_before = h.total_mass();
        w.tick = STEAM_EVERY;
        let before = sat_totals(&w).cell_total;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert!(steam_total(&w) > 0, "roofed hot water must boil to steam");
        assert_eq!(h.total_mass(), h_before, "sealed boil must not touch Humidity");
        assert_eq!(sat_totals(&w).cell_total, before, "steam boil must be mass-flat");
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
        let mut hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
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
        let mut hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
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
        let mut hot = temp_fill(&w, 140.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
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
        let mut hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 3) + steam_at(&w, 5, 3) + steam_at(&w, 5, 2) > 0,
            "confined steam must occupy the void, not only the water seat"
        );
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 6) == 0 && steam_at(&w, 4, 7) == 0,
            "steam must not pass an intact stone roof in two cadences"
        );
        assert!(steam_pressure_norm(&w, 4, 3) > 0.0);
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
        let mut cool = temp_fill(&w, 20.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut cool, &SteamConfig::default());
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
        wake_confined_head(&mut w, None);
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
        let mut hot = temp_fill(&w, 110.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
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
    fn open_wet_boil_site_stays_out_of_steam() {
        // Open boil seats stay out of World.steam (weather path).
        let mut w = World::new(13);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(4, 1, Cell::water());
        for y in 2..16 {
            w.set_cell(4, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 150.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert_eq!(steam_total(&w), 0);
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert_eq!(steam_total(&w), 0);
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
        let mut hot = temp_fill(&w, 140.0);
        w.tick = STEAM_EVERY;
        let before = sat_totals(&w).cell_total;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert!(steam_total(&w) > 0);
        assert!(
            w.get_cell(4, 1).unwrap().sat.0 < crate::cell::water_capacity(MaterialId::Sand)
        );
        assert_eq!(sat_totals(&w).cell_total, before);
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
        // Soft lid must open to free atmosphere — buried sand under stone is
        // no longer treated as a burst pipe.
        w.set_cell(4, 4, Cell::solid(MaterialId::Sand));
        w.set_cell(5, 4, Cell::solid(MaterialId::Sand));
        for y in 5..12 {
            w.set_cell(4, y, Cell::air());
            w.set_cell(5, y, Cell::air());
        }
        add_steam(&mut w, 4, 3, 220);
        add_steam(&mut w, 5, 3, 220);
        let mut hot = temp_fill(&w, 130.0);
        let cfg = SteamConfig {
            escape_pressure_min: 0.02,
            ..SteamConfig::default()
        };
        for i in 1..12 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
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
        let mut cool = temp_fill(&w, 10.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut cool, &SteamConfig::default());
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
        let mut hot = temp_fill(&w, 130.0);
        for i in 1..20 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &SteamConfig::default());
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
        let before = sat_totals(&w).cell_total;
        let mut hot = temp_fill(&w, 160.0);
        let cfg = SteamConfig {
            enable_escape: false,
            phase_expansion_drive: 16,
            reverse_seep_hops: 4,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..10 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let sat_above1 = w.get_cell(4, 2).unwrap().sat.0 as i32
            + w.get_cell(4, 3).unwrap().sat.0 as i32;
        let host = w.get_cell(4, 1).unwrap();
        let reverse_or_crack = sat_above1 > sat_above0
            || host.pore > pore0
            || host.material == MaterialId::Air
            || host.sat.0 < cap
            || steam_total(&w) > 0;
        assert!(
            reverse_or_crack,
            "phase expansion must reverse-seep, crack, or vent steam \
             (above {sat_above0}→{sat_above1}, pore {pore0}→{}, sat={}, steam={})",
            host.pore,
            host.sat.0,
            steam_total(&w)
        );
        assert_eq!(sat_totals(&w).cell_total, before, "phase boil must stay mass-flat");
    }

    #[test]
    fn phase_expansion_drive_exceeds_boiled_mass() {
        // Expansion factor is force: reverse push budget ≫ sat converted.
        assert!(PHASE_EXPANSION_DRIVE >= 8);
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
    fn phase_heat_drive_scale_spikes_above_boil() {
        assert_eq!(phase_heat_drive_scale(100.0, 100.0), 1.0);
        assert_eq!(phase_heat_drive_scale(80.0, 100.0), 1.0);
        let warm = phase_heat_drive_scale(140.0, 100.0);
        let hot = phase_heat_drive_scale(180.0, 100.0);
        assert!(warm > 1.5, "over-boil must raise drive (warm={warm})");
        assert!(hot >= warm, "hotter must not weaken drive");
        assert!(hot <= 3.0 + 1e-3, "must stay well below Clausius 1700× (hot={hot})");
        let cool_drive = expansion_drive_units(10, 16, 100.0, 100.0);
        let hot_drive = expansion_drive_units(10, 16, 180.0, 100.0);
        assert!(
            hot_drive > cool_drive,
            "superheat must enlarge reverse-seep budget ({cool_drive} → {hot_drive})"
        );
    }

    #[test]
    fn reverse_seep_prefers_higher_permeability_path() {
        // Hot wet limestone with two upward exits: sand (high perm) vs stone
        // (low/zero). Expansion must shove water into the sand path.
        let mut w = World::new(41);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 0..7 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        let lim_cap = crate::cell::water_capacity(MaterialId::Limestone).max(1);
        let mut boil = Cell::solid(MaterialId::Limestone);
        boil.sat = Sat(lim_cap);
        boil.pore = 120;
        w.set_cell(4, 1, boil);
        // Left diagonal: impermeable stone (no room for least-resistance).
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
        // Right diagonal: permeable wet sand — the easy path.
        let sand_cap = crate::cell::water_capacity(MaterialId::Sand).max(1);
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(sand_cap / 8);
        sand.pore = 200;
        w.set_cell(5, 2, sand);
        // Seal the left/up with stone so sand is clearly preferred.
        w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 3, Cell::solid(MaterialId::Stone));
        let sand0 = w.get_cell(5, 2).unwrap().sat.0;
        let mut hot = temp_fill(&w, 170.0);
        let cfg = SteamConfig {
            enable_escape: false,
            phase_expansion_drive: 24,
            reverse_seep_hops: 4,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..12 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let sand1 = w.get_cell(5, 2).unwrap().sat.0;
        assert!(
            sand1 > sand0 || steam_total(&w) > 0 || w.get_cell(4, 1).unwrap().sat.0 < lim_cap,
            "least-resistance reverse seep must wet the sand path or vent              (sand {sand0}→{sand1}, host sat={}, steam={})",
            w.get_cell(4, 1).unwrap().sat.0,
            steam_total(&w)
        );
    }

    #[test]
    fn try_place_steam_reports_clip_not_request() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 2, Cell::air());
        add_steam(&mut w, 2, 2, 250);
        let accepted = try_place_steam(&mut w, 2, 2, 20, 64);
        assert_eq!(accepted, 5, "must report only what fit under 255");
        assert_eq!(steam_at(&w, 2, 2), 255);
        assert_eq!(steam_total(&w), 255);
    }

    #[test]
    fn flood_equalize_preserves_mass_above_255() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Roofed pocket of void air.
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
        add_steam(&mut w, 3, 2, 200);
        add_steam(&mut w, 4, 2, 200);
        add_steam(&mut w, 5, 2, 200);
        let before = steam_total(&w);
        assert!(before > 255);
        let mut hot = temp_fill(&w, 120.0);
        w.tick = STEAM_EVERY;
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            ..SteamConfig::default()
        };
        apply_steam(&mut w, &mut hot, &cfg);
        assert_eq!(
            steam_total(&w),
            before,
            "flood equalize must not destroy multi-cell vapour totals"
        );
    }

    #[test]
    fn reverse_push_carries_heat_into_cold_neighbour() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Wet limestone column with room above.
        w.set_cell(4, 1, {
            let mut c = Cell::solid(MaterialId::Limestone);
            c.sat = Sat(180);
            c
        });
        w.set_cell(4, 2, {
            let mut c = Cell::solid(MaterialId::Limestone);
            c.sat = Sat(20);
            c
        });
        let mut temp = temp_fill(&w, 20.0);
        // Stamp source tile hot, destination cold.
        let (shx, shy) = temp.tile_of(4, 1);
        let (dhx, dhy) = temp.tile_of(4, 2);
        temp.set_tile_c(shx, shy, 140.0);
        if (dhx, dhy) != (shx, shy) {
            temp.set_tile_c(dhx, dhy, 20.0);
        }
        let before_dest = temp.at_cell(4, 2);
        let moved = reverse_push_pore_water(&mut w, &mut temp, 4, 1, 40);
        assert!(moved > 0, "expected reverse push to move pore water");
        let after_dest = temp.at_cell(4, 2);
        if (dhx, dhy) != (shx, shy) {
            assert!(
                after_dest > before_dest + 0.5,
                "hot reverse seep must warm the destination tile ({before_dest} → {after_dest})"
            );
        }
    }

    #[test]
    fn cavity_pressure_warms_cold_wall_below_boil() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..7 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 2, Cell::air());
        w.set_cell(4, 3, {
            let mut c = Cell::solid(MaterialId::Limestone);
            c.sat = Sat(120);
            c
        });
        add_steam(&mut w, 4, 2, 180);
        let mut temp = temp_fill(&w, 40.0); // well below boil
        let before = temp.at_cell(4, 3);
        let cfg = SteamConfig::default();
        transmit_cavity_pressure(&mut w, &mut temp, &cfg);
        let after = temp.at_cell(4, 3);
        assert!(
            after > before + 0.25,
            "dense cavity humidity should deposit heat into adjacent wet rock ({before} → {after})"
        );
    }

    fn count_mat(world: &World, mat: MaterialId) -> usize {
        let mut n = 0usize;
        for chunk in world.chunks.values() {
            for cell in &chunk.cells {
                if cell.material == mat {
                    n += 1;
                }
            }
        }
        n
    }

    #[test]
    fn burst_sand_lid_relocates_grain_mass() {
        let mut w = World::new(31);
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
        // Soft lid open to sky — not buried under competent rock.
        w.set_cell(4, 4, Cell::solid(MaterialId::Sand));
        w.set_cell(5, 4, Cell::solid(MaterialId::Sand));
        for y in 5..12 {
            w.set_cell(4, y, Cell::air());
            w.set_cell(5, y, Cell::air());
        }
        add_steam(&mut w, 4, 3, 220);
        add_steam(&mut w, 5, 3, 220);
        let sand_before = count_mat(&w, MaterialId::Sand);
        let sediment_before = crate::audit::sediment_total(&w);
        let mut hot = temp_fill(&w, 130.0);
        let cfg = SteamConfig {
            escape_pressure_min: 0.02,
            ..SteamConfig::default()
        };
        for i in 1..12 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let sand_after = count_mat(&w, MaterialId::Sand);
        let sediment_after = crate::audit::sediment_total(&w);
        assert_eq!(
            sand_after as i64
                + (sediment_after - sediment_before) / i64::from(SEDIMENT_PER_CELL),
            sand_before as i64,
            "sand lid mass must relocate as grain or suspended ({sand_before} → sand {sand_after}, sed Δ {})",
            sediment_after - sediment_before
        );
        assert!(
            w.get_cell(4, 4).unwrap().material == MaterialId::Air
                || w.get_cell(5, 4).unwrap().material == MaterialId::Air
                || steam_at(&w, 4, 4) > 0
                || steam_at(&w, 5, 4) > 0,
            "atmosphere soft lid should still open"
        );
        // Debris must not refill the sealed chamber as a collapsing pillar.
        assert_ne!(
            w.get_cell(4, 3).unwrap().material,
            MaterialId::Sand,
            "burst must not dump sand into the pressure chamber"
        );
        assert_ne!(
            w.get_cell(5, 3).unwrap().material,
            MaterialId::Sand,
            "burst must not dump sand into the pressure chamber"
        );
    }

    #[test]
    fn buried_sand_lens_does_not_mint_air_pipe() {
        let mut w = World::new(43);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..7 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Limestone));
            }
        }
        w.set_cell(4, 2, Cell::air());
        w.set_cell(4, 3, Cell::air());
        // Buried soft lens under competent limestone — must not burst into a pipe.
        w.set_cell(4, 4, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 5, Cell::solid(MaterialId::Limestone));
        w.set_cell(4, 6, Cell::solid(MaterialId::Limestone));
        add_steam(&mut w, 4, 3, 240);
        let mut hot = temp_fill(&w, 130.0);
        let cfg = SteamConfig {
            escape_pressure_min: 0.02,
            ..SteamConfig::default()
        };
        for i in 1..16 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        assert_eq!(
            w.get_cell(4, 4).unwrap().material,
            MaterialId::Sand,
            "buried sand under limestone must not burst into an air pipe"
        );
        assert_ne!(
            w.get_cell(4, 5).unwrap().material,
            MaterialId::Air,
            "competent limestone roof must not mint an air column from steam assault"
        );
        assert_ne!(
            w.get_cell(4, 6).unwrap().material,
            MaterialId::Air,
            "competent limestone column must stay rock, not a void pipe"
        );
    }

    #[test]
    fn burst_clay_lid_banks_suspended_sediment() {
        let mut w = World::new(37);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..6 {
            for y in 1..5 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 2, Cell::air());
        w.set_cell(4, 3, Cell::air());
        w.set_cell(4, 4, Cell::solid(MaterialId::Clay));
        w.set_cell(4, 5, Cell::solid(MaterialId::Stone));
        add_steam(&mut w, 4, 3, 240);
        let before = crate::audit::sediment_total(&w);
        assert!(burst_grain_tube(&mut w, 4, 3, 4, 4, 64));
        assert_eq!(w.get_cell(4, 4).unwrap().material, MaterialId::Air);
        assert_eq!(
            crate::audit::sediment_total(&w),
            before,
            "clay burst must bank suspended load"
        );
    }

    #[test]
    fn assault_widen_limestone_keeps_mineral_total() {
        let mut w = World::new(41);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..6 {
            for y in 1..5 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(3, 2, Cell::air());
        w.set_cell(4, 2, Cell::air());
        let mut lime = Cell::solid(MaterialId::Limestone);
        lime.sat = Sat(80);
        lime.pore = 200;
        w.set_cell(3, 3, lime);
        w.set_cell(3, 4, Cell::solid(MaterialId::Stone));
        add_steam(&mut w, 3, 2, 200);
        add_steam(&mut w, 4, 2, 200);
        let before = crate::audit::mineral_total(&w);
        let mut hot = temp_fill(&w, 120.0);
        for i in 1..10 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &SteamConfig::default());
        }
        assert_eq!(
            crate::audit::mineral_total(&w),
            before,
            "steam assault widen must conserve mineral (solid + dissolved)"
        );
    }

    #[test]
    fn reverse_push_widens_conduit_along_path() {
        // Pressurized reverse seep must carve high-aperture rock conduits —
        // not only move water.
        let mut w = World::new(61);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for y in 1..6 {
            let mut rock = Cell::solid(MaterialId::Limestone);
            rock.sat = Sat(200);
            rock.pore = 40;
            w.set_cell(4, y, rock);
        }
        w.set_cell(4, 6, Cell::air());
        add_dissolved(&mut w, 4, 1, 120);
        let pore0 = w.get_cell(4, 2).unwrap().pore
            + w.get_cell(4, 3).unwrap().pore
            + w.get_cell(4, 4).unwrap().pore;
        let mut hot = temp_fill(&w, 40.0);
        for _ in 0..24 {
            let _ = reverse_push_pore_water(&mut w, &mut hot, 4, 1, 200);
            // Walk the chain a few hops like reverse_seep_chain would.
            let _ = reverse_push_pore_water(&mut w, &mut hot, 4, 2, 180);
            let _ = reverse_push_pore_water(&mut w, &mut hot, 4, 3, 160);
        }
        let pore1 = w.get_cell(4, 2).unwrap().pore
            + w.get_cell(4, 3).unwrap().pore
            + w.get_cell(4, 4).unwrap().pore;
        assert!(
            pore1 > pore0,
            "reverse-push conduit must widen rock along the path ({pore0} → {pore1})"
        );
        assert_ne!(
            w.get_cell(4, 2).unwrap().material,
            MaterialId::Air,
            "path carving must stay high-aperture rock, not mint Air pipes"
        );
    }

    #[test]
    fn reverse_seep_hops_default_reaches_farther() {
        assert!(
            REVERSE_SEEP_HOPS >= 8,
            "default reverse-seep range must be long enough to feed conduits"
        );
        assert!(
            PHASE_EXPANSION_DRIVE >= 40,
            "default phase drive must shove more than a token pulse"
        );
        let mut w = World::new(67);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Embed the conduit in competent rock so reverse-push cannot vent into
        // the default surrounding Air.
        for x in 3..8 {
            for y in 0..13 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for y in 1..12 {
            let mut rock = Cell::solid(MaterialId::Limestone);
            rock.sat = Sat(if y == 1 { 220 } else { 0 });
            rock.pore = 200;
            w.set_cell(5, y, rock);
        }
        // Roofed — no free Air seat beside the column.
        w.set_cell(5, 12, Cell::solid(MaterialId::Stone));
        let mut hot = temp_fill(&w, 50.0);
        reverse_seep_chain(&mut w, &mut hot, 5, 1, 255, REVERSE_SEEP_HOPS);
        for _ in 0..5 {
            if let Some(mut c) = w.get_cell(5, 1) {
                c.sat = Sat(220);
                w.set_cell(5, 1, c);
            }
            reverse_seep_chain(&mut w, &mut hot, 5, 1, 255, REVERSE_SEEP_HOPS);
        }
        // Mid-column cells may drain as the pulse climbs; success is water or
        // aperture work well above the source.
        let far_wet = (7..=11).any(|y| w.get_cell(5, y).unwrap().sat.0 > 0);
        let far_carved = (3..=8).any(|y| w.get_cell(5, y).unwrap().pore > 200);
        assert!(
            far_wet || far_carved,
            "default hop budget must carry water/carve several cells up-column"
        );
    }

    #[test]
    fn reverse_push_vent_deposits_dissolved_load() {
        let mut w = World::new(71);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        let mut rock = Cell::solid(MaterialId::Limestone);
        rock.sat = Sat(200);
        rock.pore = 60;
        w.set_cell(3, 2, rock);
        w.set_cell(3, 3, Cell::air());
        add_dissolved(&mut w, 3, 2, 220);
        let load0 = dissolved_at(&w, 3, 2) + dissolved_at(&w, 3, 3);
        let pore0 = w.get_cell(3, 2).unwrap().pore;
        let mut hot = temp_fill(&w, 30.0);
        for _ in 0..20 {
            let _ = reverse_push_pore_water(&mut w, &mut hot, 3, 2, 200);
            if let Some(mut c) = w.get_cell(3, 2) {
                if c.material != MaterialId::Air {
                    c.sat = Sat(c.sat.0.max(150));
                    w.set_cell(3, 2, c);
                }
            }
        }
        let load1 = dissolved_at(&w, 3, 2) + dissolved_at(&w, 3, 3);
        let floor_closed = w
            .get_cell(3, 2)
            .is_some_and(|c| c.material != MaterialId::Air && c.pore < pore0);
        let vent_solid = w
            .get_cell(3, 3)
            .is_some_and(|c| c.material != MaterialId::Air);
        assert!(
            load1 < load0 || floor_closed || vent_solid,
            "artesian vent must drop dissolved mineral (load {load0}→{load1}, pore {pore0})"
        );
    }



    #[test]
    fn hot_saturated_rock_reports_pore_flash_pressure() {
        let mut w = World::new(91);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut rock = Cell::solid(MaterialId::LooseRock);
        rock.pore = 64;
        let cap = water_capacity_cell(rock, &w.hydro).max(1);
        rock.sat = Sat(cap);
        w.set_cell(5, 3, rock);
        let (p, kind) = cell_pressure_norm(&w, 5, 3, 107.0);
        assert!(
            p > 0.05,
            "hot fully-wet rock must show pore pressure (got {p})"
        );
        assert_eq!(kind, CellPressureKind::PoreFlash);
        let (cold, cold_kind) = cell_pressure_norm(&w, 5, 3, 20.0);
        assert!(
            cold < 0.02 && cold_kind == CellPressureKind::None,
            "cold wet rock without cavity vapour stays quiet ({cold}, {cold_kind:?})"
        );
    }

    #[test]
    fn cavity_steam_pressure_reads_as_cavity_kind() {
        let mut w = World::new(93);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..7 {
            for y in 1..5 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(4, 2, Cell::air());
        w.set_cell(4, 3, Cell::air());
        add_steam(&mut w, 4, 2, 180);
        let (p, kind) = cell_pressure_norm(&w, 4, 2, 120.0);
        assert!(p > 0.2, "dense cavity humidity must read pressure (got {p})");
        assert_eq!(kind, CellPressureKind::Cavity);
    }

}
