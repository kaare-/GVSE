//! Cell-resolution steam pipe.
//!
//! Temperature only ignites. Steam lives on the water grid as `expand`
//! units (1 sat = `expand` units). Seepage still moves liquid. Face mix
//! decides collapse vs live displacement. See `docs/VOXEL_PIPE.md`.

use wk_material::MaterialId;

use crate::cell::{is_grain, permeability_cell, water_capacity_cell, Cell, Sat};
use crate::displace::park_orphan_water;
use crate::fasthash::{FxHashMap, FxHashSet};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::mineral::{carry_with_water, dissolved_at, precipitate_vent_mouth};
use crate::steam::{
    add_steam, choke_leak_mass, steam_at, void_is_confined, SteamConfig, BOIL_POINT_C,
    MAX_STEAM_CELLS, PHASE_EXPANSION_DRIVE, STEAM_EVERY,
};
use crate::temperature::Temperature;
use crate::worldgen::{live_skin_y, live_surface_y, LIVE_SURFACE_SEARCH};

/// Incoming puff sees one side of the cell, not the whole pond.
pub const PIPE_SIDES: u32 = 4;
pub const PIPE_MAX_LEN: usize = 512;
pub const PIPE_STROKE_DEFAULT: u32 = 1400;
/// A hill may hold a few genuinely independent springs, but a soak that
/// reaches this cap is drawing parallel needles rather than a network —
/// see `PIPE_JOIN_BIAS`, which is what should collapse them into a trunk.
const PIPE_MAX_MAINS: usize = 4;
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
    /// Every cell on any straw, indexed for O(1) overlay lookup.
    ///
    /// The `P` overlay asks "is this cell on the pipe?" once per visible
    /// cell per frame. Scanning `mains`/`feeders` for that answer is
    /// O(total path cells) per query, which cost ~14 FPS on a soaked
    /// mountain (1.6k path cells × ~150k screen cells per frame).
    path_cells: FxHashSet<(i32, i32)>,
    /// Mouth of each straw, so the overlay can mark the discharge point.
    mouths: FxHashSet<(i32, i32)>,
    /// `path_cells ∪ claimed` size, cached so the HUD never clones a
    /// 32k-entry set per frame.
    cells_total: usize,
}

impl PipeMemo {
    /// Rebuild the overlay indexes after any change to `mains` / `feeders`
    /// / `claimed`. Cheap relative to a walk; must not be skipped or the
    /// overlay paints a stale straw.
    fn reindex(&mut self) {
        self.path_cells.clear();
        self.mouths.clear();
        for path in self.mains.iter().chain(self.feeders.iter()) {
            self.path_cells.extend(path.cells.iter().copied());
            self.mouths.insert(path.mouth);
        }
        self.cells_total = self
            .path_cells
            .iter()
            .chain(self.claimed.iter())
            .collect::<FxHashSet<_>>()
            .len();
    }
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
    let s = pipe_network_stats(world);
    (s.mains + s.feeders, s.cells)
}

/// Playtest-facing counters for the pipe network.
#[derive(Debug, Clone, Copy, Default)]
pub struct PipeNetworkStats {
    /// Independent main straws (one root each, walked to the free surface).
    pub mains: usize,
    /// Feeder straws attached onto a main.
    pub feeders: usize,
    /// Unique cells across all straws + claimed boiling cells.
    pub cells: usize,
}

pub fn pipe_network_stats(world: &World) -> PipeNetworkStats {
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        if memo.world_id != world.chunk_cache_id.get() {
            return PipeNetworkStats::default();
        }
        PipeNetworkStats {
            mains: memo.mains.len(),
            feeders: memo.feeders.len(),
            cells: memo.cells_total,
        }
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
        memo.world_id == world.chunk_cache_id.get() && memo.path_cells.contains(&(gx, gy))
    })
}

fn is_pipe_mouth_cell(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == world.chunk_cache_id.get() && memo.mouths.contains(&(gx, gy))
    })
}

/// `P` overlay bands, low to high. The bands are deliberately far apart:
/// a claimed boiling body is thousands of cells and the straw threading it
/// is one cell wide, so 0.24 vs 0.30 made the route invisible against its
/// own reservoir.
const PACK_BOILER: f32 = 0.12;
const PACK_STRAW: f32 = 0.52;
const PACK_MOUTH: f32 = 1.0;
const PACK_LIVE_LO: f32 = 0.62;
const PACK_LIVE_HI: f32 = 0.95;

/// P overlay: claimed boiling body (dim), locked straw (mid), mouth (top),
/// live puff (bright ramp).
pub fn pipe_overlay_pack(world: &World, gx: i32, gy: i32) -> Option<f32> {
    let gx = world.wrap_x(gx);
    // Mouth first: the operator's main question is "where does it vent?".
    if is_pipe_mouth_cell(world, gx, gy) {
        return Some(PACK_MOUTH);
    }
    let live = pipe_live_at(world, gx, gy);
    if live > 0 {
        let cap = world
            .get_cell(gx, gy)
            .map(|c| water_capacity_cell(c, &world.hydro).max(1) as u32)
            .unwrap_or(1);
        let drive = (live as f32 / (cap as f32 * 8.0).max(1.0)).clamp(0.0, 1.0);
        return Some((PACK_LIVE_LO + drive * (PACK_LIVE_HI - PACK_LIVE_LO)).clamp(PACK_LIVE_LO, PACK_LIVE_HI));
    }
    if on_pipe_path(world, gx, gy) {
        return Some(PACK_STRAW);
    }
    if on_pipe_boiler(world, gx, gy) {
        return Some(PACK_BOILER);
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

#[derive(Debug, Clone, Copy, PartialEq)]
enum MouthKind {
    Sky,
    SealedVoid,
    Rock,
}

fn classify_mouth(world: &World, gx: i32, gy: i32, cell: Cell) -> MouthKind {
    if cell.material != MaterialId::Air {
        return MouthKind::Rock;
    }
    if void_is_confined(world, gx, gy) {
        MouthKind::SealedVoid
    } else {
        MouthKind::Sky
    }
}

/// Route pipe residuals into `World.steam` cavity humidity when the mouth is
/// a sealed void. Mass-flat: pool the incoming units with the existing
/// mouth-cell residual so multiple pulses can mint one sat once
/// `expand` units are on the book. Sub-`expand` remainder stays in
/// `pipe_res` at the mouth. Returns units consumed from `steam`.
fn deposit_cavity_humidity(
    world: &mut World,
    mouth: (i32, i32),
    steam: u32,
    expand: u16,
) -> u32 {
    let exp = expand.max(1) as u32;
    let (mx, my) = (world.wrap_x(mouth.0), mouth.1);
    let banked = pipe_res_at(world, mx, my);
    let total = steam.saturating_add(banked);
    let want_sat = total / exp;
    if want_sat == 0 {
        add_residual(world, mx, my, steam);
        return steam;
    }
    let cur = steam_at(world, mx, my) as u32;
    if cur == 0 && world.steam.len() >= MAX_STEAM_CELLS {
        return 0;
    }
    let room = 255u32.saturating_sub(cur);
    let take = want_sat.min(room).min(255);
    if take == 0 {
        return 0;
    }
    let placed = add_steam(world, mx, my, take as u8) as u32;
    let placed_units = placed * exp;
    world.pipe_res.remove(&(mx, my));
    let banked_left = total.saturating_sub(placed_units);
    if banked_left > 0 {
        add_residual(world, mx, my, banked_left);
    }
    // Report how many *incoming* units we absorbed. Anything not banked as
    // sat or residual returns to the caller so it can try elsewhere; but we
    // banked all of `steam` (either into sat or into pipe_res).
    steam
}

const WALK_SCAN_HALFWIDTH: i32 = 16;

/// How far a straw may wander past its best distance-to-vent to follow a
/// bed. Measured against the best reached so far, so drift cannot compound.
const WALK_DETOUR_SLACK: i32 = 2;

/// How far up a column to hunt for a genuinely sky-open cell.
const PIPE_VENT_SEARCH: i32 = 512;

/// Altitude of the first cell in this column that is Air **and open to the
/// sky** — the true vent, not merely the first non-solid cell.
///
/// [`live_surface_y`] stops at the first non-solid cell, and an *enclosed*
/// cavity is non-solid. Under a mountain that made the walker adopt the
/// first internal void as its surface: it arrived there, could no longer
/// reduce its distance to the target, and terminated inside sealed rock.
/// The straw then had no discharge and every beat's units banked in the
/// lumen — the "no route to the surface" report.
///
/// Cost is one upward pass per column. The `material == Air` test
/// short-circuits before [`void_is_confined`], so solid rock (the bulk of
/// a mountain) never pays the roof probe, and the probe itself is memoized
/// in the sky cache.
fn sky_open_y(world: &World, gx: i32, from_y: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    for dy in 0..PIPE_VENT_SEARCH {
        let y = from_y + dy;
        let Some(cell) = world.get_cell(gx, y) else {
            // Ran off the top of the loaded world: the last loaded cell is
            // as close to sky as this column gets.
            return (dy > 0).then_some(y - 1);
        };
        if cell.material == MaterialId::Air && !void_is_confined(world, gx, y) {
            return Some(y);
        }
    }
    None
}

/// Nearest column with the lowest **sky-open** vent in a small window.
/// Wrap-aware. Ties break toward the root so a symmetric mountain does not
/// oscillate. Columns with no vent at all are skipped rather than treated
/// as easy, so the scan cannot bolt off the edge of the loaded world.
fn easiest_target(world: &World, root: (i32, i32)) -> (i32, i32) {
    let (rx, ry) = root;
    let fallback = live_surface_y(world, rx, ry, LIVE_SURFACE_SEARCH);
    let mut best: Option<(i32, i32)> = sky_open_y(world, rx, ry).map(|y| (rx, y));
    for d in 1..=WALK_SCAN_HALFWIDTH {
        for sign in [-1, 1] {
            let x = world.wrap_x(rx + sign * d);
            if world.get_cell(x, ry).is_none() {
                continue;
            }
            let Some(y) = sky_open_y(world, x, ry) else {
                continue;
            };
            if best.map(|(_, by)| y < by).unwrap_or(true) {
                best = Some((x, y));
            }
        }
    }
    best.unwrap_or((rx, fallback))
}

/// Greedy most-open walk that still reduces distance to a projected mouth
/// column. When the straight-up column is capped by stone but an easier
/// column sits nearby, the horizontal pull term lets the walker sidestep
/// toward it instead of plowing through rock.
pub fn walk_pipe(world: &World, root: (i32, i32)) -> PipePath {
    let (rx, ry) = (world.wrap_x(root.0), root.1);
    let (target_x, target_y) = easiest_target(world, (rx, ry));
    let mut cells = vec![(rx, ry)];
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert((rx, ry));
    let mut cur = (rx, ry);
    let dx_here = |x: i32| wrap_dx(world, x, target_x).abs();
    let dy_here = |y: i32| (target_y - y).abs();
    // Best distance-to-vent reached so far. The detour budget is measured
    // against this, not against the current cell, so drift cannot compound:
    // the walker may wander `WALK_DETOUR_SLACK` past its best but has to
    // beat it to earn more room.
    let mut best_manh = dy_here(ry) + dx_here(rx);
    for _ in 0..PIPE_MAX_LEN {
        let Some(here) = world.get_cell(cur.0, cur.1) else {
            break;
        };
        if is_pipe_mouth(world, cur.0, cur.1, here) && cells.len() > 1 {
            break;
        }
        let here_dy = dy_here(cur.1);
        let here_manh = here_dy + dx_here(cur.0);
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
            let step_dy = dy_here(ny);
            let step_manh = step_dy + dx_here(nx);
            // Strata run sideways. Demanding that every step reduce the
            // distance to the vent forbade travelling along one at all, so a
            // straw bored dead-straight up through whatever happened to be
            // above it. A small detour budget lets it slide along a bed to
            // reach an easier crossing while still being drawn to the vent.
            if step_manh > best_manh + WALK_DETOUR_SLACK
                && !is_pipe_mouth(world, nx, ny, n)
            {
                continue;
            }
            // Permeability separates beds that `openness_rank` lumps
            // together — the difference between a tight and an open band of
            // the same rock. Kept well under the rank term so Air still
            // beats stone outright; this only chooses among comparable rock.
            let perm = i32::from(permeability_cell(n, &world.hydro)) / 4;
            // Climbing is the point; dipping back down is a last resort, or
            // the straw wobbles into a bed it has already crossed.
            let up = match dy.cmp(&0) {
                std::cmp::Ordering::Greater => 40,
                std::cmp::Ordering::Equal => 0,
                std::cmp::Ordering::Less => -40,
            };
            let score = (rank as i32) * 1000 + perm - step_manh * 16 + up;
            if best.map(|(s, _, _, _, _)| score > s).unwrap_or(true) {
                best = Some((score, rank, step_dy, nx, ny));
            }
        }
        let Some((_, _, _, nx, ny)) = best else {
            break;
        };
        cells.push((nx, ny));
        cur = (nx, ny);
        best_manh = best_manh.min(dy_here(ny) + dx_here(nx));
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
///
/// Flashes the whole cell. Prefer [`pipe_flash_capped`] on the sim path:
/// an unbounded flash mints far more volume per beat than one stroke can
/// carry away, so live + residual grows without bound.
pub fn pipe_flash(world: &mut World, gx: i32, gy: i32, t_c: f32, expand: u16) -> u32 {
    pipe_flash_capped(world, gx, gy, t_c, expand, u8::MAX)
}

/// [`pipe_flash`] bounded to `max_sat` of liquid per call.
///
/// The boiler is a full water cell often enough (255 sat) that an
/// uncapped flash mints `255 × expand` units in one beat — 24 480 at the
/// play default — while the pulse only carries `pipe_stroke` (1400). The
/// surplus banked as live / residual forever, which is what drove the HUD
/// `sat=` counter from 316 to 653 over a soak and left the straw looking
/// like it had no route out. Capping the flash to one stroke's worth of
/// sat puts intake and throughput on the same scale.
pub fn pipe_flash_capped(
    world: &mut World,
    gx: i32,
    gy: i32,
    t_c: f32,
    expand: u16,
    max_sat: u8,
) -> u32 {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    let paid = cell.sat.0.min(max_sat);
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

/// [`deliver_liquid`] for a packet that can exceed a `u8`, handed over in
/// 255-sat chunks. Returns what did not fit.
///
/// A pulse accumulates condensate across every hop, so on a long lumen the
/// carried packet outgrows `u8`. `saturating_add` into a `u8` truncated it
/// and deleted the overflow outright.
fn deliver_liquid_bulk(
    world: &mut World,
    from: (i32, i32),
    dest: (i32, i32),
    mut amt: u32,
) -> u32 {
    while amt > 0 {
        let chunk = amt.min(u32::from(u8::MAX)) as u8;
        let left = deliver_liquid(world, from, dest, chunk);
        amt -= u32::from(chunk - left);
        if left > 0 {
            break;
        }
    }
    amt
}

/// Mass-weighted blend of two steam packet temperatures.
fn blend_packet_t(a_units: u32, a_t: f32, b_units: u32, b_t: f32) -> f32 {
    let total = a_units as f32 + b_units as f32;
    if total <= 0.0 {
        return a_t;
    }
    (a_t * a_units as f32 + b_t * b_units as f32) / total
}

/// Cap on the packet a single pulse may sweep up, as a multiple of stroke.
/// Keeps one beat's work bounded on a long lumen without letting steam
/// stagnate.
const PIPE_SWEEP_STROKES: u32 = 8;

/// One stroke along a path. Mix before displace on every hop.
///
/// The pulse is a **conveyor**, not a single shot: at every hop it first
/// lifts whatever live steam a previous beat parked in that cell and adds
/// it to the packet it is carrying. Without that sweep, `HopKind::Park`
/// and the `steam_stays` half of `HopKind::Displace` stranded units in the
/// lumen permanently — `pulse_path` only ever drew from `cells[0]`, so
/// live piled up cell by cell, the puff never reached the mouth, and the
/// HUD `sat=` counter climbed without bound. That is what made a working
/// spring look like it had no route to the surface.
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
    let sweep_cap = stroke.saturating_mul(PIPE_SWEEP_STROKES);
    let dye_t = steam_t;
    // Wide: condensate accumulates across hops and a `u8` truncated the
    // packet on a long lumen, deleting the overflow.
    let mut liquid = 0u32;
    let mut liquid_from = root;
    for &dest in &path.cells[1..] {
        if liquid > 0 {
            liquid = deliver_liquid_bulk(world, liquid_from, dest, liquid);
            if liquid == 0 {
                liquid_from = dest;
            }
        }
        // Sweep this cell's parked live into the packet so a previous
        // beat's puff keeps marching toward the mouth. Bounded so one
        // beat cannot drag the whole lumen at once.
        let room = sweep_cap.saturating_sub(steam);
        if room > 0 {
            let (picked, picked_t) = take_live(world, dest.0, dest.1, room);
            if picked > 0 {
                steam_t = blend_packet_t(steam, steam_t, picked, picked_t);
                steam = steam.saturating_add(picked);
            }
        }
        if steam > 0 {
            let (steam_out, liquid_out, t_out) =
                apply_arrival(world, temp, dest, steam, steam_t, expand, boil, sides);
            steam = steam_out;
            steam_t = t_out;
            if liquid_out > 0 {
                liquid += u32::from(liquid_out);
                liquid_from = dest;
            }
        }
        couple_path_heat(temp, dest.0, dest.1, dye_t);
    }
    if steam > 0 {
        let mouth = path.mouth;
        let mouth_cell = world.get_cell(mouth.0, mouth.1);
        let mouth_kind = mouth_cell.map(|c| classify_mouth(world, mouth.0, mouth.1, c));
        match mouth_kind {
            Some(MouthKind::Sky) => {
                leak_pipe_mouth(world, temp, humidity, mouth, steam, expand, boil);
            }
            Some(MouthKind::SealedVoid) => {
                let placed = deposit_cavity_humidity(world, mouth, steam, expand);
                let stranded = steam.saturating_sub(placed);
                if stranded > 0 {
                    add_residual(world, mouth.0, mouth.1, stranded);
                }
            }
            _ => {
                add_residual(world, mouth.0, mouth.1, steam);
            }
        }
    }
    if liquid > 0 {
        let left = deliver_liquid_bulk(world, liquid_from, path.mouth, liquid);
        if left > 0 {
            add_residual(
                world,
                path.mouth.0,
                path.mouth.1,
                left.saturating_mul(u32::from(expand.max(1))),
            );
        }
    }
    deposit_pipe_mouth(world, path);
}

/// March existing pore water one hop toward the path mouth (main line
/// or sky). Mass-flat: unplaced sat returns to the donor.
///
/// Walks the path **mouth-first**. Root-first only ever moved water into
/// a destination that was still full from the previous beat, so `room`
/// was 0 for every pair below the front and the groundwater table barely
/// twitched. Draining the front cell first opens room for the cell behind
/// it, so one beat advances the whole column by a hop instead of only the
/// last pair.
fn pump_water_along(world: &mut World, path: &PipePath, max_sat: u8) {
    if max_sat == 0 || path.cells.len() < 2 {
        return;
    }
    for w in path.cells.windows(2).rev() {
        hand_sat(world, w[0], w[1], max_sat);
    }
}

/// Move up to `max_sat` of liquid one hop, bounded by what `dest` can hold.
/// Returns what actually landed. Mass-flat: anything `deliver_liquid`
/// refuses goes back where it came from.
fn hand_sat(world: &mut World, from: (i32, i32), dest: (i32, i32), max_sat: u8) -> u8 {
    let Some(cell) = world.get_cell(from.0, from.1) else {
        return 0;
    };
    if cell.sat.0 == 0 || cell.material == MaterialId::Air {
        return 0;
    }
    let Some(dest_cell) = world.get_cell(dest.0, dest.1) else {
        return 0;
    };
    let dest_cap = water_capacity_cell(dest_cell, &world.hydro);
    let room = dest_cap.saturating_sub(dest_cell.sat.0);
    let want = cell.sat.0.min(max_sat).min(room);
    if want == 0 {
        return 0;
    }
    let took = take_sat(world, from.0, from.1, want);
    if took == 0 {
        return 0;
    }
    let left = deliver_liquid(world, from, dest, took);
    if left > 0 {
        let _ = add_sat(world, from.0, from.1, left);
    }
    took - left
}

/// Draw the claimed reservoir toward the straw that drains it.
///
/// Without this the claimed body was only paint. Intake was whatever sat
/// happened to stand in the straw's own cells, refilled purely by ordinary
/// seepage through its walls — so a 5.8k-cell boiling reservoir fed a
/// one-cell-wide straw by sipping, and the spring ran at seepage rate no
/// matter how much hot water stood behind it.
///
/// Multi-source BFS outward from the straw across claimed cells, then each
/// claimed cell hands `max_sat` to the neighbour one hop closer in. BFS
/// order means the cells touching the straw move first, so draining the
/// front opens room for the cell behind it and one beat advances the whole
/// body by a hop — the same ordering that `pump_water_along` needs.
/// Seeded from **every** straw at once, not once per path. A soak with 24
/// feeders over a 21k-cell body ran this per path, so the same reservoir
/// was flooded 24 times a beat. One sweep is also more correct: each cell
/// drains toward whichever straw is actually nearest.
fn wick_reservoir<'a>(
    world: &mut World,
    paths: impl Iterator<Item = &'a PipePath>,
    claimed: &FxHashSet<(i32, i32)>,
    max_sat: u8,
) {
    if max_sat == 0 || claimed.is_empty() {
        return;
    }
    let seeds: Vec<(i32, i32)> = paths.flat_map(|p| p.cells.iter().copied()).collect();
    if seeds.is_empty() {
        return;
    }
    let mut seen: FxHashSet<(i32, i32)> = seeds.iter().copied().collect();
    let mut frontier: Vec<(i32, i32)> = seeds;
    while !frontier.is_empty() {
        let mut next = Vec::new();
        for &(cx, cy) in &frontier {
            for (dx, dy) in [(0, 1), (0, -1), (1, 0), (-1, 0)] {
                let n = (world.wrap_x(cx + dx), cy + dy);
                if !claimed.contains(&n) || !seen.insert(n) {
                    continue;
                }
                // Discovery order *is* nearest-first, so handing inward the
                // moment a cell is reached gives the right cascade without
                // a parent map: `(cx, cy)` is by construction one hop
                // closer to a straw than `n`.
                let _ = hand_sat(world, n, (cx, cy), max_sat);
                next.push(n);
            }
        }
        frontier = next;
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
        // `add_sat` takes a u8, so anything past 255 sat stays as units.
        // Deriving `keep` from what was actually minted covers both the
        // sub-expand remainder and that clamp.
        let mint = (sum / exp).min(u32::from(u8::MAX)) as u8;
        let mut keep = sum - u32::from(mint) * exp;
        let park = cells
            .iter()
            .max_by_key(|c| c.2)
            .map(|&(x, y, _)| (x, y))
            .unwrap_or((cells[0].0, cells[0].1));
        for &(gx, gy, _) in &cells {
            world.pipe_res.remove(&(gx, gy));
        }
        // A full cell refuses the sat, and discarding `add_sat`'s shortfall
        // deleted that water outright — a saturated reservoir leaked several
        // hundred sat per beat.
        let mut left = mint;
        for &(gx, gy, _) in &cells {
            if left == 0 {
                break;
            }
            left -= add_sat(world, gx, gy, left);
        }
        // Under a saturated hill the whole tile refuses, and merely holding
        // the mint as residual banked it forever: the book grew every beat
        // and the water was never seen again, which reads in play as water
        // vanishing the moment it reaches the pipe. Let it run off as
        // standing water instead, the same escape `leak_pipe_mouth` uses.
        if left > 0 {
            left = park_orphan_water(world, park.0, park.1, u32::from(left))
                .min(u32::from(u8::MAX)) as u8;
        }
        keep += u32::from(left) * exp;
        if keep > 0 {
            world.pipe_res.insert(park, keep);
        }
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
            memo.reindex();
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

/// How much cheaper an existing conduit is than boring fresh rock.
///
/// The bare comparison `dist_to_path < surface_dist` weighs two unlike
/// things: reaching a straw is measured through whatever rock is in the
/// way, while `surface_dist` is a plain vertical count that ignores how
/// hard the climb would be. So a spring ten cells under a broad hill
/// always preferred its own bore over a trunk forty cells sideways, and a
/// soak grew eight parallel mains straight to the surface instead of one
/// mainline. Reaching a conduit that already exists is worth several times
/// its distance in fresh rock.
const PIPE_JOIN_BIAS: i32 = 4;

/// Cheaper to reach the locked straw than to bore to this cell's surface.
fn should_feed_main(world: &World, boiler: (i32, i32), main: &PipePath) -> bool {
    dist_to_path(world, boiler, main) < surface_dist(world, boiler) * PIPE_JOIN_BIAS
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
        let mut memo = slot.borrow_mut();
        memo.claimed = taken;
        memo.reindex();
    });
}

fn rewalk_network(world: &World) {
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        for main in &mut memo.mains {
            *main = walk_pipe(world, main.root);
        }
        memo.mains.retain(|p| p.cells.len() >= 2);
        // With no main on the book every feeder is an orphan. Keep trying
        // orphans until one walks; the old code spent a single attempt on
        // `feeders[0]` and, when that root was unroutable, left the whole
        // book frozen with no main and every feeder stale.
        if memo.mains.is_empty() {
            let mut rest = Vec::new();
            for o in std::mem::take(&mut memo.feeders) {
                if !memo.mains.is_empty() {
                    rest.push(o);
                    continue;
                }
                let p = walk_pipe(world, o.root);
                if p.cells.len() >= 2 {
                    memo.mains.push(p);
                }
                // An unroutable orphan is dropped rather than kept: if its
                // root is still a live boiler it is re-detected next beat.
            }
            memo.feeders = rest;
        }
        let mains = memo.mains.clone();
        // A feeder that cannot land on a main is dropped. Leaving it in
        // place meant it was never rewalked again — a straw frozen over
        // terrain that had since changed, which is what put routes in
        // mid-air, and its stale cells still drove `rebuild_claimed`.
        memo.feeders.retain_mut(|feeder| {
            let Some(i) = nearest_main_idx(world, feeder.root, &mains) else {
                return false;
            };
            *feeder = make_feeder(world, feeder.root, &mains[i]);
            feeder.cells.len() >= 2
        });
        // One straw per root. A root on the book twice is the same spring
        // drawn twice: it doubles its intake and paints a second needle
        // beside the first. The soak showed 24 feeders where the claim
        // should have collapsed them into far fewer springs.
        let mut roots: FxHashSet<(i32, i32)> = memo.mains.iter().map(|p| p.root).collect();
        memo.feeders.retain(|f| roots.insert(f.root));
        memo.reindex();
    });
}

/// Flash one hot wet cell per straw, bounded to what the pulse can carry.
///
/// `max_sat` is one stroke's worth so intake matches throughput. Without
/// it a full water cell mints ~17× a stroke every beat and the network
/// banks the difference forever.
fn reflash_network(
    world: &mut World,
    temp: &Temperature,
    expand: u16,
    boil: f32,
    max_sat: u8,
) {
    let paths = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.mains
            .iter()
            .chain(memo.feeders.iter())
            .cloned()
            .collect::<Vec<_>>()
    });
    // One flash per cell per beat, however many straws run through it.
    // Paths overlap — a feeder splices onto a main and shares its cells —
    // so keying off the path alone let a shared cell flash once per path,
    // multiplying intake by the overlap and defeating the per-beat cap.
    let mut fired: FxHashSet<(i32, i32)> = FxHashSet::default();
    for path in paths {
        let Some((gx, gy, t)) = path.cells.iter().copied().find_map(|(gx, gy)| {
            let cell = world.get_cell(gx, gy)?;
            if cell.sat.0 == 0 || cell.material == MaterialId::Air {
                return None;
            }
            if fired.contains(&(gx, gy)) {
                return None;
            }
            let t = temp.at_cell(gx, gy);
            (t >= boil).then_some((gx, gy, t))
        }) else {
            continue;
        };
        fired.insert((gx, gy));
        let _ = pipe_flash_capped(world, gx, gy, t, expand, max_sat);
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
///
/// Returns whether the book changed, so the caller can skip a second
/// rewalk + reclaim on the overwhelming majority of beats where it did not.
fn attach_new_boilers(world: &World, temp: &Temperature, boil: f32) -> bool {
    let (mains, feeders) = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        (memo.mains.clone(), memo.feeders.clone())
    });
    let room = (PIPE_MAX_MAINS - mains.len().min(PIPE_MAX_MAINS))
        + (PIPE_MAX_FEEDERS - feeders.len().min(PIPE_MAX_FEEDERS));
    let skip = claimed_cells();
    let cands = collect_boiler_cands(world, temp, boil, &skip, room);
    if cands.is_empty() {
        return false;
    }
    PIPE_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        let before = (memo.mains.len(), memo.feeders.len());
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
        memo.reindex();
        (memo.mains.len(), memo.feeders.len()) != before
    })
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
    let wet_hot = |world: &World, x: i32, y: i32| -> bool {
        let Some(cell) = world.get_cell(x, y) else {
            return false;
        };
        cell.sat.0 > 0 && cell.material != MaterialId::Air && temp.at_cell(x, y) >= boil
    };
    // Claim on **push**, not on pop. Popping meant a cell discovered by
    // several neighbours paid for a chunk lookup and a temperature lookup
    // once per discovering edge — up to eight times per cell across a 30k
    // cell reservoir.
    let mut stack = Vec::new();
    if wet_hot(world, gx, gy) {
        if taken.insert((gx, gy)) {
            stack.push((gx, gy));
        }
    } else {
        for dy in -1..=1 {
            for dx in -1..=1 {
                if dx == 0 && dy == 0 {
                    continue;
                }
                let nx = world.wrap_x(gx + dx);
                let ny = gy + dy;
                if wet_hot(world, nx, ny) && taken.insert((nx, ny)) {
                    stack.push((nx, ny));
                }
            }
        }
    }
    if stack.is_empty() {
        return;
    }
    let mut n = 0usize;
    while let Some((x, y)) = stack.pop() {
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
                taken.insert((nx, ny));
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
    // Reclaiming floods the whole body, ~3.7ms on a 31k-cell reservoir, and
    // it only needs redoing when a boiler actually joined the book.
    if attach_new_boilers(world, temp, boil) {
        rewalk_network(world);
        rebuild_claimed(world, temp, boil);
    }
    let (feeders, mains) = PIPE_MEMO.with(|slot| {
        let memo = slot.borrow();
        (memo.feeders.clone(), memo.mains.clone())
    });
    let water = stroke_sat(stroke, expand);
    // Charge the straws from their reservoirs before firing, so a beat's
    // flash is fed by the standing body and not only by wall seepage.
    let claimed = claimed_cells();
    wick_reservoir(world, feeders.iter().chain(mains.iter()), &claimed, water);
    reflash_network(world, temp, expand, boil, stroke_sat(stroke, expand));
    for path in feeders.iter() {
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
    let is_erupt = (world.tick / beat) % PIPE_ERUPT_PERIOD == 0;
    for path in mains.iter() {
        let main_stroke = main_stroke_for_beat(world, path.root, stroke, is_erupt);
        if main_stroke > 0 {
            pulse_path(
                world,
                temp,
                path,
                main_stroke,
                expand,
                boil,
                sides,
                humidity.as_deref_mut(),
            );
        }
        pump_water_along(world, path, water);
    }
    pool_residuals(world, expand);
}

/// Every `PIPE_ERUPT_PERIOD` beats a main erupts: it unleashes everything
/// live has managed to bank at the root. In between it simmers at one
/// period's worth so most of a beat's flash accumulates for the next
/// eruption. A geyser's rhythm — not a constant thin puff.
const PIPE_ERUPT_PERIOD: u64 = 8;

fn main_stroke_for_beat(world: &World, root: (i32, i32), stroke: u32, is_erupt: bool) -> u32 {
    let root_live = pipe_live_at(world, root.0, root.1);
    if is_erupt {
        // Unleash everything the root has banked. `take_live` inside
        // pulse_path is the actual bounded consumer, so passing the full
        // live count fires up to `root_live` units and leaves nothing.
        root_live.max(stroke)
    } else {
        // Simmer at 1/period so most of a beat's flash stays as live
        // for the next eruption. Never zero so the P overlay keeps the
        // straw painted between shots.
        let simmer = stroke / PIPE_ERUPT_PERIOD as u32;
        simmer.max(1)
    }
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
        assert_eq!(
            mouth.material,
            MaterialId::Air,
            "mouth={:?} path={:?}",
            path.mouth,
            path.cells
        );
    }

    #[test]
    fn long_soak_of_apply_pipe_motor_is_mass_flat() {
        // Simulate a long run of a working hot spring under a wide sky:
        // the aquifer is refilled every beat by hand so the boiler never
        // starves, and the pipe motor spends 200 beats moving units to
        // the mouth. audit::cell_total must not drift a unit.
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..8 {
            w.set_cell(6, y, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for y in 1..8 {
            let (hx, hy) = hot.tile_of(6, y);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let cfg = pipe_cfg();
        let opening = sat_totals(&w).cell_total;
        let mut budget = 0i64;
        for t in 1..=1000u64 {
            w.tick = t;
            // Mimic seepage recharge: top up the boiler root so the pipe
            // has a steady supply of sat to flash. Track the delta so the
            // audit knows what we injected.
            if t % cfg.pipe_beat == 0 {
                if let Some(cell) = w.get_cell(6, 1) {
                    let cap = water_capacity_cell(cell, &w.hydro);
                    let room = cap.saturating_sub(cell.sat.0);
                    if room > 0 {
                        let mut next = cell;
                        next.sat = Sat(cap);
                        w.set_cell(6, 1, next);
                        budget += room as i64;
                    }
                }
            }
            apply_pipe_motor(&mut w, &mut hot, &cfg, Some(&mut h));
        }
        let cells = sat_totals(&w).cell_total;
        let h_mass = h.total_mass() as i64;
        assert!(cells >= 0, "cell_total went negative");
        assert!(h_mass >= 0, "humidity went negative");
        // Strict now that `pool_residuals` no longer discards `add_sat`'s
        // shortfall and the carried condensate is no longer truncated to a
        // `u8`. 1000 beats must not lose or mint a single sat.
        assert_eq!(
            opening + budget,
            cells + h_mass,
            "1000 beats drifted: opened {opening} + recharged {budget} \
             != cells {cells} + humidity {h_mass}"
        );
        assert!(
            w.pipe_steam.len() < 512,
            "pipe_steam book grew unbounded: {}",
            w.pipe_steam.len()
        );
        let banked = pipe_units_total(&w);
        let exp = pipe_expand(&w) as i64;
        let banked_sat = banked / exp;
        assert!(
            budget > 0,
            "recharge should have topped up the boiler at least once"
        );
        let stroke_sat = cfg.pipe_stroke as i64 / exp;
        assert!(
            banked_sat <= stroke_sat * 16,
            "banked mass should not explode past a handful of strokes ({banked_sat} vs cap {})",
            stroke_sat * 16
        );
    }

    /// Playtest regression, and the one that explains the report. `pulse_path`
    /// only ever drew live from `cells[0]`, so the units `HopKind::Park` and
    /// `Displace` left behind sat in the lumen forever: the puff never
    /// reached the mouth (looked like "no route to the surface") and the
    /// live book grew every beat (HUD `sat=` 316 → 418 → 653). The pulse is
    /// now a conveyor that sweeps parked live along with it.
    #[test]
    fn a_pulse_sweeps_live_parked_by_earlier_beats() {
        let mut w = plot();
        let expand = PHASE_EXPANSION_DRIVE;
        w.pipe_expand = expand;
        for x in 0..16 {
            for y in 1..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 10..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..10 {
            w.set_cell(4, y, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 150.0);
        let path = PipePath {
            root: (4, 1),
            cells: (1..=10).map(|y| (4, y)).collect(),
            mouth: (4, 10),
        };
        // Strand live in the middle of the lumen, as an earlier beat would.
        let stranded = u32::from(expand) * 3;
        set_live(&mut w, 4, 5, stranded, 150.0);
        let mid_before = pipe_live_at(&w, 4, 5);
        assert_eq!(mid_before, stranded, "setup: mid-lumen should hold live");
        // Now pulse from the root. The packet must pick the stranded units up.
        set_live(&mut w, 4, 1, u32::from(expand) * 2, 150.0);
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        pulse_path(
            &mut w,
            &mut hot,
            &path,
            u32::from(expand) * 2,
            expand,
            100.0,
            PIPE_SIDES,
            Some(&mut h),
        );
        let mid_after = pipe_live_at(&w, 4, 5);
        assert!(
            mid_after < mid_before,
            "the pulse must sweep mid-lumen live onward ({mid_before} → {mid_after})"
        );
    }

    /// A working spring must actually converge: with intake capped and the
    /// lumen swept, repeated beats should not grow the live book without
    /// bound even when the boiler is refilled every beat.
    #[test]
    fn a_recharged_spring_converges_instead_of_banking_forever() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 10..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..10 {
            w.set_cell(4, y, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for y in 1..10 {
            let (hx, hy) = hot.tile_of(4, y);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let cfg = pipe_cfg();
        let exp = cfg.phase_expansion_drive as i64;
        let mut early_peak = 0i64;
        let mut late_peak = 0i64;
        for t in 1..=600u64 {
            w.tick = t;
            // Recharge the whole boiler column every beat.
            if t % cfg.pipe_beat == 0 {
                for y in 1..10 {
                    w.set_cell(4, y, wet_gravel(&w));
                }
            }
            apply_pipe_motor(&mut w, &mut hot, &cfg, Some(&mut h));
            let banked = pipe_units_total(&w) / exp;
            if t <= 150 {
                early_peak = early_peak.max(banked);
            } else if t > 450 {
                late_peak = late_peak.max(banked);
            }
        }
        // The late window must not be dramatically worse than the early one.
        // Pre-fix this grew monotonically because nothing swept the lumen.
        assert!(
            late_peak <= early_peak.max(16) * 4,
            "live book keeps growing: early peak {early_peak} sat, late peak {late_peak} sat"
        );
    }

    /// Playtest regression. The HUD `sat=` counter climbed 316 → 418 → 653
    /// over a soak because `reflash_network` flashed a whole 255-sat water
    /// cell every beat (24 480 units at expand 96) while the pulse only
    /// carried one 1400-unit stroke. Everything above throughput banked as
    /// live / residual forever, which also made the straw read as having no
    /// route out. Intake is now capped to one stroke's worth of sat.
    #[test]
    fn a_full_water_boiler_does_not_bank_mass_without_bound() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // The boiler is a FULL water cell — the case the playtest hit.
        for y in 1..8 {
            w.set_cell(6, y, Cell::water());
        }
        let mut hot = temp_at(&w, 20.0);
        for y in 1..8 {
            let (hx, hy) = hot.tile_of(6, y);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let cfg = pipe_cfg();
        let exp = cfg.phase_expansion_drive as i64;
        let stroke_sat = cfg.pipe_stroke as i64 / exp;
        let mut peak_banked_sat = 0i64;
        for t in 1..=600u64 {
            w.tick = t;
            // Keep the boiler topped up to full every beat: the worst case
            // for intake-vs-throughput balance.
            if t % cfg.pipe_beat == 0 {
                w.set_cell(6, 1, Cell::water());
            }
            apply_pipe_motor(&mut w, &mut hot, &cfg, Some(&mut h));
            peak_banked_sat = peak_banked_sat.max(pipe_units_total(&w) / exp);
        }
        // One eruption period banks at most period × stroke. Allow 4× that
        // for pooling slack; the pre-fix behaviour blew past 600.
        let ceiling = stroke_sat * PIPE_ERUPT_PERIOD as i64 * 4;
        assert!(
            peak_banked_sat <= ceiling,
            "banked mass ran away: peak {peak_banked_sat} sat vs ceiling {ceiling} \
             (stroke={stroke_sat} sat, period={PIPE_ERUPT_PERIOD})"
        );
    }

    #[test]
    fn overlay_bands_are_visually_distinct() {
        // The playtest could not see the route because the straw painted
        // 0.30 against a claimed body at 0.24 — a 0.06 step in the colour
        // ramp across thousands of cells. Bands must stay far apart.
        let steps = [PACK_BOILER, PACK_STRAW, PACK_LIVE_LO, PACK_MOUTH];
        for pair in steps.windows(2) {
            assert!(
                pair[1] - pair[0] >= 0.08,
                "overlay bands {} and {} are too close to tell apart",
                pair[0],
                pair[1]
            );
        }
        assert!(PACK_LIVE_HI <= PACK_MOUTH, "live ramp must not exceed mouth");
    }

    #[test]
    fn flash_cap_bounds_one_beat_of_intake() {
        let mut w = plot();
        w.set_cell(4, 1, Cell::water());
        let full = w.get_cell(4, 1).unwrap().sat.0;
        assert!(full > 20, "water cell should start near full, got {full}");
        let before = sat_totals(&w).cell_total;
        let units = pipe_flash_capped(&mut w, 4, 1, 150.0, 96, 14);
        assert_eq!(units, 14 * 96, "cap should bound the minted volume");
        assert_eq!(
            w.get_cell(4, 1).unwrap().sat.0,
            full - 14,
            "only the capped sat should be paid"
        );
        assert_eq!(sat_totals(&w).cell_total, before, "capped flash is mass-flat");
    }

    #[test]
    fn erupt_cadence_unleashes_live_on_the_beat_and_simmers_between() {
        let mut w = plot();
        w.pipe_expand = EXP;
        set_live(&mut w, 4, 1, 20_000, 150.0);
        let stroke = 1400u32;
        let erupt = main_stroke_for_beat(&w, (4, 1), stroke, true);
        let simmer = main_stroke_for_beat(&w, (4, 1), stroke, false);
        assert!(
            erupt >= 20_000,
            "erupt pulse should unleash all root live, got {erupt}"
        );
        assert!(
            simmer >= 1 && simmer < stroke,
            "simmer must be a small non-zero fraction of stroke, got {simmer}"
        );
        // The simmer stroke is a small fraction so most of a beat's flash
        // stays as live for the next eruption.
        assert!(
            simmer * (PIPE_ERUPT_PERIOD as u32) >= stroke.saturating_sub(PIPE_ERUPT_PERIOD as u32),
            "period simmer strokes should sum to about one stroke, got {simmer} × {PIPE_ERUPT_PERIOD}"
        );
    }

    /// Playtest regression: "the route treated the first air block it
    /// encountered as a surface, even if it was enclosed completely."
    ///
    /// A sealed cavity sits between the boiler and the real sky. The target
    /// used to be that cavity (because `live_surface_y` stops at the first
    /// non-solid cell), so the walker arrived, could not reduce distance,
    /// and terminated in sealed rock with no discharge.
    #[test]
    fn a_sealed_cavity_is_not_mistaken_for_the_surface() {
        let mut w = plot();
        // Wide plateau: the ±16 vent scan must stay inside rock, otherwise
        // the walker legitimately escapes to the open air beyond the plot.
        for x in 0..64 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..30 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 30..40 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Fully enclosed cavity at y=8..=11, well below the real sky at 30.
        for x in 30..=34 {
            for y in 8..=11 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..8 {
            w.set_cell(32, y, wet_gravel(&w));
        }
        // The cavity must really be sealed, or the test proves nothing.
        assert!(
            void_is_confined(&w, 32, 10),
            "setup: the cavity should read as confined"
        );
        assert!(
            !void_is_confined(&w, 32, 31),
            "setup: the sky should read as open"
        );
        // The naive surface probe still reports the cavity floor...
        let naive = live_surface_y(&w, 32, 1, LIVE_SURFACE_SEARCH);
        assert!(
            naive < 30,
            "setup: live_surface_y should stop at the cavity ({naive})"
        );
        // ...but the vent probe must climb past it to the real sky.
        let vent = sky_open_y(&w, 32, 1).expect("column should have a vent");
        assert!(
            vent >= 30,
            "vent must be the real sky, not the sealed cavity (got {vent})"
        );
        let path = walk_pipe(&w, (32, 1));
        let mouth = w.get_cell(path.mouth.0, path.mouth.1).unwrap();
        assert_eq!(
            mouth.material,
            MaterialId::Air,
            "mouth should be air, got {:?} at {:?}",
            mouth.material,
            path.mouth
        );
        assert!(
            !void_is_confined(&w, path.mouth.0, path.mouth.1),
            "mouth must be sky-open, not the sealed cavity, got {:?}",
            path.mouth
        );
        assert!(
            path.mouth.1 >= 30,
            "mouth should reach the real surface, got {:?}",
            path.mouth
        );
    }

    /// Soak feedback: "the crawl of the pipes [should respect] the stratas
    /// and geography of the rock better, less straight up pipe."
    ///
    /// A tight bed with one open window offset sideways. The straw should
    /// track along the bed to the window rather than bore straight through
    /// the tight rock above its root.
    #[test]
    fn the_crawl_follows_a_bed_to_an_easier_crossing() {
        let mut w = plot();
        for x in 0..64 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..20 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 20..26 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // A tight clay bed across the hill, broken by a loose-rock window
        // three columns to the right of the root.
        for x in 0..64 {
            for y in 8..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Clay));
            }
        }
        for x in 9..=11 {
            for y in 8..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::LooseRock));
            }
        }
        let path = walk_pipe(&w, (8, 1));
        // It must still get out.
        let mouth = w.get_cell(path.mouth.0, path.mouth.1).unwrap();
        assert_eq!(
            mouth.material,
            MaterialId::Air,
            "the straw still has to reach the sky, ended at {:?}",
            path.mouth
        );
        // And it must cross the tight bed through the window, not beside it.
        let crossing: Vec<(i32, i32)> = path
            .cells
            .iter()
            .copied()
            .filter(|&(_, y)| (8..12).contains(&y))
            .collect();
        assert!(
            !crossing.is_empty(),
            "the path should cross the bed: {:?}",
            path.cells
        );
        // It entered the bed through the window rather than boring straight
        // up from the root at x=8.
        let entry = crossing[0];
        assert!(
            (9..=11).contains(&entry.0),
            "the straw entered the tight bed at {entry:?} instead of \
             tracking to the open window at x=9..=11: crossing {crossing:?}"
        );
        // And it is not a dead-straight column.
        let columns: FxHashSet<i32> = path.cells.iter().map(|&(x, _)| x).collect();
        assert!(
            columns.len() > 1,
            "the crawl should follow the rock, not bore one column: {:?}",
            path.cells
        );
    }

    #[test]
    fn walker_sidesteps_around_a_capped_column() {
        let mut w = plot();
        // Wide stone plateau with a narrow Air chimney offset from the
        // boiler. The walker used to plow straight up through stone; with
        // the horizontal target pull it should drift toward the chimney.
        for x in 0..16 {
            for y in 1..14 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        for y in 1..3 {
            w.set_cell(4, y, wet_gravel(&w));
        }
        for y in 3..14 {
            w.set_cell(9, y, Cell::air());
        }
        let path = walk_pipe(&w, (4, 1));
        let mouth = w.get_cell(path.mouth.0, path.mouth.1).unwrap();
        assert_eq!(
            mouth.material,
            MaterialId::Air,
            "walker should reach the offset chimney, mouth={:?} path={:?}",
            path.mouth,
            path.cells
        );
        // The path should drift toward x=9 rather than staying pinned to x=4.
        let ended_near_chimney = (path.mouth.0 - 9).abs() <= 1;
        assert!(
            ended_near_chimney,
            "mouth should be at or beside the chimney (x=9), got {:?}",
            path.mouth
        );
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

    /// The claimed reservoir must actually feed the straw. Before the wick
    /// it was only paint: intake was whatever stood in the straw's own
    /// cells, so a wide boiling body drained no faster than wall seepage.
    #[test]
    fn the_reservoir_feeds_the_straw_and_stays_mass_flat() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // A wide saturated body, one column of which becomes the straw.
        let cap = water_capacity_cell(Cell::solid(MaterialId::Sand), &w.hydro);
        for x in 2..14 {
            for y in 1..8 {
                let mut c = Cell::solid(MaterialId::Sand);
                c.sat = Sat(cap);
                w.set_cell(x, y, c);
            }
        }
        let mut hot = temp_at(&w, 20.0);
        for x in 2..14 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
        }
        let before = sat_totals(&w).cell_total;
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let cfg = pipe_cfg();

        // Water standing far from the straw must end up closer to it.
        let far_before: u32 = (1..8)
            .filter_map(|y| w.get_cell(2, y).map(|c| c.sat.0 as u32))
            .sum();
        for t in 1..=40u64 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, Some(&mut h));
        }
        let far_after: u32 = (1..8)
            .filter_map(|y| w.get_cell(2, y).map(|c| c.sat.0 as u32))
            .sum();
        assert!(
            far_after < far_before,
            "the far edge of the reservoir should have drained toward the \
             straw, {far_before} -> {far_after}"
        );

        // `sat_totals::cell_total` already folds in `pipe_mass_sat`.
        let after = sat_totals(&w).cell_total;
        let h_mass = h.total_mass() as i64;
        assert_eq!(
            before, after + h_mass,
            "wicking the reservoir must be mass-flat: before={before} \
             after={after} h={h_mass}"
        );
    }

    /// The wick is a conveyor, so one beat may only advance the body by a
    /// hop. Nearest-first ordering is what makes that work; reversing it
    /// stalls behind full cells the way root-first pumping did.
    #[test]
    fn the_wick_advances_the_body_one_hop_per_beat() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let cap = water_capacity_cell(Cell::solid(MaterialId::Sand), &w.hydro);
        for x in 4..=8 {
            let mut c = Cell::solid(MaterialId::Sand);
            c.sat = Sat(cap);
            w.set_cell(x, 1, c);
        }
        let mut hot = temp_at(&w, 20.0);
        for x in 4..=8 {
            let (hx, hy) = hot.tile_of(x, 1);
            hot.set_tile_c(hx, hy, 150.0);
        }
        // The straw needs room, or the body is gridlocked at capacity and
        // correctly refuses to move — flash is what normally makes room.
        let mut drained = Cell::solid(MaterialId::Sand);
        drained.sat = Sat(0);
        w.set_cell(4, 1, drained);
        let claimed: FxHashSet<(i32, i32)> = (5..=8).map(|x| (x, 1)).collect();
        let path = PipePath {
            root: (4, 1),
            cells: vec![(4, 1)],
            mouth: (4, 1),
        };
        let before = sat_totals(&w).cell_total;
        let tail_before = w.get_cell(8, 1).unwrap().sat.0;
        wick_reservoir(&mut w, std::iter::once(&path), &claimed, 8);
        assert!(
            w.get_cell(8, 1).unwrap().sat.0 < tail_before,
            "the far end of the body should hand water inward"
        );
        assert_eq!(
            before,
            sat_totals(&w).cell_total,
            "a wick pass only moves water, it never mints or drops it"
        );
    }

    /// Soak regression: `sat=` climbed 5.6k -> 32k with `P=8+24`.
    ///
    /// Only a **main mouth** vents; a feeder discharges onto its main, which
    /// just moves units around inside the book. Flash was capped per path,
    /// so 32 straws drew 32 strokes of intake a beat against the one stroke
    /// of egress eight simmering mains could carry. The aquifer emptied into
    /// the lumen and stayed there — water that "disappears once it reaches
    /// the pipe".
    #[test]
    fn a_many_straw_network_does_not_bank_the_aquifer() {
        let mut w = plot();
        for x in 0..64 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 10..18 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // A wide saturated hot slab: many springs, one shared sky.
        let cap = water_capacity_cell(Cell::solid(MaterialId::Sand), &w.hydro);
        for x in 4..60 {
            for y in 1..10 {
                let mut c = Cell::solid(MaterialId::Sand);
                c.sat = Sat(cap);
                w.set_cell(x, y, c);
            }
        }
        let mut hot = temp_at(&w, 20.0);
        for x in 4..60 {
            for y in 1..10 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 160.0);
            }
        }
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 128, 64);
        let cfg = pipe_cfg();
        let opening = sat_totals(&w).cell_total;
        let mut peak = 0i64;
        for t in 1..=300u64 {
            w.tick = t * cfg.pipe_beat;
            apply_pipe_motor(&mut w, &mut hot, &cfg, Some(&mut h));
            peak = peak.max(pipe_mass_sat(&w));
        }
        let stats = pipe_network_stats(&w);
        let banked = pipe_mass_sat(&w);
        // The book may hold a working charge, but not the aquifer. Bound it
        // by what the network could plausibly have in flight: a few strokes
        // per straw.
        let straws = (stats.mains + stats.feeders).max(1) as i64;
        let ceiling = stroke_sat(cfg.pipe_stroke, EXP) as i64 * straws * 8;
        assert!(
            banked <= ceiling,
            "pipe banked {banked} sat (peak {peak}) across {straws} straws, \
             ceiling {ceiling}; opening world sat was {opening}"
        );
        assert!(
            sat_totals(&w).cell_total + h.total_mass() as i64 == opening,
            "and it must stay mass-flat while doing so"
        );
    }

    /// One straw per root. A duplicated root is the same spring drawn
    /// twice: double intake and a second needle beside the first.
    #[test]
    fn one_root_never_carries_two_straws() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..8 {
            w.set_cell(6, y, wet_gravel(&w));
        }
        let main = PipePath {
            root: (6, 1),
            cells: (1..=8).map(|y| (6, y)).collect(),
            mouth: (6, 8),
        };
        PIPE_MEMO.with(|slot| {
            let mut memo = slot.borrow_mut();
            memo.world_id = w.chunk_cache_id.get();
            memo.mains.clear();
            memo.feeders.clear();
            memo.mains.push(main.clone());
            // The same root smuggled onto the book five more times.
            for _ in 0..5 {
                memo.feeders.push(main.clone());
            }
            memo.reindex();
        });

        rewalk_network(&w);

        let roots = PIPE_MEMO.with(|slot| {
            let memo = slot.borrow();
            memo.mains
                .iter()
                .chain(memo.feeders.iter())
                .map(|p| p.root)
                .collect::<Vec<_>>()
        });
        let unique: FxHashSet<(i32, i32)> = roots.iter().copied().collect();
        assert_eq!(
            roots.len(),
            unique.len(),
            "the same root appears on several straws: {roots:?}"
        );
    }

    /// Overlapping straws must not multiply intake. A feeder splices onto a
    /// main and shares its cells, so flashing "the first hot wet cell of
    /// each path" fired a shared cell once per path and blew straight
    /// through the per-beat cap the flash cap exists to enforce.
    #[test]
    fn a_shared_cell_flashes_once_per_beat_however_many_straws_cross_it() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let cap = water_capacity_cell(Cell::solid(MaterialId::Sand), &w.hydro);
        for y in 1..8 {
            let mut c = Cell::solid(MaterialId::Sand);
            c.sat = Sat(cap);
            w.set_cell(6, y, c);
        }
        let hot = temp_at(&w, 150.0);
        // Eight straws over the very same column.
        let shared = PipePath {
            root: (6, 1),
            cells: (1..=8).map(|y| (6, y)).collect(),
            mouth: (6, 8),
        };
        PIPE_MEMO.with(|slot| {
            let mut memo = slot.borrow_mut();
            memo.world_id = w.chunk_cache_id.get();
            memo.mains.clear();
            memo.feeders.clear();
            memo.mains.push(shared.clone());
            for _ in 0..7 {
                memo.feeders.push(shared.clone());
            }
            memo.reindex();
        });
        let sat_before = w.get_cell(6, 1).unwrap().sat.0;
        let one_stroke = stroke_sat(1400, EXP);
        reflash_network(&mut w, &hot, EXP, 100.0, one_stroke);
        let spent = sat_before - w.get_cell(6, 1).unwrap().sat.0;
        assert!(
            spent <= one_stroke,
            "eight straws over one column flashed {spent} sat, cap is \
             {one_stroke} for the beat"
        );
    }

    /// Soak regression: the HUD read `P=0+24/21132` — no mains, feeders
    /// pinned at the cap, with straws overlapping and "some ending in the
    /// air".
    ///
    /// `rewalk_network` rewalked mains every beat but left a feeder alone
    /// whenever `nearest_main_idx` came back `None`. With no main on the
    /// book that is every feeder, so all of them kept whatever path they
    /// were built with and were never recomputed again: zombie straws over
    /// terrain that had changed under them. Their stale cells also drove
    /// `rebuild_claimed`, so the claim followed geometry that no longer
    /// existed.
    #[test]
    fn a_feeder_with_no_main_is_never_left_stale() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..8 {
            w.set_cell(6, y, wet_gravel(&w));
        }

        // A stale straw: a path through cells that are now open air, of the
        // kind a rewalk would never produce.
        let stale = PipePath {
            root: (6, 1),
            cells: vec![(6, 1), (6, 9), (6, 10), (6, 11)],
            mouth: (6, 11),
        };
        PIPE_MEMO.with(|slot| {
            let mut memo = slot.borrow_mut();
            memo.world_id = w.chunk_cache_id.get();
            memo.mains.clear();
            memo.feeders.clear();
            memo.feeders.push(stale.clone());
            memo.reindex();
        });

        rewalk_network(&w);

        let (mains, feeders) = PIPE_MEMO.with(|slot| {
            let memo = slot.borrow();
            (memo.mains.clone(), memo.feeders.clone())
        });
        assert!(
            !feeders.iter().any(|f| f.cells == stale.cells),
            "a feeder with no main kept its stale path: {:?}",
            feeders.iter().map(|f| &f.cells).collect::<Vec<_>>()
        );
        assert!(
            mains.len() + feeders.len() > 0,
            "the root was still a live boiler, so it should have been \
             promoted to a main rather than dropped on the floor"
        );
        for p in mains.iter().chain(feeders.iter()) {
            assert!(
                p.cells.len() >= 2,
                "every retained path needs a real route: {p:?}"
            );
        }
    }

    /// Every feeder must be promoted when there is no main, not just the
    /// first one. Promoting one per rewalk left the rest stale for that
    /// beat, and if that one walk failed the whole book stayed frozen.
    #[test]
    fn all_orphan_feeders_get_a_chance_to_be_promoted() {
        let mut w = plot();
        for x in 0..16 {
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for y in 1..8 {
            w.set_cell(6, y, wet_gravel(&w));
        }
        // First orphan cannot walk at all (root off the loaded world), so
        // the old code consumed it, left mains empty, and froze the rest.
        assert!(
            walk_pipe(&w, (5000, 5000)).cells.len() < 2,
            "setup: the first orphan must fail to promote"
        );
        PIPE_MEMO.with(|slot| {
            let mut memo = slot.borrow_mut();
            memo.world_id = w.chunk_cache_id.get();
            memo.mains.clear();
            memo.feeders.clear();
            memo.feeders.push(PipePath {
                root: (5000, 5000),
                cells: vec![(5000, 5000), (5000, 5001)],
                mouth: (5000, 5001),
            });
            memo.feeders.push(PipePath {
                root: (6, 1),
                cells: vec![(6, 1), (6, 9)],
                mouth: (6, 9),
            });
            memo.reindex();
        });

        rewalk_network(&w);

        let stats = pipe_network_stats(&w);
        assert!(
            stats.mains > 0,
            "a promotable orphan behind an unpromotable one must still \
             become a main: mains={} feeders={}",
            stats.mains,
            stats.feeders
        );
    }

    #[test]
    fn shallow_springs_stay_two_pipes() {
        let mut w = plot();
        for x in 0..64 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Gap far beyond `PIPE_JOIN_BIAS × climb`: even valuing an existing
        // conduit at several times fresh rock, these two do not share one.
        for &x in &[4, 56] {
            w.set_cell(x, 1, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for &x in &[4, 56] {
            let (hx, hy) = hot.tile_of(x, 1);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let cfg = pipe_cfg();
        for t in 1..=20 {
            w.tick = t;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let stats = pipe_network_stats(&w);
        assert_eq!(
            (stats.mains, stats.feeders),
            (2, 0),
            "a 52-cell gap against a 7-cell climb is its own spring, got \
             {} mains {} feeders",
            stats.mains,
            stats.feeders
        );
    }

    /// Soak regression: "we very soon got plenty of main pipes going to the
    /// surface, wish they would have joined a mainline instead".
    ///
    /// Springs spread across one broad hill share a trunk. The bare
    /// `dist_to_path < surface_dist` rule weighed reaching a straw through
    /// rock against a plain vertical count, so every spring preferred its
    /// own bore and the soak grew parallel mains to the cap.
    #[test]
    fn springs_across_one_hill_share_a_mainline() {
        let mut w = plot();
        for x in 0..64 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            for y in 8..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // A 20-cell gap against a 7-cell climb: farther than the bare
        // vertical count, well inside `PIPE_JOIN_BIAS × climb`. This is the
        // case that decides trunk-vs-needles, and the soak got needles.
        let springs = [4, 24];
        for &x in &springs {
            w.set_cell(x, 1, wet_gravel(&w));
        }
        let mut hot = temp_at(&w, 20.0);
        for &x in &springs {
            let (hx, hy) = hot.tile_of(x, 1);
            hot.set_tile_c(hx, hy, 150.0);
        }
        let cfg = pipe_cfg();
        for t in 1..=30u64 {
            w.tick = t * cfg.pipe_beat;
            apply_pipe_motor(&mut w, &mut hot, &cfg, None);
        }
        let stats = pipe_network_stats(&w);
        assert_eq!(
            stats.mains, 1,
            "both springs should ride one trunk, got {} mains {} feeders",
            stats.mains, stats.feeders
        );
        assert_eq!(
            stats.feeders, 1,
            "the second spring should be a feeder onto that trunk, got {} \
             mains {} feeders",
            stats.mains,
            stats.feeders
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
            let mut c = Cell::solid(MaterialId::Sand);
            c.pore = 255;
            c.sat = Sat(2);
            w.set_cell(x, 1, c);
        }
        let mut src = Cell::solid(MaterialId::Sand);
        src.pore = 255;
        src.sat = Sat(30);
        w.set_cell(12, 1, src);
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
        let before_dst = w.get_cell(4, 1).unwrap().sat.0;
        let before_sum = sat_sum(&w);
        pump_water_along(&mut w, &path, 6);
        let after_src = w.get_cell(12, 1).unwrap().sat.0;
        let after_dst = w.get_cell(4, 1).unwrap().sat.0;
        assert!(after_src < before_src, "feeder sat {before_src} → {after_src}");
        assert!(
            after_dst > before_dst,
            "water should arrive at the main, dest {before_dst} → {after_dst}"
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
    fn feeder_pump_carries_solute_downstream_and_is_mass_flat() {
        use crate::audit::mineral_total;
        use crate::mineral::{add_dissolved, dissolved_at};
        let mut w = plot();
        let expand = PHASE_EXPANSION_DRIVE;
        w.pipe_expand = expand;
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        // Sand column so the pump has real capacity and receiving room.
        for y in 1..=4 {
            let mut c = Cell::solid(MaterialId::Sand);
            let cap = water_capacity_cell(c, &w.hydro);
            c.sat = Sat(cap / 2);
            w.set_cell(4, y, c);
        }
        // Load sits on the mid-column donor. pump_water_along should
        // carry it up alongside the water it moves.
        add_dissolved(&mut w, 4, 2, 40);
        let before_mineral = mineral_total(&w);
        let before_at_donor = dissolved_at(&w, 4, 2);
        let path = PipePath {
            root: (4, 1),
            cells: vec![(4, 1), (4, 2), (4, 3), (4, 4)],
            mouth: (4, 4),
        };
        for _ in 0..8 {
            pump_water_along(&mut w, &path, 8);
        }
        let donor_after = dissolved_at(&w, 4, 2);
        let downstream: u32 = (3..=4)
            .map(|y| dissolved_at(&w, 4, y) as u32)
            .sum();
        assert!(
            donor_after < before_at_donor,
            "donor should shed solute as water leaves ({before_at_donor} → {donor_after})"
        );
        assert!(
            downstream > 0,
            "downstream cells should hold the shed load (downstream={downstream})"
        );
        assert_eq!(
            mineral_total(&w),
            before_mineral,
            "solute + rock mineral must stay flat"
        );
    }

    #[test]
    fn sealed_mouth_deposits_cavity_humidity_not_sky_h() {
        let mut w = plot();
        let expand = PHASE_EXPANSION_DRIVE;
        w.pipe_expand = expand;
        for x in 0..16 {
            for y in 1..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        // Sealed cavity: 3×3 Air under a stone roof, surrounded by stone.
        for x in 4..=6 {
            for y in 3..=5 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Boiler cell just below the cavity roof-floor.
        w.set_cell(4, 2, wet_gravel(&w));
        let mut hot = temp_at(&w, 150.0);
        let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 64);
        let before_h = h.total_mass();
        let path = PipePath {
            root: (4, 2),
            cells: vec![(4, 2), (4, 3)],
            mouth: (4, 3),
        };
        let puff = u32::from(expand) * 4;
        set_live(&mut w, 4, 2, puff, 150.0);
        let before_cells = sat_totals(&w).cell_total;
        pulse_path(&mut w, &mut hot, &path, puff, expand, 100.0, PIPE_SIDES, Some(&mut h));
        assert!(
            crate::steam::steam_at(&w, 4, 3) > 0,
            "sealed void should hold cavity humidity, steam={}",
            crate::steam::steam_at(&w, 4, 3)
        );
        assert!(
            (h.total_mass() - before_h).abs() < 0.5,
            "sealed steam must not leak to sky H (H {before_h} → {})",
            h.total_mass()
        );
        assert_eq!(
            sat_totals(&w).cell_total,
            before_cells,
            "cavity route is mass-flat"
        );
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
