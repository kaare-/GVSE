//! Pressurized **cavity humidity** — sparse sealed / semi-sealed void store (P3).
//!
//! **Store:** `World.steam` is the wire name for capped sparse **cavity humidity
//! under pressure** in roofed / side-closed voids. It is **not** sky humidity.
//! Cool sealed ambient still uses [`crate::cave_humidity`]; this map is the
//! over-capacity / flash / pressure mode of closed-cavity moisture — not a
//! separate "steam gas" species.
//!
//! **Weather vs boiler:** leftover volume, not “can I see the sky.”
//! Open hot rock / a wide U is evap → sky [`Humidity`]. A fat pocket with a
//! pinprick throat (or a sealed cave) is a boiler: `mass × expand` while hot
//! is overpressure; cool collapses back to mass. A choked mouth may leak
//! **mass** into H; volume never does. Pure gas carries no solute.
//!
//! **Motor:** boil roofed free + pore water above ~100 °C → sparse cavity
//! humidity. Liquid→gas expansion budgets reverse pore seepage + aperture
//! work. Flowing hot water **carries heat** into cooler rock so channels warm
//! over time. Existing cavity humidity keeps pushing pore water by density
//! even through rock below boil. Confined pockets flood-equalize; overpressure
//! assaults wet rock as **high-aperture conduits** (no instant Air pipes) and
//! only bursts soft lids that open to free atmosphere. Cool → liquid + sinter.
//! Buried grain lenses under competent rock are not treated as sand pillars; hot saturated grains sinter to competent rock and reverse-seep widens conduits without minting Air pipes.
//!
//! See docs/VOXEL_GEYSER.md + VOXEL_THERMAL.md.

use std::cell::RefCell;

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
    precipitate_vent_mouth, pressure_sinter_cell, widen_aperture, VENT_PIPE_LUMEN,
};
use crate::sediment::{add_suspended, is_suspendable, SEDIMENT_PER_CELL};
use crate::humidity::Humidity;
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
/// Real steam is ~1000–1700× liquid volume; we use a capped sim factor as
/// *force* (reverse seepage + aperture work), not minted water. The old
/// `boiled × expand` product saturated a `u8` drive at expand≈6 for any
/// real pore flash, so Tab's 64× ceiling did nothing on hot cores. Expand
/// is now the primary force knob (default 96); heat scales it; boiled mass
/// only mildly amplifies. Still deliberately far below Clausius 1700×.
pub const PHASE_EXPANSION_DRIVE: u8 = 96;

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
    // Expand is the force knob (sim volume-ratio stand-in). Heat scales it.
    // Boiled mass only swings ±50% so a token flash does not shove like a
    // full pore — without `boiled * expand`, which hit the u8 ceiling at
    // tiny expand values and made Tab 64× identical to 16× on hot rock.
    let boil_amp = 0.5 + 0.5 * ((boiled as f32) / 40.0).clamp(0.0, 1.0);
    let raw = (expand.max(1) as f32) * heat * boil_amp;
    raw.round().clamp(1.0, 255.0) as u8
}

/// How many reverse-seepage hops a phase-expansion pulse may travel.
///
/// Hot sealed cores need range after sinter; 10 left pipes stubby. Extra
/// hops scale further from expand in [`boil_hot_pores`].
pub const REVERSE_SEEP_HOPS: u8 = 16;

/// Throat lining stops once aperture falls to this — walls mineralize,
/// the lumen stays a pipe. Below it, only the vent mouth deposits.
const PIPE_LUMEN_FLOOR: u8 = VENT_PIPE_LUMEN;

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
    let placed = *slot - before;
    if placed > 0 {
        world.steam_rev = world.steam_rev.wrapping_add(1);
    }
    placed
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
    if took > 0 {
        world.steam_rev = world.steam_rev.wrapping_add(1);
    }
    took
}

/// Thread-local memo for sky / roof probes + steam haze.
///
/// `air_void_open_to_sky` used to BFS up to 192 Air cells **per call**.
/// Haze wash + evap + reverse-seep + flood equalize all hammered it, so a
/// paused `[sim skip]` frame with a 300-cell boiler still cost hundreds of
/// milliseconds. One BFS now paints every visited seat.
#[derive(Default)]
struct SkyProbeCache {
    world_id: u64,
    topo: u64,
    confined: FxHashMap<(i32, i32), bool>,
    open: FxHashMap<(i32, i32), bool>,
    /// Roofed Air: fat/choked vessel (true) vs leaky weather pocket (false).
    boiler: FxHashMap<(i32, i32), bool>,
}

struct SteamHazeMemo {
    world_id: u64,
    tick: u64,
    topo: u64,
    fp: (u64, u64, u64),
    warm_fp: u64,
    samples: Vec<SteamHazeSample>,
}

#[derive(Default)]
struct PressMemo {
    world_id: u64,
    tick: u64,
    steam_rev: u64,
    topo: u64,
    /// Inclusive padded influence box; `None` if steam is empty.
    bbox: Option<(i32, i32, i32, i32)>,
    map: FxHashMap<(i32, i32), f32>,
}

thread_local! {
    static SKY_PROBE: RefCell<SkyProbeCache> = RefCell::new(SkyProbeCache::default());
    static STEAM_HAZE_MEMO: RefCell<Option<SteamHazeMemo>> = const { RefCell::new(None) };
    static PRESS_MEMO: RefCell<PressMemo> = RefCell::new(PressMemo::default());
}

fn bind_sky_probe(world: &World) {
    SKY_PROBE.with(|slot| {
        let mut c = slot.borrow_mut();
        let id = world.chunk_cache_id.get();
        let topo = world.sky_topo_gen;
        if c.world_id != id || c.topo != topo {
            c.world_id = id;
            c.topo = topo;
            c.confined.clear();
            c.open.clear();
            c.boiler.clear();
        }
    });
}

pub(crate) fn sparse_amt_fp(map: &FxHashMap<(i32, i32), u8>) -> (u64, u64, u64) {
    let mut n = 0u64;
    let mut k = 0u64;
    let mut v = 0u64;
    for (&(x, y), &amt) in map {
        n += 1;
        k = k
            .wrapping_add(x as u64)
            .wrapping_add((y as u64).wrapping_mul(0x9E37_79B9));
        v = v.wrapping_add(amt as u64);
    }
    (n, k, v)
}

/// True when this Air void sits under a solid roof (cave / conduit).
pub fn void_is_confined(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    bind_sky_probe(world);
    if let Some(hit) = SKY_PROBE.with(|c| c.borrow().confined.get(&(gx, gy)).copied()) {
        return hit;
    }
    let mut confined = false;
    for dy in 1..=ROOF_PROBE {
        match world.get_cell(gx, gy + dy) {
            None => {
                confined = false;
                break;
            }
            Some(c) if c.material == MaterialId::Air => continue,
            Some(_) => {
                confined = true;
                break;
            }
        }
    }
    SKY_PROBE.with(|c| {
        c.borrow_mut().confined.insert((gx, gy), confined);
    });
    confined
}

/// Sky-connected Air (open shaft / cave vent) — weather Humidity may sit here.
///
/// Cheap path: upward probe ([`void_is_confined`]). If roofed, a short Air BFS
/// looks for any neighbour that opens to sky (side entrance / skylight offset).
/// Sealed pockets return false so they stay off the rain lottery.
///
/// The first miss for a pocket BFS-paints every visited Air seat so later
/// calls in the same topology are O(1).
pub fn air_void_open_to_sky(world: &World, gx: i32, gy: i32) -> bool {
    // Flank lakes / long flooded side channels need more than a tiny BFS —
    // 96 steps missed many surface-connected vents the player can carve.
    const BFS_BUDGET: usize = 192;
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return true;
    };
    if cell.material != MaterialId::Air {
        return false;
    }
    bind_sky_probe(world);
    if let Some(hit) = SKY_PROBE.with(|c| c.borrow().open.get(&(gx, gy)).copied()) {
        return hit;
    }
    if !void_is_confined(world, gx, gy) {
        SKY_PROBE.with(|c| {
            c.borrow_mut().open.insert((gx, gy), true);
        });
        return true;
    }
    let mut q: Vec<(i32, i32)> = vec![(gx, gy)];
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert((gx, gy));
    let mut steps = 0usize;
    let mut opened = false;
    while let Some((x, y)) = q.pop() {
        steps += 1;
        if steps > BFS_BUDGET {
            opened = false;
            break;
        }
        for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
            let nx = world.wrap_x(x + dx);
            let ny = y + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            match world.get_cell(nx, ny) {
                None => {
                    opened = true;
                    break;
                }
                Some(c) if c.material == MaterialId::Air => {
                    if !void_is_confined(world, nx, ny) {
                        opened = true;
                        break;
                    }
                    q.push((nx, ny));
                }
                Some(_) => {}
            }
        }
        if opened {
            break;
        }
    }
    // Budget miss: only cache the seed so we don't mark a nearby vent sealed.
    SKY_PROBE.with(|c| {
        let mut c = c.borrow_mut();
        if opened || steps <= BFS_BUDGET {
            for &(x, y) in &seen {
                if world
                    .get_cell(x, y)
                    .is_some_and(|cell| cell.material == MaterialId::Air)
                {
                    c.open.insert((x, y), opened);
                }
            }
        } else {
            c.open.insert((gx, gy), false);
        }
    });
    opened
}

/// Air that counts as gas volume (not a standing-water lake cell).
#[inline]
fn is_steam_void(cell: Cell) -> bool {
    cell.material == MaterialId::Air && cell.sat.0 <= STEAM_VOID_SAT_MAX
}

/// Sealed from weather: roofed **and** no lateral sky path.
///
/// [`void_is_confined`] is a 48-cell upward probe. A lake under a cliff
/// slope is "roofed" even when it opens to the horizon; flash / cavity
/// heat must not treat that as a boiler.
///
/// Prefer [`vessel_is_boiler`] for flash-vs-evap. A fat cave with a
/// 1-wide sky chimney is open to weather probes but still a boiler:
/// leftover volume cannot leave as fast as it is made.
pub fn steam_is_pressure_confined(world: &World, gx: i32, gy: i32) -> bool {
    void_is_confined(world, gx, gy) && !air_void_open_to_sky(world, gx, gy)
}

/// Weather film vs choked / sealed pressure vessel.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VesselKind {
    /// Vapour leaves as fast as it is made — evap / sky Humidity.
    Weather,
    /// Surplus volume packs the pocket — marble-tube / gas chimney.
    Boiler,
}

/// Volume of `mass` while hot (`mass × expand`) or cold (`mass`).
///
/// Mass is never multiplied. 14 liquid becomes 1400 volume units above
/// boil and collapses back to 14 when cool. Do not write this into `sat`.
#[inline]
pub fn vapor_volume_units(mass: u32, temp_c: f32, boil_c: f32, expand: u8) -> u32 {
    if mass == 0 || !temp_c.is_finite() {
        return 0;
    }
    if temp_c >= boil_c && boil_c.is_finite() {
        mass.saturating_mul(expand.max(1) as u32)
    } else {
        mass
    }
}

/// Leftover volume that does not fit equilibrium seats.
#[inline]
pub fn overpressure_units(volume: u32, seat: u32) -> u32 {
    volume.saturating_sub(seat)
}

/// Mass (not volume) a 1-wide throat may emit this cadence.
///
/// Huge surplus + tiny hole keeps the vessel packed. Never larger than
/// the vapour **mass** that produced the surplus.
#[inline]
pub fn choke_leak_mass(surplus_volume: u32, expand: u8, throat: u8) -> u8 {
    if surplus_volume == 0 {
        return 0;
    }
    let expand = expand.max(1) as u32;
    let excess_mass = surplus_volume / expand;
    let pipe = (throat.max(1) as u32).saturating_mul(6);
    excess_mass.min(pipe).clamp(1, 24) as u8
}

/// Fat roofed pocket vs pinprick sky gate (or sealed).
///
/// Open ground and a wide U are [`VesselKind::Weather`]. A cathedral with
/// a mousehole — including the 100-wide cave + 1×800 chimney — is a
/// [`VesselKind::Boiler`] even though a bird can fly out the top.
pub fn classify_air_vessel(world: &World, gx: i32, gy: i32) -> VesselKind {
    if vessel_is_boiler(world, gx, gy) {
        VesselKind::Boiler
    } else {
        VesselKind::Weather
    }
}

/// True when flashing this Air cell would pack a vessel, not feed weather.
pub fn vessel_is_boiler(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if cell.material != MaterialId::Air {
        return false;
    }
    bind_sky_probe(world);
    if let Some(hit) = SKY_PROBE.with(|c| c.borrow().boiler.get(&(gx, gy)).copied()) {
        return hit;
    }
    let kind = probe_vessel_kind(world, gx, gy);
    kind == VesselKind::Boiler
}

fn probe_vessel_kind(world: &World, gx: i32, gy: i32) -> VesselKind {
    const BFS_BUDGET: usize = 192;
    // Unroofed seed is the atmosphere or a vent mouth — weather.
    // The fat pocket below a chimney is classified from its own cells.
    if !void_is_confined(world, gx, gy) {
        SKY_PROBE.with(|c| {
            c.borrow_mut().boiler.insert((gx, gy), false);
        });
        return VesselKind::Weather;
    }
    let mut q: Vec<(i32, i32)> = vec![(gx, gy)];
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert((gx, gy));
    let mut roofed = 0u32;
    let mut sky_gates = 0u32;
    let mut steps = 0usize;
    while let Some((x, y)) = q.pop() {
        steps += 1;
        if steps > BFS_BUDGET {
            break;
        }
        if !void_is_confined(world, x, y) {
            sky_gates = sky_gates.saturating_add(1);
            continue;
        }
        roofed = roofed.saturating_add(1);
        for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
            let nx = world.wrap_x(x + dx);
            let ny = y + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            match world.get_cell(nx, ny) {
                None => {
                    sky_gates = sky_gates.saturating_add(1);
                }
                Some(c) if c.material == MaterialId::Air => {
                    if !void_is_confined(world, nx, ny) {
                        sky_gates = sky_gates.saturating_add(1);
                    } else {
                        q.push((nx, ny));
                    }
                }
                Some(_) => {}
            }
        }
    }
    let boiler = if roofed == 0 {
        false
    } else if sky_gates == 0 {
        true
    } else if roofed >= 8 && sky_gates <= 2 {
        true
    } else {
        roofed >= sky_gates.saturating_mul(8)
    };
    let mut roofed_seats: Vec<(i32, i32)> = Vec::new();
    for &(x, y) in &seen {
        if world
            .get_cell(x, y)
            .is_some_and(|cell| cell.material == MaterialId::Air)
            && void_is_confined(world, x, y)
        {
            roofed_seats.push((x, y));
        }
    }
    SKY_PROBE.with(|c| {
        let mut c = c.borrow_mut();
        for &(x, y) in &roofed_seats {
            c.boiler.insert((x, y), boiler);
        }
        c.boiler.insert((gx, gy), boiler);
    });
    if boiler {
        VesselKind::Boiler
    } else {
        VesselKind::Weather
    }
}

/// Reverse-seep / steam discharge sink: free-sky Air **or** standing water.
///
/// Underwater hydrothermal vents dump into the lake even when the pool
/// sits under a rock lid (`!air_void_open_to_sky`). Dry sealed voids stay
/// boilers — they are not vents.
fn is_steam_discharge_vent(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if cell.material != MaterialId::Air {
        return false;
    }
    if air_void_open_to_sky(world, gx, gy) {
        return true;
    }
    cell.sat.0 > STEAM_VOID_SAT_MAX || crate::rules::is_standing_water(world, gx, gy)
}

/// Move steam off a cell that is no longer a void (editor brick, collapse).
///
/// Only touches [`World::steam`] — safe to call from [`World::set_cell`].
pub fn evict_steam_seat(world: &mut World, gx: i32, gy: i32) {
    let amt = take_steam(world, gx, gy, u8::MAX);
    if amt == 0 {
        return;
    }
    const DELTAS: [(i32, i32); 8] = [
        (0, 1),
        (0, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (1, 1),
        (-1, -1),
        (1, -1),
    ];
    for (dx, dy) in DELTAS {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        if world.get_cell(tx, ty).is_some_and(is_steam_void) {
            let _ = add_steam(world, tx, ty, amt);
            return;
        }
    }
    // No neighbour void: park on any remaining steam key so mass stays.
    if let Some((&(x, y), slot)) = world.steam.iter_mut().next() {
        let before = *slot;
        *slot = before.saturating_add(amt);
        if *slot != before {
            world.steam_rev = world.steam_rev.wrapping_add(1);
        }
        let _ = (x, y);
        return;
    }
    // Last resort: keep a ghost key one cell up so apply_steam can park
    // liquid. Overlay ignores non-Air steam.
    let _ = add_steam(world, gx, gy + 1, amt);
}

/// Relocate steam sitting on rock / missing cells (mass-flat).
fn scrub_invalid_steam_seats(world: &mut World, max_cells: usize) {
    if world.steam.is_empty() {
        return;
    }
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    for (gx, gy) in keys {
        let Some(cell) = world.get_cell(gx, gy) else {
            evict_steam_seat(world, gx, gy);
            continue;
        };
        if cell.material != MaterialId::Air {
            let amt = take_steam(world, gx, gy, u8::MAX);
            if amt == 0 {
                continue;
            }
            let placed = inject_steam_near(world, gx, gy, amt, max_cells);
            let left = amt.saturating_sub(placed);
            if left > 0 {
                let parked = crate::displace::park_orphan_water(world, gx, gy, left as u32);
                if parked > 0 {
                    let _ = add_steam(world, gx, gy, parked.min(255) as u8);
                }
            }
            continue;
        }
        // Free sky is sky Humidity / evap, not cavity steam. Leftover
        // markers here paint puffy clouds on top of the H field.
        if !void_is_confined(world, gx, gy) {
            let amt = take_steam(world, gx, gy, u8::MAX);
            if amt > 0 {
                let parked = crate::displace::park_orphan_water(world, gx, gy, amt as u32);
                if parked > 0 {
                    // Could not seat liquid — only restore into a roofed void.
                    let _ = inject_steam_near(world, gx, gy, parked.min(255) as u8, max_cells);
                }
            }
        }
    }
}

fn bind_press_memo(world: &World) {
    PRESS_MEMO.with(|slot| {
        let mut c = slot.borrow_mut();
        let id = world.chunk_cache_id.get();
        if c.world_id != id
            || c.tick != world.tick
            || c.steam_rev != world.steam_rev
            || c.topo != world.sky_topo_gen
        {
            c.world_id = id;
            c.tick = world.tick;
            c.steam_rev = world.steam_rev;
            c.topo = world.sky_topo_gen;
            rebuild_press_field(world, &mut c);
        }
    });
}

fn press_bbox_include(bbox: &mut Option<(i32, i32, i32, i32)>, x: i32, y: i32) {
    match bbox {
        None => *bbox = Some((x, x, y, y)),
        Some((x0, x1, y0, y1)) => {
            *x0 = (*x0).min(x);
            *x1 = (*x1).max(x);
            *y0 = (*y0).min(y);
            *y1 = (*y1).max(y);
        }
    }
}

/// Equalized cavity fill on the connected void, plus wet rock that faces it.
///
/// The old 3×16 downward box painted a candle that punched through granite
/// and ignored cave walls. Pressure here is the pocket mean (same idea as
/// flood-equalize) so a large chamber reads as one field in the cave's shape.
fn rebuild_press_field(world: &World, memo: &mut PressMemo) {
    memo.map.clear();
    memo.bbox = None;
    if world.steam.is_empty() {
        return;
    }
    let budget = VOID_FLOOD_BUDGET;
    let mut visited: FxHashSet<(i32, i32)> = FxHashSet::default();
    let seeds: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    for (sx, sy) in seeds {
        let sx = world.wrap_x(sx);
        if !visited.insert((sx, sy)) {
            continue;
        }
        let Some(seed) = world.get_cell(sx, sy) else {
            continue;
        };
        if seed.material != MaterialId::Air || !void_is_confined(world, sx, sy) {
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
            if cell.material != MaterialId::Air {
                continue;
            }
            voids.push((cx, cy));
            // Vent lip / well next to the pocket counts, but do not walk
            // free sky or the overlay grows a candle up the shaft.
            let expand = void_is_confined(world, cx, cy) || steam_at(world, cx, cy) > 0;
            if !expand {
                continue;
            }
            for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                let nx = world.wrap_x(cx + dx);
                let ny = cy + dy;
                if !visited.insert((nx, ny)) {
                    continue;
                }
                match world.get_cell(nx, ny) {
                    Some(n) if n.material == MaterialId::Air => {
                        queue.push((nx, ny));
                    }
                    _ => {}
                }
            }
        }
        if voids.is_empty() {
            continue;
        }
        let total: u32 = voids
            .iter()
            .map(|&(x, y)| steam_at(world, x, y) as u32)
            .sum();
        if total == 0 {
            continue;
        }
        let peak = voids
            .iter()
            .map(|&(x, y)| steam_at(world, x, y) as u32)
            .max()
            .unwrap_or(0) as f32
            / 255.0;
        let mean = (total as f32 / (voids.len() as f32 * 255.0)).clamp(0.0, 1.0);
        // Volume-mean, but a sealed room stays readable: the old candle
        // hid the cave and made one column look packed.
        let fill = mean.max(peak * 0.35).clamp(0.0, 1.0);
        for &(x, y) in &voids {
            let e = memo.map.entry((x, y)).or_insert(0.0);
            *e = (*e).max(fill);
            press_bbox_include(&mut memo.bbox, x, y);
            // Wet permeable walls / floor inherit the pocket — not dry granite.
            for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0)] {
                let nx = world.wrap_x(x + dx);
                let ny = y + dy;
                let Some(n) = world.get_cell(nx, ny) else {
                    continue;
                };
                if n.material == MaterialId::Air {
                    continue;
                }
                if permeability_cell(n, &world.hydro) == 0 || n.sat.0 == 0 {
                    continue;
                }
                let cap = water_capacity_cell(n, &world.hydro).max(1);
                let wet = (n.sat.0 as f32 / cap as f32).clamp(0.0, 1.0);
                if wet < 0.05 {
                    continue;
                }
                let e = memo.map.entry((nx, ny)).or_insert(0.0);
                *e = (*e).max(fill);
                press_bbox_include(&mut memo.bbox, nx, ny);
            }
        }
    }
}

/// 0..=1 pressure — equalized fill of the connected confined pocket.
pub fn steam_pressure_norm(world: &World, gx: i32, gy: i32) -> f32 {
    if world.steam.is_empty() {
        return 0.0;
    }
    let gx = world.wrap_x(gx);
    bind_press_memo(world);
    let outside = world.wrap_width.is_none()
        && PRESS_MEMO.with(|c| {
            c.borrow()
                .bbox
                .is_some_and(|(x0, x1, y0, y1)| gx < x0 || gx > x1 || gy < y0 || gy > y1)
        });
    if outside {
        return 0.0;
    }
    if let Some(hit) = PRESS_MEMO.with(|c| c.borrow().map.get(&(gx, gy)).copied()) {
        return hit;
    }
    // Orphan / unroofed marker: local density only — never a downward smear.
    steam_at(world, gx, gy) as f32 / 255.0
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
    /// Leftover pore volume in rock — no cave required.
    PoreFlash,
    /// Negligible.
    None,
}

/// How much expanded volume does not fit the equilibrium seat (0..=1).
///
/// `1 - seat/volume` so 7/14 rock at expand 100 is nearly as packed as
/// 14/14. Cool collapse (`volume == mass`) is zero leftover.
#[inline]
pub fn leftover_pack_norm(volume: u32, seat: u32) -> f32 {
    if seat == 0 || volume <= seat {
        return 0.0;
    }
    (1.0 - seat as f32 / volume as f32).clamp(0.0, 1.0)
}

/// Unified 0..=1 pressure readout for inspector + overlay.
///
/// Leftover volume (`mass × expand − seat`) while T ≥ boil. Saturated
/// stone does **not** need a cave. Weather air (open ground, wide U)
/// stays unlit even when hot.
pub fn cell_pressure_norm(world: &World, gx: i32, gy: i32, temp_c: f32) -> (f32, CellPressureKind) {
    cell_pressure_norm_with_boil(world, gx, gy, temp_c, BOIL_POINT_C, PHASE_EXPANSION_DRIVE)
}

/// [`cell_pressure_norm`] against the live Tab boil point and expand.
pub fn cell_pressure_norm_with_boil(
    world: &World,
    gx: i32,
    gy: i32,
    temp_c: f32,
    boil_c: f32,
    expand: u8,
) -> (f32, CellPressureKind) {
    let steam = steam_at(world, gx, gy);
    let Some(cell) = world.get_cell(gx, gy) else {
        return (0.0, CellPressureKind::None);
    };

    let boil = if boil_c.is_finite() {
        boil_c
    } else {
        BOIL_POINT_C
    };
    let expand = expand.max(1);

    if cell.material == MaterialId::Air {
        // Open landscape / wide U: weather, not a pressure overlay.
        if !vessel_is_boiler(world, gx, gy) {
            return (0.0, CellPressureKind::None);
        }
        let seat = water_capacity_cell(cell, &world.hydro).max(1) as u32;
        let vol = vapor_volume_units(steam as u32, temp_c, boil, expand);
        let pack = leftover_pack_norm(vol, seat);
        return if pack > 0.02 {
            (pack.clamp(0.0, 1.0), CellPressureKind::Cavity)
        } else {
            (0.0, CellPressureKind::None)
        };
    }

    // Rock / pores: the cell is the vessel. No Air seat required.
    let cap = water_capacity_cell(cell, &world.hydro);
    let vol = vapor_volume_units(cell.sat.0 as u32, temp_c, boil, expand);
    let mut pack = leftover_pack_norm(vol, cap as u32);
    if steam > 0 {
        let gas_vol = vapor_volume_units(steam as u32, temp_c, boil, expand);
        pack = pack.max(leftover_pack_norm(gas_vol, cap.max(1) as u32));
    }
    if pack < 0.02 {
        return (0.0, CellPressureKind::None);
    }
    (pack, CellPressureKind::PoreFlash)
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
    // Cavity humidity stays under a roof. Open-sky seats become the
    // little steam puffs on top of the humidity wash.
    if !void_is_confined(world, gx, gy) {
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

/// Recondense a slice of pocket vapour into surface-connected vent cells.
///
/// When flood equalize finds the pocket touches free sky (including via a
/// flooded side channel), convert some steam → liquid on the open seats so
/// pressure bleeds toward the vent. Liquid in the vent lake — never a dump
/// into sky Humidity.
fn bleed_steam_into_open_vent(world: &mut World, seats: &[(i32, i32)]) {
    let mut open: Vec<(i32, i32)> = seats
        .iter()
        .copied()
        .filter(|&(x, y)| {
            world
                .get_cell(x, y)
                .is_some_and(|c| is_steam_discharge_vent(world, x, y, c))
        })
        .collect();
    if open.is_empty() {
        return;
    }
    // Prefer higher seats (closer to daylight / the carved outlet).
    open.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));

    // Pull from densest confined seats first, park into open vent seats.
    let mut sources: Vec<(i32, i32, u8)> = seats
        .iter()
        .copied()
        .filter_map(|(x, y)| {
            let s = steam_at(world, x, y);
            if s == 0 || air_void_open_to_sky(world, x, y) {
                None
            } else {
                Some((x, y, s))
            }
        })
        .collect();
    if sources.is_empty() {
        // Steam already sitting on open seats — recondense in place.
        sources = open
            .iter()
            .copied()
            .filter_map(|(x, y)| {
                let s = steam_at(world, x, y);
                (s > 0).then_some((x, y, s))
            })
            .collect();
    }
    sources.sort_by(|a, b| b.2.cmp(&a.2));

    let mut budget: u32 = sources
        .iter()
        .map(|(_, _, s)| *s as u32)
        .sum::<u32>()
        .saturating_div(8)
        .clamp(1, 48);
    let mut oi = 0usize;
    for (sx, sy, _) in sources {
        if budget == 0 {
            break;
        }
        let have = steam_at(world, sx, sy);
        if have == 0 {
            continue;
        }
        let take = (have as u32).min(budget).min(24) as u8;
        let got = take_steam(world, sx, sy, take);
        if got == 0 {
            continue;
        }
        let (vx, vy) = open[oi % open.len()];
        oi += 1;
        let left = crate::displace::park_orphan_water(world, vx, vy, got as u32);
        if left > 0 {
            // Could not seat liquid — restore vapour so we stay mass-flat.
            let _ = add_steam(world, sx, sy, left.min(255) as u8);
            budget = budget.saturating_sub((got as u32).saturating_sub(left));
        } else {
            budget = budget.saturating_sub(got as u32);
        }
    }
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
        if c.material == MaterialId::Air && c.sat.0 <= STEAM_VOID_SAT_MAX {
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
    apply_steam_with_weather(world, temp, cfg, None);
}

/// [`apply_steam`] plus a sky-Humidity mouth for choked hot vents.
///
/// Only **mass** that already reached free air may enter `humidity`.
/// Volume never writes the weather store. Sealed surplus stays in the vessel.
pub fn apply_steam_with_weather(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    humidity: Option<&mut Humidity>,
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

    scrub_invalid_steam_seats(world, max_cells);
    recondense_cool(world, temp, recondense_below);
    boil_hot_air(world, temp, cfg, max_cells);
    if cfg.enable_pore_boil {
        boil_hot_pores(world, temp, cfg, max_cells);
    }
    // Flood + assault only on cadence (every-tick flood crushed FPS).
    if !world.steam.is_empty() {
        flood_equalize_steam(world, cfg, max_cells);
        leak_choked_boiler_mouth(world, temp, cfg, humidity);
        // Density-driven push + heat deposit continue past the boil isotherm.
        transmit_cavity_pressure(world, temp, cfg);
        assault_steam_walls(world, temp, cfg);
        if cfg.enable_escape {
            let escaped = escape_pressurized(world, temp, cfg, max_cells);
            // Second flood is only needed when a burst / tube actually
            // relocated vapour. Pore-only widen leaves the field in place.
            if escaped > 0 && !world.steam.is_empty() {
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
            // Distilled collapse into the host pores — gas does not mint solute.
            let want = steam_at(world, gx, gy);
            let cap = water_capacity_cell(cell, &world.hydro);
            let room = cap.saturating_sub(cell.sat.0);
            let put = want.min(room);
            if put > 0 {
                let took = take_steam(world, gx, gy, put);
                let mut next = cell;
                next.sat = Sat(cell.sat.0.saturating_add(took));
                world.set_cell(gx, gy, next);
            }
            let left = steam_at(world, gx, gy);
            if left > 0 {
                if let Some(up) = world.get_cell(gx, gy + 1) {
                    if is_steam_void(up) {
                        let room_up = 255u8.saturating_sub(steam_at(world, gx, gy + 1));
                        let put_up = left.min(room_up);
                        if put_up > 0 {
                            let moved = take_steam(world, gx, gy, put_up);
                            let placed = add_steam(world, gx, gy + 1, moved);
                            if placed < moved {
                                let _ = crate::displace::park_orphan_water(
                                    world,
                                    gx,
                                    gy + 1,
                                    (moved - placed) as u32,
                                );
                            }
                        }
                    }
                }
            }
            let leftover = steam_at(world, gx, gy);
            if leftover > 0 {
                let unplaced = crate::displace::park_orphan_water(world, gx, gy, leftover as u32);
                let parked = (leftover as u32).saturating_sub(unplaced);
                if parked > 0 {
                    let _ = take_steam(world, gx, gy, parked.min(255) as u8);
                }
                // Unplaced stays as steam — never delete water.
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
/// field. A fully open shaft parks as liquid — it must not pack a plume
/// into free sky (leftover puffs on the humidity field).
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
                        // Confined pockets may cross standing-water Air to reach
                        // a surface-connected flooded vent. Open plumes stay on
                        // gas voids so they don't crawl through lakes.
                        if seed_confined
                            || is_steam_void(n)
                            || steam_at(world, nx, ny) > 0
                        {
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

        // Mid-map hillside / flank vents never hit `get_cell(..+1) == None`
        // (world top). Detect surface connectivity the same way rain does.
        if !open_to_sky {
            open_to_sky = component
                .iter()
                .any(|&(x, y)| air_void_open_to_sky(world, x, y));
        }

        let mut total: u32 = 0;
        for &(x, y) in &component {
            total += take_steam(world, x, y, steam_at(world, x, y)) as u32;
        }
        if total == 0 {
            continue;
        }

        // Prefer dry gas voids for seating, but wet Air that contributed steam
        // must remain eligible seats — excluding them (or truncating share to
        // u8) silently destroyed multi-cell vapour totals.
        let mut voids: Vec<(i32, i32)> = component
            .iter()
            .copied()
            .filter(|&(x, y)| world.get_cell(x, y).is_some_and(is_steam_void))
            .collect();
        let seats_all = component.clone();
        if voids.is_empty() {
            let left = spill_steam_across(world, &seats_all, total, max_cells);
            if left > 0 {
                // Cap / full seats: convert leftover vapour to liquid store.
                let seed = seats_all.first().copied().unwrap_or((sx, sy));
                let _ = crate::displace::park_orphan_water(world, seed.0, seed.1, left);
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
            // Seat across the roofed pocket only. Vent-column / free-sky
            // Air used to take a share and draw as puffs above the H field.
            let mut seats: Vec<(i32, i32)> = seats_all
                .iter()
                .copied()
                .filter(|&(x, y)| void_is_confined(world, x, y))
                .collect();
            if seats.is_empty() {
                let seed = seats_all.first().copied().unwrap_or((sx, sy));
                let _ = crate::displace::park_orphan_water(world, seed.0, seed.1, total);
                continue;
            }
            seats.sort_by(|a, b| {
                let va = world.get_cell(a.0, a.1).is_some_and(is_steam_void);
                let vb = world.get_cell(b.0, b.1).is_some_and(is_steam_void);
                let ca = void_is_confined(world, a.0, a.1);
                let cb = void_is_confined(world, b.0, b.1);
                vb.cmp(&va)
                    .then(cb.cmp(&ca))
                    .then(b.1.cmp(&a.1))
                    .then(a.0.cmp(&b.0))
            });
            let n = seats.len() as u32;
            let base = total / n;
            let mut rem = total % n;
            let mut left = 0u32;
            for &(x, y) in &seats {
                let mut need = base;
                if rem > 0 {
                    need += 1;
                    rem -= 1;
                }
                while need > 0 {
                    let chunk = need.min(255) as u8;
                    let placed = try_place_steam(world, x, y, chunk, max_cells);
                    if placed == 0 {
                        left += need;
                        break;
                    }
                    need -= placed as u32;
                }
            }
            if left > 0 {
                let left = spill_steam_across(world, &seats, left, max_cells);
                if left > 0 {
                    let seed = seats.first().copied().unwrap_or((sx, sy));
                    let _ = crate::displace::park_orphan_water(world, seed.0, seed.1, left);
                }
            }
            // Surface-connected flooded vents: recondense a slice of pocket
            // vapour into the open path so pressure bleeds toward daylight
            // (liquid in the vent lake — not a dump into sky Humidity).
            // Dry leaky chimneys still equalize as a field without this bleed.
            if open_to_sky
                && seats.iter().any(|&(x, y)| {
                    world.get_cell(x, y).is_some_and(|c| {
                        c.material == MaterialId::Air
                            && c.sat.0 > STEAM_VOID_SAT_MAX
                            && is_steam_discharge_vent(world, x, y, c)
                    })
                })
            {
                bleed_steam_into_open_vent(world, &seats);
            }
        } else {
            // Free open shaft: do not pack a buoyant steam plume into the
            // sky. That was the leftover puffy clouds sitting on the
            // humidity field. Park as liquid at the seed (mass-flat).
            let seed = seats_all.first().copied().unwrap_or((sx, sy));
            let _ = crate::displace::park_orphan_water(world, seed.0, seed.1, total);
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
    let fp = sparse_amt_fp(&world.steam);
    let warm_fp = match temp {
        None => 0,
        Some(t) => world
            .steam
            .keys()
            .next()
            .map(|&(x, y)| t.at_cell(x, y).to_bits() as u64)
            .unwrap_or(0)
            ^ 1,
    };
    if let Some(hit) = STEAM_HAZE_MEMO.with(|slot| {
        let m = slot.borrow();
        m.as_ref().and_then(|m| {
            if m.world_id == world.chunk_cache_id.get()
                && m.tick == world.tick
                && m.topo == world.sky_topo_gen
                && m.fp == fp
                && m.warm_fp == warm_fp
            {
                Some(m.samples.clone())
            } else {
                None
            }
        })
    }) {
        return hit;
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
        // Open-sky leftover must not seed a wash above the humidity field.
        if !void_is_confined(world, gx, gy) {
            continue;
        }
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
        if !void_is_confined(world, sx, sy) {
            continue;
        }
        let seed_confined = steam_is_pressure_confined(world, sx, sy);
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
        // Marker bins already recorded per-tile pressure. Re-walking a
        // 16-deep column for every void was an FPS cliff on big boilers.
        let press = (total / voids.len() as f32) / 255.0;
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
                // Humidity of the conduit: only roofed Air vapour, not rock /
                // lake plugs or free sky above the H field.
                if cell.material != MaterialId::Air || !is_steam_void(cell) {
                    continue;
                }
                if !void_is_confined(world, gx, gy) {
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
    STEAM_HAZE_MEMO.with(|slot| {
        *slot.borrow_mut() = Some(SteamHazeMemo {
            world_id: world.chunk_cache_id.get(),
            tick: world.tick,
            topo: world.sky_topo_gen,
            fp,
            warm_fp,
            samples: out.clone(),
        });
    });
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
/// Cavity humidity density keeps shoving pore water past the boil isotherm —
/// pressure does not die the moment rock is <100 °C.
///
/// Heat is **not** invented here. A previous `boil + press × 35` lamp
/// (135 °C at default boil) made a self-sustaining boiler on maps whose
/// overburden never reaches 100 °C. Existing heat may ride water
/// ([`Temperature::advect_with_mass`]); geothermal is the only source.
///
/// Below-boil boilers used to get only a **single** reverse-push hop, so a
/// sintered mountain lid never saw the multi-hop carve motor that pore-boil
/// enjoys. Dense cavity vapour now runs [`reverse_seep_chain`] with the Tab
/// hop budget so warm mineral water can climb and bleed pressure.
fn transmit_cavity_pressure(world: &mut World, temp: &mut Temperature, cfg: &SteamConfig) {
    if world.steam.is_empty() {
        return;
    }
    let mut keys: Vec<(i32, i32, u8)> = world
        .steam
        .iter()
        .filter_map(|(&(x, y), &d)| (d >= 10).then_some((x, y, d)))
        .collect();
    keys.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
    let mut work = 0u8;
    let max_work = cfg.max_escapes_per_tick.saturating_mul(2).max(12);
    for (gx, gy, dens) in keys {
        if work >= max_work {
            break;
        }
        // Laterally open lake / sky seats are weather, not a boiler lamp.
        if !steam_is_pressure_confined(world, gx, gy) {
            continue;
        }
        let press = steam_pressure_norm(world, gx, gy).max(dens as f32 / 255.0);
        let src_t = temp.at_cell(gx, gy);
        // Pressure buys reach past the boil isotherm (same hop family as phase boil).
        let hops = cfg
            .reverse_seep_hops
            .max(1)
            .saturating_add((press * 10.0).round() as u8)
            .min(32);
        for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0), (-1, 1), (1, 1)] {
            let tx = world.wrap_x(gx + dx);
            let ty = gy + dy;
            // Share heat the cavity already has — never mix toward an
            // invented boil+superheat target.
            if src_t > temp.at_cell(tx, ty) + 0.05 {
                let carry = ((dens as f32) * 0.12).round() as u8;
                if carry > 0 {
                    temp.advect_with_mass(gx, gy, tx, ty, carry);
                }
            }
            let Some(wall) = world.get_cell(tx, ty) else {
                continue;
            };
            if wall.material == MaterialId::Air || wall.material == MaterialId::Bedrock {
                continue;
            }
            let drive = ((dens as f32) * (0.2 + press) * 0.85).round() as u8;
            if drive < 4 {
                continue;
            }
            if wall.sat.0 > 0 && permeability_cell(wall, &world.hydro) > 0 {
                // Multi-hop climb — not a single shove that dies under a lid.
                reverse_seep_chain(world, temp, tx, ty, drive, hops);
                work = work.saturating_add(1);
            } else if crate::cell::is_competent_rock(wall.material) && press >= 0.12 {
                // Dry roof above a pressurized cavity: crack pore space so the
                // next wet pulse has somewhere to climb (never mint Air).
                let thr = drive.max(24);
                let scale = 2.2 + press * 2.5;
                if widen_aperture(world, tx, ty, thr, scale, 0xCA71_u64, false) {
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
    let mut keys: Vec<(i32, i32, u8)> = world
        .steam
        .iter()
        .filter_map(|(&(x, y), &d)| (d >= 12).then_some((x, y, d)))
        .collect();
    keys.sort_by(|a, b| b.2.cmp(&a.2).then(b.1.cmp(&a.1)).then(a.0.cmp(&b.0)));
    let mut assaults = 0u8;
    let max_a = cfg.max_escapes_per_tick.saturating_mul(2).max(8);
    for (gx, gy, steam) in keys {
        if assaults >= max_a {
            break;
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

fn chunk_tiles_any(
    temp: &Temperature,
    coord: ChunkCoord,
    pred: impl Fn(f32) -> bool,
) -> bool {
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
            if pred(temp.at_tile(hx, hy)) {
                return true;
            }
        }
    }
    false
}

/// True when any temperature tile covering this chunk is near/above `min_c`.
fn chunk_overlaps_hot(temp: &Temperature, coord: ChunkCoord, min_c: f32) -> bool {
    chunk_tiles_any(temp, coord, |t| t >= min_c)
}

/// True when any temperature tile covering this chunk is at/below `max_c`.
///
/// Freeze scans use this so a hot wet mountain does not walk 4096 cells
/// per chunk looking for pore ice that cannot form.
pub(crate) fn chunk_overlaps_cold(temp: &Temperature, coord: ChunkCoord, max_c: f32) -> bool {
    chunk_tiles_any(temp, coord, |t| t <= max_c)
}

/// Walk cells whose tile temperature satisfies `keep`.
///
/// `Temperature::at_cell` is tile-constant, so a 64×64 wet chunk still
/// paid 4096 hashmap reads after a chunk-level gate let it in. Tiles
/// that fail `keep` are skipped; kept tiles reuse one `at_tile`.
pub(crate) fn for_each_tile_cell_where(
    world: &World,
    temp: &Temperature,
    coord: ChunkCoord,
    keep: impl Fn(f32) -> bool,
    mut visit: impl FnMut(i32, i32, f32, crate::cell::Cell),
) {
    let Some(chunk) = world.chunks.get(&coord) else {
        return;
    };
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let tc = temp.tile_cols.max(1);
    let x0 = coord.cx * cw;
    let y0 = coord.cy * ch;
    let hx0 = x0.div_euclid(tc);
    let hy0 = y0.div_euclid(tc);
    let hx1 = (x0 + cw - 1).div_euclid(tc);
    let hy1 = (y0 + ch - 1).div_euclid(tc);
    for hy in hy0..=hy1 {
        for hx in hx0..=hx1 {
            let t_c = temp.at_tile(hx, hy);
            if !keep(t_c) {
                continue;
            }
            let tx0 = hx * tc;
            let ty0 = hy * tc;
            let lx0 = (tx0 - x0).max(0) as u32;
            let ly0 = (ty0 - y0).max(0) as u32;
            let lx1 = (tx0 + tc - x0).min(cw) as u32;
            let ly1 = (ty0 + tc - y0).min(ch) as u32;
            for ly in ly0..ly1 {
                for lx in lx0..lx1 {
                    let cell = chunk.get(lx as usize, ly as usize);
                    let gx = world.wrap_x(x0 + lx as i32);
                    let gy = y0 + ly as i32;
                    visit(gx, gy, t_c, cell);
                }
            }
        }
    }
}

/// Walk cells whose temperature tile is ≥ `min_c`.
fn for_each_hot_tile_cell(
    world: &World,
    temp: &Temperature,
    coord: ChunkCoord,
    min_c: f32,
    visit: impl FnMut(i32, i32, f32, crate::cell::Cell),
) {
    for_each_tile_cell_where(world, temp, coord, |t| t >= min_c, visit);
}

fn boil_hot_air(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    max_cells: usize,
) {
    let boil = cfg.boil_point_c;
    let prefer_confined = world.steam.len() + 32 >= max_cells;
    let mut jobs: Vec<(u16, i32, i32, u8)> = Vec::new();
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(coord, c)| {
            (c.has_wet_air || c.has_standing_air) && chunk_overlaps_hot(temp, **coord, boil)
        })
        .map(|(k, _)| *k)
        .collect();
    for coord in coords {
        for_each_hot_tile_cell(world, temp, coord, boil, |gx, gy, t_c, cell| {
            if cell.material != MaterialId::Air || cell.sat.0 == 0 {
                return;
            }
            // Weather films (open ground, wide U) stay on evap.
            // Fat/choked vessels flash even when a bird can see the sky.
            if !vessel_is_boiler(world, gx, gy) {
                return;
            }
            let _ = prefer_confined; // reserved if we later prioritize seats
            let heat = ((t_c - boil) / 50.0).clamp(0.0, 2.0);
            let cap = ((cfg.boil_max_per_cell as f32) * (1.0 + heat))
                .round()
                .clamp(1.0, 255.0) as u8;
            let boil_amt = cell.sat.0.min(cap);
            if boil_amt > 0 {
                jobs.push((pore_boil_priority(t_c, boil, boil_amt), gx, gy, boil_amt));
            }
        });
    }
    // Open seats already dropped; hotter superheat wins the steam-cell budget.
    jobs.sort_by(|a, b| b.0.cmp(&a.0).then(b.3.cmp(&a.3)));
    for (_, gx, gy, amt) in jobs {
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
        // Unroofed / weather seats are filtered at collect time.
        if !vessel_is_boiler(world, gx, gy) {
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
        // No heat lamp: flashing does not mix the roof toward max(T, boil).
    }
}

/// Sparse geothermal solute for hot pore pulses. Real hydrothermal fluids
/// carry dissolved load from depth; without a source term, silicate hot spots
/// never cement or sinter-deposit. Caps locally so audits stay bounded.
fn hydrothermal_solute_pulse(world: &mut World, gx: i32, gy: i32, drive: u8) {
    if drive < 8 {
        return;
    }
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    // Silicate grain hosts only — carbonate rock already carries ledger mineral;
    // injecting here would break assault conservation audits.
    if !is_grain(cell.material) || cell.sat.0 == 0 {
        return;
    }
    let cur = dissolved_at(world, gx, gy);
    if cur >= 96 {
        return;
    }
    let add = ((drive as u16) / 8).clamp(2, 12);
    add_dissolved(world, gx, gy, add.min(96 - cur));
}

/// Keep the same top-N the old collect-all + stable-sort would apply.
///
/// Pore boil walks every hot wet cell but only applies `max_work` (~16)
/// jobs. `push` ranks by amount (legacy tests). Pore/air boil use
/// [`push_scored`] so superheat beats raw wetness when Tab boil drops.
struct SteamAmtTopK {
    items: Vec<(u16, u32, i32, i32, u8)>,
    cap: usize,
    next_idx: u32,
    cutoff: u16,
}

impl SteamAmtTopK {
    fn new(cap: usize) -> Self {
        Self {
            items: Vec::with_capacity(cap.max(1)),
            cap: cap.max(1),
            next_idx: 0,
            cutoff: 0,
        }
    }

    #[cfg(test)]
    fn push(&mut self, gx: i32, gy: i32, amt: u8) {
        self.push_scored(gx, gy, amt, amt as u16);
    }

    fn push_scored(&mut self, gx: i32, gy: i32, amt: u8, score: u16) {
        if amt == 0 || score == 0 {
            return;
        }
        self.next_idx = self.next_idx.saturating_add(1);
        let idx = self.next_idx;
        if self.items.len() < self.cap {
            self.items.push((score, idx, gx, gy, amt));
            if self.items.len() == self.cap {
                self.cutoff = self.items.iter().map(|i| i.0).min().unwrap_or(0);
            }
            return;
        }
        if score <= self.cutoff {
            return;
        }
        let worst = self
            .items
            .iter()
            .enumerate()
            .min_by(|(_, a), (_, b)| a.0.cmp(&b.0).then(b.1.cmp(&a.1)))
            .map(|(i, _)| i);
        if let Some(i) = worst {
            self.items[i] = (score, idx, gx, gy, amt);
            self.cutoff = self.items.iter().map(|it| it.0).min().unwrap_or(0);
        }
    }

    fn into_sorted_jobs(mut self) -> Vec<(i32, i32, u8)> {
        self.items
            .sort_by(|a, b| b.0.cmp(&a.0).then(a.1.cmp(&b.1)));
        self.items
            .into_iter()
            .map(|(_, _, gx, gy, amt)| (gx, gy, amt))
            .collect()
    }
}

/// Rank a pore-boil candidate so superheat beats raw wetness.
///
/// When Tab boil drops (60 °C), most of a wet mountain becomes eligible.
/// Ranking by `sat` alone lets lukewarm swamps take the whole job budget
/// while an 85 °C core sits idle. A quarter-degree of superheat outranks
/// a full saturation difference.
fn pore_boil_priority(temp_c: f32, boil_c: f32, amt: u8) -> u16 {
    if amt == 0 || !temp_c.is_finite() || temp_c < boil_c {
        return 0;
    }
    let over = (temp_c - boil_c).max(0.0);
    let heat = (over * 4.0).round() as u16;
    heat.saturating_mul(256).saturating_add(amt as u16)
}

fn boil_hot_pores(world: &mut World, temp: &mut Temperature, cfg: &SteamConfig, max_cells: usize) {
    let boil = cfg.boil_point_c;
    let expand = cfg.phase_expansion_drive.max(1);
    // Stronger expansion buys more reach: +1 hop per 32 drive units.
    let hops = cfg
        .reverse_seep_hops
        .max(1)
        .saturating_add(expand / 32)
        .min(32);
    let max_work = cfg.max_escapes_per_tick.saturating_mul(3).max(16);
    let mut top = SteamAmtTopK::new(max_work as usize);
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(coord, c)| c.has_wet_pores && chunk_overlaps_hot(temp, **coord, boil))
        .map(|(k, _)| *k)
        .collect();
    for coord in coords {
        for_each_hot_tile_cell(world, temp, coord, boil, |gx, gy, t_c, cell| {
            if cell.material == MaterialId::Air || cell.sat.0 == 0 {
                return;
            }
            if permeability_cell(cell, &world.hydro) == 0 {
                return;
            }
            let heat = ((t_c - boil) / 50.0).clamp(0.0, 2.0);
            let cap = ((cfg.pore_boil_max_per_cell as f32) * (1.0 + heat))
                .round()
                .clamp(1.0, 255.0) as u8;
            let amt = cell.sat.0.min(cap);
            top.push_scored(gx, gy, amt, pore_boil_priority(t_c, boil, amt));
        });
    }
    let jobs = top.into_sorted_jobs();
    let mut work = 0u8;
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
        let t_c = temp.at_cell(gx, gy);
        let cap = water_capacity_cell(cell, &world.hydro) as u32;
        let vol = vapor_volume_units(cell.sat.0 as u32, t_c, boil, expand);
        let surplus = overpressure_units(vol, cap);
        let drive = expansion_drive_units(
            take.max((surplus / expand.max(1) as u32).min(255) as u8),
            expand,
            t_c,
            boil,
        );

        // Roofed void may take distilled gas (cavity flash). Then leftover
        // shoves the groundwater straw. Dry-rock fumaroles are last.
        let flashed = try_gas_climb(world, gx, gy, take, max_cells, false);
        reverse_seep_chain(world, temp, gx, gy, drive, hops);
        let sat_left = world
            .get_cell(gx, gy)
            .map(|c| c.sat.0)
            .unwrap_or(0);
        if sat_left > 0 && flashed == 0 {
            let _ = try_gas_climb(world, gx, gy, take.min(sat_left), max_cells, true);
        }
        if world.get_cell(gx, gy).is_some_and(|c| {
            matches!(
                c.material,
                MaterialId::LooseRock | MaterialId::Gravel
            )
        }) {
            let _ = pressure_sinter_cell(world, gx, gy);
            hydrothermal_solute_pulse(world, gx, gy, drive);
        }
        phase_crack_host(world, gx, gy, take, expand);
        work = work.saturating_add(1);
    }
}

/// Move **mass** (not volume) into a dry/open upward seat as pore/cavity gas.
///
/// Distilled: no [`carry_with_water`]. A pure gas hop cannot rain mineral.
fn try_gas_climb(
    world: &mut World,
    gx: i32,
    gy: i32,
    want: u8,
    max_cells: usize,
    allow_dry_rock: bool,
) -> u8 {
    if want == 0 {
        return 0;
    }
    let Some(src) = world.get_cell(gx, gy) else {
        return 0;
    };
    if src.material == MaterialId::Air || src.sat.0 == 0 {
        return 0;
    }
    let mut best: Option<(i32, i32, i32)> = None; // score, tx, ty
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        let Some(dst) = world.get_cell(tx, ty) else {
            continue;
        };
        let score = if dst.material == MaterialId::Air && is_steam_void(dst) {
            8_000 + dy.max(0) * 80
        } else if allow_dry_rock
            && crate::cell::is_competent_rock(dst.material)
            && dst.sat.0 == 0
            && permeability_cell(dst, &world.hydro) > 0
        {
            // Dry competent pores — fumarole chimney. Loose grains stay
            // on the liquid marble-tube path.
            6_000 + dy.max(0) * 80 + dst.pore as i32
        } else {
            continue;
        };
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, tx, ty));
        }
    }
    let Some((_, tx, ty)) = best else {
        return 0;
    };
    let placed = try_place_steam(world, tx, ty, want.min(src.sat.0), max_cells);
    if placed == 0 {
        return 0;
    }
    let before = src.sat.0;
    let mut next = src;
    next.sat = Sat(before - placed);
    world.set_cell(gx, gy, next);
    // Gas does not carry solute. Load stays on the wet host.
    if world.get_cell(gx, gy).is_some_and(|c| c.sat.0 == 0) {
        precipitate_dry_cell(world, gx, gy);
    }
    placed
}

/// Choked boiler → sky: emit **mass** at the mouth, never volume, never solute.
///
/// Mouth T ≥ boil joins sky Humidity (same store as evap). Cooler mouths
/// collapse to distilled liquid at the lip. Rejected H is parked as water.
fn leak_choked_boiler_mouth(
    world: &mut World,
    temp: &Temperature,
    cfg: &SteamConfig,
    mut humidity: Option<&mut Humidity>,
) {
    if world.steam.is_empty() {
        return;
    }
    let boil = cfg.boil_point_c;
    let expand = cfg.phase_expansion_drive.max(1);
    let keys: Vec<(i32, i32)> = world.steam.keys().copied().collect();
    let mut leaked = 0u8;
    for (gx, gy) in keys {
        if leaked >= cfg.max_escapes_per_tick.max(1) {
            break;
        }
        if !world
            .get_cell(gx, gy)
            .is_some_and(|c| c.material == MaterialId::Air)
        {
            // Pore-gas on rock: climb or collapse, no weather from mid-chimney.
            continue;
        }
        if !vessel_is_boiler(world, gx, gy) || !air_void_open_to_sky(world, gx, gy) {
            continue;
        }
        let steam = steam_at(world, gx, gy);
        if steam == 0 {
            continue;
        }
        let t_c = temp.at_cell(gx, gy);
        let vol = vapor_volume_units(steam as u32, t_c, boil, expand);
        let surplus = overpressure_units(vol, 255);
        if surplus == 0 && t_c < boil {
            continue;
        }
        let leak = choke_leak_mass(surplus.max(steam as u32), expand, 1).min(steam);
        if leak == 0 {
            continue;
        }
        let Some((mx, my)) = find_sky_mouth(world, gx, gy) else {
            continue;
        };
        let took = take_steam(world, gx, gy, leak);
        if took == 0 {
            continue;
        }
        let mouth_t = temp.at_cell(mx, my);
        let mut left = took as u32;
        if mouth_t >= boil {
            if let Some(h) = humidity.as_deref_mut() {
                let accepted = h.try_add(mx, my, left as f32).round() as u32;
                left = left.saturating_sub(accepted);
            }
        }
        if left > 0 {
            left = crate::displace::park_orphan_water(world, mx, my, left);
        }
        if left > 0 {
            // Could not seat as H or liquid — keep vapour mass in the vessel.
            let back = add_steam(world, gx, gy, left.min(255) as u8);
            left = left.saturating_sub(back as u32);
            if left > 0 {
                let _ = crate::displace::park_orphan_water(world, gx, gy, left);
            }
        }
        leaked = leaked.saturating_add(1);
    }
}

fn find_sky_mouth(world: &World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    let x = world.wrap_x(gx);
    let mut y = gy;
    for _ in 0..48 {
        if !void_is_confined(world, x, y) {
            return Some((x, y));
        }
        let up = y + 1;
        match world.get_cell(x, up) {
            None => return Some((x, y)),
            Some(c) if c.material == MaterialId::Air => {
                y = up;
            }
            Some(_) => {
                // Sidestep toward an unroofed neighbour.
                for dx in [-1, 1] {
                    let nx = world.wrap_x(x + dx);
                    if world
                        .get_cell(nx, y)
                        .is_some_and(|c| c.material == MaterialId::Air)
                        && !void_is_confined(world, nx, y)
                    {
                        return Some((nx, y));
                    }
                }
                return Some((x, y));
            }
        }
    }
    Some((x, y))
}

/// Prefer nearby Air (especially above) for freshly boiled pore steam.
#[allow(dead_code)]
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
        if !is_steam_void(c) {
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
#[allow(dead_code)]
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
            if !is_steam_void(wall) {
                continue;
            }
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
                // Dry lid frontier: crack upward competent rock so the next
                // cadence can wet/widen a climb path (never mint Air).
                crack_dry_upward_frontier(world, gx, gy, drive);
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

/// Score one reverse-seep neighbour. Higher = easier path.
///
/// Order we want: discharge voids (sky / standing water) ≫ loose grains ≫
/// already-open conduits ≫ tight competent rock. Permeability is
/// **superlinear** so a small head start (widened pore, wet sand) locks in
/// the same way aperture growth channelizes. Linear `perm × 8` treated a
/// slightly open limestone and a sand lens as near-ties, so flow wandered
/// and never lined a pipe.
fn reverse_seep_path_score(
    world: &World,
    dst: Cell,
    tx: i32,
    ty: i32,
    dy: i32,
) -> Option<(i32, bool)> {
    let vent = is_steam_discharge_vent(world, tx, ty, dst);
    let cap = water_capacity_cell(dst, &world.hydro);
    if cap == 0 && !vent {
        return None;
    }
    let room = cap.saturating_sub(dst.sat.0);
    // Packed wet rock is the marble straw: leftover at the hot end
    // shoves the next marble through a full seat. Dry / empty seats
    // still need room (or a vent) so we do not invent water.
    let packed = room == 0
        && dst.sat.0 > 0
        && dst.material != MaterialId::Air
        && permeability_cell(dst, &world.hydro) > 0;
    if room == 0 && !vent && !packed {
        return None;
    }
    if dst.material == MaterialId::Air && !vent {
        // Sealed dry cavity Air would swallow reverse-seep back into the boiler.
        return None;
    }
    if !vent {
        let p = permeability_cell(dst, &world.hydro);
        if p == 0 {
            return None;
        }
    }
    let score = if vent {
        // Voids and water-filled voids / lakes — the actual outlet.
        50_000 + dst.sat.0 as i32 * 8 + room as i32 + dy.max(0) * 80
    } else {
        let perm = permeability_cell(dst, &world.hydro) as i32;
        let mut s = perm * perm / 16 + perm * 10;
        if dst.sat.0 > 0 {
            s += dst.sat.0 as i32 * 4;
        }
        if dst.pore > 128 {
            // Already-carved conduit: lock onto the open throat.
            s += (dst.pore as i32 - 128) * 40;
        }
        if is_grain(dst.material) || is_flow_erodible(dst.material) {
            s += 4_000;
        }
        if dst.material == MaterialId::Flowstone && dst.pore > 128 {
            // Forming sinter chimney / lined pipe: keep the same mouth
            // instead of wandering into a nearby sand lens.
            s += 3_500;
        }
        let room_score = if room > 0 { room as i32 } else { 64 };
        s + room_score * 2 + dy.max(0) * 50
    };
    Some((score, vent && room == 0))
}

fn consider_reverse_seep_step(
    world: &World,
    best: &mut Option<(i32, i32, i32, Cell, bool)>,
    tx: i32,
    ty: i32,
    dy: i32,
    dst: Cell,
) {
    let Some((score, overflow_vent)) = reverse_seep_path_score(world, dst, tx, ty, dy) else {
        return;
    };
    if best.is_none_or(|(s, _, _, _, _)| score > s) {
        *best = Some((score, tx, ty, dst, overflow_vent));
    }
}

/// Push pore water one step along the path of least resistance.
///
/// Candidates prefer voids, standing water, sand / loose, and already-open
/// conduits. Throughput widens that path so the next pulse follows it.
/// Venting into Air or a lake drops dissolved load as a warm spring /
/// underwater sinter. Returns how much moved.
fn reverse_push_pore_water(world: &mut World, temp: &mut Temperature, gx: i32, gy: i32, drive: u8) -> u8 {
    reverse_push_pore_water_to(world, temp, gx, gy, drive).0
}

fn reverse_push_pore_water_to(world: &mut World, temp: &mut Temperature, gx: i32, gy: i32, drive: u8) -> (u8, Option<(i32, i32)>) {
    let mut seen = FxHashSet::default();
    reverse_push_pore_water_inner(world, temp, gx, gy, drive, 16, &mut seen)
}

fn reverse_push_pore_water_inner(
    world: &mut World,
    temp: &mut Temperature,
    gx: i32,
    gy: i32,
    drive: u8,
    depth: u8,
    seen: &mut FxHashSet<(i32, i32)>,
) -> (u8, Option<(i32, i32)>) {
    if drive == 0 || !seen.insert((gx, gy)) {
        return (0, None);
    }
    let Some(src) = world.get_cell(gx, gy) else {
        return (0, None);
    };
    if src.material == MaterialId::Air || src.sat.0 == 0 {
        return (0, None);
    }
    let want = drive.min(src.sat.0).max(1);
    let mut best: Option<(i32, i32, i32, Cell, bool)> = None; // score, tx, ty, dst, overflow_vent
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        if seen.contains(&(tx, ty)) {
            continue;
        }
        let Some(dst) = world.get_cell(tx, ty) else {
            continue;
        };
        consider_reverse_seep_step(world, &mut best, tx, ty, dy, dst);
    }
    // Full-sat deadlock: sinter grains + widen competent neighbours, then rescan.
    if best.is_none() && drive > 0 {
        let _ = pressure_sinter_cell(world, gx, gy);
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
            let tx = world.wrap_x(gx + dx);
            let ty = gy + dy;
            let Some(dst) = world.get_cell(tx, ty) else {
                continue;
            };
            if is_grain(dst.material) {
                let _ = pressure_sinter_cell(world, tx, ty);
            }
            let Some(dst) = world.get_cell(tx, ty) else {
                continue;
            };
            if crate::cell::is_competent_rock(dst.material) {
                let thr = drive.max(24);
                let _ = widen_aperture(world, tx, ty, thr, 2.5, 0x5EE1_u64, false);
            }
        }
        if world
            .get_cell(gx, gy)
            .is_some_and(|c| crate::cell::is_competent_rock(c.material))
        {
            let thr = drive.max(24);
            let _ = widen_aperture(world, gx, gy, thr, 2.0, 0x5EE2_u64, false);
        }
        // Rescan now that capacity may have opened.
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
            let tx = world.wrap_x(gx + dx);
            let ty = gy + dy;
            if seen.contains(&(tx, ty)) {
                continue;
            }
            let Some(dst) = world.get_cell(tx, ty) else {
                continue;
            };
            consider_reverse_seep_step(world, &mut best, tx, ty, dy, dst);
        }
    }
    let Some((_, tx, ty, mut dst, mut overflow_vent)) = best else {
        return (0, None);
    };
    let mut cap = water_capacity_cell(dst, &world.hydro);
    let mut room = cap.saturating_sub(dst.sat.0);
    // Full wet seat: shove its marble onward first, then occupy the hole.
    if room == 0 && !overflow_vent && dst.material != MaterialId::Air && depth > 0 {
        let _ = reverse_push_pore_water_inner(world, temp, tx, ty, drive, depth - 1, seen);
        let Some(now) = world.get_cell(tx, ty) else {
            return (0, None);
        };
        dst = now;
        cap = water_capacity_cell(dst, &world.hydro);
        room = cap.saturating_sub(dst.sat.0);
        overflow_vent = is_steam_discharge_vent(world, tx, ty, dst) && room == 0;
    }
    let moved = if overflow_vent {
        // Full open-sky lake: discharge by overflowing through the vent column.
        want
    } else {
        want.min(room)
    };
    if moved == 0 {
        return (0, None);
    }
    let before = src.sat.0;
    let mut s = world.get_cell(gx, gy).unwrap();
    s.sat = Sat(s.sat.0 - moved);
    world.set_cell(gx, gy, s);
    let fit = moved.min(room);
    let overflow = moved.saturating_sub(fit);
    if fit > 0 {
        let mut d = dst;
        d.sat = Sat(d.sat.0 + fit);
        world.set_cell(tx, ty, d);
    }
    if overflow > 0 {
        let left = crate::displace::park_orphan_water(world, tx, ty + 1, overflow as u32);
        if left > 0 {
            // Put unplaced overflow back on the source — mass-flat.
            if let Some(mut back) = world.get_cell(gx, gy) {
                let put = left.min(255) as u8;
                let room_back = u8::MAX.saturating_sub(back.sat.0);
                let add = put.min(room_back);
                back.sat = Sat(back.sat.0.saturating_add(add));
                world.set_cell(gx, gy, back);
            }
        }
    }
    let actually_moved = {
        let now = world.get_cell(gx, gy).map(|c| c.sat.0).unwrap_or(before);
        before.saturating_sub(now)
    };
    if actually_moved == 0 {
        return (0, None);
    }
    carry_with_water(world, (gx, gy), (tx, ty), actually_moved, before);
    temp.advect_with_mass(gx, gy, tx, ty, actually_moved);
    // Self-amplifying conduit: throughput opens the cell it just used
    // (mint_void=false — high-aperture rock, not Air pipes). A slightly
    // higher scale makes a steady route win the next score by more.
    if dst.material != MaterialId::Air && crate::cell::is_competent_rock(dst.material) {
        let thr = actually_moved.max(12);
        let _ = widen_aperture(world, tx, ty, thr, 4.2, 0x5EEF_u64, false);
    }
    if crate::cell::is_competent_rock(src.material) {
        let thr = actually_moved.max(8);
        let _ = widen_aperture(world, gx, gy, thr, 2.4, 0x5EE0_u64, false);
    }
    // Loose / sand path: scour opens the same way rock widening does, so
    // the next pulse prefers this lens over a dry neighbour.
    if dst.material != MaterialId::Air
        && (is_grain(dst.material) || is_flow_erodible(dst.material))
    {
        if let Some(mut g) = world.get_cell(tx, ty) {
            if g.pore < 220 {
                g.pore = g.pore.saturating_add(1);
                world.set_cell(tx, ty, g);
            }
        }
    }
    if dst.material == MaterialId::Air || overflow_vent {
        // Surface / underwater vent: depressurising spring drops load.
        // A few pulses so a slow steady route can grow sinter / pipes
        // instead of banking dissolved mineral in the lake.
        let warmth = steam_pressure_norm(world, gx, gy)
            .max(drive as f32 / 255.0)
            .clamp(0.35, 1.0);
        for _ in 0..3 {
            let _ = precipitate_vent_mouth(world, tx, ty, warmth);
        }
    } else if dissolved_at(world, tx, ty) > 0 && actually_moved >= 4 {
        // Line the last hops before daylight / a pool — not the sealed
        // boiler core (that self-sealed mountains before they could vent).
        // Skip once the lumen is already tight so lining cannot plug the pipe.
        if near_discharge_vent(world, tx, ty)
            && world
                .get_cell(tx, ty)
                .is_some_and(|c| c.pore > PIPE_LUMEN_FLOOR)
        {
            let warmth = (drive as f32 / 255.0).clamp(0.15, 0.7);
            let _ = precipitate_artesian_warm(world, tx, ty, warmth);
        }
    }
    (actually_moved, Some((tx, ty)))
}

/// True when `(gx, gy)` borders Air that opens to free sky (a real vent).
fn near_open_air_vent(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(c) = world.get_cell(nx, ny) else {
            continue;
        };
        if c.material == MaterialId::Air && is_steam_discharge_vent(world, nx, ny, c) {
            return true;
        }
    }
    false
}

/// Throat of a discharge: the vent cell or one hop behind it.
///
/// Lets a steady route mineralize the last competent cells (pipe lining)
/// without cementing the sealed boiler.
fn near_discharge_vent(world: &World, gx: i32, gy: i32) -> bool {
    if near_open_air_vent(world, gx, gy) {
        return true;
    }
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if near_open_air_vent(world, nx, ny) {
            return true;
        }
    }
    false
}

/// Crack dry competent rock at a stalled reverse-seep front (up + lateral).
fn crack_dry_upward_frontier(world: &mut World, gx: i32, gy: i32, drive: u8) {
    let thr = drive.max(24);
    // Include pure lateral cracks so flank vents (side caverns) can open —
    // not only chimney climbs.
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(c) = world.get_cell(nx, ny) else {
            continue;
        };
        if !crate::cell::is_competent_rock(c.material) {
            continue;
        }
        // Already wet/permeable neighbours are handled by the wet hop path.
        if c.sat.0 > 0 && permeability_cell(c, &world.hydro) > 0 {
            continue;
        }
        let scale = if dy > 0 { 2.8 } else { 2.2 };
        let _ = widen_aperture(world, nx, ny, thr, scale, 0xD41D_u64, false);
    }
}

fn escape_pressurized(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    max_cells: usize,
) -> u8 {
    if world.steam.is_empty() {
        return 0;
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
    escapes
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
        let mut left = leftover;
        if let Some(tube) = world.get_cell(tube_x, tube_y) {
            if tube.material == MaterialId::Air {
                let room = u8::MAX.saturating_sub(tube.sat.0);
                let put = left.min(room);
                if put > 0 {
                    let mut next = tube;
                    next.sat = Sat(tube.sat.0.saturating_add(put));
                    world.set_cell(tube_x, tube_y, next);
                    left -= put;
                }
            }
        }
        if left > 0 {
            let _ = crate::displace::park_orphan_water(world, tube_x, tube_y, left as u32);
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
    use crate::audit::{mineral_total, sat_totals};
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
    fn open_sky_steam_does_not_paint_puffs_on_the_humidity_field() {
        // Leftover markers in free air used to bloom a 4×4 plume above the
        // ridge — little steam clouds sitting on the H wash.
        let mut w = World::new(17);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
        }
        for y in 2..20 {
            w.set_cell(4, y, Cell::air());
        }
        add_steam(&mut w, 4, 8, 200);
        add_steam(&mut w, 4, 12, 180);
        let haze = steam_haze_wash(&w, None);
        assert!(
            haze.iter().all(|s| void_is_confined(&w, s.gx, s.gy)),
            "steam haze must not paint unroofed sky cells"
        );
        assert!(
            !haze.iter().any(|s| s.gy >= 2),
            "no puffy steam above the ridge"
        );

        let before = sat_totals(&w).cell_total;
        let mut cool = temp_fill(&w, 20.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut cool, &SteamConfig::default());
        assert_eq!(
            steam_total(&w),
            0,
            "open-sky leftover must leave the steam map"
        );
        assert_eq!(
            sat_totals(&w).cell_total,
            before,
            "scrub must park vapour as liquid, not dump sky Humidity"
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
        // Free shafts used to pack a buoyant steam plume. That painted
        // leftover puffs on the humidity field. Open air is evap / sky H.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..12 {
            w.set_cell(5, y, Cell::air());
        }
        add_steam(&mut w, 5, 2, 40);
        assert!(!void_is_confined(&w, 5, 2));
        let before = sat_totals(&w).cell_total;
        let mut hot = temp_fill(&w, 110.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert_eq!(steam_total(&w), 0, "open-sky steam must leave the map");
        assert_eq!(sat_totals(&w).cell_total, before, "mass stays as liquid");
        assert!(
            steam_haze_wash(&w, None).is_empty(),
            "no leftover steam wash in free sky"
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
        // Roofed void so pore flash stays cavity humidity, not a sky puff.
        w.set_cell(4, 2, Cell::air());
        w.set_cell(4, 3, Cell::solid(MaterialId::Stone));
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
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
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
        assert!(PHASE_EXPANSION_DRIVE >= 64);
        let boiled = 10u8;
        let drive = expansion_drive_units(boiled, PHASE_EXPANSION_DRIVE, 100.0, 100.0);
        assert!(
            drive > boiled.saturating_mul(4),
            "drive={drive} must dwarf boiled={boiled}"
        );
        // Hotter rock must push harder (until the u8 ceiling).
        let cool = expansion_drive_units(10, 64, 100.0, 100.0);
        let hot = expansion_drive_units(10, 64, 180.0, 100.0);
        assert!(hot > cool, "superheat must raise drive ({cool} → {hot})");
        // Tab-high expand must out-shove a token expand at the same heat.
        let low = expansion_drive_units(20, 16, 100.0, 100.0);
        let high = expansion_drive_units(20, 96, 100.0, 100.0);
        assert!(high > low, "expand knob must matter ({low} → {high})");
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
    fn reverse_seep_path_score_prefers_vents_then_grains() {
        let mut w = World::new(13);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(20);
        let mut tight = Cell::solid(MaterialId::Stone);
        tight.sat = Sat(4);
        tight.pore = 20;
        let mut open_limestone = Cell::solid(MaterialId::Limestone);
        open_limestone.sat = Sat(12);
        open_limestone.pore = 210;
        let mut packed_pore = Cell::solid(MaterialId::Limestone);
        packed_pore.sat = Sat(8);
        packed_pore.pore = 200;
        let mut sinter_pipe = Cell::solid(MaterialId::Flowstone);
        sinter_pipe.sat = Sat(4);
        sinter_pipe.pore = 210;
        w.set_cell(3, 2, sand);
        w.set_cell(4, 2, tight);
        w.set_cell(5, 2, open_limestone);
        w.set_cell(6, 2, packed_pore);
        w.set_cell(7, 2, sinter_pipe);
        w.set_cell(3, 8, Cell::air());
        let sand_s = reverse_seep_path_score(&w, sand, 3, 2, 0).unwrap().0;
        let tight_s = reverse_seep_path_score(&w, tight, 4, 2, 0).unwrap().0;
        let open_s = reverse_seep_path_score(&w, open_limestone, 5, 2, 0).unwrap().0;
        let packed_s = reverse_seep_path_score(&w, packed_pore, 6, 2, 0).unwrap().0;
        let pipe_s = reverse_seep_path_score(&w, sinter_pipe, 7, 2, 0).unwrap().0;
        let vent_s = reverse_seep_path_score(&w, Cell::air(), 3, 8, 1).unwrap().0;
        assert!(sand_s > tight_s, "sand should beat tight stone ({sand_s} vs {tight_s})");
        assert!(
            open_s > sand_s,
            "already-open high-pore limestone should lock in over sand ({open_s} vs {sand_s})"
        );
        assert!(
            vent_s > sand_s * 2,
            "open vents should dominate grains ({vent_s} vs {sand_s})"
        );
        assert!(
            packed_s > tight_s * 3,
            "widened pore should stay preferred over tight rock ({packed_s} vs {tight_s})"
        );
        assert!(
            pipe_s > sand_s,
            "a forming sinter pipe must keep the route over sand ({pipe_s} vs {sand_s})"
        );
    }

    #[test]
    fn reverse_push_lines_throat_behind_an_open_vent() {
        let mut w = World::new(17);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
        let mut src = Cell::solid(MaterialId::Limestone);
        src.sat = Sat(220);
        src.pore = 200;
        w.set_cell(4, 2, src);
        let mut throat = Cell::solid(MaterialId::Limestone);
        throat.sat = Sat(40);
        throat.pore = 200;
        w.set_cell(4, 3, throat);
        w.set_cell(4, 4, Cell::air());
        add_dissolved(&mut w, 4, 2, 200);
        let before = crate::audit::mineral_total(&w);
        let load0 = dissolved_at(&w, 4, 2) + dissolved_at(&w, 4, 3) + dissolved_at(&w, 4, 4);
        let pore0 = w.get_cell(4, 3).unwrap().pore;
        let mut hot = temp_fill(&w, 30.0);
        for _ in 0..12 {
            let _ = reverse_push_pore_water(&mut w, &mut hot, 4, 2, 200);
            if let Some(mut c) = w.get_cell(4, 2) {
                if c.material != MaterialId::Air {
                    c.sat = Sat(c.sat.0.max(180));
                    w.set_cell(4, 2, c);
                }
            }
        }
        assert_eq!(crate::audit::mineral_total(&w), before);
        let load1 = dissolved_at(&w, 4, 2) + dissolved_at(&w, 4, 3) + dissolved_at(&w, 4, 4);
        let after = w.get_cell(4, 3).unwrap();
        assert!(
            load1 < load0 || after.pore < pore0 || after.material == MaterialId::Flowstone,
            "open throat should drop load or line ({load0}→{load1}, pore {pore0}→{})",
            after.pore
        );
        assert_ne!(after.material, MaterialId::Air, "lining must not mint an Air pipe");
        assert!(
            after.pore > 128,
            "lining must keep a lumen, got pore {}",
            after.pore
        );
    }

    #[test]
    fn reverse_push_underwater_vent_deposits_load() {
        let mut w = World::new(19);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..7 {
            for y in 1..7 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut src = Cell::solid(MaterialId::Limestone);
        src.sat = Sat(220);
        src.pore = 180;
        w.set_cell(4, 2, src);
        let mut lake = Cell::air();
        lake.sat = Sat(255);
        w.set_cell(4, 3, lake);
        w.set_cell(4, 4, Cell::solid(MaterialId::Stone));
        add_dissolved(&mut w, 4, 2, 220);
        let before = crate::audit::mineral_total(&w);
        let load0 = dissolved_at(&w, 4, 2) + dissolved_at(&w, 4, 3);
        let mut hot = temp_fill(&w, 40.0);
        for _ in 0..16 {
            let _ = reverse_push_pore_water(&mut w, &mut hot, 4, 2, 200);
            if let Some(mut c) = w.get_cell(4, 2) {
                if c.material != MaterialId::Air {
                    c.sat = Sat(c.sat.0.max(180));
                    w.set_cell(4, 2, c);
                }
            }
        }
        assert_eq!(crate::audit::mineral_total(&w), before);
        let load1 = dissolved_at(&w, 4, 2) + dissolved_at(&w, 4, 3);
        let vent = w.get_cell(4, 3).unwrap();
        let floor_closed = w
            .get_cell(4, 2)
            .is_some_and(|c| c.material != MaterialId::Air && c.pore < 180);
        assert!(
            load1 < load0 || vent.material != MaterialId::Air || floor_closed,
            "underwater vent must drop dissolved mineral (load {load0}→{load1}, vent {vent:?})"
        );
    }

    #[test]
    fn try_place_steam_reports_clip_not_request() {
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 2, Cell::air());
        w.set_cell(2, 3, Cell::solid(MaterialId::Stone));
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
    fn flood_equalize_does_not_truncate_wet_air_steam() {
        // Wet lake Air (sat > STEAM_VOID_SAT_MAX) holds steam and joins the
        // flood component, but used to be excluded from void seats while
        // `(total/n) as u8` truncated shares >255 — vapour vanished.
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..9 {
            for y in 1..7 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        // Two dry voids + five wet pool cells under a roof.
        for x in 3..8 {
            let mut wet = Cell::air();
            wet.sat = Sat(255);
            w.set_cell(x, 2, wet);
        }
        w.set_cell(3, 3, Cell::air());
        w.set_cell(4, 3, Cell::air());
        w.set_cell(5, 3, Cell::air());
        w.set_cell(3, 4, Cell::air());
        w.set_cell(4, 4, Cell::air());
        // Pack steam onto wet seats so total/n_voids would exceed 255.
        for x in 3..8 {
            add_steam(&mut w, x, 2, 251);
        }
        let before = steam_total(&w);
        assert!(before > 255 * 2, "fixture must stress the old u8 truncate path");
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            ..SteamConfig::default()
        };
        flood_equalize_steam(&mut w, &cfg, cfg.max_steam_cells as usize);
        assert_eq!(
            steam_total(&w),
            before,
            "flood must preserve steam mass ({before} → {})",
            steam_total(&w)
        );
    }

    #[test]
    fn flood_equalize_share_above_255_is_not_cast_to_u8() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..5 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        // One dry void + three wet steam seats.
        w.set_cell(3, 2, Cell::air());
        for x in 4..7 {
            let mut wet = Cell::air();
            wet.sat = Sat(220);
            w.set_cell(x, 2, wet);
        }
        add_steam(&mut w, 3, 2, 200);
        add_steam(&mut w, 4, 2, 255);
        add_steam(&mut w, 5, 2, 255);
        add_steam(&mut w, 6, 2, 255);
        let before = steam_total(&w);
        // If only the dry void seats, share = before/1 > 255 → old `as u8` loss.
        assert!(before > 255);
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            ..SteamConfig::default()
        };
        flood_equalize_steam(&mut w, &cfg, cfg.max_steam_cells as usize);
        assert_eq!(steam_total(&w), before, "u32 share must not truncate");
    }

    #[test]
    fn flood_equalize_parks_leftover_when_steam_map_is_full() {
        // With max_cells=1, flood takes every seat then can only re-place on
        // one marker. Leftover must park into sat/cave_humidity — not vanish.
        let mut w = World::new(5);
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
        add_steam(&mut w, 3, 2, 200);
        add_steam(&mut w, 4, 2, 200);
        add_steam(&mut w, 5, 2, 200);
        let before = sat_totals(&w).cell_total;
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            max_steam_cells: 1,
            ..SteamConfig::default()
        };
        flood_equalize_steam(&mut w, &cfg, 1);
        assert_eq!(
            sat_totals(&w).cell_total,
            before,
            "steam-cap leftover must stay in the cell water budget"
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
    fn cavity_pressure_does_not_mint_a_heat_lamp() {
        // `boil + press × 35` used to pull every steam tile toward 135 °C.
        // A 60 °C bootstrap then a 100 °C reload kept the boiler running
        // on maps whose overburden never supplies that heat.
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
        add_steam(&mut w, 4, 2, 220);
        let mut temp = temp_fill(&w, 40.0);
        let before = temp.at_cell(4, 2);
        let cfg = SteamConfig::default();
        transmit_cavity_pressure(&mut w, &mut temp, &cfg);
        let after = temp.at_cell(4, 2);
        assert!(
            after < before + 0.05,
            "cavity pressure must not invent heat ({before:.2} → {after:.2})"
        );
        assert!(
            after < 50.0,
            "must stay near the rock temperature, not climb toward boil+35 ({after:.1})"
        );
    }

    #[test]
    fn cavity_pressure_shares_existing_heat_into_a_cold_wall() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..10 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(3, 2, Cell::air());
        w.set_cell(4, 2, {
            let mut c = Cell::solid(MaterialId::Limestone);
            c.sat = Sat(120);
            c
        });
        add_steam(&mut w, 3, 2, 200);
        let mut temp = temp_fill(&w, 20.0);
        let (shx, shy) = temp.tile_of(3, 2);
        let (whx, why) = temp.tile_of(4, 2);
        assert_ne!((shx, shy), (whx, why), "precondition: wall is another tile");
        temp.set_tile_c(shx, shy, 90.0);
        temp.set_tile_c(whx, why, 20.0);
        let before_src = temp.at_cell(3, 2);
        let before_wall = temp.at_cell(4, 2);
        transmit_cavity_pressure(&mut w, &mut temp, &SteamConfig::default());
        let after_src = temp.at_cell(3, 2);
        let after_wall = temp.at_cell(4, 2);
        assert!(
            after_wall > before_wall + 0.2,
            "hot cavity should share heat into the wall ({before_wall:.1} → {after_wall:.1})"
        );
        assert!(
            after_src < before_src - 0.05,
            "source tile must cool (not a lamp) ({before_src:.1} → {after_src:.1})"
        );
        assert!(
            after_wall < before_src,
            "wall must not exceed the heat the cavity already had"
        );
    }

    #[test]
    fn cavity_pressure_below_boil_climbs_wet_column() {
        // Sealed wet limestone column under a dense cavity: below-boil vapour
        // must multi-hop warm water upward (not die after one shove).
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Thick bedrock jacket — including far above — so a (0,2) hop cannot
        // vent into free sky Air and short-circuit the climb.
        for x in 0..16 {
            for y in 0..16 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        w.set_cell(4, 2, Cell::air());
        for y in 3..9 {
            let mut c = Cell::solid(MaterialId::Limestone);
            c.sat = Sat(if y == 3 { 200 } else { 8 });
            c.pore = 200;
            w.set_cell(4, y, c);
        }
        add_steam(&mut w, 4, 2, 220);
        let mut temp = temp_fill(&w, 80.0); // below boil
        let sat_top0 = w.get_cell(4, 7).unwrap().sat.0;
        let before = sat_totals(&w).cell_total;
        let cfg = SteamConfig {
            enable_escape: false,
            enable_pore_boil: false,
            reverse_seep_hops: 12,
            ..SteamConfig::default()
        };
        for _ in 0..6 {
            transmit_cavity_pressure(&mut w, &mut temp, &cfg);
        }
        let sat_top1 = w.get_cell(4, 7).unwrap().sat.0;
        let column: Vec<u8> = (3..9).map(|y| w.get_cell(4, y).unwrap().sat.0).collect();
        assert_eq!(
            sat_totals(&w).cell_total,
            before,
            "climb must stay mass-flat; column={column:?}"
        );
        assert!(
            sat_top1 > sat_top0,
            "below-boil cavity pressure must climb wet column ({sat_top0} → {sat_top1}, column={column:?})"
        );
        for y in 3..9 {
            assert_ne!(
                w.get_cell(4, y).unwrap().material,
                MaterialId::Air,
                "climb must not mint Air at y={y}"
            );
        }
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
        // not only move water. Packed cells vent sideways; source-side
        // widen is what opens the column (in-pipe hops are room-capped
        // and almost never pass the aperture roll).
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
            REVERSE_SEEP_HOPS >= 12,
            "default reverse-seep range must be long enough to feed conduits"
        );
        assert!(
            PHASE_EXPANSION_DRIVE >= 64,
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
    fn pore_flash_follows_live_boil_point() {
        // Tab boil 60 °C used to keep HUD flash glued to 100 °C, so an 85 °C
        // wet source looked dead while a 60 °C source that had already
        // flashed painted cavity pressure.
        let mut w = World::new(91);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut rock = Cell::solid(MaterialId::LooseRock);
        rock.pore = 64;
        let cap = water_capacity_cell(rock, &w.hydro).max(1);
        rock.sat = Sat(cap);
        w.set_cell(5, 3, rock);
        let (p60, kind60) = cell_pressure_norm_with_boil(&w, 5, 3, 85.0, 60.0, 100);
        assert!(
            p60 > 0.05 && kind60 == CellPressureKind::PoreFlash,
            "85 °C wet rock must flash against boil=60 (got {p60}, {kind60:?})"
        );
        let (p100, kind100) = cell_pressure_norm_with_boil(&w, 5, 3, 85.0, 100.0, 100);
        assert!(
            p100 < p60 * 0.5,
            "same 85 °C cell stays quiet against boil=100 (got {p100} vs {p60})"
        );
        assert!(
            kind100 != CellPressureKind::PoreFlash || p100 < 0.05,
            "85 °C must not look like a 100 °C boiler ({p100}, {kind100:?})"
        );
        let (below, below_kind) = cell_pressure_norm_with_boil(&w, 5, 3, 40.0, 60.0, 100);
        assert!(
            below < p60 * 0.5,
            "below-boil volume must collapse ({below} vs {p60}, {below_kind:?})"
        );
    }

    #[test]
    fn pore_boil_priority_ranks_superheat_above_wetness() {
        let luke = pore_boil_priority(61.0, 60.0, 255);
        let hot = pore_boil_priority(85.0, 60.0, 1);
        assert!(
            hot > luke,
            "0.25 °C of superheat must outrank a full sat swing ({hot} vs {luke})"
        );
        assert_eq!(pore_boil_priority(50.0, 60.0, 255), 0);
    }

    #[test]
    fn vapor_volume_multiplies_only_while_hot() {
        assert_eq!(vapor_volume_units(14, 200.0, 60.0, 100), 1400);
        assert_eq!(vapor_volume_units(14, 20.0, 60.0, 100), 14);
        assert_eq!(overpressure_units(1400, 14), 1386);
        assert_eq!(overpressure_units(14, 14), 0);
        assert!((leftover_pack_norm(1400, 14) - (1.0 - 14.0 / 1400.0)).abs() < 1e-5);
        assert!((leftover_pack_norm(700, 14) - (1.0 - 14.0 / 700.0)).abs() < 1e-5);
        assert_eq!(leftover_pack_norm(14, 14), 0.0);
        assert!(choke_leak_mass(1386, 100, 1) <= 24);
        assert!(choke_leak_mass(1386, 100, 1) > 0);
    }

    #[test]
    fn full_pore_reads_packed_against_live_expand() {
        let mut w = World::new(91);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut rock = Cell::solid(MaterialId::LooseRock);
        rock.pore = 14;
        let cap = water_capacity_cell(rock, &w.hydro).max(1);
        rock.sat = Sat(cap);
        w.set_cell(5, 3, rock);
        let (hot, kind) = cell_pressure_norm_with_boil(&w, 5, 3, 120.0, 60.0, 100);
        let expect = leftover_pack_norm(vapor_volume_units(cap as u32, 120.0, 60.0, 100), cap as u32);
        assert!(
            (hot - expect).abs() < 0.02 && kind == CellPressureKind::PoreFlash,
            "full pore ×100 at 120 °C must read leftover pack {expect} (got {hot}, {kind:?})"
        );
        let (cold, cold_kind) = cell_pressure_norm_with_boil(&w, 5, 3, 20.0, 60.0, 100);
        assert!(
            cold < 0.02 && cold_kind == CellPressureKind::None,
            "cool collapse must drop surplus ({cold}, {cold_kind:?})"
        );
    }

    #[test]
    fn half_full_hot_pore_is_still_packed_leftover() {
        let mut w = World::new(91);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut rock = Cell::solid(MaterialId::LooseRock);
        rock.pore = 14;
        let cap = water_capacity_cell(rock, &w.hydro).max(2);
        let mass = (cap / 2).max(1);
        rock.sat = Sat(mass);
        w.set_cell(5, 3, rock);
        let (p, kind) = cell_pressure_norm_with_boil(&w, 5, 3, 120.0, 60.0, 100);
        let expect =
            leftover_pack_norm(vapor_volume_units(mass as u32, 120.0, 60.0, 100), cap as u32);
        assert!(
            (p - expect).abs() < 0.02 && p > 0.85 && kind == CellPressureKind::PoreFlash,
            "half-full ×100 is leftover volume {expect}, not half-wetness (got {p}, {kind:?})"
        );
    }

    #[test]
    fn open_weather_air_does_not_paint_pressure() {
        let mut w = World::new(215);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 0, Cell::solid(MaterialId::Bedrock));
        let mut puddle = Cell::air();
        puddle.sat = Sat(200);
        w.set_cell(4, 1, puddle);
        for y in 2..12 {
            w.set_cell(4, y, Cell::air());
        }
        add_steam(&mut w, 4, 1, 80);
        let (p, kind) = cell_pressure_norm_with_boil(&w, 4, 1, 180.0, 60.0, 100);
        assert_eq!(
            kind,
            CellPressureKind::None,
            "open hot puddle is weather, not a P overlay ({p}, {kind:?})"
        );
    }

    #[test]
    fn fat_pocket_one_wide_chimney_is_a_boiler() {
        let mut w = World::new(211);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..24 {
            for y in 0..20 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 4..20 {
            for y in 2..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 12..20 {
            w.set_cell(12, y, Cell::air());
        }
        for y in 20..28 {
            w.set_cell(12, y, Cell::air());
        }
        w.set_cell(8, 3, Cell::water());
        assert_eq!(
            classify_air_vessel(&w, 8, 4),
            VesselKind::Boiler,
            "fat cave + 1-wide sky straw is a boiler, not weather"
        );
        assert_eq!(
            classify_air_vessel(&w, 12, 26),
            VesselKind::Weather,
            "unroofed chimney lip is the weather mouth"
        );
        let mut hot = temp_fill(&w, 210.0);
        let before = sat_totals(&w).cell_total;
        let minerals = mineral_total(&w);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert!(
            steam_total(&w) > 0,
            "choked boiler must flash cavity mass, not dump the pocket as evap"
        );
        assert_eq!(sat_totals(&w).cell_total, before, "flash is mass-flat");
        assert_eq!(mineral_total(&w), minerals, "flash must not mint or eat mineral");
    }

    #[test]
    fn open_u_hollow_is_weather() {
        let mut w = World::new(212);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..6 {
                if (3..=12).contains(&x) && y < 5 {
                    w.set_cell(x, y, Cell::air());
                } else {
                    w.set_cell(x, y, Cell::solid(MaterialId::Stone));
                }
            }
        }
        // Open top: no lid.
        for x in 3..=12 {
            w.set_cell(x, 5, Cell::air());
            w.set_cell(x, 6, Cell::air());
        }
        w.set_cell(7, 2, Cell::water());
        assert_eq!(
            classify_air_vessel(&w, 7, 2),
            VesselKind::Weather,
            "wide open U stays weather"
        );
        let mut hot = temp_fill(&w, 210.0);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut hot, &SteamConfig::default());
        assert_eq!(
            steam_total(&w),
            0,
            "open U must not mint boiler steam"
        );
    }

    #[test]
    fn marble_tube_moves_liquid_and_solute_together() {
        let mut w = World::new(213);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 0..7 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let lim_cap = crate::cell::water_capacity(MaterialId::Limestone).max(1);
        let mut host = Cell::solid(MaterialId::Limestone);
        host.sat = Sat(lim_cap);
        host.pore = 120;
        w.set_cell(4, 1, host);
        let sand_cap = crate::cell::water_capacity(MaterialId::Sand).max(1);
        let mut dest = Cell::solid(MaterialId::Sand);
        dest.sat = Sat(sand_cap / 8);
        dest.pore = 200;
        w.set_cell(5, 2, dest);
        w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
        crate::mineral::add_dissolved(&mut w, 4, 1, 24);
        let water0 = sat_totals(&w).cell_total;
        let min0 = mineral_total(&w);
        let dest0 = w.get_cell(5, 2).unwrap().sat.0;
        let host0 = w.get_cell(4, 1).unwrap().sat.0;
        let mut hot = temp_fill(&w, 170.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 100,
            boil_point_c: 60.0,
            reverse_seep_hops: 4,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..12 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let dest1 = w.get_cell(5, 2).unwrap().sat.0;
        let host1 = w.get_cell(4, 1).unwrap().sat.0;
        assert!(
            dest1 > dest0 || host1 < host0,
            "surplus must shove groundwater along the straw (dest {dest0}→{dest1}, host {host0}→{host1})"
        );
        assert_eq!(sat_totals(&w).cell_total, water0, "marble tube is mass-flat");
        assert_eq!(mineral_total(&w), min0, "solute rides liquid, never minted");
    }

    #[test]
    fn marble_tube_shoves_through_a_saturated_column() {
        // Full seats used to reject reverse-seep (needed room). A packed
        // groundwater straw must still spit a marble at the mouth.
        let mut w = World::new(221);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..8 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=5 {
            let mut rock = Cell::solid(MaterialId::Stone);
            let cap = water_capacity_cell(rock, &w.hydro).max(1);
            rock.sat = Sat(cap);
            w.set_cell(5, y, rock);
        }
        w.set_cell(5, 6, Cell::air());
        let water0 = sat_totals(&w).cell_total;
        let bot0 = w.get_cell(5, 1).unwrap().sat.0;
        let mouth0 = w.get_cell(5, 6).unwrap().sat.0;
        let mut hot = temp_fill(&w, 110.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 96,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..16 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let bot1 = w.get_cell(5, 1).unwrap().sat.0;
        let mouth1 = w.get_cell(5, 6).unwrap().sat.0;
        assert_eq!(sat_totals(&w).cell_total, water0, "packed straw is mass-flat");
        assert!(
            mouth1 > mouth0 || bot1 < bot0,
            "leftover must shove a marble through the full column (mouth {mouth0}→{mouth1}, base {bot0}→{bot1})"
        );
    }

    #[test]
    fn gas_climb_does_not_carry_solute() {
        let mut w = World::new(214);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..9 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 4, Cell::air());
        w.set_cell(5, 5, Cell::air());
        let mut host = Cell::solid(MaterialId::Sand);
        host.sat = Sat(crate::cell::water_capacity(MaterialId::Sand));
        w.set_cell(5, 3, host);
        crate::mineral::add_dissolved(&mut w, 5, 3, 50);
        let min0 = mineral_total(&w);
        let water0 = sat_totals(&w).cell_total;
        let mut hot = temp_fill(&w, 180.0);
        w.tick = STEAM_EVERY;
        apply_steam(
            &mut w,
            &mut hot,
            &SteamConfig {
                enable_pore_boil: true,
                boil_point_c: 60.0,
                phase_expansion_drive: 100,
                ..SteamConfig::default()
            },
        );
        assert_eq!(mineral_total(&w), min0, "gas hop must not mint or drop solute");
        assert_eq!(sat_totals(&w).cell_total, water0, "gas hop is mass-flat");
        assert_eq!(
            crate::mineral::dissolved_at(&w, 5, 4) + crate::mineral::dissolved_at(&w, 5, 5),
            0,
            "distilled vapour must not carry load into the void"
        );
    }

    #[test]
    fn hot_loose_rock_sinters_and_moves_water_upward() {
        // Buried hot saturated LooseRock must leave the grain stall: sinter to
        // competent rock, shove water upward, and widen without Air pipes.
        let mut w = World::new(101);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        // Fully saturated LooseRock column under competent roof — no Air seat.
        for y in 1..6 {
            let mut rock = Cell::solid(MaterialId::LooseRock);
            rock.pore = 64;
            let cap = water_capacity_cell(rock, &w.hydro).max(1);
            rock.sat = Sat(cap);
            w.set_cell(5, y, rock);
        }
        w.set_cell(5, 6, Cell::solid(MaterialId::Stone));
        let mut hot = temp_fill(&w, 180.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            phase_expansion_drive: 96,
            reverse_seep_hops: 10,
            pore_boil_max_per_cell: 48,
            ..SteamConfig::default()
        };
        for i in 1..40 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let host = w.get_cell(5, 1).unwrap();
        assert!(
            crate::cell::is_competent_rock(host.material),
            "hot LooseRock must sinter to competent rock, got {:?}",
            host.material
        );
        assert_ne!(host.material, MaterialId::Air, "must not mint Air pipes");
        let upward = (2..=5).any(|y| {
            w.get_cell(5, y).is_some_and(|c| {
                c.sat.0 > 0 && (c.pore > 64 || crate::cell::is_competent_rock(c.material))
            })
        });
        let carved = (1..=5).any(|y| {
            w.get_cell(5, y)
                .is_some_and(|c| crate::cell::is_competent_rock(c.material) && c.pore > 64)
        });
        assert!(
            upward || carved,
            "pressurized sinter path must move water or widen conduit upward"
        );
        // Soft-lid / assault must not have punched an Air column through the roof.
        assert_ne!(
            w.get_cell(5, 6).unwrap().material,
            MaterialId::Air,
            "roof must stay rock (no cheap sand/Air pipe)"
        );
    }

    #[test]
    fn hot_conduit_can_deposit_with_hydrothermal_solute() {
        let mut w = World::new(103);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..7 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut rock = Cell::solid(MaterialId::LooseRock);
        rock.pore = 80;
        let cap = water_capacity_cell(rock, &w.hydro).max(1);
        rock.sat = Sat(cap);
        w.set_cell(5, 1, rock);
        // Open vent above a short column so artesian deposit can seat.
        for y in 2..5 {
            let mut mid = Cell::solid(MaterialId::LooseRock);
            mid.pore = 100;
            let c = water_capacity_cell(mid, &w.hydro).max(1);
            mid.sat = Sat(c / 2);
            w.set_cell(5, y, mid);
        }
        w.set_cell(5, 5, Cell::air());
        let mut hot = temp_fill(&w, 175.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            phase_expansion_drive: 96,
            reverse_seep_hops: 10,
            pore_boil_max_per_cell: 64,
            ..SteamConfig::default()
        };
        let load0 = dissolved_at(&w, 5, 1) + dissolved_at(&w, 5, 5);
        for i in 1..50 {
            w.tick = STEAM_EVERY * i;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let load1 = (0..8).map(|y| dissolved_at(&w, 5, y)).sum::<u16>();
        let deposited = (0..8).any(|y| {
            w.get_cell(5, y)
                .is_some_and(|c| c.material == MaterialId::Flowstone)
        });
        let cemented = (1..=4).any(|y| {
            w.get_cell(5, y).is_some_and(|c| {
                matches!(
                    c.material,
                    MaterialId::Conglomerate | MaterialId::Stone | MaterialId::Sandstone
                )
            })
        });
        assert!(
            load1 > load0 || deposited || cemented,
            "hydrothermal solute should appear, cement, or deposit along the hot path"
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
        let density = 180.0 / 255.0;
        assert!(
            p > density + 0.1,
            "boiler P is leftover pack, not steam/255 density ({p} vs {density})"
        );
        let (cold, cold_kind) = cell_pressure_norm(&w, 4, 2, 20.0);
        assert!(
            cold < 0.02 && cold_kind == CellPressureKind::None,
            "cool vessel leftover must collapse ({cold}, {cold_kind:?})"
        );
    }

    #[test]
    fn wide_u_weather_does_not_paint_pressure() {
        let mut w = World::new(219);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 4..12 {
            for y in 2..6 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 4..12 {
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        add_steam(&mut w, 8, 3, 80);
        assert_eq!(
            classify_air_vessel(&w, 8, 3),
            VesselKind::Weather,
            "wide U is weather, not a pinprick boiler"
        );
        let (p, kind) = cell_pressure_norm_with_boil(&w, 8, 3, 180.0, 60.0, 100);
        assert_eq!(
            kind,
            CellPressureKind::None,
            "wide U must stay dark on P ({p}, {kind:?})"
        );
    }

    #[test]
    fn flooded_side_channel_to_surface_bleeds_confined_steam() {
        // Boiler pocket + flooded flank corridor opening on a mid-map hillside.
        // Standing water used to block flood BFS; pressure must still bleed
        // toward the open vent (steam → liquid in the channel).
        let mut w = World::new(101);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            for y in 0..14 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        // Confined boiler chamber (roofed).
        for x in 4..8 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Flooded corridor left to a hillside vent (open sky above x=1..2).
        for x in 1..5 {
            let mut lake = Cell::air();
            lake.sat = Sat(255);
            w.set_cell(x, 3, lake);
        }
        w.set_cell(1, 4, Cell::air());
        w.set_cell(1, 5, Cell::air());
        w.set_cell(2, 4, Cell::air());
        w.set_cell(2, 5, Cell::air());
        // Clear sky column above the hillside mouth.
        for y in 6..14 {
            w.set_cell(1, y, Cell::air());
            w.set_cell(2, y, Cell::air());
        }
        assert!(void_is_confined(&w, 6, 3), "boiler must stay roofed");
        assert!(
            air_void_open_to_sky(&w, 1, 3),
            "flooded hillside mouth must count as open to sky"
        );
        add_steam(&mut w, 6, 3, 200);
        add_steam(&mut w, 5, 3, 180);
        let steam0 = steam_total(&w);
        let sat0 = sat_totals(&w).cell_total;
        // Equalize only — boiling lake water would mint steam and mask bleed.
        flood_equalize_steam(&mut w, &SteamConfig::default(), 4096);
        let steam1 = steam_total(&w);
        let sat1 = sat_totals(&w).cell_total;
        assert!(
            steam1 < steam0,
            "open flooded vent must bleed confined steam ({steam0} → {steam1})"
        );
        // sat_totals already folds steam — bleed recondenses vapour into liquid
        // seats / park without changing the water ledger.
        assert_eq!(sat1, sat0, "bleed must stay mass-flat ({sat0} → {sat1})");
        assert!(
            (1..=4).any(|x| w.get_cell(x, 3).unwrap().sat.0 > 0)
                || (1..=2).any(|x| (3..=6).any(|y| w.get_cell(x, y).unwrap().sat.0 > 0)),
            "vent corridor must still hold liquid after bleed"
        );
    }

    #[test]
    fn reverse_push_discharges_into_full_open_sky_lake() {
        // Full surface-connected lake used to be skipped (room==0). Overflow
        // through the vent must still move pore water out of the boiler wall.
        let mut w = World::new(103);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut rock = Cell::solid(MaterialId::Limestone);
        rock.sat = Sat(200);
        rock.pore = 180;
        w.set_cell(5, 2, rock);
        // Pack the whole open vent column full so the only discharge path is
        // overflow (no empty Air seat to prefer over the lake).
        for y in 2..8 {
            let mut lake = Cell::air();
            lake.sat = Sat(255);
            w.set_cell(4, y, lake);
        }
        for y in 8..10 {
            w.set_cell(4, y, Cell::air());
        }
        assert!(air_void_open_to_sky(&w, 4, 2));
        let sat0 = sat_totals(&w).cell_total;
        let src0 = w.get_cell(5, 2).unwrap().sat.0;
        let mut temp = temp_fill(&w, 40.0);
        let (moved, dest) = reverse_push_pore_water_to(&mut w, &mut temp, 5, 2, 80);
        assert!(moved > 0, "full open-sky lake must accept reverse-seep discharge");
        assert!(
            dest.is_some_and(|(x, y)| x == 4 && (2..8).contains(&y)),
            "must discharge into the open vent column, got {dest:?}"
        );
        let src1 = w.get_cell(5, 2).unwrap().sat.0;
        assert!(src1 < src0, "source pore water must leave the wall");
        assert_eq!(
            sat_totals(&w).cell_total,
            sat0,
            "overflow discharge must stay mass-flat"
        );
    }

    #[test]
    fn sky_probe_memo_matches_repeat_and_invalidates_on_carve() {
        let mut w = World::new(111);
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
        assert!(void_is_confined(&w, 4, 3));
        assert!(!air_void_open_to_sky(&w, 4, 3));
        assert_eq!(
            air_void_open_to_sky(&w, 4, 3),
            air_void_open_to_sky(&w, 5, 3),
            "connected seats must share the painted probe"
        );
        // Side vent to daylight — topology bump must drop the sealed memo.
        w.set_cell(3, 5, Cell::air());
        for y in 6..12 {
            w.set_cell(3, y, Cell::air());
        }
        assert!(
            air_void_open_to_sky(&w, 4, 3),
            "carve to sky must invalidate the sealed probe memo"
        );
    }

    #[test]
    fn steam_haze_wash_is_stable_across_repeat_calls() {
        let mut w = World::new(113);
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
        add_steam(&mut w, 4, 3, 180);
        add_steam(&mut w, 5, 3, 160);
        let a = steam_haze_wash(&w, None);
        let b = steam_haze_wash(&w, None);
        assert_eq!(a, b, "paused-frame haze memo must be identical");
        assert!(!a.is_empty(), "boiler haze must paint");
    }

    #[test]
    fn steam_pressure_norm_is_zero_far_from_boiler() {
        let mut w = World::new(121);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..7 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        add_steam(&mut w, 4, 3, 200);
        assert!(steam_pressure_norm(&w, 4, 3) > 0.0);
        assert_eq!(
            steam_pressure_norm(&w, 4, 3),
            steam_pressure_norm(&w, 4, 3),
            "repeat pressure reads must hit the memo"
        );
        assert_eq!(
            steam_pressure_norm(&w, 40, 3),
            0.0,
            "cells outside the steam bbox must skip the column walk"
        );
        assert_eq!(
            steam_pressure_rate_scale(&w, 40, 3),
            1.0,
            "far confined wells must not pay boiler pressure"
        );
    }

    #[test]
    fn steam_pressure_equalizes_across_a_wide_cavity() {
        // A 3-wide downward candle would light only the steam column.
        // The pocket mean must fill the whole roofed chamber.
        let mut w = World::new(141);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..12 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..11 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        add_steam(&mut w, 4, 3, 200);
        let under = steam_pressure_norm(&w, 4, 3);
        let far = steam_pressure_norm(&w, 10, 3);
        let mid = steam_pressure_norm(&w, 7, 2);
        assert!(under > 0.15, "steam seat must read pocket pressure ({under})");
        assert!(
            (under - far).abs() < 0.02,
            "far side of the same cave must equalize (under={under} far={far})"
        );
        assert!(
            (under - mid).abs() < 0.02,
            "roof and floor of the pocket must match (under={under} mid={mid})"
        );
    }

    #[test]
    fn steam_pressure_does_not_cross_a_stone_wall() {
        let mut w = World::new(143);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..12 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 3..6 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 7..10 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // x=6 stays stone — a wall between two chambers.
        add_steam(&mut w, 4, 3, 200);
        assert!(steam_pressure_norm(&w, 4, 3) > 0.05);
        assert_eq!(
            steam_pressure_norm(&w, 8, 3),
            0.0,
            "pressure must not punch through the stone wall"
        );
        // Dry stone in the wall stays dark (the old column walk lit it).
        assert_eq!(
            steam_pressure_norm(&w, 6, 3),
            0.0,
            "dry impermeable wall must not inherit cavity fill"
        );
    }

    #[test]
    fn steam_pressure_does_not_paint_a_sky_candle() {
        let mut w = World::new(147);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..9 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 4..8 {
            for y in 2..5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Shaft opening on a confined roof cell — lip only, no sky walk.
        w.set_cell(5, 5, Cell::air());
        for y in 6..14 {
            w.set_cell(5, y, Cell::air());
        }
        add_steam(&mut w, 6, 3, 200);
        assert!(steam_pressure_norm(&w, 6, 3) > 0.1);
        assert!(
            steam_pressure_norm(&w, 5, 4) > 0.0,
            "the vent lip may inherit"
        );
        assert_eq!(
            steam_pressure_norm(&w, 5, 10),
            0.0,
            "free sky above the vent must not grow a candle"
        );
    }

    #[test]
    fn steam_pressure_wets_the_facing_floor_not_dry_granite() {
        let mut w = World::new(145);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 3, Cell::air());
        w.set_cell(5, 4, Cell::air());
        add_steam(&mut w, 5, 4, 180);
        let mut floor = Cell::solid(MaterialId::Limestone);
        floor.sat = Sat(30);
        floor.pore = 160;
        w.set_cell(5, 2, floor);
        assert!(
            steam_pressure_norm(&w, 5, 2) > 0.05,
            "wet limestone floor facing the pocket must inherit fill"
        );
        assert_eq!(
            steam_pressure_norm(&w, 4, 3),
            0.0,
            "dry stone beside the pocket must stay dark"
        );
    }

    #[test]
    fn boil_hot_air_finds_hot_tile_in_mostly_cold_chunk() {
        // 64×64 chunks are mostly cold above the isotherm; the boil
        // walk must still flash a single hot confined pocket.
        let mut w = World::new(131);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 40..48 {
            for y in 40..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 41..47 {
            for y in 41..46 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w.set_cell(44, 41, Cell::water());
        assert!(void_is_confined(&w, 44, 41));
        let mut temp = temp_fill(&w, 12.0);
        let (hx, hy) = temp.tile_of(44, 41);
        temp.set_tile_c(hx, hy, 118.0);
        let before = sat_totals(&w).cell_total;
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut temp, &SteamConfig::default());
        assert!(
            steam_total(&w) > 0,
            "hot confined water in a cold chunk must still boil"
        );
        assert_eq!(sat_totals(&w).cell_total, before, "tile-skip boil is mass-flat");
    }

    #[test]
    fn steam_amt_topk_keeps_highest_then_earliest_ties() {
        let mut top = SteamAmtTopK::new(3);
        top.push(0, 0, 10);
        top.push(1, 0, 40);
        top.push(2, 0, 10);
        top.push(3, 0, 30);
        top.push(4, 0, 40);
        top.push(5, 0, 20);
        assert_eq!(
            top.into_sorted_jobs(),
            vec![(1, 0, 40), (4, 0, 40), (3, 0, 30)],
            "top-K must match collect-all + stable amount sort"
        );
    }

    #[test]
    fn boil_hot_pores_finds_hot_tile_in_mostly_cold_chunk() {
        let mut w = World::new(132);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 8..16 {
            for y in 48..56 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 9..15 {
            for y in 49..54 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(crate::cell::water_capacity(MaterialId::Sand));
        w.set_cell(12, 49, sand);
        let mut temp = temp_fill(&w, 12.0);
        let (hx, hy) = temp.tile_of(12, 49);
        temp.set_tile_c(hx, hy, 140.0);
        let before = sat_totals(&w).cell_total;
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut temp, &SteamConfig::default());
        assert!(
            steam_total(&w) > 0,
            "hot wet pores in a cold chunk must still flash"
        );
        assert!(
            w.get_cell(12, 49).unwrap().sat.0 < crate::cell::water_capacity(MaterialId::Sand)
        );
        assert_eq!(sat_totals(&w).cell_total, before, "tile-skip pore boil is mass-flat");
    }

    #[test]
    fn boil_hot_pores_prefers_superheat_over_wet_lukewarm() {
        // When Tab boil drops, a wet mountain lights up. Sat-only ranking
        // spends the ~16-job budget on lukewarm swamps and starves an 85 °C
        // core. Superheat ranking must flash the hot cell first.
        let mut w = World::new(141);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 4..28 {
            for y in 40..56 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for x in 10..16 {
            for y in 46..52 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let sand_cap = crate::cell::water_capacity(MaterialId::Sand);
        let mut luke_n = 0u32;
        for x in 6..10 {
            for y in 42..54 {
                let mut sand = Cell::solid(MaterialId::Sand);
                sand.sat = Sat(sand_cap);
                w.set_cell(x, y, sand);
                luke_n += 1;
            }
        }
        assert!(luke_n > 16, "need more lukewarm cells than the job budget");
        let mut hot_sand = Cell::solid(MaterialId::Sand);
        hot_sand.sat = Sat(8);
        // Adjacent to the cave so a selected job can actually seat vapour.
        w.set_cell(16, 48, hot_sand);
        let mut temp = temp_fill(&w, 61.0);
        let (hx, hy) = temp.tile_of(16, 48);
        temp.set_tile_c(hx, hy, 85.0);
        let sat0 = w.get_cell(16, 48).unwrap().sat.0;
        let before = sat_totals(&w).cell_total;
        w.tick = STEAM_EVERY;
        apply_steam(
            &mut w,
            &mut temp,
            &SteamConfig {
                boil_point_c: 60.0,
                max_escapes_per_tick: 1,
                ..SteamConfig::default()
            },
        );
        let sat1 = w.get_cell(16, 48).unwrap().sat.0;
        assert!(
            sat1 < sat0,
            "85 °C drier core must flash before 61 °C swamps ({sat0}→{sat1}, steam {})",
            steam_total(&w)
        );
        assert_eq!(sat_totals(&w).cell_total, before, "ranked pore boil is mass-flat");
    }

    #[test]
    fn overhang_open_lake_is_not_a_pressure_boiler() {
        // Solid within 48 cells above (cliff slope) but a side column to sky.
        let mut w = World::new(201);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=4 {
                let mut lake = Cell::air();
                lake.sat = Sat(255);
                w.set_cell(x, y, lake);
            }
        }
        // Overhang over x=2..6; x=8 is a clear sky column.
        for x in 2..=6 {
            w.set_cell(x, 10, Cell::solid(MaterialId::Stone));
        }
        assert!(void_is_confined(&w, 4, 4), "overhang roofs the column probe");
        assert!(
            air_void_open_to_sky(&w, 4, 4),
            "lake must reach sky around the overhang"
        );
        assert!(
            !steam_is_pressure_confined(&w, 4, 4),
            "laterally open lake is not a boiler"
        );
        let mut temp = temp_fill(&w, 130.0);
        let before_steam = steam_total(&w);
        w.tick = STEAM_EVERY;
        apply_steam(&mut w, &mut temp, &SteamConfig::default());
        assert_eq!(
            steam_total(&w),
            before_steam,
            "open overhang lake must not flash into World.steam"
        );
    }

    #[test]
    fn reverse_push_discharges_into_roofed_standing_water() {
        // Underwater vent: full pool under a rock lid, no sky path.
        let mut w = World::new(202);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..10 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut rock = Cell::solid(MaterialId::Limestone);
        rock.sat = Sat(200);
        rock.pore = 180;
        w.set_cell(5, 2, rock);
        for y in 2..=4 {
            let mut lake = Cell::air();
            lake.sat = Sat(255);
            w.set_cell(4, y, lake);
        }
        // Solid lid — no sky column.
        for x in 0..10 {
            w.set_cell(x, 7, Cell::solid(MaterialId::Stone));
        }
        assert!(!air_void_open_to_sky(&w, 4, 3));
        let src0 = w.get_cell(5, 2).unwrap().sat.0;
        let sat0 = sat_totals(&w).cell_total;
        let mut temp = temp_fill(&w, 80.0);
        let (moved, dest) = reverse_push_pore_water_to(&mut w, &mut temp, 5, 2, 80);
        assert!(moved > 0, "roofed standing water must accept a hydrothermal vent");
        assert!(
            dest.is_some_and(|(x, _)| x == 4),
            "must discharge into the pool, got {dest:?}"
        );
        assert!(w.get_cell(5, 2).unwrap().sat.0 < src0);
        assert_eq!(sat_totals(&w).cell_total, sat0, "underwater vent is mass-flat");
    }

    #[test]
    fn brick_vent_clears_surface_cavity_pressure() {
        let mut w = World::new(203);
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
        // Surface vent cell
        w.set_cell(4, 5, Cell::air());
        for y in 6..12 {
            w.set_cell(4, y, Cell::air());
        }
        let _ = add_steam(&mut w, 4, 3, 200);
        let _ = add_steam(&mut w, 4, 5, 80);
        assert!(steam_at(&w, 4, 5) > 0);
        w.set_cell(4, 5, Cell::solid(MaterialId::Stone));
        assert_eq!(
            steam_at(&w, 4, 5),
            0,
            "bricking the vent must evict steam from that cell"
        );
        let (p, kind) = cell_pressure_norm(&w, 4, 5, 20.0);
        assert!(
            kind != CellPressureKind::Cavity || p < 0.03,
            "bricked rock must not keep a cavity overlay ({kind:?} p={p})"
        );
    }

}
