//! Cell-resolution steam pipe.
//!
//! Temperature only ignites. Steam lives on the water grid as `expand`
//! units (1 sat = `expand` units). Seepage still moves liquid. Face mix
//! decides collapse vs live displacement. See `docs/VOXEL_PIPE.md`.

use wk_material::MaterialId;

use crate::cell::{is_grain, water_capacity_cell, Cell, Sat};
use crate::displace::park_orphan_water;
use crate::fasthash::{FxHashMap, FxHashSet};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::mineral::{carry_with_water, dissolved_at, precipitate_vent_mouth};
use crate::steam::{
    choke_leak_mass, void_is_confined, SteamConfig, BOIL_POINT_C, PHASE_EXPANSION_DRIVE,
    STEAM_EVERY,
};
use crate::temperature::Temperature;
use crate::worldgen::{live_skin_y, live_surface_y, LIVE_SURFACE_SEARCH};

/// Incoming puff sees one side of the cell, not the whole pond.
pub const PIPE_SIDES: u32 = 4;
pub const PIPE_MAX_LEN: usize = 512;
pub const PIPE_STROKE_DEFAULT: u32 = 1400;
const PIPE_MAX_MAINS: usize = 8;
const PIPE_MAX_FEEDERS: usize = 24;
const PIPE_CLAIM_BUDGET: usize = 32_768;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PipeSeat {
    pub live: u32,
    pub residual: u32,
    pub t_c: f32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PipePath {
    pub root: (i32, i32),
    pub cells: Vec<(i32, i32)>,
    pub mouth: (i32, i32),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum HopKind {
    /// mix_T < boil. Steam becomes water / residual. No live displace.
    Collapse {
        minted_sat: u8,
        residual: u32,
        liquid_out: u8,
        mix_t: f32,
    },
    /// mix_T ≥ boil and dest has spare occupancy. Steam parks beside liquid.
    Park { parked: u32, mix_t: f32 },
    /// mix_T ≥ boil and dest is full. Steam takes seats; liquid continues.
    Displace {
        steam_stays: u32,
        liquid_out: u8,
        steam_out: u32,
        mix_t: f32,
    },
}

#[derive(Default)]
struct PipeMemo {
    world_id: u64,
    /// Pipe motor has run on this world — leftover overlay stays off.
    active: bool,
    mains: Vec<PipePath>,
    feeders: Vec<PipePath>,
    claimed: FxHashSet<(i32, i32)>,
}

thread_local! {
    static PIPE_MEMO: std::cell::RefCell<PipeMemo> =
        std::cell::RefCell::new(PipeMemo::default());
}

#[inline]
pub fn pipe_expand(world: &World) -> u16 {
    world.pipe_expand.max(1)
}

#[inline]
pub fn pipe_live_at(world: &World, gx: i32, gy: i32) -> u32 {
    world
        .pipe_steam
        .get(&(world.wrap_x(gx), gy))
        .copied()
        .unwrap_or(0)
}

#[inline]
pub fn pipe_res_at(world: &World, gx: i32, gy: i32) -> u32 {
    world
        .pipe_res
        .get(&(world.wrap_x(gx), gy))
        .copied()
        .unwrap_or(0)
}

#[inline]
pub fn pipe_steam_t_at(world: &World, gx: i32, gy: i32) -> f32 {
    world
        .pipe_steam_t
        .get(&(world.wrap_x(gx), gy))
        .copied()
        .unwrap_or(0.0)
}

/// Integer sat equivalent of live + residual units.
pub fn pipe_mass_sat(world: &World) -> i64 {
    let exp = pipe_expand(world) as i64;
    let units = pipe_units_total(world);
    units / exp
}

pub fn pipe_units_total(world: &World) -> i64 {
    let live: i64 = world.pipe_steam.values().map(|&v| v as i64).sum();
    let res: i64 = world.pipe_res.values().map(|&v| v as i64).sum();
    live + res
}

pub fn pipe_path_stats(world: &World) -> (usize, usize) {
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        if memo.world_id != world.chunk_cache_id.get() {
            return (0, 0);
        }
        let mut cells = memo.claimed.clone();
        for path in memo.mains.iter().chain(memo.feeders.iter()) {
            cells.extend(path.cells.iter().copied());
        }
        (memo.mains.len() + memo.feeders.len(), cells.len())
    })
}

pub fn pipe_painting(world: &World) -> bool {
    if !world.pipe_steam.is_empty() || !world.pipe_res.is_empty() {
        return true;
    }
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == world.chunk_cache_id.get() && memo.active
    })
}

fn on_pipe_path(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == world.chunk_cache_id.get()
            && memo
                .mains
                .iter()
                .chain(memo.feeders.iter())
                .any(|p| p.cells.iter().any(|&c| c == (gx, gy)))
    })
}

/// P overlay: locked straw, live puff, and claimed boiling cells.
pub fn pipe_overlay_pack(world: &World, gx: i32, gy: i32) -> Option<f32> {
    let gx = world.wrap_x(gx);
    let live = pipe_live_at(world, gx, gy);
    if live > 0 {
        let cap = world
            .get_cell(gx, gy)
            .map(|c| water_capacity_cell(c, &world.hydro).max(1) as u32)
            .unwrap_or(1);
        let drive = (live as f32 / (cap as f32 * 8.0).max(1.0)).clamp(0.0, 1.0);
        return Some((0.42 + drive * 0.50).clamp(0.42, 0.92));
    }
    if on_pipe_path(world, gx, gy) {
        return Some(0.30);
    }
    if on_pipe_boiler(world, gx, gy) {
        return Some(0.24);
    }
    None
}

fn on_pipe_boiler(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == world.chunk_cache_id.get() && memo.claimed.contains(&(gx, gy))
    })
}

pub fn face_water_units(liquid: u8, expand: u16, sides: u32) -> f32 {
    let sides = sides.max(1) as f32;
    (liquid as f32) * (expand.max(1) as f32) / sides
}

/// Mix T on the incoming face (`heat / sides`).
pub fn face_mix_t(
    liquid: u8,
    t_water: f32,
    steam: u32,
    t_steam: f32,
    expand: u16,
    sides: u32,
) -> f32 {
    let fw = face_water_units(liquid, expand, sides);
    let s = steam as f32;
    let den = fw + s;
    if den <= 0.0 {
        return t_water;
    }
    (fw * t_water + s * t_steam) / den
}

/// Classify a steam packet arriving at a dest seat. Mix before displace.
pub fn classify_hop(
    dest_liquid: u8,
    dest_cap: u8,
    dest_live: u32,
    dest_t: f32,
    steam: u32,
    steam_t: f32,
    expand: u16,
    boil: f32,
    sides: u32,
) -> HopKind {
    if steam == 0 {
        return HopKind::Park {
            parked: 0,
            mix_t: dest_t,
        };
    }
    let mix_t = face_mix_t(dest_liquid, dest_t, steam, steam_t, expand, sides);
    if mix_t < boil {
        let exp = expand.max(1) as u32;
        let minted = (steam / exp).min(u32::from(u8::MAX)) as u8;
        let residual = steam % exp;
        let sum = dest_liquid.saturating_add(minted);
        let liquid_out = if sum > dest_cap { sum - dest_cap } else { 0 };
        return HopKind::Collapse {
            minted_sat: minted,
            residual,
            liquid_out,
            mix_t,
        };
    }
    let occ = dest_liquid as u32 + dest_live;
    let cap = dest_cap as u32;
    let spare = cap.saturating_sub(occ);
    if steam <= spare {
        return HopKind::Park {
            parked: steam,
            mix_t,
        };
    }
    let parked = spare;
    let remaining = steam - spare;
    let liquid_out = dest_liquid.min(remaining.min(u32::from(u8::MAX)) as u8);
    let steam_stays = parked + u32::from(liquid_out);
    let steam_out = remaining.saturating_sub(u32::from(liquid_out));
    HopKind::Displace {
        steam_stays,
        liquid_out,
        steam_out,
        mix_t,
    }
}

/// Higher = more open. Air > snow/water/ice > loose > sand > gravel > stone > bedrock.
pub fn openness_rank(cell: Cell) -> u8 {
    match cell.material {
        MaterialId::Bedrock => 0,
        MaterialId::Air => 90,
        MaterialId::Water => 82,
        MaterialId::Snow => 78,
        MaterialId::Ice => 74,
        MaterialId::LooseRock | MaterialId::LooseLimestone => 60,
        MaterialId::Sand => 52,
        MaterialId::Gravel => 46,
        MaterialId::Soil | MaterialId::Organic => 40,
        MaterialId::Sandstone | MaterialId::Conglomerate | MaterialId::Flowstone => 32,
        MaterialId::Limestone => 28,
        MaterialId::Clay => 22,
        MaterialId::Stone => 18,
        _ => {
            if is_grain(cell.material) {
                50
            } else {
                16
            }
        }
    }
}

fn is_pipe_mouth(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    cell.material == MaterialId::Air && !void_is_confined(world, gx, gy)
}

/// Greedy most-open walk that still reduces distance to the column surface.
pub fn walk_pipe(world: &World, root: (i32, i32)) -> PipePath {
    let (rx, ry) = (world.wrap_x(root.0), root.1);
    let hint = live_surface_y(world, rx, ry, LIVE_SURFACE_SEARCH);
    let mut cells = vec![(rx, ry)];
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert((rx, ry));
    let mut cur = (rx, ry);
    for _ in 0..PIPE_MAX_LEN {
        let Some(here) = world.get_cell(cur.0, cur.1) else {
            break;
        };
        if is_pipe_mouth(world, cur.0, cur.1, here) && cells.len() > 1 {
            break;
        }
        let here_dist = (hint - cur.1).abs();
        let mut best: Option<(i32, u8, i32, i32, i32)> = None;
        for (dx, dy) in [
            (0, 1),
            (-1, 1),
            (1, 1),
            (-1, 0),
            (1, 0),
            (0, -1),
            (-1, -1),
            (1, -1),
        ] {
            let nx = world.wrap_x(cur.0 + dx);
            let ny = cur.1 + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            let Some(n) = world.get_cell(nx, ny) else {
                continue;
            };
            let rank = openness_rank(n);
            if rank == 0 {
                continue;
            }
            let dist = (hint - ny).abs();
            if dist > here_dist && !is_pipe_mouth(world, nx, ny, n) {
                continue;
            }
            let up = if dy > 0 { 40 } else { 0 };
            let score = (rank as i32) * 1000 - dist + up;
            if best.map(|(s, _, _, _, _)| score > s).unwrap_or(true) {
                best = Some((score, rank, dist, nx, ny));
            }
        }
        let Some((_, _, _, nx, ny)) = best else {
            break;
        };
        cells.push((nx, ny));
        cur = (nx, ny);
        if world
            .get_cell(nx, ny)
            .is_some_and(|c| is_pipe_mouth(world, nx, ny, c))
        {
            break;
        }
    }
    let mouth = *cells.last().unwrap_or(&(rx, ry));
    PipePath {
        root: (rx, ry),
        cells,
        mouth,
    }
}

/// Greedy most-open walk that closes on `targets` (another reservoir / straw).
fn walk_toward(world: &World, from: (i32, i32), targets: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let (rx, ry) = (world.wrap_x(from.0), from.1);
    if targets.is_empty() {
        return vec![(rx, ry)];
    }
    let nearest = |p: (i32, i32)| {
        targets
            .iter()
            .map(|&t| wrap_chebyshev(world, p, t))
            .min()
            .unwrap_or(0)
    };
    let mut cells = vec![(rx, ry)];
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert((rx, ry));
    let mut cur = (rx, ry);
    for _ in 0..PIPE_MAX_LEN {
        if nearest(cur) <= 1 {
            break;
        }
        let here = nearest(cur);
        let mut best: Option<(i32, i32, i32)> = None;
        for (dx, dy) in [
            (0, 1),
            (-1, 1),
            (1, 1),
            (-1, 0),
            (1, 0),
            (0, -1),
            (-1, -1),
            (1, -1),
        ] {
            let nx = world.wrap_x(cur.0 + dx);
            let ny = cur.1 + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            let Some(n) = world.get_cell(nx, ny) else {
                continue;
            };
            let rank = openness_rank(n);
            if rank == 0 {
                continue;
            }
            let dist = nearest((nx, ny));
            if dist > here {
                continue;
            }
            let score = (rank as i32) * 1000 - dist;
            if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                best = Some((score, nx, ny));
            }
        }
        let Some((_, nx, ny)) = best else {
            break;
        };
        cells.push((nx, ny));
        cur = (nx, ny);
    }
    cells
}

fn set_live(world: &mut World, gx: i32, gy: i32, live: u32, t_c: f32) {
    let key = (world.wrap_x(gx), gy);
    if live == 0 {
        world.pipe_steam.remove(&key);
        world.pipe_steam_t.remove(&key);
    } else {
        world.pipe_steam.insert(key, live);
        world.pipe_steam_t.insert(key, t_c);
    }
}

fn add_residual(world: &mut World, gx: i32, gy: i32, add: u32) {
    if add == 0 {
        return;
    }
    let key = (world.wrap_x(gx), gy);
    let e = world.pipe_res.entry(key).or_insert(0);
    *e = e.saturating_add(add);
}

fn take_live(world: &mut World, gx: i32, gy: i32, want: u32) -> (u32, f32) {
    let t = pipe_steam_t_at(world, gx, gy);
    let live = pipe_live_at(world, gx, gy);
    let take = live.min(want);
    set_live(world, gx, gy, live - take, t);
    (take, t)
}

fn add_sat(world: &mut World, gx: i32, gy: i32, add: u8) -> u8 {
    if add == 0 {
        return 0;
    }
    let Some(mut cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    let cap = water_capacity_cell(cell, &world.hydro);
    let room = cap.saturating_sub(cell.sat.0);
    let put = add.min(room);
    cell.sat = Sat(cell.sat.0.saturating_add(put));
    world.set_cell(gx, gy, cell);
    put
}

fn take_sat(world: &mut World, gx: i32, gy: i32, want: u8) -> u8 {
    if want == 0 {
        return 0;
    }
    let Some(mut cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    let take = cell.sat.0.min(want);
    cell.sat = Sat(cell.sat.0.saturating_sub(take));
    world.set_cell(gx, gy, cell);
    take
}

fn blend_tile_t(temp: &mut Temperature, gx: i32, gy: i32, mix_t: f32) {
    let (hx, hy) = temp.tile_of(gx, gy);
    let t = temp.at_cell(gx, gy);
    if !t.is_finite() || !mix_t.is_finite() {
        return;
    }
    // Unseen faces keep T; one face is mix_T.
    let next = (t * 3.0 + mix_t) / 4.0;
    temp.set_tile_c(hx, hy, next);
}

/// Weak heat dye up the straw. Does not mint T; only pulls a cool tile
/// a little toward the packet.
fn couple_path_heat(temp: &mut Temperature, gx: i32, gy: i32, steam_t: f32) {
    if !steam_t.is_finite() {
        return;
    }
    let (hx, hy) = temp.tile_of(gx, gy);
    let t = temp.at_cell(gx, gy);
    if !t.is_finite() || steam_t <= t + 0.4 {
        return;
    }
    temp.set_tile_c(hx, hy, t + (steam_t - t) * 0.16);
}

/// Flash liquid on a cell into live steam units. Mass-flat.
pub fn pipe_flash(world: &mut World, gx: i32, gy: i32, t_c: f32, expand: u16) -> u32 {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    let paid = cell.sat.0;
    if paid == 0 {
        return 0;
    }
    let units = paid as u32 * expand.max(1) as u32;
    let _ = take_sat(world, gx, gy, paid);
    let already = pipe_live_at(world, gx, gy);
    set_live(world, gx, gy, already.saturating_add(units), t_c);
    world.pipe_expand = expand.max(1);
    units
}

fn apply_arrival(
    world: &mut World,
    temp: &mut Temperature,
    dest: (i32, i32),
    steam: u32,
    steam_t: f32,
    expand: u16,
    boil: f32,
    sides: u32,
) -> (u32, u8, f32) {
    let (dx, dy) = (world.wrap_x(dest.0), dest.1);
    let Some(cell) = world.get_cell(dx, dy) else {
        add_residual(world, dx, dy, steam);
        return (0, 0, steam_t);
    };
    let cap = water_capacity_cell(cell, &world.hydro);
    let dest_t = temp.at_cell(dx, dy);
    let dest_live = pipe_live_at(world, dx, dy);
    let hop = classify_hop(
        cell.sat.0, cap, dest_live, dest_t, steam, steam_t, expand, boil, sides,
    );
    match hop {
        HopKind::Collapse {
            minted_sat,
            residual,
            liquid_out,
            mix_t,
        } => {
            let keep = minted_sat.saturating_sub(liquid_out);
            let _ = add_sat(world, dx, dy, keep);
            add_residual(world, dx, dy, residual);
            blend_tile_t(temp, dx, dy, mix_t);
            (0, liquid_out, mix_t)
        }
        HopKind::Park { parked, mix_t } => {
            set_live(
                world,
                dx,
                dy,
                dest_live.saturating_add(parked),
                if dest_live == 0 {
                    steam_t
                } else {
                    (pipe_steam_t_at(world, dx, dy) + steam_t) * 0.5
                },
            );
            blend_tile_t(temp, dx, dy, mix_t);
            (0, 0, steam_t)
        }
        HopKind::Displace {
            steam_stays,
            liquid_out,
            steam_out,
            mix_t,
        } => {
            if liquid_out > 0 {
                let _ = take_sat(world, dx, dy, liquid_out);
            }
            set_live(
                world,
                dx,
                dy,
                dest_live.saturating_add(steam_stays),
                steam_t,
            );
            blend_tile_t(temp, dx, dy, mix_t);
            (steam_out, liquid_out, steam_t)
        }
    }
}

fn deliver_liquid(world: &mut World, from: (i32, i32), dest: (i32, i32), amt: u8) -> u8 {
    if amt == 0 {
        return 0;
    }
    let donor = world
        .get_cell(from.0, from.1)
        .map(|c| c.sat.0.saturating_add(amt))
        .unwrap_or(amt);
    let put = add_sat(world, dest.0, dest.1, amt);
    if put > 0 {
        carry_with_water(world, from, dest, put, donor);
    }
    amt.saturating_sub(put)
}

/// One stroke along a path. Mix before displace on every hop.
pub fn pulse_path(
    world: &mut World,
    temp: &mut Temperature,
    path: &PipePath,
    stroke: u32,
    expand: u16,
    boil: f32,
    sides: u32,
    humidity: Option<&mut Humidity>,
) {
    if path.cells.len() < 2 || stroke == 0 {
        return;
    }
    let root = path.cells[0];
    let (mut steam, mut steam_t) = take_live(world, root.0, root.1, stroke);
    if steam == 0 {
        return;
    }
    let dye_t = steam_t;
    let mut liquid = 0u8;
    let mut liquid_from = root;
    for &dest in &path.cells[1..] {
        if liquid > 0 {
            liquid = deliver_liquid(world, liquid_from, dest, liquid);
            if liquid == 0 {
                liquid_from = dest;
            }
        }
        if steam > 0 {
            let (steam_out, liquid_out, t_out) =
                apply_arrival(world, temp, dest, steam, steam_t, expand, boil, sides);
            steam = steam_out;
            steam_t = t_out;
            if liquid_out > 0 {
                liquid = liquid.saturating_add(liquid_out);
                liquid_from = dest;
            }
        }
        couple_path_heat(temp, dest.0, dest.1, dye_t);
    }
    if steam > 0 {
        let mouth = path.mouth;
        if world
            .get_cell(mouth.0, mouth.1)
            .is_some_and(|c| is_pipe_mouth(world, mouth.0, mouth.1, c))
        {
            leak_pipe_mouth(world, temp, humidity, mouth, steam, expand, boil);
        } else {
            add_residual(world, mouth.0, mouth.1, steam);
        }
    }
    if liquid > 0 {
        let left = deliver_liquid(world, liquid_from, path.mouth, liquid);
        if left > 0 {
            add_residual(
                world,
                path.mouth.0,
                path.mouth.1,
                left as u32 * expand.max(1) as u32,
            );
        }
    }
    deposit_pipe_mouth(world, path);
}

/// March existing pore water one hop toward the path mouth (main line
/// or sky). Mass-flat: unplaced sat returns to the donor.
fn pump_water_along(world: &mut World, path: &PipePath, max_sat: u8) {
    if max_sat == 0 || path.cells.len() < 2 {
        return;
    }
    for w in path.cells.windows(2) {
        let from = w[0];
        let dest = w[1];
        let Some(cell) = world.get_cell(from.0, from.1) else {
            continue;
        };
        if cell.sat.0 == 0 || cell.material == MaterialId::Air {
            continue;
        }
        let took = take_sat(world, from.0, from.1, cell.sat.0.min(max_sat));
        if took == 0 {
            continue;
        }
        let left = deliver_liquid(world, from, dest, took);
        if left > 0 {
            let _ = add_sat(world, from.0, from.1, left);
        }
    }
}

fn stroke_sat(stroke: u32, expand: u16) -> u8 {
    (stroke / u32::from(expand.max(1))).clamp(1, 255) as u8
}

/// Open-sky mouth: mass only. Hot → sky H. Cool → distilled liquid at the lip.
fn leak_pipe_mouth(
    world: &mut World,
    temp: &Temperature,
    humidity: Option<&mut Humidity>,
    mouth: (i32, i32),
    steam: u32,
    expand: u16,
    boil: f32,
) {
    let exp = expand.max(1) as u32;
    let (mx, my) = (world.wrap_x(mouth.0), mouth.1);
    let (parked, _) = take_live(world, mx, my, u32::MAX);
    let steam = steam.saturating_add(parked);
    let leak_mass = u32::from(choke_leak_mass(steam, expand, 1));
    let leak_units = (leak_mass * exp).min(steam);
    let keep = steam.saturating_sub(leak_units);
    if keep > 0 {
        add_residual(world, mx, my, keep);
    }
    let mut mass = leak_units / exp;
    let frac = leak_units % exp;
    if frac > 0 {
        add_residual(world, mx, my, frac);
    }
    if mass == 0 {
        return;
    }
    let mouth_t = temp.at_cell(mx, my);
    if mouth_t >= boil {
        if let Some(h) = humidity {
            let accepted = h.try_add(mx, my, mass as f32).round().max(0.0) as u32;
            mass = mass.saturating_sub(accepted);
        }
    }
    if mass > 0 {
        mass = park_orphan_water(world, mx, my, mass);
    }
    if mass > 0 {
        add_residual(world, mx, my, mass * exp);
    }
}

/// Depressurise at the mouth only. Never sinter cells on the live lumen.
fn deposit_pipe_mouth(world: &mut World, path: &PipePath) {
    let (mx, my) = (world.wrap_x(path.mouth.0), path.mouth.1);
    if dissolved_at(world, mx, my) == 0 {
        return;
    }
    let _ = precipitate_vent_mouth(world, mx, my, 0.55);
}

/// Pool residuals inside each 4×4 heat tile into sat when ≥ expand.
pub fn pool_residuals(world: &mut World, expand: u16) {
    let exp = expand.max(1) as u32;
    if world.pipe_res.is_empty() {
        return;
    }
    let mut buckets: FxHashMap<(i32, i32), Vec<(i32, i32, u32)>> = FxHashMap::default();
    for (&(gx, gy), &amt) in &world.pipe_res {
        if amt == 0 {
            continue;
        }
        buckets
            .entry((gx.div_euclid(4), gy.div_euclid(4)))
            .or_default()
            .push((gx, gy, amt));
    }
    for cells in buckets.into_values() {
        let sum: u32 = cells.iter().map(|c| c.2).sum();
        if sum < exp {
            continue;
        }
        let mint = (sum / exp).min(u32::from(u8::MAX)) as u8;
        let keep = sum % exp;
        let park = cells
            .iter()
            .max_by_key(|c| c.2)
            .map(|&(x, y, _)| (x, y))
            .unwrap_or((cells[0].0, cells[0].1));
        for &(gx, gy, _) in &cells {
            world.pipe_res.remove(&(gx, gy));
        }
        if keep > 0 {
            world.pipe_res.insert(park, keep);
        }
        let _ = add_sat(world, park.0, park.1, mint);
    }
}

fn bind_memo(world: &World) {
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        if memo.world_id != world.chunk_cache_id.get() {
            memo.world_id = world.chunk_cache_id.get();
            memo.mains.clear();
            memo.feeders.clear();
            memo.claimed.clear();
        }
        memo.active = true;
    });
}

fn wrap_dx(world: &World, ax: i32, bx: i32) -> i32 {
    let d = (ax - bx).abs();
    match world.wrap_width {
        Some(w) if w > 0 => d.min(w - d),
        _ => d,
    }
}

fn wrap_chebyshev(world: &World, a: (i32, i32), b: (i32, i32)) -> i32 {
    wrap_dx(world, a.0, b.0).max((a.1 - b.1).abs())
}

fn free_surface_y(world: &World, gx: i32, gy: i32) -> i32 {
    let rock = live_surface_y(world, gx, gy, LIVE_SURFACE_SEARCH);
    live_skin_y(world, gx, rock)
}

fn surface_dist(world: &World, p: (i32, i32)) -> i32 {
    (free_surface_y(world, p.0, p.1) - p.1).max(0)
}

fn claimed_cells() -> FxHashSet<(i32, i32)> {
    PIPE_MEMO.with(|slot| slot.borrow().claimed.clone())
}

fn dist_to_path(world: &World, p: (i32, i32), path: &PipePath) -> i32 {
    path.cells
        .iter()
        .map(|&c| wrap_chebyshev(world, p, c))
        .min()
        .unwrap_or(i32::MAX)
}

fn nearest_main_idx(world: &World, p: (i32, i32), mains: &[PipePath]) -> Option<usize> {
    mains
        .iter()
        .enumerate()
        .min_by_key(|(_, m)| dist_to_path(world, p, m))
        .map(|(i, _)| i)
}

/// Closer to the locked straw than to this cell's free surface.
fn should_feed_main(world: &World, boiler: (i32, i32), main: &PipePath) -> bool {
    dist_to_path(world, boiler, main) < surface_dist(world, boiler)
}

fn make_feeder(world: &World, root: (i32, i32), main: &PipePath) -> PipePath {
    let cells = walk_toward(world, root, &main.cells);
    let mouth = *cells.last().unwrap_or(&root);
    PipePath {
        root: (world.wrap_x(root.0), root.1),
        cells,
        mouth,
    }
}

fn rebuild_claimed(world: &World, temp: &Temperature, boil: f32) {
    let paths = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.mains
            .iter()
            .chain(memo.feeders.iter())
            .cloned()
            .collect::<Vec<_>>()
    });
    let mut taken = FxHashSet::default();
    for path in &paths {
        claim_wet_hot_body(world, temp, path.root.0, path.root.1, boil, &mut taken);
        for &(x, y) in &path.cells {
            claim_wet_hot_body(world, temp, x, y, boil, &mut taken);
        }
    }
    PIPE_MEMO.with(|slot| {
        slot.borrow_mut().claimed = taken;
    });
}

fn rewalk_network(world: &World) {
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        for main in &mut memo.mains {
            *main = walk_pipe(world, main.root);
        }
        memo.mains.retain(|p| p.cells.len() >= 2);
        if memo.mains.is_empty() && !memo.feeders.is_empty() {
            let first = memo.feeders.remove(0);
            memo.mains.push(walk_pipe(world, first.root));
            memo.mains.retain(|p| p.cells.len() >= 2);
        }
        let mains = memo.mains.clone();
        for feeder in &mut memo.feeders {
            if let Some(i) = nearest_main_idx(world, feeder.root, &mains) {
                *feeder = make_feeder(world, feeder.root, &mains[i]);
            }
        }
        memo.feeders.retain(|p| p.cells.len() >= 2);
    });
}

fn reflash_network(world: &mut World, temp: &Temperature, expand: u16, boil: f32) {
    let paths = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.mains
            .iter()
            .chain(memo.feeders.iter())
            .cloned()
            .collect::<Vec<_>>()
    });
    for path in paths {
        let Some((gx, gy, t)) = path.cells.iter().copied().find_map(|(gx, gy)| {
            let cell = world.get_cell(gx, gy)?;
            if cell.sat.0 == 0 || cell.material == MaterialId::Air {
                return None;
            }
            let t = temp.at_cell(gx, gy);
            (t >= boil).then_some((gx, gy, t))
        }) else {
            continue;
        };
        let _ = pipe_flash(world, gx, gy, t, expand);
    }
}

fn collect_boiler_cands(
    world: &World,
    temp: &Temperature,
    boil: f32,
    skip: &FxHashSet<(i32, i32)>,
    room: usize,
) -> Vec<(i32, i32, f32)> {
    if room == 0 {
        return Vec::new();
    }
    let tc = temp.tile_cols.max(1);
    let mut tiles: Vec<(i32, i32, f32)> = temp
        .cells
        .iter()
        .filter_map(|(&(hx, hy), &t)| (t >= boil).then_some((hx, hy, t)))
        .collect();
    tiles.sort_by_key(|&(hx, hy, _)| (hy, hx));
    let mut cands = Vec::new();
    let mut seen = skip.clone();
    for (hx, hy, t) in tiles {
        if cands.len() >= room {
            break;
        }
        for ly in 0..tc {
            for lx in 0..tc {
                let gx = world.wrap_x(hx * tc + lx);
                let gy = hy * tc + ly;
                if seen.contains(&(gx, gy)) {
                    continue;
                }
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cell.sat.0 == 0 || cell.material == MaterialId::Air {
                    continue;
                }
                cands.push((gx, gy, t));
                claim_wet_hot_body(world, temp, gx, gy, boil, &mut seen);
                if cands.len() >= room {
                    break;
                }
            }
            if cands.len() >= room {
                break;
            }
        }
    }
    cands
}

/// New boilers walk to the main straw when that is closer than the surface.
fn attach_new_boilers(world: &World, temp: &Temperature, boil: f32) {
    let (mains, feeders) = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        (memo.mains.clone(), memo.feeders.clone())
    });
    let room = (PIPE_MAX_MAINS - mains.len().min(PIPE_MAX_MAINS))
        + (PIPE_MAX_FEEDERS - feeders.len().min(PIPE_MAX_FEEDERS));
    let skip = claimed_cells();
    let cands = collect_boiler_cands(world, temp, boil, &skip, room);
    if cands.is_empty() {
        return;
    }
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        for (gx, gy, _) in cands {
            let feed = nearest_main_idx(world, (gx, gy), &memo.mains)
                .map(|i| memo.mains[i].clone());
            if let Some(main) = feed.as_ref() {
                if should_feed_main(world, (gx, gy), main)
                    && memo.feeders.len() < PIPE_MAX_FEEDERS
                {
                    memo.feeders.push(make_feeder(world, (gx, gy), main));
                    continue;
                }
            }
            if memo.mains.len() < PIPE_MAX_MAINS {
                let path = walk_pipe(world, (gx, gy));
                if path.cells.len() >= 2 {
                    memo.mains.push(path);
                }
            } else if memo.feeders.len() < PIPE_MAX_FEEDERS {
                if let Some(main) = feed.as_ref() {
                    memo.feeders.push(make_feeder(world, (gx, gy), main));
                }
            }
        }
    });
}

/// Mark the connected wet body at/above boil so one hill is one root.
fn claim_wet_hot_body(
    world: &World,
    temp: &Temperature,
    gx: i32,
    gy: i32,
    boil: f32,
    taken: &mut FxHashSet<(i32, i32)>,
) {
    let gx = world.wrap_x(gx);
    let Some(start) = world.get_cell(gx, gy) else {
        return;
    };
    if start.sat.0 == 0 || start.material == MaterialId::Air || temp.at_cell(gx, gy) < boil {
        return;
    }
    let mut stack = vec![(gx, gy)];
    let mut n = 0usize;
    while let Some((x, y)) = stack.pop() {
        if !taken.insert((x, y)) {
            continue;
        }
        n += 1;
        if n > PIPE_CLAIM_BUDGET {
            break;
        }
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = world.wrap_x(x + dx);
                let ny = y + dy;
                if taken.contains(&(nx, ny)) {
                    continue;
                }
                let Some(cell) = world.get_cell(nx, ny) else {
                    continue;
                };
                if cell.sat.0 == 0 || cell.material == MaterialId::Air {
                    continue;
                }
                if temp.at_cell(nx, ny) < boil {
                    continue;
                }
                stack.push((nx, ny));
            }
        }
    }
}

/// Rewalk, attach feeders, pulse steam + water. Off-beat only binds memo.
pub fn apply_pipe_motor(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    mut humidity: Option<&mut Humidity>,
) {
    if !cfg.enable_pipe {
        return;
    }
    let expand = cfg.phase_expansion_drive.max(1);
    let boil = if cfg.boil_point_c.is_finite() {
        cfg.boil_point_c
    } else {
        BOIL_POINT_C
    };
    let sides = u32::from(cfg.pipe_sides.max(1));
    let stroke = cfg.pipe_stroke.max(1);
    let beat = cfg.pipe_beat.max(1);
    world.pipe_expand = expand;
    bind_memo(world);
    if world.tick % beat != 0 {
        return;
    }
    rewalk_network(world);
    rebuild_claimed(world, temp, boil);
    attach_new_boilers(world, temp, boil);
    rewalk_network(world);
    rebuild_claimed(world, temp, boil);
    reflash_network(world, temp, expand, boil);
    let (feeders, mains) = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        (memo.feeders.clone(), memo.mains.clone())
    });
    let water = stroke_sat(stroke, expand);
    for path in feeders.iter().chain(mains.iter()) {
        pulse_path(
            world,
            temp,
            path,
            stroke,
            expand,
            boil,
            sides,
            humidity.as_deref_mut(),
        );
        pump_water_along(world, path, water);
    }
    pool_residuals(world, expand);
}

pub fn default_pipe_expand() -> u16 {
    PHASE_EXPANSION_DRIVE
}

pub fn default_pipe_beat() -> u64 {
    STEAM_EVERY
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::sat_totals;
    use crate::chunk::ChunkCoord;
    use crate::steam::PHASE_EXPANSION_DRIVE_MAX;

    const EXP: u16 = PHASE_EXPANSION_DRIVE_MAX;

    fn plot() -> World {
        let mut w = World::new(0x51CE);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.pipe_expand = EXP;
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    fn temp_at(world: &World, c: f32) -> Temperature {
        let mut t = Temperature::with_world_bounds(4, 0, 0, 64, 64, world.seed.0, 64, 20, false);
        for v in t.cells.values_mut() {
            *v = c;
        }
        t
    }

    #[test]
    fn face_mix_1000_into_cold_stone_collapses() {
        let t = face_mix_t(14, 20.0, 1000, 150.0, EXP, PIPE_SIDES);
        assert!((t - 42.03).abs() < 0.05, "mix_t={t}");
        let hop = classify_hop(14, 14, 0, 20.0, 1000, 150.0, EXP, 100.0, PIPE_SIDES);
        match hop {
            HopKind::Collapse {
                minted_sat,
                residual,
                liquid_out,
                ..
            } => {
                assert_eq!(minted_sat, 0);
                assert_eq!(residual, 1000);
                assert_eq!(liquid_out, 0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn face_mix_1400_into_80c_stone_is_condensate_marble() {
        let t = face_mix_t(14, 80.0, 1400, 150.0, EXP, PIPE_SIDES);
        assert!((t - 95.56).abs() < 0.05, "mix_t={t}");
        let hop = classify_hop(14, 14, 0, 80.0, 1400, 150.0, EXP, 100.0, PIPE_SIDES);
        match hop {
            HopKind::Collapse {
                minted_sat,
                residual,
                liquid_out,
                mix_t,
            } => {
                assert_eq!(minted_sat, 1);
                assert_eq!(residual, 0);
                assert_eq!(liquid_out, 1);
                assert!(mix_t < 100.0);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn face_mix_1400_into_86c_stone_lives() {
        let t = face_mix_t(14, 86.0, 1400, 150.0, EXP, PIPE_SIDES);
        assert!(t >= 100.0, "mix_t={t}");
        let hop = classify_hop(14, 14, 0, 86.0, 1400, 150.0, EXP, 100.0, PIPE_SIDES);
        match hop {
            HopKind::Displace {
                liquid_out,
                steam_out,
                steam_stays,
                ..
            } => {
                assert_eq!(liquid_out, 14);
                assert_eq!(steam_stays, 14);
                assert_eq!(steam_out, 1386);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn cold_sand_parks_then_collapses() {
        let hop = classify_hop(57, 99, 0, 10.0, 20, 115.0, EXP, 100.0, PIPE_SIDES);
        match hop {
            HopKind::Collapse {
                minted_sat,
                residual,
                liquid_out,
                mix_t,
            } => {
                assert_eq!(minted_sat, 0);
                assert_eq!(residual, 20);
                assert_eq!(liquid_out, 0);
                assert!((mix_t - 10.1).abs() < 0.1, "mix_t={mix_t}");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn flash_is_mass_flat() {
        let mut w = plot();
        let mut stone = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(stone, &w.hydro);
        stone.sat = Sat(cap.min(14));
        w.set_cell(4, 1, stone);
        let paid = w.get_cell(4, 1).unwrap().sat.0;
        let before = sat_totals(&w).cell_total;
        let units = pipe_flash(&mut w, 4, 1, 150.0, EXP);
        assert_eq!(units, paid as u32 * EXP as u32);
        assert_eq!(w.get_cell(4, 1).unwrap().sat.0, 0);
        assert_eq!(pipe_live_at(&w, 4, 1), units);
        assert_eq!(sat_totals(&w).cell_total, before);
    }

    #[test]
    fn collapse_1400_mints_one_sat_and_passes_liquid() {
        let mut w = plot();
        let mut b = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(b, &w.hydro);
        b.sat = Sat(cap);
        w.set_cell(4, 2, b);
        w.set_cell(4, 3, {
            let mut c = Cell::solid(MaterialId::Stone);
            let cap = water_capacity_cell(c, &w.hydro);
            c.sat = Sat(cap.saturating_sub(2));
            c
        });
        let mut a = Cell::solid(MaterialId::Stone);
        a.sat = Sat(1);
        w.set_cell(4, 1, a);
        let before = sat_totals(&w).cell_total;
        let _ = pipe_flash(&mut w, 4, 1, 150.0, EXP);
        // Force a 1400-unit puff (1 sat of steam).
        set_live(&mut w, 4, 1, 1400, 150.0);
        let mut hot = temp_at(&w, 80.0);
        let path = PipePath {
            root: (4, 1),
            cells: vec![(4, 1), (4, 2), (4, 3)],
            mouth: (4, 3),
        };
        pulse_path(&mut w, &mut hot, &path, 1400, EXP, 100.0, PIPE_SIDES, None);
        assert_eq!(pipe_live_at(&w, 4, 2), 0, "steam died on contact");
        let c_sat = w.get_cell(4, 3).unwrap().sat.0;
        assert!(c_sat > 0, "condensate marble reached C, sat={c_sat}");
        assert_eq!(sat_totals(&w).cell_total, before);
    }

    #[test]
    fn openness_prefers_air_over_stone() {
        assert!(openness_rank(Cell::air()) > openness_rank(Cell::solid(MaterialId::Gravel)));
        assert!(
            openness_rank(Cell::solid(MaterialId::Gravel))
                > openness_rank(Cell::solid(MaterialId::Stone))
        );
        assert_eq!(openness_rank(Cell::solid(MaterialId::Bedrock)), 0);
    }

    #[test]
    fn walk_climbs_to_open_sky() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..14 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for y in 1..10 {
            w.set_cell(5, y, {
                let mut c = Cell::solid(MaterialId::Gravel);
                c.sat = Sat(8);
                c
            });
        }
        for y in 10..14 {
            w.set_cell(5, y, Cell::air());
        }
        let path = walk_pipe(&w, (5, 1));
        assert!(path.cells.len() >= 3, "path={:?}", path.cells);
        let mouth = w.get_cell(path.mouth.0, path.mouth.1).unwrap();
        assert_eq!(mouth.material, MaterialId::Air);
    }

    #[test]
    fn pipe_motor_stays_on_a_path() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for y in 1..8 {
            w.set_cell(6, y, {
                let mut c = Cell::solid(MaterialId::Gravel);
                let cap = water_capacity_cell(c, &w.hydro);
                c.sat = Sat(cap.min(10));
                c
            });
        }
        for y in 8..12 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_at(&w, 20.0);
        for y in 1..8 {
            let (hx, hy) = hot.tile_of(6, y);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let cfg = SteamConfig {
            enable_pipe: true,
            enable_leftover_field: false,
            phase_expansion_drive: EXP,
            boil_point_c: 100.0,
            pipe_beat: 5,
            pipe_stroke: 1400,
            ..SteamConfig::default()
        };
        let before = sat_totals(&w).cell_total;
        for t in 1..=40 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        assert_eq!(sat_totals(&w).cell_total, before);
        let (roots, cells) = pipe_path_stats(&w);
        assert_eq!(roots, 1, "one hill is one pipe, roots={roots}");
        assert!(cells >= 3, "path cells={cells}");
        assert!(
            w.pipe_steam.len() + w.pipe_res.len() <= cells + 8,
            "book={} path={cells}",
            w.pipe_steam.len() + w.pipe_res.len()
        );
        assert!(crate::steam::leftover_soak_stats(&w).zone == 0);
        let first = pipe_path_stats(&w);
        for t in 41..=50 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        assert_eq!(
            pipe_path_stats(&w),
            first,
            "later beats must reuse the straw"
        );
        w.pipe_steam.clear();
        assert!(
            pipe_overlay_pack(&w, 6, 3).is_some(),
            "P paints the locked path, not only the puff"
        );
        assert!(
            pipe_overlay_pack(&w, 2, 3).is_none(),
            "side rock stays off the pipe overlay"
        );
    }

    #[test]
    fn pulse_dyes_cool_path_tiles() {
        let mut w = plot();
        for y in 1..6 {
            w.set_cell(4, y, Cell::solid(MaterialId::Stone));
        }
        w.set_cell(4, 6, Cell::air());
        set_live(&mut w, 4, 1, 1400, 150.0);
        let mut cool = temp_at(&w, 20.0);
        let path = PipePath {
            root: (4, 1),
            cells: vec![(4, 1), (4, 2), (4, 3), (4, 4)],
            mouth: (4, 4),
        };
        pulse_path(&mut w, &mut cool, &path, 1400, EXP, 100.0, PIPE_SIDES, None);
        let up = cool.at_cell(4, 4);
        assert!(up > 22.0, "weak heat should climb the straw, T={up}");
        assert!(up < 80.0, "must stay a dye, not a slam, T={up}");
    }

    fn pipe_cfg() -> SteamConfig {
        SteamConfig {
            enable_pipe: true,
            enable_leftover_field: false,
            phase_expansion_drive: EXP,
            boil_point_c: 100.0,
            pipe_beat: 5,
            pipe_stroke: 1400,
            ..SteamConfig::default()
        }
    }

    fn wet_gravel(world: &World) -> Cell {
        let mut c = Cell::solid(MaterialId::Gravel);
        let cap = water_capacity_cell(c, &world.hydro);
        c.sat = Sat(cap.min(10));
        c
    }

    #[test]
    fn shallow_springs_stay_two_pipes() {
        let mut w = plot();
        for x in 0..32 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for &x in &[4, 24] {
            w.set_cell(x, 1, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for &x in &[4, 24] {
            let (hx, hy) = hot.tile_of(x, 1);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let cfg = pipe_cfg();
        for t in 1..=20 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let (roots, _) = pipe_path_stats(&w);
        assert_eq!(
            roots, 2,
            "20-cell gap is farther than a 7-cell climb, roots={roots}"
        );
    }

    #[test]
    fn deep_reservoirs_join_when_closer_than_surface() {
        let mut w = plot();
        for x in 0..32 {
            for y in 1..32 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 32..36 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for &x in &[4, 24] {
            w.set_cell(x, 1, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for &x in &[4, 24] {
            let (hx, hy) = hot.tile_of(x, 1);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let cfg = pipe_cfg();
        for t in 1..=20 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let (n, _) = pipe_path_stats(&w);
        let feeders = PIPE_MEMO.with(|slot| slot.borrow().feeders.len());
        let mains = PIPE_MEMO.with(|slot| slot.borrow().mains.len());
        assert_eq!(mains, 1, "one main straw, mains={mains}");
        assert!(
            feeders >= 1 || n >= 2,
            "deep neighbour walks onto the main, n={n} feeders={feeders}"
        );
        assert!(
            pipe_overlay_pack(&w, 4, 1).is_some() && pipe_overlay_pack(&w, 24, 1).is_some(),
            "both boilers stay on the pipe"
        );
    }

    #[test]
    fn feeder_pumps_water_toward_the_main() {
        let mut w = plot();
        for x in 0..16 {
            w.set_cell(x, 1, {
                let mut c = Cell::solid(MaterialId::Stone);
                c.sat = Sat(8);
                c
            });
        }
        w.set_cell(4, 1, {
            let mut c = wet_gravel(&w);
            c.sat = Sat(2);
            c
        });
        w.set_cell(12, 1, {
            let mut c = wet_gravel(&w);
            c.sat = Sat(20);
            c
        });
        let path = PipePath {
            root: (12, 1),
            cells: (4..=12).rev().map(|x| (x, 1)).collect(),
            mouth: (4, 1),
        };
        let sat_sum = |w: &World| {
            (0..16)
                .map(|x| w.get_cell(x, 1).map(|c| c.sat.0 as u32).unwrap_or(0))
                .sum::<u32>()
        };
        let before_src = w.get_cell(12, 1).unwrap().sat.0;
        let before_mid = w.get_cell(8, 1).unwrap().sat.0;
        let before_sum = sat_sum(&w);
        pump_water_along(&mut w, &path, 6);
        let after_src = w.get_cell(12, 1).unwrap().sat.0;
        let after_mid = w.get_cell(8, 1).unwrap().sat.0;
        assert!(after_src < before_src, "feeder sat {before_src} → {after_src}");
        assert!(
            after_mid > before_mid,
            "water should march toward the main, mid {before_mid} → {after_mid}"
        );
        assert_eq!(sat_sum(&w), before_sum, "pump is mass-flat");
    }

    #[test]
    fn main_rewalks_when_mouth_is_blocked() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 10..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..10 {
            w.set_cell(6, y, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for y in 1..10 {
            let (hx, hy) = hot.tile_of(6, y);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let cfg = pipe_cfg();
        for t in 1..=20 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let mouth = PIPE_MEMO.with(|slot| slot.borrow().mains[0].mouth);
        w.set_cell(mouth.0, mouth.1, Cell::solid(MaterialId::Stone));
        w.set_cell(mouth.0 + 1, mouth.1, Cell::air());
        w.set_cell(mouth.0 + 1, mouth.1 + 1, Cell::air());
        for t in 21..=40 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let new_mouth = PIPE_MEMO.with(|slot| slot.borrow().mains[0].mouth);
        assert_ne!(
            new_mouth, mouth,
            "blocked sky mouth must rewalk, still {mouth:?}"
        );
    }

    #[test]
    fn overlay_marks_boiling_cells_on_the_pipe() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 10..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 5..=6 {
            for y in 1..8 {
                w.set_cell(x, y, wet_gravel(&w));
            }
        }
        let mut hot = temp_at(&w, 20.0);
        for x in 5..=6 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
        }
        let cfg = pipe_cfg();
        for t in 1..=20 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        assert_eq!(pipe_path_stats(&w).0, 1);
        assert!(
            pipe_overlay_pack(&w, 5, 2).is_some() && pipe_overlay_pack(&w, 6, 2).is_some(),
            "P marks the whole boiling block, not only the 1-cell straw"
        );
    }

    #[test]
    fn cool_mouth_parks_distilled_liquid() {
        let mut w = plot();
        w.set_cell(4, 7, Cell::solid(MaterialId::Stone));
        for y in 8..12 {
            w.set_cell(4, y, Cell::air());
        }
        set_live(&mut w, 4, 7, 1400, 150.0);
        let before = sat_totals(&w).cell_total;
        let mut cool = temp_at(&w, 20.0);
        let path = PipePath {
            root: (4, 7),
            cells: vec![(4, 7), (4, 8)],
            mouth: (4, 8),
        };
        pulse_path(&mut w, &mut cool, &path, 1400, EXP, 100.0, PIPE_SIDES, None);
        let parked = (8..12)
            .filter_map(|y| w.get_cell(4, y).map(|c| c.sat.0 as u32))
            .sum::<u32>();
        assert!(parked >= 1, "distilled lip sat={parked}");
        assert_eq!(sat_totals(&w).cell_total, before);
        assert_eq!(w.get_cell(4, 7).unwrap().material, MaterialId::Stone);
    }

    #[test]
    fn hot_mouth_leaks_to_sky_h() {
        let mut w = plot();
        w.set_cell(4, 7, Cell::solid(MaterialId::Stone));
        for y in 8..12 {
            w.set_cell(4, y, Cell::air());
        }
        set_live(&mut w, 4, 7, 1400, 150.0);
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let before_cells = sat_totals(&w).cell_total;
        let before_h = h.total_mass();
        let mut hot = temp_at(&w, 150.0);
        let path = PipePath {
            root: (4, 7),
            cells: vec![(4, 7), (4, 8)],
            mouth: (4, 8),
        };
        pulse_path(
            &mut w,
            &mut hot,
            &path,
            1400,
            EXP,
            100.0,
            PIPE_SIDES,
            Some(&mut h),
        );
        let after_cells = sat_totals(&w).cell_total;
        let after_h = h.total_mass();
        assert!(after_h > before_h, "sky H {before_h} → {after_h}");
        let before = before_cells as f64 + before_h as f64;
        let after = after_cells as f64 + after_h as f64;
        assert!((before - after).abs() < 0.6, "book+H {before} → {after}");
    }

    #[test]
    fn mouth_deposit_skips_the_lumen() {
        let mut w = plot();
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.sat = Sat(8);
        gravel.pore = 200;
        for y in 1..8 {
            w.set_cell(5, y, gravel);
        }
        for y in 8..12 {
            w.set_cell(5, y, Cell::air());
        }
        crate::mineral::add_dissolved(&mut w, 5, 8, 400);
        set_live(&mut w, 5, 1, 1400, 150.0);
        let mut cool = temp_at(&w, 20.0);
        let path = PipePath {
            root: (5, 1),
            cells: (1..=8).map(|y| (5, y)).collect(),
            mouth: (5, 8),
        };
        pulse_path(&mut w, &mut cool, &path, 1400, EXP, 100.0, PIPE_SIDES, None);
        for y in 1..8 {
            assert_eq!(
                w.get_cell(5, y).unwrap().material,
                MaterialId::Gravel,
                "live lumen sintered at y={y}"
            );
        }
    }

    #[test]
    fn pool_residuals_mint_one_sat() {
        let mut w = plot();
        let mut s = Cell::solid(MaterialId::Sand);
        s.sat = Sat(10);
        w.set_cell(2, 2, s);
        w.set_cell(3, 2, s);
        add_residual(&mut w, 2, 2, 800);
        add_residual(&mut w, 3, 2, 600);
        let before = sat_totals(&w).cell_total;
        pool_residuals(&mut w, EXP);
        assert_eq!(pipe_res_at(&w, 2, 2) + pipe_res_at(&w, 3, 2), 0);
        let sat = w.get_cell(2, 2).unwrap().sat.0 + w.get_cell(3, 2).unwrap().sat.0;
        assert!(sat >= 21, "one sat minted into the tile, sat={sat}");
        assert_eq!(sat_totals(&w).cell_total, before);
    }
}
