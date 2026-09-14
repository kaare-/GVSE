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
use crate::worldgen::{live_surface_y, LIVE_SURFACE_SEARCH};

/// Incoming puff sees one side of the cell, not the whole pond.
pub const PIPE_SIDES: u32 = 4;
pub const PIPE_MAX_LEN: usize = 512;
pub const PIPE_STROKE_DEFAULT: u32 = 1400;
const PIPE_MAX_ROOTS: usize = 8;
const PIPE_CLAIM_BUDGET: usize = 32_768;
/// Boiling blocks this close share one straw (Chebyshev, cells).
const PIPE_JOIN_RADIUS: i32 = 48;

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
    paths: Vec<PipePath>,
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
        let cells: usize = memo.paths.iter().map(|p| p.cells.len()).sum();
        (memo.paths.len(), cells)
    })
}

pub fn pipe_painting(world: &World) -> bool {
    if !world.pipe_steam.is_empty() || !world.pipe_res.is_empty() {
        return true;
    }
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == world.chunk_cache_id.get() && !memo.paths.is_empty()
    })
}

fn on_pipe_path(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == world.chunk_cache_id.get()
            && memo
                .paths
                .iter()
                .any(|p| p.cells.iter().any(|&c| c == (gx, gy)))
    })
}

/// P overlay: locked straw plus the live puff. Not the leftover hill.
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
    None
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
    let mut liquid = 0u8;
    let mut liquid_from = root;
    for &dest in &path.cells[1..] {
        if liquid > 0 {
            liquid = deliver_liquid(world, liquid_from, dest, liquid);
            if liquid == 0 {
                liquid_from = dest;
            }
        }
        if steam == 0 {
            continue;
        }
        let (steam_out, liquid_out, t_out) =
            apply_arrival(world, temp, dest, steam, steam_t, expand, boil, sides);
        steam = steam_out;
        steam_t = t_out;
        if liquid_out > 0 {
            liquid = liquid.saturating_add(liquid_out);
            liquid_from = dest;
        }
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
            memo.paths.clear();
            memo.claimed.clear();
        }
    });
}

fn upsert_path(world: &World, path: PipePath) {
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        if let Some(old) = memo.paths.iter_mut().find(|p| p.root == path.root) {
            *old = path;
        } else {
            memo.paths.push(path);
        }
        let _ = world;
    });
}

fn chebyshev(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

fn within_join_radius(a: (i32, i32), b: (i32, i32)) -> bool {
    chebyshev(a, b) <= PIPE_JOIN_RADIUS
}

fn paths_touch(a: &PipePath, b: &PipePath) -> bool {
    if within_join_radius(a.root, b.root) || within_join_radius(a.mouth, b.mouth) {
        return true;
    }
    a.cells
        .iter()
        .any(|&p| b.cells.iter().any(|&q| within_join_radius(p, q)))
}

fn claimed_cells() -> FxHashSet<(i32, i32)> {
    PIPE_MEMO.with(|slot| slot.borrow().claimed.clone())
}

fn remember_claimed(cells: &FxHashSet<(i32, i32)>) {
    PIPE_MEMO.with(|slot| {
        slot.borrow_mut().claimed.extend(cells.iter().copied());
    });
}

fn path_halo() -> FxHashSet<(i32, i32)> {
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        let mut cells = FxHashSet::default();
        for path in &memo.paths {
            for &(x, y) in &path.cells {
                for dy in -1..=1 {
                    for dx in -1..=1 {
                        cells.insert((x + dx, y + dy));
                    }
                }
            }
        }
        cells
    })
}

fn join_adjacent_paths() {
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        if memo.paths.len() < 2 {
            return;
        }
        let n = memo.paths.len();
        let mut parent: Vec<usize> = (0..n).collect();
        let find = |parent: &mut [usize], mut i: usize| {
            while parent[i] != i {
                parent[i] = parent[parent[i]];
                i = parent[i];
            }
            i
        };
        for i in 0..n {
            for j in (i + 1)..n {
                if paths_touch(&memo.paths[i], &memo.paths[j]) {
                    let pi = find(&mut parent, i);
                    let pj = find(&mut parent, j);
                    if pi != pj {
                        parent[pj] = pi;
                    }
                }
            }
        }
        let mut best: FxHashMap<usize, usize> = FxHashMap::default();
        for i in 0..n {
            let p = find(&mut parent, i);
            let len = memo.paths[i].cells.len();
            match best.get(&p).copied() {
                Some(j) if memo.paths[j].cells.len() >= len => {}
                _ => {
                    best.insert(p, i);
                }
            }
        }
        let keep: FxHashSet<(i32, i32)> = best.values().map(|&i| memo.paths[i].root).collect();
        memo.paths.retain(|p| keep.contains(&p.root));
    });
}

fn existing_path_count() -> usize {
    PIPE_MEMO.with(|slot| slot.borrow().paths.len())
}

/// Pay sat on an existing straw. Do not walk a new route.
fn reflash_existing(world: &mut World, temp: &Temperature, expand: u16, boil: f32) {
    let paths = PIPE_MEMO.with(|slot| slot.borrow().paths.clone());
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

fn path_near_taken(gx: i32, gy: i32) -> bool {
    PIPE_MEMO.with(|slot| {
        slot.borrow()
            .paths
            .iter()
            .any(|p| p.cells.iter().any(|&c| within_join_radius(c, (gx, gy))))
    })
}

fn cand_near_taken(cands: &[(i32, i32, f32)], gx: i32, gy: i32) -> bool {
    cands
        .iter()
        .any(|&(x, y, _)| within_join_radius((x, y), (gx, gy)))
}

fn claim_path_feed(world: &World, temp: &Temperature, path: &PipePath, boil: f32) {
    let mut taken = claimed_cells();
    if let Some(&(x0, y0)) = path.cells.first() {
        let mut xmin = x0;
        let mut xmax = x0;
        let mut ymin = y0;
        let mut ymax = y0;
        for &(x, y) in &path.cells {
            xmin = xmin.min(x);
            xmax = xmax.max(x);
            ymin = ymin.min(y);
            ymax = ymax.max(y);
        }
        let r = PIPE_JOIN_RADIUS;
        for y in (ymin - r)..=(ymax + r) {
            for x in (xmin - r)..=(xmax + r) {
                taken.insert((world.wrap_x(x), y));
            }
        }
    }
    for &(x, y) in &path.cells {
        claim_wet_hot_body(world, temp, x, y, boil, &mut taken);
    }
    remember_claimed(&taken);
}

/// Flash wet cells on 4×4 tiles that are already ≥ boil. Not a wet-world scan.
fn ignite_roots(world: &mut World, temp: &Temperature, expand: u16, boil: f32) -> usize {
    let already = existing_path_count();
    if already >= PIPE_MAX_ROOTS {
        return 0;
    }
    let tc = temp.tile_cols.max(1);
    let room = PIPE_MAX_ROOTS - already;
    let mut tiles: Vec<(i32, i32, f32)> = temp
        .cells
        .iter()
        .filter_map(|(&(hx, hy), &t)| (t >= boil).then_some((hx, hy, t)))
        .collect();
    tiles.sort_by_key(|&(hx, hy, _)| (hy, hx));
    let mut taken = claimed_cells();
    taken.extend(path_halo());
    let mut cands: Vec<(i32, i32, f32)> = Vec::new();
    for (hx, hy, t) in tiles {
        if cands.len() >= room {
            break;
        }
        for ly in 0..tc {
            for lx in 0..tc {
                let gx = world.wrap_x(hx * tc + lx);
                let gy = hy * tc + ly;
                if taken.contains(&(gx, gy))
                    || path_near_taken(gx, gy)
                    || cand_near_taken(&cands, gx, gy)
                {
                    continue;
                }
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cell.sat.0 == 0 || cell.material == MaterialId::Air {
                    continue;
                }
                if pipe_live_at(world, gx, gy) > 0 {
                    continue;
                }
                cands.push((gx, gy, t));
                claim_wet_hot_body(world, temp, gx, gy, boil, &mut taken);
                if cands.len() >= room {
                    break;
                }
            }
            if cands.len() >= room {
                break;
            }
        }
    }
    remember_claimed(&taken);
    for (gx, gy, t) in cands.iter().copied() {
        let _ = pipe_flash(world, gx, gy, t, expand);
        let path = walk_pipe(world, (gx, gy));
        upsert_path(world, path.clone());
        claim_path_feed(world, temp, &path, boil);
    }
    cands.len()
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
    let mut stack = vec![(world.wrap_x(gx), gy)];
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

/// Ignite, pulse, join, pool. Off-beat is a no-op besides memo bind.
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
    reflash_existing(world, temp, expand, boil);
    ignite_roots(world, temp, expand, boil);
    join_adjacent_paths();
    let paths = PIPE_MEMO.with(|slot| slot.borrow().paths.clone());
    for path in &paths {
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
    fn boiling_blocks_within_radius_share_one_straw() {
        let mut w = plot();
        for x in 0..32 {
            for y in 1..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for &x in &[4, 24] {
            for y in 1..8 {
                w.set_cell(x, y, {
                    let mut c = Cell::solid(MaterialId::Gravel);
                    let cap = water_capacity_cell(c, &w.hydro);
                    c.sat = Sat(cap.min(10));
                    c
                });
            }
            for y in 8..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut hot = temp_at(&w, 20.0);
        for &x in &[4, 24] {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
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
        for t in 1..=20 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let (roots, _) = pipe_path_stats(&w);
        assert_eq!(roots, 1, "20-cell gap is one boiling block, roots={roots}");
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
