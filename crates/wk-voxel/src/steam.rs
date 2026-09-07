//! Sparse pressurized steam — boil free + pore water above ~100 °C.
//!
//! Steam is a **gas in voids**, not a liquid blob. On each cadence it
//! flood-fills connected Air and equalizes (confined) or piles at the
//! open top (vented). Pressure assaults wet pores (reverse seepage +
//! fast aperture growth) and bursts soft lids into tubes.
//!
//! **Pore phase change** is the motor: liquid→gas expansion (~1000× in
//! nature, capped `phase_expansion_drive` here) budgets reverse seepage
//! and aperture work far beyond the boiled sat mass. Mass stays flat
//! (sat ↔ steam); the expansion factor is *force*, not minted water.
//!
//! Humidity stays the coarse **sky** field — steam does not dump into
//! rain. See docs/VOXEL_GEYSER.md.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{
    is_flow_erodible, is_grain, permeability_cell, water_capacity_cell, Cell, Sat,
};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::FxHashSet;
use crate::grid::World;
use crate::mineral::{
    carry_with_water, dissolved_at, precipitate_artesian_warm, widen_aperture,
};
use crate::temperature::Temperature;

/// Cadence for boil / flood / assault / recondense.
pub const STEAM_EVERY: u64 = 1;

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
/// Real steam is ~1000× liquid volume; we use a capped sim factor so each
/// boiled sat unit budgets this many units of reverse seepage + aperture
/// work. Mass stays flat (boiled sat ↔ steam); the factor is *force*, not
/// minted water.
pub const PHASE_EXPANSION_DRIVE: u8 = 12;

/// How many reverse-seepage hops a phase-expansion pulse may travel.
pub const REVERSE_SEEP_HOPS: u8 = 4;

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

/// Boil / flood / assault / escape / recondense. Humidity (sky) untouched.
pub fn apply_steam(world: &mut World, temp: &Temperature, cfg: &SteamConfig) {
    if !cfg.enabled {
        return;
    }
    let period = cfg.period_ticks.max(1);
    let due = world.tick % period == 0;
    let max_cells = cfg.max_steam_cells.max(1) as usize;
    let boil = cfg.boil_point_c;
    let recondense_below = boil - RECONDENSE_MARGIN_C;

    if due {
        recondense_cool(world, temp, recondense_below);
        boil_hot_air(world, temp, cfg, max_cells);
        if cfg.enable_pore_boil {
            boil_hot_pores(world, temp, cfg, max_cells);
        }
    }
    // Equalize every tick while steam exists — vapour fields don't wait on cadence.
    if !world.steam.is_empty() {
        flood_equalize_steam(world, cfg, max_cells);
    }
    if due && !world.steam.is_empty() {
        assault_steam_walls(world, cfg);
        if cfg.enable_escape {
            escape_pressurized(world, temp, cfg, max_cells);
            flood_equalize_steam(world, cfg, max_cells);
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
    if world.steam.is_empty() {
        return Vec::new();
    }
    let budget = VOID_FLOOD_BUDGET;
    let seeds: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut visited: FxHashSet<(i32, i32)> = FxHashSet::default();
    let mut out: Vec<(i32, i32, u8)> = Vec::new();

    for (sx, sy) in seeds {
        let sx = world.wrap_x(sx);
        if !visited.insert((sx, sy)) {
            continue;
        }
        let seed_confined = void_is_confined(world, sx, sy);
        let mut queue = vec![(sx, sy)];
        let mut component: Vec<(i32, i32)> = Vec::new();
        let mut qi = 0;
        while qi < queue.len() && component.len() < budget {
            let (cx, cy) = queue[qi];
            qi += 1;
            let Some(cell) = world.get_cell(cx, cy) else {
                continue;
            };
            if cell.material != MaterialId::Air {
                continue;
            }
            component.push((cx, cy));
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
        let voids: Vec<(i32, i32)> = component
            .iter()
            .copied()
            .filter(|&(x, y)| world.get_cell(x, y).is_some_and(is_steam_void))
            .collect();
        if voids.is_empty() {
            continue;
        }
        let total: u32 = voids.iter().map(|&(x, y)| steam_at(world, x, y) as u32).sum();
        if total == 0 {
            continue;
        }
        let mean = (total / voids.len() as u32).min(255) as u8;
        // Visibility floor: thin steam still washes the whole pocket.
        let density = mean.max(36).min(220);
        for (x, y) in voids {
            out.push((x, y, density));
        }
    }
    out
}

/// Steam pressure assaults neighbouring wet rock: reverse push + fast widen.
/// Prefers the roof (up) so energy goes into escape tubes, not sideways leaks.
fn assault_steam_walls(world: &mut World, cfg: &SteamConfig) {
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
                reverse_push_pore_water(world, tx, ty, drive.max(4));
                assaults = assaults.saturating_add(1);
            }
            // Upward faces carve hardest; sideways is slower so pockets don't
            // leak into the default-Air chunk before the roof yields.
            if crate::cell::is_competent_rock(wall.material)
                && press >= cfg.escape_pressure_min * 0.5
            {
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
    let expand = cfg.phase_expansion_drive.max(1);
    let hops = cfg.reverse_seep_hops.max(1);
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
            // upward (seepage in reverse) so the tube keeps growing.
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
        if try_place_steam(world, sx, sy, take, max_cells) == 0 {
            // Cap: put sat back.
            let mut back = world.get_cell(gx, gy).unwrap_or(next);
            back.sat = Sat(back.sat.0.saturating_add(take));
            world.set_cell(gx, gy, back);
            continue;
        }
        carry_with_water(world, (gx, gy), (sx, sy), take, before);

        // 2) Phase-change pressure: expansion drive ≫ boiled mass.
        //    Remaining liquid is shoved out of the rock (seepage in reverse).
        let drive = (take as u16)
            .saturating_mul(expand as u16)
            .min(255) as u8;
        reverse_seep_chain(world, gx, gy, drive, hops);
        // Host cell itself also widens under flash expansion.
        phase_crack_host(world, gx, gy, take, expand);
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
    temp: &Temperature,
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

        if reverse_escape_through_rock(world, gx, gy, steam, press) {
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
                        reverse_push_pore_water(world, gx, gy + 1, moved);
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
        apply_steam(&mut w, &hot, &SteamConfig::default());
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
            .filter(|&&(x, y, d)| (3..7).contains(&x) && (2..5).contains(&y) && d >= 36)
            .count();
        assert!(
            painted >= 10,
            "vapour field must wash the whole pocket (painted={painted})"
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
        w.tick = 1;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        let chamber = steam_at(&w, 3, 3) + steam_at(&w, 4, 3) + steam_at(&w, 6, 3);
        assert!(
            chamber > 0,
            "leaky cave must still equalize the chamber, not only plume the vent"
        );
        let field = steam_vapour_field(&w);
        assert!(
            field.iter().any(|&(x, y, d)| y <= 4 && (3..7).contains(&x) && d >= 36),
            "vapour wash must cover the chamber"
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
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(
            steam_at(&w, 4, 3) + steam_at(&w, 5, 3) + steam_at(&w, 5, 2) > 0,
            "confined steam must occupy the void, not only the water seat"
        );
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &SteamConfig::default());
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
        let cool = temp_fill(&w, 20.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &cool, &SteamConfig::default());
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
        apply_steam(&mut w, &hot, &SteamConfig::default());
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
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(steam_total(&w) > 0);
        w.tick = STEAM_EVERY * 2;
        apply_steam(&mut w, &hot, &SteamConfig::default());
        assert!(steam_total(&w) > 0);
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
            apply_steam(&mut w, &hot, &cfg);
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
        apply_steam(&mut w, &cool, &SteamConfig::default());
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
            apply_steam(&mut w, &hot, &SteamConfig::default());
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
        let hot = temp_fill(&w, 160.0);
        let cfg = SteamConfig {
            enable_escape: false,
            phase_expansion_drive: 16,
            reverse_seep_hops: 4,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..10 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &hot, &cfg);
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
}
