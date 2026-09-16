//! Legacy leftover-pressure motor.
//!
//! The pre-pipe steam engine: a Dijkstra pressure field over the hot body
//! that picks a least-resistance route to the surface, pins a chimney, and
//! pushes volume along it. Superseded by the cell pipe in [`crate::pipe`],
//! which owns the `P` overlay and the boiling loop whenever
//! `SteamConfig::enable_pipe` is set — see `docs/VOXEL_PIPE.md`.
//!
//! Kept because `enable_leftover_field` still selects it and a body of tests
//! pins its behaviour. Split out of `steam.rs` so the surviving steam code —
//! the sky probe, vessel classification, cavity humidity and haze, all of
//! which the pipe does use — is readable on its own.

use super::*;


/// Leftover volume spreading from boiling pores along least resistance.
#[derive(Default)]
pub(super) struct LeftoverMemo {
    pub(super) world_id: u64,
    pub(super) tick: u64,
    pub(super) boil_bits: u32,
    pub(super) expand: u16,
    pub(super) map: FxHashMap<(i32, i32), f32>,
    /// Overlay-only leftover P (block contour). Straw still reads `map`.
    pub(super) view: FxHashMap<(i32, i32), f32>,
    pub(super) view_ready: bool,
    /// Zone outlets `(x, y, zone head)`. Adjacent boiling cells share
    /// one head — not a grid of tiny seeds.
    pub(super) seeds: Vec<(i32, i32, u32)>,
    pub(super) seed_zone: Vec<(i32, i32)>,
    /// Undischarged leftover per zone id (deepest cell). Grows until
    /// relief; drops when a straw vents or fills vadose.
    pub(super) heads: FxHashMap<(i32, i32), u32>,
    pub(super) released: FxHashMap<(i32, i32), u32>,
    /// Boiling wet cells that share a leftover vessel. Vadose park
    /// inside this set is not relief — the hole is still the boiler.
    pub(super) zone: FxHashSet<(i32, i32)>,
    /// Exterior leftover-arm parents (child → parent). Same tree the
    /// P overlay paints — heat and the straw follow this channel.
    pub(super) parents: FxHashMap<(i32, i32), (i32, i32)>,
    /// Path cost from the zone rim, used to pick one winning mouth.
    pub(super) costs: FxHashMap<(i32, i32), u32>,
    /// Winner-takes-all next hop (from → to) along the locked chimney.
    pub(super) route_next: FxHashMap<(i32, i32), (i32, i32)>,
    /// Persisted chimney. Survives leftover rebuilds so a grain of sand
    /// cannot retarget the spring. Cleared only when the pipe is sealed
    /// or the vessel is gone. Sinter / wear along the route does not
    /// retarget — pulse erosion is how the planned path opens.
    pub(super) pin_path: Vec<(i32, i32)>,
    pub(super) pin_next: FxHashMap<(i32, i32), (i32, i32)>,
    pub(super) pin_seed: (i32, i32),
    pub(super) pin_id: (i32, i32),
    /// Path cells that were loose when pinned (debug / overlay).
    pub(super) pin_loose: Vec<bool>,
    /// `route_next` / `pin_next` keys **and** dests. Straw membership
    /// used to walk both maps every hop (`O(pin)`).
    pub(super) route_set: FxHashSet<(i32, i32)>,
    /// Last rebuild reused the live pin (skipped exterior halo / Dijkstra).
    pub(super) reused_pin: bool,
    /// Last rebuild grew last tick's zone instead of walking the body.
    pub(super) reused_zone: bool,
    pub(super) last_cands_us: u32,
    pub(super) last_flood_us: u32,
    pub(super) last_lock_us: u32,
}

/// Soak / F1 counters for leftover + sky-probe growth.
#[derive(Debug, Clone, Copy, Default)]
pub struct LeftoverSoakStats {
    pub zone: usize,
    pub pin: usize,
    pub map: usize,
    pub route_set: usize,
    pub reused_pin: bool,
    pub reused_zone: bool,
    pub sky_topo: u64,
    pub probe_confined: usize,
    pub probe_open: usize,
    pub probe_boiler: usize,
    pub steam_cells: usize,
    pub cands_us: u32,
    pub flood_us: u32,
    pub lock_us: u32,
}

/// Exterior leftover arms from a zone boundary.
pub(super) const LEFTOVER_FIELD_CELLS: usize = 4096;

/// Heat tiles are 4×4. A newly boiling tile next to last tick's vessel
/// is still this vessel, not a second boiler.
pub(super) const LEFTOVER_GROW_RADIUS: i32 = 4;

/// Exterior Dijkstra budget for the planned sky walk. Only the high
/// upward rim is seeded — a huge perimeter next to the ocean must not
/// spend this on a 20-cell halo.
pub(super) const LEFTOVER_PLAN_CELLS: usize = 16384;

/// Faint P-overlay floor for the planned pin. Visible magenta, not the
/// banned hidden 0.02 pack. Used when leftover head has not charged the
/// cell yet.
pub(super) const LEFTOVER_PLAN_TRACE: f32 = 0.26;

/// Fade leftover *display* by path cost along relief arms.
pub(super) const LEFTOVER_COST_FADE: f32 = 32.0;

/// Magenta pin floor so a cool gravel chimney still reads as a line.
pub(super) const LEFTOVER_PIN_PACK: f32 = 0.58;

/// Leftover zone cells and pinned chimney length (HUD / soak counters).
pub fn leftover_field_stats(world: &World) -> (usize, usize) {
    let s = leftover_soak_stats(world);
    (s.zone, s.pin)
}

/// Leftover body + sky-probe sizes. Grows with soak if leftover heat
/// enlarges the 100 °C vessel or Air topology keeps invalidating probes.
pub fn leftover_soak_stats(world: &World) -> LeftoverSoakStats {
    let id = world.chunk_cache_id.get();
    let mut stats = LeftoverSoakStats {
        sky_topo: world.sky_topo_gen,
        steam_cells: world.steam.len(),
        ..LeftoverSoakStats::default()
    };
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        if memo.world_id != id {
            return;
        }
        stats.zone = memo.zone.len();
        stats.pin = memo.pin_path.len();
        stats.map = memo.map.len();
        stats.route_set = memo.route_set.len();
        stats.reused_pin = memo.reused_pin;
        stats.reused_zone = memo.reused_zone;
        stats.cands_us = memo.last_cands_us;
        stats.flood_us = memo.last_flood_us;
        stats.lock_us = memo.last_lock_us;
    });
    SKY_PROBE.with(|slot| {
        let c = slot.borrow();
        stats.probe_confined = c.confined.len();
        stats.probe_open = c.open.len();
        stats.probe_boiler = c.boiler.len();
    });
    stats
}

pub(super) fn leftover_field_lookup(world: &World, gx: i32, gy: i32, boil_c: f32, expand: u16) -> f32 {
    let gx = world.wrap_x(gx);
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        if !leftover_memo_bound(&memo, world, boil_c, expand) {
            return 0.0;
        }
        if memo.view_ready {
            memo.view.get(&(gx, gy)).copied().unwrap_or(0.0)
        } else {
            memo.map.get(&(gx, gy)).copied().unwrap_or(0.0)
        }
    })
}

pub(super) fn leftover_field_bound(world: &World, boil_c: f32, expand: u16) -> bool {
    LEFTOVER_MEMO.with(|slot| leftover_memo_bound(&slot.borrow(), world, boil_c, expand))
}

pub(super) fn leftover_memo_bound(memo: &LeftoverMemo, world: &World, boil_c: f32, expand: u16) -> bool {
    let boil_bits = if boil_c.is_finite() {
        boil_c.to_bits()
    } else {
        BOIL_POINT_C.to_bits()
    };
    memo.world_id == world.chunk_cache_id.get()
        && memo.boil_bits == boil_bits
        && memo.expand == expand.max(1)
}

#[allow(dead_code)]
pub(super) fn leftover_step_cost(perm: u8, dy: i32) -> u32 {
    leftover_step_cost_ex(perm, 0, dy, false)
}

pub(super) fn leftover_is_loose(mat: MaterialId) -> bool {
    is_grain(mat) || matches!(mat, MaterialId::LooseRock | MaterialId::Gravel)
}

/// Mouth sinter (gravel → sandstone / flowstone) is still the chimney.
/// Only sealed stone / bedrock should break a live pin.
pub(super) fn leftover_is_chimney_skin(mat: MaterialId) -> bool {
    leftover_is_loose(mat)
        || matches!(
            mat,
            MaterialId::Sandstone | MaterialId::Conglomerate | MaterialId::Flowstone
        )
}

/// First real dump: open weather above this cell.
///
/// Side-of-cliff sky must not cut a chimney that is still climbing.
/// After this fires, leftover must not walk ridge sand back into the
/// mountain above or below the mouth.
pub(super) fn leftover_has_upward_mouth(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if leftover_is_surface_mouth(world, nx, ny, n) {
            return true;
        }
    }
    false
}

pub(super) fn leftover_touches_chimney_skin(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [
        (0, 1),
        (0, -1),
        (-1, 0),
        (1, 0),
        (-1, 1),
        (1, 1),
        (-1, -1),
        (1, -1),
    ] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if world
            .get_cell(nx, ny)
            .is_some_and(|c| leftover_is_chimney_skin(c.material))
        {
            return true;
        }
    }
    false
}

/// Pin / straw dump: open weather, or pipe that already sees sky above.
pub(super) fn leftover_is_dump_cell(world: &World, gx: i32, gy: i32) -> bool {
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if leftover_is_surface_mouth(world, gx, gy, cell) {
        return true;
    }
    leftover_is_chimney_skin(cell.material) && leftover_has_upward_mouth(world, gx, gy)
}

/// Loose / sinter that already sees weather — the pipe, not the vessel.
///
/// Buried gravel / sand / loose rock surrounded by stone stays in the
/// chamber. A ridge chimney that reaches sky must not join the zone
/// (that was dest-pick walking sand back into the mountain).
pub(super) fn leftover_is_open_pipe(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    // Packed stone / bedrock cannot be a sky pipe. Caching every
    // interior neighbour of a 28k vessel was leftover's HashMap tax.
    if cell.material != MaterialId::Air && !leftover_is_chimney_skin(cell.material) {
        return false;
    }
    let id = world.chunk_cache_id.get();
    let tick = world.tick;
    if let Some(hit) = OPEN_PIPE.with(|slot| {
        let c = slot.borrow();
        if c.world_id == id && c.tick == tick {
            c.map.get(&(gx, gy)).copied()
        } else {
            None
        }
    }) {
        return hit;
    }
    let hit = leftover_is_open_pipe_uncached(world, gx, gy);
    OPEN_PIPE.with(|slot| {
        let mut c = slot.borrow_mut();
        if c.world_id != id || c.tick != tick {
            c.world_id = id;
            c.tick = tick;
            c.map.clear();
        }
        c.map.insert((gx, gy), hit);
    });
    hit
}

pub(super) fn leftover_is_open_pipe_uncached(world: &World, gx: i32, gy: i32) -> bool {
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if leftover_is_dump_cell(world, gx, gy) {
        return true;
    }
    if !leftover_is_chimney_skin(cell.material) {
        return false;
    }
    if leftover_has_upward_mouth(world, gx, gy) {
        return true;
    }
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    let mut q = vec![(gx, gy)];
    seen.insert((gx, gy));
    let mut i = 0;
    while i < q.len() && i < 128 {
        let (x, y) = q[i];
        i += 1;
        if leftover_is_dump_cell(world, x, y) || leftover_has_upward_mouth(world, x, y) {
            return true;
        }
        for (dx, dy) in [
            (0, 1),
            (0, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (1, 1),
            (-1, -1),
            (1, -1),
        ] {
            let nx = world.wrap_x(x + dx);
            let ny = y + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            let Some(n) = world.get_cell(nx, ny) else {
                continue;
            };
            if n.material == MaterialId::Air {
                return true;
            }
            if leftover_is_chimney_skin(n.material) {
                q.push((nx, ny));
            }
        }
    }
    false
}

pub(super) fn leftover_touches_weather_lake(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(0, 1), (0, -1), (-1, 0), (1, 0), (-1, 1), (1, 1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if leftover_is_weather_lake(world, nx, ny, n) {
            return true;
        }
    }
    false
}

pub(super) fn leftover_pipe_touches_air(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if world
            .get_cell(nx, ny)
            .is_some_and(|n| n.material == MaterialId::Air)
        {
            return true;
        }
    }
    false
}

/// Enclosed loose / sinter / roofed void — chamber fill, not a sky pipe.
pub(super) fn leftover_is_chamber_fill(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if leftover_is_open_pipe(world, gx, gy) {
        return false;
    }
    if cell.material == MaterialId::Air {
        return leftover_is_boiler_path(world, gx, gy, cell);
    }
    leftover_is_chimney_skin(cell.material)
}

pub(super) fn leftover_step_cost_ex(perm: u8, dx: i32, dy: i32, loose: bool) -> u32 {
    let resist = 1u32 + (256 / (perm.max(1) as u32));
    let vert = if dy > 0 {
        0
    } else if dy < 0 {
        6
    } else {
        2
    };
    let mut cost = resist + vert;
    if dx != 0 && dy != 0 {
        // Diagonal stairs were why leftover "explored everywhere"
        // instead of climbing the chimney.
        cost = cost.saturating_add(6);
    }
    if loose {
        cost = cost / 4 + 1;
    }
    cost
}

pub(super) fn leftover_note_parent(
    parents: &mut FxHashMap<(i32, i32), (i32, i32)>,
    child: (i32, i32),
    parent: (i32, i32),
) {
    parents.entry(child).or_insert(parent);
}

pub(super) fn leftover_push_exterior_neighbors(
    world: &World,
    gx: i32,
    gy: i32,
    remaining: u32,
    cost: u32,
    in_zone: &FxHashSet<(i32, i32)>,
    heap: &mut BinaryHeap<(Reverse<u32>, i32, i32, u32)>,
    parents: &mut FxHashMap<(i32, i32), (i32, i32)>,
) {
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if in_zone.contains(&(nx, ny)) {
            continue;
        }
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if n.material == MaterialId::Bedrock {
            continue;
        }
        if n.material == MaterialId::Air {
            if leftover_is_surface_mouth(world, nx, ny, n) {
                continue;
            }
            if leftover_is_boiler_path(world, nx, ny, n) {
                leftover_note_parent(parents, (nx, ny), (gx, gy));
                heap.push((Reverse(cost.saturating_add(1)), nx, ny, remaining));
            }
            continue;
        }
        let perm = permeability_cell(n, &world.hydro);
        if perm == 0 && water_capacity_cell(n, &world.hydro) == 0 {
            continue;
        }
        let step = leftover_step_cost_ex(perm.max(1), dx, dy, leftover_is_loose(n.material));
        leftover_note_parent(parents, (nx, ny), (gx, gy));
        heap.push((Reverse(cost.saturating_add(step)), nx, ny, remaining));
    }
}

/// Rim cells of the leftover vessel. `up` faces a non-bedrock neighbor
/// above; `all` is every zone cell that touches the exterior.
pub(super) fn leftover_plan_rim_cells(world: &World, memo: &LeftoverMemo) -> (Vec<(i32, i32)>, Vec<(i32, i32)>) {
    let mut up: Vec<(i32, i32)> = Vec::new();
    let mut all: Vec<(i32, i32)> = Vec::new();
    for &(gx, gy) in &memo.zone {
        let mut is_rim = false;
        let mut faces_up = false;
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
            let nx = world.wrap_x(gx + dx);
            let ny = gy + dy;
            if memo.zone.contains(&(nx, ny)) {
                continue;
            }
            is_rim = true;
            if dy > 0
                && world
                    .get_cell(nx, ny)
                    .is_some_and(|n| n.material != MaterialId::Bedrock)
            {
                faces_up = true;
            }
        }
        if !is_rim {
            continue;
        }
        all.push((gx, gy));
        if faces_up {
            up.push((gx, gy));
        }
    }
    (up, all)
}

/// Highest upward-facing rim cells. The leftover vessel is the boiling
/// body — its rim is the 100 °C isotherm. A 64-cell band along that
/// front spends the plan budget on a halo and never climbs cold rock
/// to the crest. One peak, then a corridor.
pub(super) fn leftover_plan_high_upward_rim(world: &World, memo: &LeftoverMemo) -> Vec<(i32, i32)> {
    let (mut up, rim) = leftover_plan_rim_cells(world, memo);
    if up.is_empty() {
        return rim;
    }
    let max_y = up.iter().map(|&(_, y)| y).max().unwrap_or(0);
    up.retain(|&(x, y)| y == max_y && !leftover_touches_weather_lake(world, x, y));
    if up.is_empty() {
        return rim;
    }
    if up.len() > 4 {
        up.sort_by(|a, b| a.0.cmp(&b.0));
        up.truncate(4);
    }
    up
}

/// Planner step: climb first. Sideways / down around a wide 100 °C
/// body is how the faint sky trace never left the hot reservoir.
pub(super) fn leftover_plan_step_cost(perm: u8, dx: i32, dy: i32, loose: bool) -> u32 {
    let mut cost = leftover_step_cost_ex(perm, dx, dy, loose);
    if dy <= 0 {
        cost = cost.saturating_add(if dy == 0 { 32 } else { 96 });
    }
    cost
}

pub(super) fn leftover_cells_reach_open_sky(world: &World, cells: &[(i32, i32)]) -> bool {
    for &(x, y) in cells.iter().rev().take(6) {
        let Some(cell) = world.get_cell(x, y) else {
            continue;
        };
        if leftover_is_open_sky_mouth(world, x, y, cell) {
            return true;
        }
        if leftover_has_upward_mouth(world, x, y) {
            for (dx, dy) in [(0, 1), (-1, 1), (1, 1)] {
                let nx = world.wrap_x(x + dx);
                let ny = y + dy;
                let Some(n) = world.get_cell(nx, ny) else {
                    continue;
                };
                if leftover_is_open_sky_mouth(world, nx, ny, n) {
                    return true;
                }
            }
        }
    }
    false
}

pub(super) fn leftover_pin_ends_in_weather_lake(world: &World, memo: &LeftoverMemo) -> bool {
    for &(x, y) in memo.pin_path.iter().rev().take(6) {
        let Some(cell) = world.get_cell(x, y) else {
            continue;
        };
        if leftover_is_weather_lake(world, x, y, cell)
            || leftover_touches_weather_lake(world, x, y)
        {
            return true;
        }
    }
    false
}

pub(super) fn leftover_pin_has_chimney_skin(world: &World, memo: &LeftoverMemo) -> bool {
    memo.pin_path.iter().any(|&(x, y)| {
        world
            .get_cell(x, y)
            .is_some_and(|c| leftover_is_chimney_skin(c.material))
    })
}

/// Foot-ocean dump: a couple of hops into a weather U. A gravel
/// chimney that *fills* its own mouth is not this — leftover emit
/// makes that air wet, and it must stay the pin.
pub(super) fn leftover_pin_is_short_lake_dump(world: &World, memo: &LeftoverMemo) -> bool {
    memo.pin_path.len() < 8
        && leftover_pin_ends_in_weather_lake(world, memo)
        && !leftover_pin_has_chimney_skin(world, memo)
}

/// Punch-through can empty a tiny boiler. Keep a live sky / gravel
/// chimney on P; drop a foot-ocean pin.
pub(super) fn leftover_pin_keep_when_cold(world: &World, memo: &LeftoverMemo) -> bool {
    memo.pin_path.len() >= 2 && !leftover_pin_is_short_lake_dump(world, memo)
}

/// Cheapest walk to weather / a dump, ignoring this tick's leftover head.
///
/// Overlay BFS still fades with remaining volume. The pin must exist
/// before the straw can travel the whole chimney — pressure builds and
/// pulse-erodes this path until it can.
///
/// Do **not** walk the vessel at cost 0. That re-seeds the whole rim,
/// spends [`LEFTOVER_PLAN_CELLS`] on a foot-ocean halo, and never
/// climbs 200 packed seats to the crest.
pub(super) fn leftover_plan_cheapest_mouth(world: &World, memo: &LeftoverMemo) -> Option<Vec<(i32, i32)>> {
    if memo.zone.is_empty() {
        return None;
    }
    let high = leftover_plan_high_upward_rim(world, memo);
    let high = if high.is_empty() {
        memo.zone.iter().copied().collect::<Vec<_>>()
    } else {
        high
    };
    let mut lake: Option<Vec<(i32, i32)>> = None;
    if let Some(path) = leftover_plan_mouths_from_seeds(world, memo, &high, false) {
        if leftover_cells_reach_open_sky(world, &path) {
            return Some(path);
        }
        lake = Some(path);
    }
    let (_, rim) = leftover_plan_rim_cells(world, memo);
    if !rim.is_empty() && rim != high {
        if let Some(path) = leftover_plan_mouths_from_seeds(world, memo, &rim, false) {
            if leftover_cells_reach_open_sky(world, &path) {
                return Some(path);
            }
            if lake.is_none() {
                lake = Some(path);
            }
        }
    }
    if let Some(path) = lake {
        return Some(path);
    }
    leftover_plan_mouths_from_seeds(world, memo, &high, true)
}

pub(super) fn leftover_plan_mouths_from_seeds(
    world: &World,
    memo: &LeftoverMemo,
    seeds: &[(i32, i32)],
    flood_zone: bool,
) -> Option<Vec<(i32, i32)>> {
    if seeds.is_empty() {
        return None;
    }
    let mut heap: BinaryHeap<(Reverse<u32>, i32, i32)> = BinaryHeap::new();
    let mut best_cost: FxHashMap<(i32, i32), u32> = FxHashMap::default();
    let mut parents: FxHashMap<(i32, i32), (i32, i32)> = FxHashMap::default();
    let mut goals: FxHashMap<(i32, i32), u32> = FxHashMap::default();
    for &(gx, gy) in seeds {
        heap.push((Reverse(0), gx, gy));
        best_cost.insert((gx, gy), 0);
    }
    let mut expanded = 0usize;
    while let Some((Reverse(cost), gx, gy)) = heap.pop() {
        if best_cost.get(&(gx, gy)).copied().unwrap_or(u32::MAX) < cost {
            continue;
        }
        if !memo.zone.contains(&(gx, gy)) {
            if expanded >= LEFTOVER_PLAN_CELLS {
                break;
            }
            expanded += 1;
        }
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
            let nx = world.wrap_x(gx + dx);
            let ny = gy + dy;
            let Some(n) = world.get_cell(nx, ny) else {
                continue;
            };
            if n.material == MaterialId::Bedrock {
                continue;
            }
            let in_zone = memo.zone.contains(&(nx, ny));
            if in_zone && !flood_zone {
                continue;
            }
            let pipe_vent = leftover_is_dump_cell(world, nx, ny)
                || (leftover_is_chimney_skin(n.material)
                    && leftover_pipe_touches_air(world, nx, ny));
            if leftover_is_surface_mouth(world, nx, ny, n) || pipe_vent {
                let mut rank = cost.saturating_add(if in_zone { 0 } else { 1 });
                rank = rank
                    .saturating_add(8_000)
                    .saturating_sub((ny.max(0) as u32).saturating_mul(120));
                if leftover_is_weather_lake(world, nx, ny, n)
                    || leftover_touches_weather_lake(world, nx, ny)
                {
                    rank = rank.saturating_add(6_000);
                }
                if rank < goals.get(&(nx, ny)).copied().unwrap_or(u32::MAX) {
                    goals.insert((nx, ny), rank);
                    parents.insert((nx, ny), (gx, gy));
                }
                continue;
            }
            let step = if in_zone {
                0
            } else if n.material == MaterialId::Air {
                if leftover_is_boiler_path(world, nx, ny, n) {
                    1
                } else {
                    continue;
                }
            } else if leftover_pin_cell_walkable(world, nx, ny) {
                leftover_plan_step_cost(
                    permeability_cell(n, &world.hydro).max(1),
                    dx,
                    dy,
                    leftover_is_loose(n.material),
                )
            } else {
                continue;
            };
            let nc = cost.saturating_add(step);
            if nc < best_cost.get(&(nx, ny)).copied().unwrap_or(u32::MAX) {
                best_cost.insert((nx, ny), nc);
                parents.insert((nx, ny), (gx, gy));
                heap.push((Reverse(nc), nx, ny));
            }
        }
    }
    let sky_only: Vec<((i32, i32), u32)> = goals
        .iter()
        .filter_map(|(&(x, y), &c)| {
            let n = world.get_cell(x, y)?;
            if leftover_is_weather_lake(world, x, y, n)
                || leftover_touches_weather_lake(world, x, y)
            {
                None
            } else {
                Some(((x, y), c))
            }
        })
        .collect();
    let pool = if sky_only.is_empty() {
        goals.iter().map(|(&p, &c)| (p, c)).collect::<Vec<_>>()
    } else {
        sky_only
    };
    let ((mx, my), _) = *pool.iter().min_by_key(|((x, y), c)| (*c, Reverse(*y), *x))?;
    let mut path = vec![(mx, my)];
    let mut cur = (mx, my);
    for _ in 0..2048 {
        let Some(&parent) = parents.get(&cur) else {
            break;
        };
        path.push(parent);
        cur = parent;
        if memo.zone.contains(&parent) {
            break;
        }
    }
    path.reverse();
    if let Some(cut) = path
        .iter()
        .position(|&(x, y)| leftover_is_dump_cell(world, x, y))
    {
        path.truncate(cut + 1);
    }
    if path.len() < 2 || !memo.zone.contains(&path[0]) {
        return None;
    }
    Some(path)
}

pub(super) fn leftover_apply_locked_path(world: &World, memo: &mut LeftoverMemo, path: Vec<(i32, i32)>) {
    if path.len() < 2 || !memo.zone.contains(&path[0]) {
        return;
    }
    memo.route_next.clear();
    for w in path.windows(2) {
        memo.route_next.insert(w[0], w[1]);
    }
    let (ex, ey) = path[path.len() - 1];
    let mut mouth: Option<(i32, i32)> = None;
    let mut mouth_score = i32::MIN;
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0)] {
        let nx = world.wrap_x(ex + dx);
        let ny = ey + dy;
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if leftover_is_surface_mouth(world, nx, ny, n) {
            let s = 1_000 + dy.max(0) * 200 + n.sat.0 as i32;
            if s > mouth_score {
                mouth_score = s;
                mouth = Some((nx, ny));
            }
        }
    }
    if let Some(m) = mouth {
        memo.route_next.insert((ex, ey), m);
    }
    let first_ext = path.iter().copied().find(|p| !memo.zone.contains(p));
    let (sx, sy) = path
        .iter()
        .rev()
        .copied()
        .find(|&(x, y)| {
            memo.zone.contains(&(x, y)) && first_ext.is_some_and(|(ex, ey)| x == ex || y == ey)
        })
        .unwrap_or(path[0]);
    if let Some(ext) = first_ext {
        memo.route_next.insert((sx, sy), ext);
    }
    let head = memo.heads.values().copied().max().unwrap_or(1).max(1);
    let id = memo
        .heads
        .keys()
        .copied()
        .find(|k| memo.zone.contains(k))
        .unwrap_or((sx, sy));
    leftover_stitch_zone_route(world, memo, id, (sx, sy));
    memo.seeds.clear();
    memo.seed_zone.clear();
    memo.seeds.push((id.0, id.1, head));
    memo.seed_zone.push(id);
    leftover_refresh_route_set(memo);
}

/// Walk leftover through the vessel to the planned exit so the straw
/// starts at the boiler, not the highest rim (that emptied the box).
pub(super) fn leftover_stitch_zone_route(
    world: &World,
    memo: &mut LeftoverMemo,
    from: (i32, i32),
    to: (i32, i32),
) {
    if from == to || !memo.zone.contains(&from) || !memo.zone.contains(&to) {
        return;
    }
    // BFS through the vessel. Greedy 64-hop used to die in a huge
    // packed hill before the straw ever reached the planned rim.
    let mut q = vec![from];
    let mut came: FxHashMap<(i32, i32), (i32, i32)> = FxHashMap::default();
    came.insert(from, from);
    let mut i = 0usize;
    while i < q.len() {
        let cur = q[i];
        i += 1;
        if cur == to {
            break;
        }
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
            let nx = world.wrap_x(cur.0 + dx);
            let ny = cur.1 + dy;
            if !memo.zone.contains(&(nx, ny)) {
                continue;
            }
            if came.contains_key(&(nx, ny)) {
                continue;
            }
            came.insert((nx, ny), cur);
            q.push((nx, ny));
        }
    }
    if !came.contains_key(&to) {
        return;
    }
    let mut walk = vec![to];
    let mut cur = to;
    for _ in 0..q.len().saturating_add(2) {
        let Some(&prev) = came.get(&cur) else {
            break;
        };
        if prev == cur {
            break;
        }
        walk.push(prev);
        cur = prev;
    }
    walk.reverse();
    for w in walk.windows(2) {
        memo.route_next.entry(w[0]).or_insert(w[1]);
    }
}

/// Pick one chimney: cheapest walk to weather (head does not have to
/// finish it this tick). Fall back to the highest reached loose cell.
pub(super) fn leftover_lock_winning_route(world: &World, memo: &mut LeftoverMemo) {
    memo.route_next.clear();
    if memo.zone.is_empty() {
        return;
    }
    if let Some(path) = leftover_plan_cheapest_mouth(world, memo) {
        leftover_apply_locked_path(world, memo, path);
        return;
    }
    if memo.parents.is_empty() {
        return;
    }
    let mut best: Option<(i32, i32, i32)> = None; // score, x, y
    for (&(gx, gy), _) in &memo.map {
        if memo.zone.contains(&(gx, gy)) {
            continue;
        }
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if cell.material == MaterialId::Bedrock {
            continue;
        }
        let cost = memo.costs.get(&(gx, gy)).copied().unwrap_or(u32::MAX);
        let mut score = gy * 80 - cost.min(8_000) as i32;
        let pipe = leftover_is_chimney_skin(cell.material);
        if leftover_is_loose(cell.material) {
            score += 40_000;
        }
        let weather_up = leftover_has_upward_mouth(world, gx, gy);
        let mut weather_any = weather_up;
        for dx in [-1, 1] {
            let nx = world.wrap_x(gx + dx);
            let ny = gy;
            let Some(n) = world.get_cell(nx, ny) else {
                continue;
            };
            if leftover_is_surface_mouth(world, nx, ny, n) {
                weather_any = true;
            }
        }
        // Pipe→sky is the chimney. Among those, the first upward dump
        // (low cost) beats ridge sand that walks back into the mountain.
        if pipe && weather_up {
            score = 500_000 - cost.min(8_000) as i32;
        } else if pipe && weather_any {
            score = 400_000 - cost.min(8_000) as i32;
        } else if weather_up {
            score += 200_000;
        }
        if best.is_none_or(|(s, _, _)| score > s) {
            best = Some((score, gx, gy));
        }
    }
    let Some((_, mx, my)) = best else {
        return;
    };
    let mut path: Vec<(i32, i32)> = vec![(mx, my)];
    let mut cur = (mx, my);
    for _ in 0..2048 {
        let Some(&parent) = memo.parents.get(&cur) else {
            break;
        };
        path.push(parent);
        cur = parent;
        if memo.zone.contains(&parent) {
            break;
        }
    }
    path.reverse();
    // First upward dump wins. Walking past it onto ridge sand is the
    // playtest "back into the mountain" gymnastics.
    if let Some(cut) = path.iter().position(|&(x, y)| leftover_is_dump_cell(world, x, y))
    {
        path.truncate(cut + 1);
    }
    if path.len() < 2 || !memo.zone.contains(&path[0]) {
        return;
    }
    let mouth_loose = world
        .get_cell(mx, my)
        .is_some_and(|c| leftover_is_chimney_skin(c.material));
    let path_has_loose = path.iter().any(|&(x, y)| {
        world
            .get_cell(x, y)
            .is_some_and(|c| leftover_is_chimney_skin(c.material))
    });
    // Fallback: the pressure-limited halo already reached this cell.
    // Pin a loose chimney, or packed stone that already sees weather —
    // a huge 150 °C body must not drop a sky walk just because the
    // cheapest seat is still competent rock.
    if !path_has_loose && !mouth_loose {
        let mouth_sky = leftover_has_upward_mouth(world, mx, my)
            || world
                .get_cell(mx, my)
                .is_some_and(|c| leftover_is_surface_mouth(world, mx, my, c));
        if !mouth_sky {
            return;
        }
    }
    leftover_apply_locked_path(world, memo, path);
}

pub(super) fn leftover_forced_route_dest(
    world: &World,
    gx: i32,
    gy: i32,
    seen: &FxHashSet<(i32, i32)>,
) -> Option<(i32, i32, i32, Cell, bool)> {
    let gx = world.wrap_x(gx);
    let id = world.chunk_cache_id.get();
    let next = LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        if memo.world_id != id {
            return None;
        }
        memo.route_next
            .get(&(gx, gy))
            .or_else(|| memo.pin_next.get(&(gx, gy)))
            .copied()
    })?;
    if seen.contains(&next) {
        return None;
    }
    let Some(dst) = world.get_cell(next.0, next.1) else {
        return None;
    };
    if dst.material == MaterialId::Bedrock {
        return None;
    }
    let dx = (next.0 - gx).abs();
    let dy = (next.1 - gy).abs();
    if dx > 1 || dy > 2 || (dx == 0 && dy == 0) {
        return None;
    }
    let overflow = leftover_is_surface_mouth(world, next.0, next.1, dst)
        && water_capacity_cell(dst, &world.hydro).saturating_sub(dst.sat.0) == 0;
    Some((i32::MAX, next.0, next.1, dst, overflow))
}

/// Packed leftover seats with room, before leftover mass dumps at a mouth.
///
/// A pin that already sees weather would otherwise skip the water table
/// and empty the hill into the lake. Leftover pressure *is* that leftover
/// water — park it in packed vadose first.
pub(super) fn leftover_table_before_mouth(
    world: &World,
    gx: i32,
    gy: i32,
    seen: &FxHashSet<(i32, i32)>,
) -> Option<(i32, i32, i32, Cell, bool)> {
    // A loose chimney mouth with a side-stone hole must still dump
    // (playtest 2/19 next to the gravel spring). Packed leftover hills
    // park in vadose first even when weather is one cell away.
    if world
        .get_cell(gx, gy)
        .is_some_and(|c| leftover_is_loose(c.material))
        && leftover_has_upward_mouth(world, gx, gy)
    {
        return None;
    }
    let mouth_dump = leftover_forced_route_dest(world, gx, gy, seen)
        .is_some_and(|f| leftover_is_surface_mouth(world, f.1, f.2, f.3));
    let mut best: Option<(i32, i32, i32, Cell, bool)> = None;
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        if seen.contains(&(tx, ty)) {
            continue;
        }
        let Some(dst) = world.get_cell(tx, ty) else {
            continue;
        };
        if dst.material == MaterialId::Air || leftover_is_loose(dst.material) {
            continue;
        }
        let cap = water_capacity_cell(dst, &world.hydro);
        if cap == 0 || dst.sat.0 >= cap {
            continue;
        }
        if permeability_cell(dst, &world.hydro) == 0 {
            continue;
        }
        let on_pin = leftover_on_route(world, tx, ty);
        if !on_pin && !mouth_dump {
            continue;
        }
        let score = if on_pin { 2_000 } else { 0 } + dy * 40 - dx.abs();
        let row = (score, tx, ty, dst, false);
        if best.as_ref().is_none_or(|b| row.0 > b.0) {
            best = Some(row);
        }
    }
    best
}

/// Weather mouth: open sky or a true lake (U-bowl / `!boiler`).
///
/// Water-filled **boilers** are a path hop — air above that pool still
/// leads out, so the straw keeps walking. Discharge only into a vessel
/// that survives the same weather-vs-boiler test as flash.
pub(super) fn leftover_is_weather_relief(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if cell.material != MaterialId::Air {
        return false;
    }
    if !void_is_confined(world, gx, gy) {
        return true;
    }
    if vessel_is_boiler(world, gx, gy) {
        return false;
    }
    air_void_open_to_sky(world, gx, gy)
        || crate::rules::is_standing_water(world, gx, gy)
        || cell.sat.0 > STEAM_VOID_SAT_MAX
}

pub(super) fn leftover_is_weather_lake(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    leftover_is_weather_relief(world, gx, gy, cell)
        && (crate::rules::is_standing_water(world, gx, gy) || cell.sat.0 > STEAM_VOID_SAT_MAX)
}

/// True dump: open sky or an unconfined weather U / ocean.
///
/// Roofed standing water is a path hop even when a side shaft reaches
/// weather. Rain over an open mouth is a dump — leftover heat stops
/// there instead of punching through the sheet.
pub(super) fn leftover_is_surface_mouth(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if cell.material != MaterialId::Air {
        return false;
    }
    // Unroofed Air is weather. The 48-up probe is enough — a 192-cell
    // vessel BFS here was leftover's soak tax on every zone-rim sky cell.
    if !void_is_confined(world, gx, gy) {
        return true;
    }
    if vessel_is_boiler(world, gx, gy) {
        return false;
    }
    let wet = crate::rules::is_standing_water(world, gx, gy) || cell.sat.0 > STEAM_VOID_SAT_MAX;
    if wet {
        return false;
    }
    air_void_open_to_sky(world, gx, gy)
}

/// Open sky / unroofed air — not a weather lake at the hill foot.
pub(super) fn leftover_is_open_sky_mouth(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    leftover_is_surface_mouth(world, gx, gy, cell)
        && !leftover_is_weather_lake(world, gx, gy, cell)
        && !leftover_touches_weather_lake(world, gx, gy)
}

/// Heat dump at the first open / flowing water the chimney hits.
pub(super) fn leftover_is_heat_sink(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    leftover_is_surface_mouth(world, gx, gy, cell)
        || leftover_is_weather_lake(world, gx, gy, cell)
        || (cell.material == MaterialId::Air
            && cell.sat.0 > 0
            && !vessel_is_boiler(world, gx, gy)
            && !void_is_confined(world, gx, gy))
}

pub(super) fn leftover_refresh_route_set(memo: &mut LeftoverMemo) {
    memo.route_set.clear();
    for (&from, &to) in &memo.route_next {
        memo.route_set.insert(from);
        memo.route_set.insert(to);
    }
    for (&from, &to) in &memo.pin_next {
        memo.route_set.insert(from);
        memo.route_set.insert(to);
    }
}

pub(super) fn leftover_clear_pin(memo: &mut LeftoverMemo) {
    memo.pin_path.clear();
    memo.pin_next.clear();
    memo.pin_loose.clear();
    memo.pin_seed = (0, 0);
    memo.pin_id = (0, 0);
    leftover_refresh_route_set(memo);
}

pub(super) fn leftover_pin_cell_walkable(world: &World, gx: i32, gy: i32) -> bool {
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if cell.material == MaterialId::Bedrock {
        return false;
    }
    if leftover_is_loose(cell.material) || cell.material == MaterialId::Air {
        return true;
    }
    permeability_cell(cell, &world.hydro) > 0 || water_capacity_cell(cell, &world.hydro) > 0
}

pub(super) fn leftover_pin_touches_zone(world: &World, memo: &LeftoverMemo) -> bool {
    for &(x, y) in &memo.pin_path {
        if memo.zone.contains(&(x, y)) {
            return true;
        }
        for (dx, dy) in [
            (0, 1),
            (0, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (1, 1),
            (-1, -1),
            (1, -1),
        ] {
            if memo.zone.contains(&(world.wrap_x(x + dx), y + dy)) {
                return true;
            }
        }
    }
    false
}

/// Keep last tick's chimney if the pipe still holds. A grain of sand,
/// pore widen, or sat flicker must not retarget the spring.
pub(super) fn leftover_try_reuse_pin(world: &World, memo: &mut LeftoverMemo) -> bool {
    memo.reused_pin = false;
    if memo.pin_path.len() < 2 || memo.pin_next.is_empty() {
        leftover_clear_pin(memo);
        return false;
    }
    if memo.zone.is_empty() || !leftover_pin_touches_zone(world, memo) {
        leftover_clear_pin(memo);
        return false;
    }
    for &(x, y) in &memo.pin_path {
        if !leftover_pin_cell_walkable(world, x, y) {
            leftover_clear_pin(memo);
            return false;
        }
    }
    leftover_trim_pin_at_upward_mouth(world, memo);
    if memo.pin_path.len() < 2 {
        leftover_clear_pin(memo);
        return false;
    }
    // A foot-of-hill ocean pin must not lock out the crest. Sinter
    // and a gravel chimney that flooded its own mouth still keep.
    if leftover_pin_is_short_lake_dump(world, memo) {
        return false;
    }
    memo.route_next = memo.pin_next.clone();
    leftover_refresh_route_set(memo);
    let seed = if memo.zone.contains(&memo.pin_seed) && memo.pin_next.contains_key(&memo.pin_seed)
    {
        memo.pin_seed
    } else {
        memo.pin_path
            .iter()
            .copied()
            .find(|&p| memo.zone.contains(&p) && memo.pin_next.contains_key(&p))
            .or_else(|| {
                memo.pin_path
                    .iter()
                    .copied()
                    .find(|p| memo.zone.contains(p))
            })
            .unwrap_or(memo.pin_seed)
    };
    let head = memo.heads.values().copied().max().unwrap_or(1).max(1);
    let id = memo
        .heads
        .keys()
        .copied()
        .find(|k| memo.zone.contains(k))
        .unwrap_or(memo.pin_id);
    memo.seeds.clear();
    memo.seed_zone.clear();
    memo.seeds.push((seed.0, seed.1, head));
    memo.seed_zone.push(id);
    memo.pin_seed = seed;
    memo.pin_id = id;
    memo.reused_pin = true;
    true
}

pub(super) fn leftover_install_pin(
    world: &World,
    memo: &mut LeftoverMemo,
    path: Vec<(i32, i32)>,
    seed: (i32, i32),
) {
    if path.len() < 2 {
        leftover_clear_pin(memo);
        return;
    }
    memo.pin_loose = path
        .iter()
        .map(|&(x, y)| {
            world
                .get_cell(x, y)
                .is_some_and(|c| leftover_is_loose(c.material))
        })
        .collect();
    memo.pin_path = path;
    memo.pin_next.clear();
    for w in memo.pin_path.windows(2) {
        memo.pin_next.insert(w[0], w[1]);
    }
    memo.route_next = memo.pin_next.clone();
    leftover_refresh_route_set(memo);
    memo.pin_seed = seed;
    memo.pin_id = memo.seed_zone.first().copied().unwrap_or(seed);
}

pub(super) fn leftover_trim_pin_at_upward_mouth(world: &World, memo: &mut LeftoverMemo) {
    let mut keep = None;
    for (i, &(x, y)) in memo.pin_path.iter().enumerate() {
        if world
            .get_cell(x, y)
            .is_some_and(|c| leftover_is_surface_mouth(world, x, y, c))
        {
            keep = Some(i + 1);
            break;
        }
        if leftover_is_dump_cell(world, x, y) {
            let next_is_mouth = memo.pin_path.get(i + 1).is_some_and(|&(nx, ny)| {
                world
                    .get_cell(nx, ny)
                    .is_some_and(|c| leftover_is_surface_mouth(world, nx, ny, c))
            });
            if !next_is_mouth {
                keep = Some(i + 1);
                break;
            }
        }
    }
    let Some(keep) = keep else {
        return;
    };
    if keep >= memo.pin_path.len() {
        return;
    }
    let seed = memo.pin_seed;
    let path = memo.pin_path[..keep].to_vec();
    leftover_install_pin(world, memo, path, seed);
}

pub(super) fn leftover_commit_pin(world: &World, memo: &mut LeftoverMemo) {
    if memo.route_next.is_empty() || memo.seeds.is_empty() {
        leftover_clear_pin(memo);
        return;
    }
    let seed = (
        memo.seeds[0].0,
        memo.seeds[0].1,
    );
    let mut path = vec![seed];
    let mut cur = seed;
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    seen.insert(cur);
    for _ in 0..2048 {
        let Some(&next) = memo.route_next.get(&cur) else {
            break;
        };
        if !seen.insert(next) {
            break;
        }
        path.push(next);
        cur = next;
        // Stop on the weather cell itself. A pipe cell that *sees* sky
        // still needs the last hop into that mouth (heat dump / vent).
        if world
            .get_cell(next.0, next.1)
            .is_some_and(|c| leftover_is_surface_mouth(world, next.0, next.1, c))
        {
            break;
        }
    }
    leftover_install_pin(world, memo, path, seed);
}

/// 4-connected cells from `a` to `b` so a dy=2 leftover hop is not a gap.
pub(super) fn leftover_pin_segment(world: &World, a: (i32, i32), b: (i32, i32)) -> Vec<(i32, i32)> {
    let mut out = vec![a];
    let mut cur = a;
    for _ in 0..256 {
        if cur == b {
            break;
        }
        let dx = b.0 - cur.0;
        let dy = b.1 - cur.1;
        cur = if dy.abs() >= dx.abs() {
            (cur.0, cur.1 + dy.signum())
        } else {
            (world.wrap_x(cur.0 + dx.signum()), cur.1)
        };
        out.push(cur);
    }
    if *out.last().unwrap_or(&a) != b {
        out.push(b);
    }
    out
}

pub(super) fn leftover_dim_paintable_pin(world: &World, memo: &LeftoverMemo, cell: (i32, i32)) -> bool {
    if memo.zone.contains(&cell) {
        return true;
    }
    leftover_pin_cell_walkable(world, cell.0, cell.1)
}

pub(super) fn leftover_dim_off_pin_arms(world: &World, memo: &mut LeftoverMemo) {
    if memo.pin_path.len() < 2 {
        return;
    }
    // Straw map: leftover zone + 1-cell pin. Ridge sand / 4×4 smear
    // stay off. The wet-stone hill is painted on `view` only.
    let mut line: Vec<(i32, i32)> = Vec::new();
    for w in memo.pin_path.windows(2) {
        line.extend(leftover_pin_segment(world, w[0], w[1]));
    }
    let mut on_line: FxHashSet<(i32, i32)> = FxHashSet::default();
    let n = line.len().max(1);
    for (i, cell) in line.iter().copied().enumerate() {
        if !leftover_dim_paintable_pin(world, memo, cell) {
            continue;
        }
        on_line.insert(cell);
        let t = i as f32 / n.saturating_sub(1).max(1) as f32;
        let ramp = (0.40 + t * 0.26).min(LEFTOVER_PIN_PACK);
        let e = memo.map.entry(cell).or_insert(0.0);
        *e = (*e).max(ramp);
    }
    for (cell, pack) in memo.map.iter_mut() {
        if on_line.contains(cell) || memo.zone.contains(cell) {
            continue;
        }
        *pack = 0.0;
    }
}

pub(super) fn leftover_is_overlay_rock(cell: Cell) -> bool {
    cell.material != MaterialId::Air
        && cell.material != MaterialId::Bedrock
        && !leftover_is_chimney_skin(cell.material)
        && cell.sat.0 > 0
}

/// Overlay silhouette: leftover zone + leftover-connected wet packed
/// hill + 1-cell pin. Not the 4×4 heat mask. Never joins `memo.zone`.
pub(super) fn leftover_paint_hill_view(world: &World, memo: &mut LeftoverMemo) {
    memo.view.clear();
    for (&cell, &pack) in &memo.map {
        if pack > 0.0 {
            memo.view.insert(cell, pack);
        }
    }
    let mut q: Vec<(i32, i32, f32)> = Vec::new();
    let mut seen: FxHashSet<(i32, i32)> = FxHashSet::default();
    for &cell in &memo.zone {
        let pack = memo.view.get(&cell).copied().unwrap_or(0.0);
        if pack <= 0.0 {
            continue;
        }
        seen.insert(cell);
        q.push((cell.0, cell.1, pack));
    }
    let mut i = 0;
    while i < q.len() {
        if memo.view.len() >= LEFTOVER_FIELD_CELLS {
            break;
        }
        let (gx, gy, src) = q[i];
        i += 1;
        for (dx, dy) in [
            (0, 1),
            (0, -1),
            (-1, 0),
            (1, 0),
            (-1, 1),
            (1, 1),
            (-1, -1),
            (1, -1),
        ] {
            let nx = world.wrap_x(gx + dx);
            let ny = gy + dy;
            if !seen.insert((nx, ny)) {
                continue;
            }
            let Some(cell) = world.get_cell(nx, ny) else {
                continue;
            };
            if !leftover_is_overlay_rock(cell) {
                continue;
            }
            let e = memo.view.entry((nx, ny)).or_insert(0.0);
            *e = (*e).max(src);
            q.push((nx, ny, src));
        }
    }
    leftover_stamp_planned_route(world, memo);
    memo.view_ready = true;
}

/// Keep the planned pin on P even when leftover head has not charged
/// those cells yet. One cell wide; dry / cool seats stay a faint trace.
pub(super) fn leftover_stamp_planned_route(world: &World, memo: &mut LeftoverMemo) {
    if memo.pin_path.len() < 2 {
        return;
    }
    let mut line: Vec<(i32, i32)> = Vec::new();
    for w in memo.pin_path.windows(2) {
        line.extend(leftover_pin_segment(world, w[0], w[1]));
    }
    for cell in line {
        if !leftover_dim_paintable_pin(world, memo, cell) {
            continue;
        }
        let e = memo.map.entry(cell).or_insert(0.0);
        *e = (*e).max(LEFTOVER_PLAN_TRACE);
        let e = memo.view.entry(cell).or_insert(0.0);
        *e = (*e).max(LEFTOVER_PLAN_TRACE);
    }
}

/// Planned packed stone is a swell until pulse erosion opens a conduit.
/// Loose / sinter / roofed voids / high-pore rock can already carry.
pub(super) fn leftover_route_is_open(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    if leftover_is_loose(cell.material) || leftover_is_chimney_skin(cell.material) {
        return true;
    }
    if cell.material == MaterialId::Air {
        return leftover_is_boiler_path(world, gx, gy, cell)
            || leftover_is_surface_mouth(world, gx, gy, cell);
    }
    crate::cell::is_competent_rock(cell.material) && cell.pore >= VENT_PIPE_LUMEN
}

pub(super) fn leftover_has_pin(world: &World) -> bool {
    let gx_id = world.chunk_cache_id.get();
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == gx_id && memo.pin_path.len() >= 2
    })
}

pub(super) fn leftover_is_boiler_path(world: &World, gx: i32, gy: i32, cell: Cell) -> bool {
    // Roofed Air is a chamber hop (dry void or wet pool). Unroofed is
    // weather. The old vessel BFS was equivalent: every confined seat
    // already matched steam-void or standing/wet.
    cell.material == MaterialId::Air && void_is_confined(world, gx, gy)
}

/// Live pin + leftover body still the same vessel: skip the 28k flood.
///
/// Cadencing the whole rebuild emptied the hill. Absorbing a heat-front
/// into a frozen zone did the same on the straw-climb canary. Only skip
/// when every surplus seat is already in last tick's zone.
pub(super) fn leftover_cand_outside_ok(
    world: &World,
    gx: i32,
    gy: i32,
    zone: &FxHashSet<(i32, i32)>,
) -> bool {
    let Some(cell) = world.get_cell(gx, gy) else {
        return true;
    };
    // Open pipe / mouth sinter / ridge gravel — not a second packed boiler.
    // Do not leftover_is_open_pipe here: that 128-hop BFS on every chimney
    // smear was ~6ms/tick after the pin locked.
    let pipe = cell.material == MaterialId::Air || leftover_is_chimney_skin(cell.material);
    if pipe || leftover_touches_chimney_skin(world, gx, gy) {
        return !leftover_near_set(world, gx, gy, zone, LEFTOVER_GROW_RADIUS);
    }
    false
}

pub(super) fn leftover_zone_covers_cands(
    world: &World,
    zone: &FxHashSet<(i32, i32)>,
    cands: &FxHashMap<(i32, i32), (u32, u32)>,
) -> bool {
    if zone.is_empty() {
        return false;
    }
    for &key in cands.keys() {
        if zone.contains(&key) {
            continue;
        }
        // Open pipes and far 4×4 mouth-sinter smears are not the vessel.
        // A continent of leftover heat has dozens of them every tick.
        if leftover_cand_outside_ok(world, key.0, key.1, zone) {
            continue;
        }
        return false;
    }
    true
}

pub(super) fn leftover_retouch_stable_heads(
    memo: &mut LeftoverMemo,
    cands: &FxHashMap<(i32, i32), (u32, u32)>,
    old_heads: &FxHashMap<(i32, i32), u32>,
    old_released: &FxHashMap<(i32, i32), u32>,
) {
    let mut surplus = 0u32;
    let mut seats = 0u32;
    for (key, &(s, cap)) in cands {
        if memo.zone.contains(key) {
            surplus = surplus.saturating_add(s);
            seats = seats.saturating_add(cap);
        }
    }
    let id = if memo.zone.contains(&memo.pin_id) {
        memo.pin_id
    } else {
        memo.zone
            .iter()
            .copied()
            .min_by_key(|&(x, y)| (y, x))
            .unwrap_or(memo.pin_id)
    };
    let mut head = surplus;
    if let Some(&prev) = old_heads.get(&id) {
        head = prev.saturating_add(surplus);
    } else if let Some(&prev) = old_heads.values().next() {
        head = prev.saturating_add(surplus);
    }
    if let Some(&rel) = old_released.get(&id) {
        head = head.saturating_sub(rel);
    }
    let cap_head = surplus.saturating_mul(8).max(surplus);
    head = head.min(cap_head);
    memo.heads.insert(id, head);
    let _ = seats;
}

pub(super) fn leftover_near_set(
    world: &World,
    gx: i32,
    gy: i32,
    set: &FxHashSet<(i32, i32)>,
    radius: i32,
) -> bool {
    if set.contains(&(gx, gy)) {
        return true;
    }
    let r = radius.max(1);
    for dy in -r..=r {
        for dx in -r..=r {
            if dx == 0 && dy == 0 {
                continue;
            }
            if set.contains(&(world.wrap_x(gx + dx), gy + dy)) {
                return true;
            }
        }
    }
    false
}

/// Absorb heat-front surplus within one heat tile of last tick's vessel.
/// A far second packed boiler still forces a full flood.
pub(super) fn leftover_try_grow_zone(
    world: &World,
    memo: &mut LeftoverMemo,
    cands: &FxHashMap<(i32, i32), (u32, u32)>,
    _pipe_cands: &FxHashSet<(i32, i32)>,
    old_heads: &FxHashMap<(i32, i32), u32>,
    old_released: &FxHashMap<(i32, i32), u32>,
) -> bool {
    if memo.zone.is_empty() {
        return false;
    }
    let mut pin: FxHashSet<(i32, i32)> = FxHashSet::default();
    pin.extend(memo.pin_path.iter().copied());
    let mut grown: Vec<(i32, i32)> = Vec::new();
    for &key in cands.keys() {
        if memo.zone.contains(&key) {
            continue;
        }
        if leftover_cand_outside_ok(world, key.0, key.1, &memo.zone) {
            continue;
        }
        if leftover_near_set(world, key.0, key.1, &memo.zone, LEFTOVER_GROW_RADIUS)
            || leftover_near_set(world, key.0, key.1, &pin, LEFTOVER_GROW_RADIUS)
        {
            grown.push(key);
            continue;
        }
        return false;
    }
    for &(gx, gy) in &grown {
        memo.zone.insert((gx, gy));
        let e = memo.map.entry((gx, gy)).or_insert(0.0);
        *e = (*e).max(LEFTOVER_PLAN_TRACE);
    }
    leftover_retouch_stable_heads(memo, cands, old_heads, old_released);
    true
}

pub(super) fn leftover_zone_body_pack(head: u32, seats: u32, expand: u16) -> f32 {
    let denom = seats.saturating_mul(expand.max(1) as u32).saturating_mul(2);
    let t = (head as f32 / denom.max(1) as f32).clamp(0.0, 1.0);
    (0.38 + t * 0.30).clamp(0.0, 0.68)
}

/// Map leftover 0..=1 onto the P overlay hue ramp (magenta → yellow).
///
/// Burnt-flat magenta was `leftover_zone_body_pack` written to every
/// vessel cell. The pipe already uses this ramp; the reservoir must too.
pub(super) fn leftover_hue_pack(local: f32, body: f32) -> f32 {
    let h = ((body - 0.38) / 0.30).clamp(0.0, 1.0);
    let drive = (local.clamp(0.0, 1.0) * (0.40 + 0.60 * h)).clamp(0.0, 1.0);
    if drive <= 0.0 {
        0.0
    } else {
        (0.20 + drive * 0.72).clamp(0.20, 0.92)
    }
}

/// Keep last tick's continent vessel and plan sky from it. Used when
/// surplus stayed inside the zone (or the heat front was absorbed) so
/// we must not walk 28k cells again just to seed Dijkstra.
pub(super) fn leftover_plan_from_existing_zone(
    world: &World,
    memo: &mut LeftoverMemo,
    cands: &FxHashMap<(i32, i32), (u32, u32)>,
) {
    let cells: Vec<(i32, i32)> = memo.zone.iter().copied().collect();
    if cells.is_empty() {
        return;
    }
    let id = memo
        .heads
        .keys()
        .copied()
        .find(|k| memo.zone.contains(k))
        .or_else(|| cells.iter().copied().min_by_key(|&(x, y)| (y, x)))
        .unwrap_or((0, 0));
    let mut surplus = 0u32;
    for (key, &(s, _)) in cands {
        if memo.zone.contains(key) {
            surplus = surplus.saturating_add(s);
        }
    }
    let head = memo.heads.get(&id).copied().unwrap_or(surplus).max(1);
    leftover_plan_after_zone(world, memo, vec![(cells, head, surplus, id)]);
}

pub(super) fn leftover_plan_after_zone(
    world: &World,
    memo: &mut LeftoverMemo,
    painted: Vec<(Vec<(i32, i32)>, u32, u32, (i32, i32))>,
) {
    let t_lock = Instant::now();
    if leftover_try_reuse_pin(world, memo) {
        leftover_dim_off_pin_arms(world, memo);
        leftover_refresh_route_set(memo);
        memo.last_lock_us = t_lock.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
        return;
    }
    let mut heap: BinaryHeap<(Reverse<u32>, i32, i32, u32)> = BinaryHeap::new();
    for (cells, head, surplus, id) in painted {
        let in_zone: FxHashSet<(i32, i32)> = cells.iter().copied().collect();
        let mut outlets: Vec<(i32, i32, i32)> = Vec::new();
        for &(gx, gy) in &cells {
            let mut face = 0i32;
            let mut rim = false;
            for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
                let nx = world.wrap_x(gx + dx);
                let ny = gy + dy;
                if in_zone.contains(&(nx, ny)) {
                    continue;
                }
                rim = true;
                let Some(n) = world.get_cell(nx, ny) else {
                    continue;
                };
                if n.material == MaterialId::Air {
                    if leftover_is_surface_mouth(world, nx, ny, n) {
                        face = face.max(30_000 + dy.max(0) * 200);
                    } else if leftover_is_boiler_path(world, nx, ny, n) {
                        face = face.max(18_000 + dy.max(0) * 120);
                    }
                    continue;
                }
                if n.material == MaterialId::Bedrock {
                    continue;
                }
                let perm = permeability_cell(n, &world.hydro);
                if leftover_is_loose(n.material) {
                    face = face.max(14_000 + perm as i32 * 20 + dy.max(0) * 100);
                }
                if n.sat.0 <= retained_sat_cell(n, &world.hydro) && perm > 0 {
                    face = face.max(10_000 + dy.max(0) * 80);
                } else if perm > 0 {
                    face = face.max(perm as i32 + dy.max(0) * 40);
                }
            }
            if face > 0 {
                outlets.push((face, gx, gy));
            }
            if rim {
                leftover_push_exterior_neighbors(
                    world,
                    gx,
                    gy,
                    head.max(1),
                    0,
                    &in_zone,
                    &mut heap,
                    &mut memo.parents,
                );
            }
        }
        outlets.sort_by(|a, b| b.0.cmp(&a.0).then(b.2.cmp(&a.2)).then(a.1.cmp(&b.1)));
        // One outlet per vessel until the winning chimney is locked
        // after the exterior BFS. 8–24 rim seeds were why leftover
        // explored every side vein at once.
        if let Some(&(_, gx, gy)) = outlets.first() {
            memo.seeds.push((gx, gy, head.max(surplus)));
            memo.seed_zone.push(id);
        } else if let Some(&(gx, gy)) = cells.iter().max_by_key(|(_, y)| *y) {
            memo.seeds.push((gx, gy, head.max(surplus)));
            memo.seed_zone.push(id);
        }
    }
    let mut seen: FxHashSet<(i32, i32)> = memo.map.keys().copied().collect();
    let zone_painted = seen.len();
    while let Some((Reverse(cost), gx, gy, remaining)) = heap.pop() {
        if remaining == 0 || !seen.insert((gx, gy)) {
            continue;
        }
        if memo.map.len() >= zone_painted.saturating_add(LEFTOVER_FIELD_CELLS) {
            break;
        }
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if cell.material == MaterialId::Bedrock {
            continue;
        }
        if cell.material == MaterialId::Air {
            if leftover_is_surface_mouth(world, gx, gy, cell) {
                continue;
            }
            if !leftover_is_boiler_path(world, gx, gy, cell) {
                continue;
            }
        }
        memo.costs.insert((gx, gy), cost);
        let seat = water_capacity_cell(cell, &world.hydro).max(1) as u32;
        let fade = LEFTOVER_COST_FADE / (LEFTOVER_COST_FADE + cost as f32);
        let mut pack = leftover_pack_norm(remaining.saturating_add(seat), seat) * fade;
        let escape = leftover_is_loose(cell.material)
            || (cell.material == MaterialId::Air && void_is_confined(world, gx, gy));
        if !escape {
            // Homogeneous stone halo stays in the magenta body range.
            pack = pack.min(0.62);
        }
        if pack > 0.02 {
            let e = memo.map.entry((gx, gy)).or_insert(0.0);
            *e = (*e).max(pack);
        }
        let next = remaining.saturating_sub(seat.min(remaining / 8 + 1));
        if next == 0 {
            continue;
        }
        for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1)] {
            let nx = world.wrap_x(gx + dx);
            let ny = gy + dy;
            if seen.contains(&(nx, ny)) {
                continue;
            }
            let Some(n) = world.get_cell(nx, ny) else {
                continue;
            };
            if n.material == MaterialId::Bedrock {
                continue;
            }
            if n.material == MaterialId::Air {
                if leftover_is_surface_mouth(world, nx, ny, n) {
                    continue;
                }
                if leftover_is_boiler_path(world, nx, ny, n) {
                    leftover_note_parent(&mut memo.parents, (nx, ny), (gx, gy));
                    heap.push((Reverse(cost.saturating_add(1)), nx, ny, next));
                }
                continue;
            }
            let perm = permeability_cell(n, &world.hydro);
            if perm == 0 && water_capacity_cell(n, &world.hydro) == 0 {
                continue;
            }
            let step = leftover_step_cost_ex(perm.max(1), dx, dy, leftover_is_loose(n.material));
            leftover_note_parent(&mut memo.parents, (nx, ny), (gx, gy));
            heap.push((Reverse(cost.saturating_add(step)), nx, ny, next));
        }
    }
    if leftover_try_reuse_pin(world, memo) {
        leftover_dim_off_pin_arms(world, memo);
    } else {
        leftover_lock_winning_route(world, memo);
        leftover_commit_pin(world, memo);
        leftover_dim_off_pin_arms(world, memo);
    }
    leftover_refresh_route_set(memo);
    memo.last_lock_us = t_lock.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
}

/// Heat rides leftover mass already on the pin. Dry planned cells stay
/// cold — leftover volume *is* that pore water / steam, not a heat walk.
pub(super) fn leftover_conduct_wet_route(world: &World, temp: &mut Temperature) {
    let path = LEFTOVER_MEMO.with(|slot| slot.borrow().pin_path.clone());
    if path.len() < 2 {
        return;
    }
    for w in path.windows(2) {
        let (ax, ay) = w[0];
        let (bx, by) = w[1];
        let Some(a) = world.get_cell(ax, ay) else {
            break;
        };
        let Some(b) = world.get_cell(bx, by) else {
            break;
        };
        let src_mass = a.sat.0.max(steam_at(world, ax, ay));
        let dest_mass = b.sat.0.max(steam_at(world, bx, by));
        if src_mass == 0 || dest_mass == 0 {
            continue;
        }
        leftover_boost_route_heat(world, temp, ax, ay, bx, by, dest_mass.min(16).max(1));
        if leftover_is_heat_sink(world, bx, by, b) {
            break;
        }
    }
}

/// Pulse-erode the pinned chimney. Head may not finish the walk this
/// tick; widening / gravel wear is what makes the planned route open.
pub(super) fn leftover_erode_planned_route(world: &mut World) {
    let path = LEFTOVER_MEMO.with(|slot| slot.borrow().pin_path.clone());
    if path.len() < 2 {
        return;
    }
    for &(gx, gy) in &path {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if leftover_is_surface_mouth(world, gx, gy, cell) {
            break;
        }
        if crate::cell::is_competent_rock(cell.material) {
            if leftover_route_is_open(world, gx, gy, cell) {
                continue;
            }
            let _ = widen_aperture(world, gx, gy, 48, 1.6, 0x51A7_u64, false);
            continue;
        }
        leftover_pulse_loose_channel(world, gx, gy);
    }
}

/// Gravel / loose rock on the pin: cement to conglomerate when the
/// water carries carbonate, wear toward sand when already open, else
/// scour the pore. Silicate weld to stone stays on the hot-pore pulse.
pub(super) fn leftover_pulse_loose_channel(world: &mut World, gx: i32, gy: i32) {
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    if !matches!(
        cell.material,
        MaterialId::Gravel | MaterialId::LooseRock | MaterialId::Sand
    ) {
        return;
    }
    let load = dissolved_at(world, gx, gy);
    if load >= CEMENT_MIN_LOAD {
        let _ = pressure_sinter_cell(world, gx, gy);
        return;
    }
    if matches!(cell.material, MaterialId::Gravel | MaterialId::LooseRock)
        && cell.pore >= 220
    {
        let mut next = cell;
        next.material = MaterialId::Sand;
        let cap = water_capacity_cell(next, &world.hydro);
        let spill = next.sat.0.saturating_sub(cap);
        next.sat = Sat(next.sat.0.min(cap));
        world.set_cell(gx, gy, next);
        if spill > 0 {
            let _ = crate::displace::park_orphan_or_keep(
                world,
                gx,
                gy + 1,
                gx,
                gy,
                spill as u32,
            );
        }
        return;
    }
    if cell.material != MaterialId::Air && cell.pore < 240 {
        let mut g = cell;
        g.pore = g.pore.saturating_add(1);
        world.set_cell(gx, gy, g);
    }
}

pub(super) struct LeftoverStrawGuard;

impl Drop for LeftoverStrawGuard {
    fn drop(&mut self) {
        LEFTOVER_STRAW.with(|f| f.set(false));
    }
}

pub(super) fn leftover_straw_chain(
    world: &mut World,
    temp: &mut Temperature,
    mut gx: i32,
    mut gy: i32,
    hops: u16,
) -> u32 {
    LEFTOVER_STRAW.with(|f| f.set(true));
    let _guard = LeftoverStrawGuard;
    let mut drive = 255u8;
    let released = 0u32;
    for _ in 0..hops {
        let Some(here) = world.get_cell(gx, gy) else {
            return released;
        };
        if leftover_is_surface_mouth(world, gx, gy, here) {
            return released;
        }
        if here.material != MaterialId::Air && here.sat.0 <= leftover_straw_floor(world, gx, gy, here)
        {
            let mut stepped = false;
            for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (-1, 0), (1, 0), (0, -1), (0, 2)] {
                let nx = world.wrap_x(gx + dx);
                let ny = gy + dy;
                let Some(n) = world.get_cell(nx, ny) else {
                    continue;
                };
                if n.material == MaterialId::Air || n.sat.0 == 0 {
                    continue;
                }
                if permeability_cell(n, &world.hydro) == 0 {
                    continue;
                }
                // Only raid leftover zone water. Climbing onto the pin
                // and draining its table is the dry steam vent.
                if !leftover_in_zone(world, nx, ny) {
                    continue;
                }
                if n.sat.0 <= leftover_straw_floor(world, nx, ny, n) {
                    continue;
                }
                gx = nx;
                gy = ny;
                stepped = true;
                break;
            }
            if !stepped {
                return released;
            }
            continue;
        }
        let mut seen = FxHashSet::default();
        // Loose chimneys (playtest gravel cheat path) are routinely
        // longer than 24. Packed rock keeps the short hop so a sealed
        // vessel does not recirculate itself into fake relief. On the
        // leftover straw the hop budget *is* the marble walk.
        let depth = leftover_packed_depth(world, gx, gy, hops);
        let (moved, dest, parked_vadose) =
            reverse_push_pore_water_inner(world, temp, gx, gy, drive, depth, &mut seen);
        if moved == 0 {
            return released;
        }
        let Some((nx, ny)) = dest else {
            return released;
        };
        // Heat rides leftover mass (pore water / steam). Do not walk a
        // dry heat finger along the pin — that leftover *is* the pressure.
        if world
            .get_cell(nx, ny)
            .is_some_and(|c| leftover_is_surface_mouth(world, nx, ny, c))
        {
            return released.saturating_add(moved as u32);
        }
        if leftover_has_upward_mouth(world, gx, gy) && !leftover_on_route(world, nx, ny)
        {
            return released.saturating_add(moved as u32);
        }
        // Fill holes on a loose chimney, then keep walking. Parking at
        // the first vadose gravel cell was why a cheat path lit up on P
        // but never became a spring. Off-pin packed vadose is still a
        // local table swell. On the pin, stay at the leftover source so
        // the next marble still comes from surplus — walking onto the
        // seat drained the climb out the dry steam vent.
        let dest_loose = world
            .get_cell(nx, ny)
            .is_some_and(|c| leftover_is_loose(c.material));
        if parked_vadose && !leftover_in_zone(world, nx, ny) && !dest_loose {
            if leftover_on_route(world, nx, ny) {
                continue;
            } else {
                return released.saturating_add(moved as u32);
            }
        }
        // Do not recirculate the boiler — unless this hop is the locked
        // chimney walking through the vessel toward the mouth.
        if leftover_in_zone(world, nx, ny) && !dest_loose && !leftover_on_route(world, nx, ny) {
            return released;
        }
        // Stay at the leftover source. Walking onto a dest seat either
        // drained the table out the mouth or stalled in high-cap gravel.
        // Loose chimneys transmit as a leftover pipe (film + onward).
        if world
            .get_cell(nx, ny)
            .is_some_and(|c| !leftover_is_surface_mouth(world, nx, ny, c))
        {
            continue;
        }
        gx = nx;
        gy = ny;
        drive = 255;
    }
    released
}

/// Hop budget for one leftover straw. Expand 192 used to cap at 72 and
/// die underground; 1400× must be able to punch a deep column to sky.
/// A single boiling seat at 1400× is tens of thousands of leftover
/// volume — do not clamp that walk back to 192 cells.
pub(super) fn leftover_straw_hops(surplus: u32, expand: u16) -> u16 {
    let from_head = surplus / 2_000;
    let from_expand = 32 + (expand as u32 / 6);
    from_head.max(from_expand).clamp(32, 384) as u16
}

pub(super) fn leftover_packed_depth(world: &World, gx: i32, gy: i32, hops: u16) -> u8 {
    if LEFTOVER_STRAW.with(|f| f.get()) || leftover_on_route(world, gx, gy) {
        return hops.max(1).min(255) as u8;
    }
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if leftover_is_loose(n.material)
            || leftover_is_surface_mouth(world, nx, ny, n)
            || leftover_is_boiler_path(world, nx, ny, n)
        {
            return hops.max(1).min(255) as u8;
        }
    }
    24
}

pub(super) fn leftover_boost_route_heat(
    world: &World,
    temp: &mut Temperature,
    from_gx: i32,
    from_gy: i32,
    to_gx: i32,
    to_gy: i32,
    moved: u8,
) {
    if moved == 0 {
        return;
    }
    if world
        .get_cell(to_gx, to_gy)
        .is_some_and(|c| leftover_is_heat_sink(world, to_gx, to_gy, c))
    {
        temp.advect_leftover_into_water(from_gx, from_gy, to_gx, to_gy, moved);
    } else {
        // Only with leftover mass that actually moved — leftover volume
        // *is* that hot pore water / steam, not a dry overlay walk.
        temp.advect_leftover_route(from_gx, from_gy, to_gx, to_gy, moved);
    }
}

/// Room leftover mass may occupy in a dest. Loose pin seats keep a
/// wet film and transmit the rest — they are not leftover tanks.
pub(super) fn leftover_straw_dest_room(_world: &World, _tx: i32, _ty: i32, dst: Cell, cap: u8) -> u8 {
    let room = cap.saturating_sub(dst.sat.0);
    if !LEFTOVER_STRAW.with(|f| f.get()) {
        return room;
    }
    if leftover_is_loose(dst.material) {
        const LOOSE_FILM: u8 = 8;
        return LOOSE_FILM.saturating_sub(dst.sat.0);
    }
    room
}

/// Leftover volume is `mass × expand`. One marble (or a few) — never the
/// whole seat, or the reverse river empties out a dry steam vent.
pub(super) fn leftover_marble_want(drive: u8, mobile: u8) -> u8 {
    if mobile == 0 {
        return 0;
    }
    (1 + drive / 80).min(4).min(mobile)
}

/// Packed leftover seats keep a wet table. Zone cells may donate down
/// to a wet floor — leftover pressure is that leftover mass.
pub(super) fn leftover_straw_floor(world: &World, gx: i32, gy: i32, cell: Cell) -> u8 {
    if cell.material == MaterialId::Air || leftover_is_loose(cell.material) {
        return 0;
    }
    let retained = retained_sat_cell(cell, &world.hydro);
    if leftover_in_zone(world, gx, gy) && !leftover_on_route(world, gx, gy) {
        return retained.saturating_div(2).max(1);
    }
    retained
}

/// How much expanded volume does not fit the equilibrium seat (0..=1).
///
/// `1 - seat/volume` so 7/14 rock at expand 100 is nearly as packed as
/// 14/14. Cool collapse (`volume == mass`) is zero leftover.
#[inline]
pub(super) fn leftover_pack_norm(volume: u32, seat: u32) -> f32 {
    if seat == 0 || volume <= seat {
        return 0.0;
    }
    (1.0 - seat as f32 / volume as f32).clamp(0.0, 1.0)
}

pub(super) fn leftover_cell_charged(world: &World, gx: i32, gy: i32) -> bool {
    if LEFTOVER_STRAW.with(|f| f.get()) {
        return true;
    }
    let gx = world.wrap_x(gx);
    let id = world.chunk_cache_id.get();
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == id && memo.map.contains_key(&(gx, gy))
    })
}

pub(super) fn leftover_in_zone(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let id = world.chunk_cache_id.get();
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == id && memo.zone.contains(&(gx, gy))
    })
}

pub(super) fn leftover_on_arm(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let id = world.chunk_cache_id.get();
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == id && memo.map.contains_key(&(gx, gy))
    })
}

pub(super) fn leftover_on_route(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let id = world.chunk_cache_id.get();
    LEFTOVER_MEMO.with(|slot| {
        let memo = slot.borrow();
        memo.world_id == id && memo.route_set.contains(&(gx, gy))
    })
}

/// Rim sinter + dilute into a weather lake. The mouth itself stays in
/// solution (`note_leftover_lake_vent` skips the standing-lake dump).
pub(super) fn leftover_drop_load_at_weather_lake(world: &mut World, vx: i32, vy: i32) {
    let vx = world.wrap_x(vx);
    note_leftover_lake_vent(world.chunk_cache_id.get(), vx, vy);
    let load = dissolved_at(world, vx, vy);
    if load == 0 {
        return;
    }
    let edge = (load / 2).max(1).min(load);
    let mut left = edge;
    for (dx, dy) in [(-1, 0), (1, 0), (-1, 1), (1, 1), (0, 1), (-1, -1), (1, -1)] {
        if left == 0 {
            break;
        }
        let nx = world.wrap_x(vx + dx);
        let ny = vy + dy;
        let Some(shore) = world.get_cell(nx, ny) else {
            continue;
        };
        if shore.material == MaterialId::Air || shore.material == MaterialId::Bedrock {
            continue;
        }
        let take = left.min(16);
        let got = take_dissolved(world, vx, vy, take);
        if got == 0 {
            break;
        }
        add_dissolved(world, nx, ny, got);
        let _ = precipitate_at(world, nx, ny);
        left = left.saturating_sub(got);
    }
    // Remainder floats into neighbouring weather-lake cells.
    let rest = dissolved_at(world, vx, vy);
    if rest == 0 {
        return;
    }
    let mut lakes: Vec<(i32, i32)> = Vec::new();
    for (dx, dy) in [(-1, 0), (1, 0), (0, 1), (0, -1), (-1, 1), (1, 1)] {
        let nx = world.wrap_x(vx + dx);
        let ny = vy + dy;
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if leftover_is_weather_lake(world, nx, ny, n) {
            lakes.push((nx, ny));
        }
    }
    if lakes.is_empty() {
        return;
    }
    let float = rest.saturating_mul(2) / 3;
    if float == 0 {
        return;
    }
    let share = (float / lakes.len() as u16).max(1);
    for (nx, ny) in lakes {
        let got = take_dissolved(world, vx, vy, share);
        if got == 0 {
            break;
        }
        add_dissolved(world, nx, ny, got);
    }
}

/// Best leftover conduit: vadose / loose / packed rock, or a confined cavity.
/// Sky-open vents are relief, not a walk — dest-pick keeps those separately.
pub(super) fn leftover_conduit_dest(
    world: &World,
    gx: i32,
    gy: i32,
    seen: &FxHashSet<(i32, i32)>,
) -> Option<(i32, i32, i32, Cell, bool)> {
    let mut best = None;
    for (dx, dy) in [(0, 1), (-1, 1), (1, 1), (0, 2), (-1, 0), (1, 0), (0, -1)] {
        let tx = world.wrap_x(gx + dx);
        let ty = gy + dy;
        if seen.contains(&(tx, ty)) {
            continue;
        }
        let Some(dst) = world.get_cell(tx, ty) else {
            continue;
        };
        if dst.material == MaterialId::Air {
            if leftover_is_surface_mouth(world, tx, ty, dst) {
                continue;
            }
            if leftover_is_boiler_path(world, tx, ty, dst) {
                let s = 20_000 + dy.max(0) * 250;
                if best.is_none_or(|(sc, _, _, _, _)| s > sc) {
                    best = Some((s, tx, ty, dst, false));
                }
            }
            continue;
        }
        let Some((score, overflow_vent)) = reverse_seep_path_score(world, dst, tx, ty, dy) else {
            continue;
        };
        if overflow_vent {
            continue;
        }
        let perm = permeability_cell(dst, &world.hydro).max(1);
        let loose = leftover_is_loose(dst.material);
        let cost = leftover_step_cost_ex(perm, dx, dy, loose) as i32;
        let mut s = score + 8_000 - cost.min(7_500);
        if dy > 0 {
            s += 3_000;
        }
        if loose {
            s += 4_000;
        }
        if leftover_on_arm(world, tx, ty) {
            // Stay on the channel the P overlay already committed to.
            s += 5_000;
        }
        if dst.sat.0 <= retained_sat_cell(dst, &world.hydro) {
            s += 6_000;
        }
        if best.is_none_or(|(sc, _, _, _, _)| s > sc) {
            best = Some((s, tx, ty, dst, false));
        }
    }
    best
}

/// Rebuild the leftover-volume field for P overlay + water-table shove.
///
/// Adjacent boiling wet cells are **one vessel**. The zone head is the
/// sum of every cell's leftover (`mass × expand − seat`) and keeps
/// growing until a straw finds vadose or a sky-open vent. Exterior
/// arms follow cheap perm / loose / confined cavities. P paints the
/// wet packed hill plus the 1-cell pin; hue follows leftover head.
pub fn prepare_leftover_pressure(world: &World, temp: &Temperature, boil_c: f32, expand: u16) {
    let boil = if boil_c.is_finite() {
        boil_c
    } else {
        BOIL_POINT_C
    };
    let expand = expand.max(1);
    let boil_bits = boil.to_bits();
    let id = world.chunk_cache_id.get();
    LEFTOVER_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        if memo.world_id != 0 && memo.world_id != id {
            leftover_clear_pin(&mut memo);
        }
        if memo.world_id == id
            && memo.tick == world.tick
            && memo.boil_bits == boil_bits
            && memo.expand == expand
        {
            return;
        }
        memo.world_id = id;
        memo.tick = world.tick;
        memo.boil_bits = boil_bits;
        memo.expand = expand;
        rebuild_leftover_field(world, temp, boil, expand, &mut memo);
    });
}

/// P-overlay hill silhouette. Sim ticks skip this flood; call when
/// the leftover field is actually drawn.
///
/// No-op while the cell pipe owns `P`. The memo keys on `world.tick`, so a
/// running sim invalidates it every frame, and `cell_pressure_norm` then
/// discards the result for every cell — measured at 7 ms steady and 46 ms
/// on a rebuild frame, spent to produce nothing. That was the overlay
/// dropping 20 fps to 12.
pub fn ensure_leftover_hill_view(world: &World, temp: &Temperature, boil_c: f32, expand: u16) {
    if crate::pipe::pipe_painting(world) {
        return;
    }
    prepare_leftover_pressure(world, temp, boil_c, expand);
    LEFTOVER_MEMO.with(|slot| {
        let mut memo = slot.borrow_mut();
        if leftover_memo_bound(&memo, world, boil_c, expand) && !memo.view_ready {
            leftover_paint_hill_view(world, &mut memo);
        }
    });
}

fn rebuild_leftover_field(
    world: &World,
    temp: &Temperature,
    boil: f32,
    expand: u16,
    memo: &mut LeftoverMemo,
) {
    // Continent + live pin: leftover membership is already the vessel.
    // Off-cadence ticks keep the pin and skip the 28k cand walk. Small
    // climbing hills still rebuild every tick (straw-climb canary).
    if memo.reused_zone
        && memo.pin_path.len() >= 2
        && memo.zone.len() > LEFTOVER_FIELD_CELLS
        && world.tick % STEAM_EVERY != 0
    {
        memo.view.clear();
        memo.view_ready = false;
        memo.seeds.clear();
        memo.seed_zone.clear();
        memo.parents.clear();
        memo.costs.clear();
        memo.route_next.clear();
        if leftover_try_reuse_pin(world, memo) {
            leftover_dim_off_pin_arms(world, memo);
            leftover_refresh_route_set(memo);
            memo.reused_zone = true;
            memo.last_cands_us = 0;
            memo.last_flood_us = 0;
            memo.last_lock_us = 0;
            return;
        }
        memo.reused_zone = false;
    }
    let old_heads = std::mem::take(&mut memo.heads);
    let old_released = std::mem::take(&mut memo.released);
    memo.view.clear();
    memo.view_ready = false;
    memo.seeds.clear();
    memo.seed_zone.clear();
    memo.parents.clear();
    memo.costs.clear();
    memo.route_next.clear();
    memo.reused_zone = false;
    // pin_* stays. A rebuild must not retarget a live chimney.
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(coord, c)| c.has_wet_pores && chunk_overlaps_hot(temp, **coord, boil))
        .map(|(k, _)| *k)
        .collect();
    let t_cands = Instant::now();
    let mut cands: FxHashMap<(i32, i32), (u32, u32)> = FxHashMap::default();
    let mut pipe_cands: FxHashSet<(i32, i32)> = FxHashSet::default();
    for coord in coords {
        for_each_hot_tile_cell(world, temp, coord, boil, |gx, gy, t_c, cell| {
            if cell.material == MaterialId::Air || cell.sat.0 == 0 {
                return;
            }
            let cap = water_capacity_cell(cell, &world.hydro) as u32;
            if cap == 0 {
                return;
            }
            let vol = vapor_volume_units(cell.sat.0 as u32, t_c, boil, expand);
            let surplus = overpressure_units(vol, cap);
            if surplus == 0 {
                return;
            }
            let key = (world.wrap_x(gx), gy);
            cands.insert(key, (surplus, cap));
            if cell.material == MaterialId::Air || leftover_is_chimney_skin(cell.material) {
                pipe_cands.insert(key);
            }
        });
    }
    memo.last_cands_us = t_cands.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
    if cands.is_empty() {
        // Punch-through can empty a tiny boiler. A live *sky* pin stays
        // on P so the planned climb does not vanish the tick leftover
        // hits zero. Lake-only pins still drop.
        memo.zone.clear();
        memo.map.clear();
        if leftover_pin_keep_when_cold(world, memo) {
            leftover_stamp_planned_route(world, memo);
            return;
        }
        leftover_clear_pin(memo);
        return;
    }
    // Keep last tick's zone when surplus stays inside it, or absorb the
    // heat front. Continent vessels do this *before* a pin locks so the
    // 28k flood is not every search tick. Small climbing hills still
    // full-flood (straw-climb canary). A far second boiler still floods.
    if memo.pin_path.len() >= 2 || memo.zone.len() > LEFTOVER_FIELD_CELLS {
        let t_stable = Instant::now();
        let continent = memo.zone.len() > LEFTOVER_FIELD_CELLS;
        let stable = if leftover_zone_covers_cands(world, &memo.zone, &cands) {
            leftover_retouch_stable_heads(memo, &cands, &old_heads, &old_released);
            true
        } else if continent {
            leftover_try_grow_zone(
                world,
                memo,
                &cands,
                &pipe_cands,
                &old_heads,
                &old_released,
            )
        } else {
            false
        };
        if stable {
            memo.reused_zone = true;
            memo.last_flood_us = t_stable.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
            if leftover_try_reuse_pin(world, memo) {
                leftover_dim_off_pin_arms(world, memo);
                leftover_refresh_route_set(memo);
                memo.last_lock_us = 0;
                return;
            }
            if continent {
                leftover_plan_from_existing_zone(world, memo, &cands);
                return;
            }
            memo.reused_zone = false;
        }
    }
    memo.zone.clear();
    memo.map.clear();
    let t_flood = Instant::now();
    let mut unvisited: FxHashSet<(i32, i32)> = cands.keys().copied().collect();
    let mut components: Vec<(Vec<(i32, i32)>, u32, u32)> = Vec::new();
    while let Some(&start) = unvisited.iter().next() {
        // Open sky chimneys stay the pipe. Buried loose / sinter /
        // roofed voids join the vessel — they are the chamber or the
        // planned route, not ridge sand.
        if leftover_is_open_pipe(world, start.0, start.1) {
            unvisited.remove(&start);
            continue;
        }
        let mut stack = vec![start];
        unvisited.remove(&start);
        let mut cells: Vec<(i32, i32)> = Vec::new();
        let mut in_comp: FxHashSet<(i32, i32)> = FxHashSet::default();
        let mut surplus = 0u32;
        let mut seats = 0u32;
        while let Some((gx, gy)) = stack.pop() {
            if !in_comp.insert((gx, gy)) {
                continue;
            }
            if let Some(&(s, cap)) = cands.get(&(gx, gy)) {
                surplus = surplus.saturating_add(s);
                seats = seats.saturating_add(cap);
            }
            cells.push((gx, gy));
            for (dx, dy) in [
                (0, 1),
                (0, -1),
                (-1, 0),
                (1, 0),
                (-1, 1),
                (1, 1),
                (-1, -1),
                (1, -1),
            ] {
                let nx = world.wrap_x(gx + dx);
                let ny = gy + dy;
                // Packed surplus joins by adjacency. Do not walk 8
                // open-pipe probes per interior stone cell of a 28k body.
                if unvisited.contains(&(nx, ny)) {
                    if pipe_cands.contains(&(nx, ny))
                        && leftover_is_open_pipe(world, nx, ny)
                    {
                        continue;
                    }
                    unvisited.remove(&(nx, ny));
                    stack.push((nx, ny));
                    continue;
                }
                let Some(n) = world.get_cell(nx, ny) else {
                    continue;
                };
                if n.material != MaterialId::Air && !leftover_is_chimney_skin(n.material) {
                    continue;
                }
                if leftover_is_open_pipe(world, nx, ny) {
                    continue;
                }
                if leftover_is_chamber_fill(world, nx, ny, n) && !in_comp.contains(&(nx, ny)) {
                    stack.push((nx, ny));
                }
            }
        }
        components.push((cells, surplus, seats));
    }
    let max_zone = components.iter().map(|(c, _, _)| c.len()).max().unwrap_or(0);
    let mut painted: Vec<(Vec<(i32, i32)>, u32, u32, (i32, i32))> = Vec::new();
    for (cells, surplus, seats) in components {
        // 4×4 smear / mouth sinter next to a live pipe is not a vessel
        // when a real packed reservoir exists.
        if cells.len() <= 3
            && max_zone > cells.len()
            && cells
                .iter()
                .all(|&(x, y)| leftover_touches_chimney_skin(world, x, y))
        {
            continue;
        }
        let id = *cells
            .iter()
            .min_by_key(|(x, y)| (*y, *x))
            .unwrap_or(&(0, 0));
        let mut head = surplus;
        if let Some(&prev) = old_heads.get(&id) {
            head = prev.saturating_add(surplus);
        } else {
            for &(x, y) in &cells {
                if let Some(&prev) = old_heads.get(&(x, y)) {
                    head = head.max(prev.saturating_add(surplus));
                    break;
                }
            }
        }
        if let Some(&rel) = old_released.get(&id) {
            head = head.saturating_sub(rel);
        }
        let cap_head = surplus.saturating_mul(8).max(surplus);
        head = head.min(cap_head);
        memo.heads.insert(id, head);
        let body = leftover_zone_body_pack(head, seats.max(1), expand);
        let max_s = cells
            .iter()
            .filter_map(|c| cands.get(c).map(|&(s, _)| s))
            .max()
            .unwrap_or(1)
            .max(1);
        for &(gx, gy) in &cells {
            memo.zone.insert((gx, gy));
            let pack = if let Some(&(s, cap)) = cands.get(&(gx, gy)) {
                let local = leftover_pack_norm(cap.saturating_add(s), cap.max(1));
                let rel = s as f32 / max_s as f32;
                leftover_hue_pack(local * (0.5 + 0.5 * rel), body)
            } else {
                leftover_hue_pack(0.5, body)
            };
            if pack > 0.0 {
                memo.map.insert((gx, gy), pack);
            }
        }
        painted.push((cells, head, surplus, id));
    }
    memo.last_flood_us = t_flood.elapsed().as_micros().min(u128::from(u32::MAX)) as u32;
    leftover_plan_after_zone(world, memo, painted);
}

/// Standing leftover head + straw. Rebuilds the leftover field and
/// shoves leftover mass every tick so seepage cannot erase the
/// water-table bump while the boil cadence is idle. Overlay hill
/// paint is deferred to [`ensure_leftover_hill_view`].
pub(crate) fn apply_leftover_motor(
    world: &mut World,
    temp: &mut Temperature,
    cfg: &SteamConfig,
    humidity: Option<&mut Humidity>,
) {
    if !cfg.enabled {
        return;
    }
    if cfg.enable_pipe {
        crate::pipe::apply_pipe_motor(world, temp, cfg, humidity);
        // Pipe owns leftover. The 28k field and its Dijkstra walker
        // must not shove beside the straw. Cadence is off too — recondense
        // any stale cavity-humidity seat so a hot patch that cools does
        // not strand vapour when the boiler dies.
        let period = cfg.period_ticks.max(1);
        if world.tick % period == 0 && !world.steam.is_empty() {
            let below = cfg.boil_point_c - RECONDENSE_MARGIN_C;
            recondense_cool(world, temp, below);
        }
        return;
    }
    if !cfg.enable_leftover_field {
        return;
    }
    prepare_leftover_pressure(world, temp, cfg.boil_point_c, cfg.phase_expansion_drive);
    clear_leftover_lake_vents(world.chunk_cache_id.get());
    // Leftover mass *is* the pressure. Shove pore water / steam first so
    // a cadence boil cannot steal the reverse river into a dry vent.
    shove_phreatic_bump(world, temp);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{mineral_total, sat_totals};
    use crate::chunk::ChunkCoord;
    use crate::humidity::Humidity;

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
    fn flood_equalize_restores_vapour_when_park_is_jammed() {
        // Seats already hold 255 sat and 255 cave humidity. max_cells=1
        // so flood cannot re-place the three steam markers. Park has no
        // room — leftover must return as steam, not vanish.
        let mut w = World::new(19);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..8 {
            for y in 1..6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 3..6 {
            let mut air = Cell::air();
            air.sat = Sat(255);
            w.set_cell(x, 2, air);
            w.cave_humidity.insert((x, 2), 255);
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
            "jammed flood must restore vapour, not delete it"
        );
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
        let expect =
            leftover_pack_norm(vapor_volume_units(cap as u32, 120.0, 60.0, 100), cap as u32);
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
        let expect = leftover_pack_norm(
            vapor_volume_units(mass as u32, 120.0, 60.0, 100),
            cap as u32,
        );
        assert!(
            (p - expect).abs() < 0.02 && p > 0.85 && kind == CellPressureKind::PoreFlash,
            "half-full ×100 is leftover volume {expect}, not half-wetness (got {p}, {kind:?})"
        );
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
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "packed straw is mass-flat"
        );
        assert!(
            mouth1 > mouth0 || bot1 < bot0,
            "leftover must shove a marble through the full column (mouth {mouth0}→{mouth1}, base {bot0}→{bot1})"
        );
    }

    #[test]
    fn leftover_field_presses_cool_rock_above_one_boiler_cell() {
        // P used to light every hot wet cell (thermal blob). One 192×
        // boiler should charge ~192 seats along least resistance, including
        // cool rock above the source.
        let mut w = World::new(223);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..10 {
            for y in 0..14 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        let mut src = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(src, &w.hydro).max(1);
        src.sat = Sat(cap);
        w.set_cell(5, 1, src);
        let mut temp = temp_fill(&w, 20.0);
        let (hx, hy) = temp.tile_of(5, 1);
        temp.set_tile_c(hx, hy, 122.0);
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &temp, 100.0, 192);
        let (p_src, kind) = cell_pressure_norm_with_boil(&w, 5, 1, 122.0, 100.0, 192);
        assert!(
            p_src > 0.35 && kind == CellPressureKind::PoreFlash,
            "boiler cell must still pack ({p_src}, {kind:?})"
        );
        let (p_up, up_kind) = cell_pressure_norm_with_boil(&w, 5, 8, 20.0, 100.0, 192);
        assert!(
            p_up > 0.02 && up_kind == CellPressureKind::PoreFlash,
            "cool rock 7 seats above a 192× boiler must show leftover ({p_up}, {up_kind:?})"
        );
        let (p_far, far_kind) = cell_pressure_norm_with_boil(&w, 5, 13, 20.0, 100.0, 192);
        assert!(
            p_far > 0.02 && far_kind == CellPressureKind::PoreFlash,
            "the planned route must stay on P all the way to the surface ({p_far}, {far_kind:?})"
        );
        let (p_side, side_kind) = cell_pressure_norm_with_boil(&w, 8, 8, 20.0, 100.0, 192);
        assert!(
            p_side + 0.04 < p_up || side_kind == CellPressureKind::None,
            "off-route flank must stay dimmer than the planned chimney ({p_side} vs {p_up})"
        );
    }

    #[test]
    fn leftover_straw_hops_scale_with_steam_expand() {
        assert!(
            leftover_straw_hops(8_000, 192) > leftover_straw_hops(8_000, 96),
            "higher expand must buy a longer reverse river"
        );
        assert!(
            leftover_straw_hops(50_000, PHASE_EXPANSION_DRIVE_MAX) >= 240,
            "1400× leftover must be able to punch a deep column"
        );
        assert_eq!(
            vapor_volume_units(20, 120.0, 100.0, PHASE_EXPANSION_DRIVE_MAX),
            20 * PHASE_EXPANSION_DRIVE_MAX as u32,
            "20 sat at 1400× is 28k volume, not a u8 wrap"
        );
    }

    #[test]
    fn leftover_straw_climbs_the_pin_past_the_first_vadose_hump() {
        // Playtest: leftover found the crest (dry steam vent) because
        // heat walked the pin without leftover mass. Pressure *is* that
        // hot pore water — the straw must wet the climb and keep a table.
        let mut w = World::new(355);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..9 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut dry = Cell::solid(MaterialId::Stone);
        dry.pore = 40;
        dry.sat = Sat(0);
        for y in 5..40 {
            w.set_cell(6, y, dry);
        }
        for y in 40..48 {
            w.set_cell(6, y, Cell::air());
        }
        let water0 = sat_totals(&w).cell_total;
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..9 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: PHASE_EXPANSION_DRIVE_MAX,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=24 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let near = w.get_cell(6, 6).unwrap().sat.0 as u32;
        let mid = w.get_cell(6, 22).unwrap().sat.0 as u32;
        let high = w.get_cell(6, 34).unwrap().sat.0 as u32;
        let hill: u32 = (4..9)
            .flat_map(|x| (1..5).map(move |y| (x, y)))
            .map(|(x, y)| w.get_cell(x, y).unwrap().sat.0 as u32)
            .sum();
        assert_eq!(sat_totals(&w).cell_total, water0, "reverse river is mass-flat");
        assert!(
            leftover_has_pin(&w) && leftover_on_route(&w, 6, 22),
            "the packed climb must stay the leftover pin"
        );
        assert!(
            hill > 0,
            "leftover must not empty the wet hill out the mouth (hill {hill})"
        );
        assert!(
            mid > 0 || high > 0,
            "leftover must carry groundwater past the first vadose hump (near {near} mid {mid} high {high})"
        );
        assert!(
            mid + high >= near,
            "the reverse river must wet the climb, not only the seat above the boiler (near {near} mid {mid} high {high})"
        );
    }

    #[test]
    fn leftover_plans_a_surface_route_before_head_can_reach() {
        // Deep chimney: this tick's leftover cannot walk 40 seats, but
        // the pin must still map the cheapest path to sky.
        let mut w = World::new(317);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..56 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..4 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 4..50 {
            w.set_cell(6, y, gravel);
        }
        for y in 50..56 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..4 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &hot, 100.0, 32);
        assert!(
            leftover_has_pin(&w),
            "leftover must pin a surface route even when expand is 32"
        );
        assert!(
            leftover_on_route(&w, 6, 46),
            "the pin must reach the high chimney, not die where this tick's head ran out"
        );
    }

    #[test]
    fn leftover_huge_packed_reservoir_plans_sky_and_paints_a_trace() {
        // Playtest: a mountain-scale 150 °C body never grew a pin because
        // the planner spent its budget walking the vessel, then the 64-hop
        // stitch died before the deepest seed reached the rim. P must still
        // show the planned packed walk — faint if this tick's head cannot
        // finish the shove.
        let mut w = World::new(341);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 1..62 {
            for y in 0..62 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..50 {
            for y in 1..28 {
                w.set_cell(x, y, wet);
            }
        }
        let mut lid = Cell::solid(MaterialId::Stone);
        lid.pore = 40;
        lid.sat = Sat(0);
        for y in 28..52 {
            w.set_cell(26, y, lid);
        }
        for y in 52..62 {
            w.set_cell(26, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..50 {
            for y in 1..28 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &hot, 100.0, 32);
        assert!(
            leftover_has_pin(&w),
            "a huge 150 °C reservoir must pin a sky walk at expand 32"
        );
        assert!(
            leftover_on_route(&w, 26, 46),
            "the pin must climb the packed lid, not stop at the vessel rim"
        );
        let (p_trace, kind) = cell_pressure_norm_with_boil(&w, 26, 46, 20.0, 100.0, 32);
        assert!(
            p_trace >= 0.20 && kind == CellPressureKind::PoreFlash,
            "P must paint a faint planned trace above leftover head ({p_trace}, {kind:?})"
        );
        let (p_smear, smear_kind) =
            cell_pressure_norm_with_boil(&w, 28, 46, 20.0, 100.0, 32);
        assert!(
            p_smear < 0.04 || smear_kind == CellPressureKind::None,
            "planned trace must stay one cell wide ({p_smear}, {smear_kind:?})"
        );
    }

    #[test]
    fn leftover_plans_hilltop_sky_not_the_foot_ocean() {
        // Playtest: leftover=0.56 at the buried foot, ocean a few cells
        // away, crest <200 packed seats up. Seeding the whole rim pinned
        // the lake and never painted a trace up the hill.
        let mut w = World::new(351);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 0..62 {
            for y in 0..62 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 12..50 {
            for y in 1..12 {
                w.set_cell(x, y, wet);
            }
        }
        // Ocean touches the buried foot. The old planner flooded the
        // vessel at cost 0, then spent its budget on this lake halo.
        for x in 1..12 {
            for y in 1..10 {
                let mut lake = Cell::air();
                lake.sat = Sat(255);
                w.set_cell(x, y, lake);
            }
            for y in 10..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut lid = Cell::solid(MaterialId::Stone);
        lid.pore = 40;
        lid.sat = Sat(0);
        for y in 12..52 {
            w.set_cell(30, y, lid);
        }
        for y in 52..62 {
            w.set_cell(30, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 12..50 {
            for y in 1..12 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &hot, 100.0, PHASE_EXPANSION_DRIVE_MAX);
        assert!(
            leftover_has_pin(&w),
            "1400× leftover must pin a route off the buried reservoir"
        );
        assert!(
            leftover_on_route(&w, 30, 40),
            "the pin must climb the packed hill, not dump into the foot ocean"
        );
        assert!(
            !leftover_on_route(&w, 4, 6) && !leftover_on_route(&w, 11, 6),
            "the weather U at the foot is not the leftover chimney"
        );
        let (p_trace, kind) =
            cell_pressure_norm_with_boil(&w, 30, 40, 14.0, 100.0, PHASE_EXPANSION_DRIVE_MAX);
        assert!(
            p_trace >= 0.20 && kind == CellPressureKind::PoreFlash,
            "P must show a faint planned trace on the climb ({p_trace}, {kind:?})"
        );
        w.tick = STEAM_EVERY + 1;
        prepare_leftover_pressure(&w, &hot, 100.0, PHASE_EXPANSION_DRIVE_MAX);
        assert!(
            leftover_on_route(&w, 30, 40),
            "a lake pin must not lock out the crest on the next tick"
        );
    }

    #[test]
    fn leftover_plans_through_cold_strata_below_boil() {
        // Playtest: leftover overlay dies at the 100 °C isotherm. The
        // pin must still climb packed rock colder than boil. Leftover
        // mass (pore water) is the pressure; heat rides that mass.
        let mut w = World::new(353);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 0..62 {
            for y in 0..62 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 12..50 {
            for y in 1..8 {
                w.set_cell(x, y, wet);
            }
        }
        for x in 1..12 {
            for y in 1..8 {
                let mut lake = Cell::air();
                lake.sat = Sat(255);
                w.set_cell(x, y, lake);
            }
            for y in 8..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut cold = Cell::solid(MaterialId::Stone);
        cold.pore = 40;
        cold.sat = Sat(cap);
        for y in 8..52 {
            w.set_cell(30, y, cold);
        }
        for y in 52..62 {
            w.set_cell(30, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 12..50 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 150.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &hot, 100.0, PHASE_EXPANSION_DRIVE_MAX);
        assert!(
            leftover_has_pin(&w),
            "1400× leftover must pin a sky walk through rock colder than boil"
        );
        assert!(
            leftover_on_route(&w, 30, 36) && leftover_on_route(&w, 30, 51),
            "the pin must climb the 20 °C column, not stop at the 100 °C rim"
        );
        let last = LEFTOVER_MEMO.with(|s| s.borrow().pin_path.last().copied());
        assert_eq!(
            last,
            Some((30, 52)),
            "the pin must end at open sky, not the foot ocean"
        );
        let (p_trace, kind) =
            cell_pressure_norm_with_boil(&w, 30, 36, 20.0, 100.0, PHASE_EXPANSION_DRIVE_MAX);
        assert!(
            p_trace >= 0.20 && kind == CellPressureKind::PoreFlash,
            "P must show a faint planned trace on cold packed rock ({p_trace}, {kind:?})"
        );
        let t_before = hot.at_cell(30, 36);
        let sat0 = w.get_cell(30, 36).unwrap().sat.0;
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: PHASE_EXPANSION_DRIVE_MAX,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in STEAM_EVERY..=STEAM_EVERY + 8 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let t_after = hot.at_cell(30, 36);
        let sat1 = w.get_cell(30, 36).unwrap().sat.0;
        assert!(
            sat1 > 0,
            "leftover pressure is leftover mass — the cold climb must stay wet ({sat0}→{sat1})"
        );
        assert!(
            t_after > t_before + 2.0,
            "leftover mass must carry heat through rock colder than boil ({t_before} → {t_after})"
        );
    }

    #[test]
    fn leftover_enclosed_gravel_joins_the_chamber() {
        let mut w = World::new(319);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..14 {
            for y in 0..16 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..10 {
            for y in 1..8 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 180;
        gravel.sat = Sat(water_capacity_cell(gravel, &w.hydro).max(1));
        w.set_cell(6, 4, gravel);
        w.set_cell(7, 4, gravel);
        w.set_cell(6, 5, gravel);
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..10 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        assert!(
            leftover_in_zone(&w, 6, 4) && leftover_in_zone(&w, 7, 4),
            "gravel surrounded by packed stone is chamber, not chimney skin"
        );
        assert!(
            !leftover_is_open_pipe(&w, 6, 4),
            "a buried lens must not read as a sky pipe"
        );
    }

    #[test]
    fn leftover_hill_view_paints_on_demand() {
        let mut w = World::new(319);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..14 {
            for y in 0..16 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..10 {
            for y in 1..8 {
                w.set_cell(x, y, wet);
            }
        }
        w.set_cell(6, 8, Cell::air());
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..10 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let (z0, _) = leftover_field_stats(&w);
        assert!(z0 > 0, "prepare must seed leftover");
        assert!(
            leftover_in_zone(&w, 6, 4),
            "straw membership must still see the vessel"
        );
        assert!(
            !LEFTOVER_MEMO.with(|s| s.borrow().view_ready),
            "sim prepare must not flood the P-overlay hill"
        );
        ensure_leftover_hill_view(&w, &hot, 100.0, 192);
        assert!(
            LEFTOVER_MEMO.with(|s| s.borrow().view_ready),
            "P overlay must paint the leftover hill on demand"
        );
        let last = LEFTOVER_MEMO.with(|s| s.borrow().pin_path.last().copied());
        if let Some((x, y)) = last {
            assert!(
                leftover_on_route(&w, x, y),
                "route_set must include pin dests, not only keys"
            );
        }
        w.tick = STEAM_EVERY + 1;
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let last2 = LEFTOVER_MEMO.with(|s| s.borrow().pin_path.last().copied());
        assert_eq!(
            last, last2,
            "a live pin must reuse without a new exterior halo walk"
        );
        if let Some((x, y)) = last2 {
            assert!(
                leftover_on_route(&w, x, y),
                "reused pin dests stay leftover-on-route"
            );
        }
    }

    #[test]
    fn leftover_pin_survives_gravel_weld_to_stone() {
        let mut w = World::new(321);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..40 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..32 {
            w.set_cell(6, y, gravel);
        }
        for y in 32..40 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        w.tick = 1;
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        assert!(leftover_has_pin(&w), "gravel chimney must pin");
        let mut stone = Cell::solid(MaterialId::Stone);
        stone.pore = 80;
        w.set_cell(6, 20, stone);
        w.tick = 2;
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        assert!(
            leftover_has_pin(&w) && leftover_on_route(&w, 6, 20),
            "weld to stone along the pin must not retarget the spring"
        );
    }

    #[test]
    fn leftover_pulse_sinter_does_not_destroy_packed_water() {
        // Loaded full-sat sand boxed in bedrock. Leftover pulse cements
        // to sandstone and capacity drops; park has no seat.
        let mut w = World::new(331);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            for y in 0..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.pore = 220;
        sand.sat = Sat(water_capacity_cell(sand, &w.hydro).max(1));
        w.set_cell(5, 5, sand);
        add_dissolved(&mut w, 5, 5, CEMENT_MIN_LOAD + 4);
        let before = sat_totals(&w).cell_total;
        leftover_pulse_loose_channel(&mut w, 5, 5);
        assert_eq!(
            sat_totals(&w).cell_total,
            before,
            "leftover pulse sinter must not delete water"
        );
    }

    #[test]
    fn leftover_thermal_sinter_soak_is_mass_flat() {
        // Wet sand chimney + carbonate load. Leftover pulses cement /
        // wear while boil shoves the straw. Cell water (sat+steam+cave_h)
        // must stay flat — no thermal delete.
        let mut w = World::new(333);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..20 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..6 {
                w.set_cell(x, y, wet);
            }
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.pore = 200;
        let sand_cap = water_capacity_cell(sand, &w.hydro).max(1);
        sand.sat = Sat(sand_cap);
        for y in 6..14 {
            w.set_cell(6, y, sand);
            add_dissolved(&mut w, 6, y, CEMENT_MIN_LOAD);
        }
        for y in 14..20 {
            w.set_cell(6, y, Cell::air());
        }
        let water0 = sat_totals(&w).cell_total;
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..6 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=24 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "leftover + sinter soak must stay mass-flat"
        );
    }

    #[test]
    fn leftover_route_carries_heat_into_cool_rock_above() {
        let mut w = World::new(253);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..9 {
            for y in 0..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        let mut src = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(src, &w.hydro).max(1);
        src.sat = Sat(cap);
        w.set_cell(5, 1, src);
        let mut hot = temp_fill(&w, 20.0);
        let (hx, hy) = hot.tile_of(5, 1);
        hot.set_tile_c(hx, hy, 160.0);
        let t_up0 = hot.at_cell(5, 8);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            ..SteamConfig::default()
        };
        for t in 1..=10 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let t_up1 = hot.at_cell(5, 8);
        assert!(
            t_up1 > t_up0 + 4.0,
            "leftover reverse river must carry heat up the winning route ({t_up0}→{t_up1})"
        );
    }

    #[test]
    fn leftover_gravel_chimney_springs_to_open_sky() {
        // Playtest: leftover stalled mid-mountain even at 1400× until a
        // gravel cheat path was carved. P followed the gravel; the straw
        // parked at the first vadose cell / died at packed-hop 24.
        let mut w = World::new(271);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..9 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..40 {
            w.set_cell(6, y, gravel);
        }
        for y in 40..46 {
            w.set_cell(6, y, Cell::air());
        }
        let water0 = sat_totals(&w).cell_total;
        let mouth0 = w.get_cell(6, 40).unwrap().sat.0;
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..9 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=24 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let mouth1 = w.get_cell(6, 40).unwrap().sat.0;
        let column: u32 = (8..40)
            .map(|y| w.get_cell(6, y).unwrap().sat.0 as u32)
            .sum();
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "gravel spring is mass-flat"
        );
        assert!(
            mouth1 > mouth0 || column > 0,
            "leftover must walk the gravel chimney to sky (mouth {mouth0}→{mouth1}, top {column})"
        );
    }

    #[test]
    fn leftover_locked_route_springs_past_a_vadose_stone_at_the_mouth() {
        // Playtest: gravel at the ridge was 7/90 and the neighbour stone
        // 2/19 — leftover parked in that hole instead of dumping to sky.
        let mut w = World::new(277);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..9 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..40 {
            w.set_cell(6, y, gravel);
        }
        for y in 40..46 {
            w.set_cell(6, y, Cell::air());
        }
        w.set_cell(7, 39, Cell::solid(MaterialId::Stone));
        let water0 = sat_totals(&w).cell_total;
        let mouth0 = w.get_cell(6, 40).unwrap().sat.0;
        let side0 = w.get_cell(7, 39).unwrap().sat.0;
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..9 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=24 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let mouth1 = w.get_cell(6, 40).unwrap().sat.0;
        let side1 = w.get_cell(7, 39).unwrap().sat.0;
        let top = w.get_cell(6, 39).unwrap().sat.0;
        let column: u32 = (8..40)
            .map(|y| w.get_cell(6, y).unwrap().sat.0 as u32)
            .sum();
        assert_eq!(sat_totals(&w).cell_total, water0, "locked spring is mass-flat");
        assert!(
            mouth1 > mouth0 || column > 0,
            "locked gravel route must climb the chimney (mouth {mouth0}→{mouth1}, top {top}, col {column}, side {side0}→{side1})"
        );
        assert!(
            side1 <= top.max(mouth1),
            "side-stone vadose must not steal the spring (side {side1} vs gravel top {top} mouth {mouth1})"
        );
    }

    #[test]
    fn leftover_heat_follows_gravel_channel_not_side_rock() {
        let mut w = World::new(273);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..16 {
            for y in 0..40 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..36 {
            w.set_cell(6, y, gravel);
        }
        for y in 36..40 {
            w.set_cell(6, y, Cell::air());
        }
        // Isolated stone column at the same height — geothermal pyramid
        // would warm it; leftover heat must not, because it is off-channel.
        for y in 5..36 {
            w.set_cell(12, y, Cell::solid(MaterialId::Stone));
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 160.0);
            }
        }
        let t_side0 = hot.at_cell(12, 28);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            ..SteamConfig::default()
        };
        for t in 1..=24 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let t_path = hot.at_cell(6, 12);
        let t_side = hot.at_cell(12, 28);
        let sat_path: u32 = (8..=16)
            .map(|y| w.get_cell(6, y).unwrap().sat.0 as u32)
            .sum();
        assert!(
            sat_path > 0 && t_path > t_side + 8.0,
            "leftover heat must follow leftover mass in the gravel, not a heat pyramid (sat {sat_path}, {t_path} vs side {t_side}, side0 {t_side0})"
        );
    }

    #[test]
    fn leftover_pinned_route_keeps_gravel_when_a_side_pore_opens() {
        // Playtest: leftover locked a gravel cheat path, then kept
        // punching a high-pore stone slab for a second chimney.
        let mut w = World::new(279);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..9 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..40 {
            w.set_cell(6, y, gravel);
        }
        for y in 40..46 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..9 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=4 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        // Competing competent chimney — cheaper on a fresh lock, must
        // not steal a live pin.
        let mut slab = Cell::solid(MaterialId::Stone);
        slab.pore = 250;
        for y in 5..40 {
            w.set_cell(8, y, slab);
        }
        for y in 40..46 {
            w.set_cell(8, y, Cell::air());
        }
        let slab_pore0 = w.get_cell(8, 20).unwrap().pore;
        let gravel0: u32 = (10..40)
            .map(|y| w.get_cell(6, y).unwrap().sat.0 as u32)
            .sum();
        for t in 5..=16 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let gravel1: u32 = (10..40)
            .map(|y| w.get_cell(6, y).unwrap().sat.0 as u32)
            .sum();
        let slab1: u32 = (10..40)
            .map(|y| w.get_cell(8, y).unwrap().sat.0 as u32)
            .sum();
        let slab_pore1 = w.get_cell(8, 20).unwrap().pore;
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let p_g = (10..40)
            .map(|y| cell_pressure_norm_with_boil(&w, 6, y, 20.0, 100.0, 192).0)
            .fold(0.0f32, f32::max);
        let p_s = (10..40)
            .map(|y| cell_pressure_norm_with_boil(&w, 8, y, 20.0, 100.0, 192).0)
            .fold(0.0f32, f32::max);
        assert!(
            leftover_on_route(&w, 6, 20) && !leftover_on_route(&w, 8, 20),
            "live pin must stay on the gravel chimney (gravel sat {gravel1} vs slab {slab1}, was {gravel0})"
        );
        assert!(
            p_g > p_s + 0.04,
            "P overlay must stay on the pinned gravel, not the new slab ({p_g} vs {p_s})"
        );
        assert!(
            slab_pore1 <= slab_pore0.saturating_add(6),
            "leftover must not widen a competing slab ({slab_pore0}→{slab_pore1})"
        );
    }

    #[test]
    fn leftover_pin_overlay_is_one_cell_wide() {
        // Playtest: leftover P was a 4×4 pink sausage because every wet
        // neighbour in the hot temperature tile joined the zone body.
        let mut w = World::new(283);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..40 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..32 {
            w.set_cell(6, y, gravel);
        }
        // Same 4×4 temp tile as the gravel vein — used to light up as
        // a pink blob even though it is off the pin.
        w.set_cell(7, 20, wet);
        w.set_cell(5, 20, wet);
        for y in 32..36 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let (hx, hy) = hot.tile_of(6, 20);
        hot.set_tile_c(hx, hy, 122.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=6 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let (p_path, _) = cell_pressure_norm_with_boil(&w, 6, 20, 122.0, 100.0, 192);
        let (p_r, _) = cell_pressure_norm_with_boil(&w, 7, 20, 122.0, 100.0, 192);
        let (p_l, _) = cell_pressure_norm_with_boil(&w, 5, 20, 122.0, 100.0, 192);
        assert!(
            p_path > 0.08,
            "pinned gravel front must stay visible ({p_path})"
        );
        assert!(
            p_r < 0.04 && p_l < 0.04,
            "same-tile wet stone must not paint a 4×4 leftover blob (path {p_path} L {p_l} R {p_r})"
        );
        let mut p_body = 0.0f32;
        for x in 4..8 {
            for y in 1..5 {
                if w.get_cell(x, y).is_some_and(|c| c.sat.0 == 0) {
                    continue;
                }
                let (p, _) = cell_pressure_norm_with_boil(&w, x, y, 122.0, 100.0, 192);
                p_body = p_body.max(p);
            }
        }
        assert!(
            p_body > 0.20,
            "leftover reservoir must stay on P (got {p_body})"
        );
    }

    #[test]
    fn leftover_boiling_hill_is_not_a_magenta_sticker() {
        // Playtest: leftover P filled the whole connected wet hill as a
        // flat magenta flood (17 °C stone in a hot 4×4 tile lit up).
        let mut w = World::new(311);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..24 {
            for y in 0..40 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..22 {
            for y in 1..20 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 20..32 {
            w.set_cell(6, y, gravel);
        }
        for y in 32..36 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..22 {
            for y in 1..20 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=6 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let mut on = 0u32;
        for x in 4..22 {
            for y in 1..20 {
                let Some(c) = w.get_cell(x, y) else {
                    continue;
                };
                if c.material == MaterialId::Air
                    || leftover_is_chimney_skin(c.material)
                    || c.sat.0 == 0
                {
                    continue;
                }
                let (p, _) = cell_pressure_norm_with_boil(&w, x, y, 122.0, 100.0, 192);
                if p > 0.08 {
                    on += 1;
                }
            }
        }
        let (p_pipe, _) = cell_pressure_norm_with_boil(&w, 6, 26, 20.0, 100.0, 192);
        assert!(
            on > 8,
            "leftover reservoir must light up ({on} wet packed cells)"
        );
        assert!(p_pipe > 0.08, "chimney must stay on P ({p_pipe})");
        let lo = leftover_hue_pack(0.08, 0.38);
        let hi = leftover_hue_pack(1.0, 0.68);
        assert!(
            lo >= 0.20 && lo < 0.40,
            "low leftover must stay magenta ({lo})"
        );
        assert!(
            hi > 0.80,
            "high leftover must reach orange/yellow ({hi})"
        );
        assert!(
            hi - lo > 0.40,
            "leftover hues must span the P ramp, not one burnt magenta ({lo}→{hi})"
        );
    }

    #[test]
    fn leftover_p_follows_wet_hill_not_heat_tiles() {
        // Heat is 4×4. P is the leftover vessel: wet packed stone on
        // the same hill, plus the 1-cell pin. Cool wet rock stays on;
        // a wet pocket behind bedrock stays off.
        let mut w = World::new(313);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..24 {
            for y in 0..40 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..16 {
            for y in 1..12 {
                w.set_cell(x, y, wet);
            }
        }
        for y in 4..8 {
            w.set_cell(20, y, wet);
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 12..28 {
            w.set_cell(6, y, gravel);
        }
        for y in 28..32 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        let (hx, hy) = hot.tile_of(6, 6);
        for dx in 0..=1 {
            for dy in 0..=1 {
                hot.set_tile_c(hx + dx, hy + dy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=6 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        for dx in 0..=1 {
            for dy in 0..=1 {
                hot.set_tile_c(hx + dx, hy + dy, 122.0);
            }
        }
        for x in 4..16 {
            for y in 1..12 {
                if let Some(mut c) = w.get_cell(x, y) {
                    if c.material == MaterialId::Stone {
                        c.sat = Sat(cap);
                        w.set_cell(x, y, c);
                    }
                }
            }
        }
        for y in 4..8 {
            if let Some(mut c) = w.get_cell(20, y) {
                if c.material == MaterialId::Stone {
                    c.sat = Sat(cap);
                    w.set_cell(20, y, c);
                }
            }
        }
        w.tick = 7;
        ensure_leftover_hill_view(&w, &hot, 100.0, 192);
        let (p_core, _) = cell_pressure_norm_with_boil(&w, 6, 6, 122.0, 100.0, 192);
        let (p_hill, _) = cell_pressure_norm_with_boil(&w, 14, 6, 20.0, 100.0, 192);
        let (p_iso, _) = cell_pressure_norm_with_boil(&w, 20, 6, 20.0, 100.0, 192);
        let (p_pipe, _) = cell_pressure_norm_with_boil(&w, 6, 20, 20.0, 100.0, 192);
        assert!(p_core > 0.20, "hot leftover core must stay on P ({p_core})");
        assert!(
            p_hill > 0.20,
            "leftover-connected wet packed stone stays on even when the heat tile is cool ({p_hill})"
        );
        assert!(
            p_iso <= 0.0,
            "wet packed stone behind a bedrock gap stays off ({p_iso})"
        );
        assert!(p_pipe > 0.08, "chimney must stay on P ({p_pipe})");
        assert!(
            leftover_is_overlay_rock(w.get_cell(14, 6).unwrap()),
            "the cool hill is wet packed stone on the same vessel"
        );
    }

    #[test]
    fn leftover_pin_does_not_reenter_the_ridge_past_the_mouth() {
        // Playtest: leftover dumped at the crest then walked ridge sand
        // back into the mountain above and below the mouth.
        let mut w = World::new(301);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..16 {
            for y in 0..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..9 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..40 {
            w.set_cell(6, y, gravel);
        }
        for y in 40..48 {
            w.set_cell(6, y, Cell::air());
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.pore = 200;
        for x in 7..12 {
            w.set_cell(x, 38, sand);
            w.set_cell(x, 41, sand);
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..9 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=10 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let (p_pipe, _) = cell_pressure_norm_with_boil(&w, 6, 25, 20.0, 100.0, 192);
        let (p_back, _) = cell_pressure_norm_with_boil(&w, 10, 38, 20.0, 100.0, 192);
        let (p_above, _) = cell_pressure_norm_with_boil(&w, 10, 41, 20.0, 100.0, 192);
        let (p_lip, _) = cell_pressure_norm_with_boil(&w, 7, 38, 20.0, 100.0, 192);
        let (p_reenter, _) = cell_pressure_norm_with_boil(&w, 7, 41, 20.0, 100.0, 192);
        let mut p_body = 0.0f32;
        for x in 4..9 {
            for y in 1..5 {
                let cell = w.get_cell(x, y).unwrap();
                if cell.sat.0 == 0 {
                    continue;
                }
                let (p, _) = cell_pressure_norm_with_boil(&w, x, y, 122.0, 100.0, 192);
                p_body = p_body.max(p);
            }
        }
        assert!(p_pipe > 0.08, "chimney must stay on P ({p_pipe})");
        assert!(
            leftover_has_pin(&w),
            "a working sky pin must survive even if the tiny boiler has already punched through (body {p_body})"
        );
        assert!(
            p_back < 0.04
                && p_above < 0.04
                && p_lip < 0.04
                && p_reenter < 0.04,
            "leftover must not re-enter the ridge past the mouth (back {p_back} above {p_above} lip {p_lip} reenter {p_reenter})"
        );
    }

    #[test]
    fn leftover_pin_survives_mouth_sandstone() {
        let mut w = World::new(303);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..12 {
            for y in 0..40 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..32 {
            w.set_cell(6, y, gravel);
        }
        for y in 32..36 {
            w.set_cell(6, y, Cell::air());
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=6 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        w.set_cell(6, 31, Cell::solid(MaterialId::Sandstone));
        prepare_leftover_pressure(&w, &hot, 100.0, 192);
        let (p_path, _) = cell_pressure_norm_with_boil(&w, 6, 20, 20.0, 100.0, 192);
        assert!(
            p_path > 0.08,
            "mouth sinter must not break the leftover pin ({p_path})"
        );
    }

    #[test]
    fn leftover_heat_stops_at_open_water_mouth() {
        // Playtest: rain sheet over the mouth. Leftover heat dumped into
        // the first water tile, not a 16-cell finger through the stream.
        let mut w = World::new(281);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..30 {
            for y in 0..24 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..8 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..16 {
            w.set_cell(6, y, gravel);
        }
        for x in 6..24 {
            let mut water = Cell::air();
            water.sat = Sat(255);
            w.set_cell(x, 16, water);
            for y in 17..24 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..8 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 160.0);
            }
        }
        let t_far0 = hot.at_cell(22, 16);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=6 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let t_mouth = hot.at_cell(6, 16);
        let t_far = hot.at_cell(22, 16);
        assert!(
            t_mouth > t_far + 8.0,
            "leftover heat must dump at the first open water ({t_mouth} vs far {t_far})"
        );
        assert!(
            t_far < t_far0 + 6.0,
            "downstream rain must not take leftover heat ({t_far0}→{t_far})"
        );
    }

    #[test]
    fn leftover_raises_a_water_table_bump_not_the_side_lake() {
        let mut w = World::new(225);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..10 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        let mut src = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(src, &w.hydro).max(1);
        src.sat = Sat(cap);
        w.set_cell(5, 1, src);
        // Packed flank so leftover cannot park beside the lake.
        for y in 1..6 {
            w.set_cell(4, y, src);
        }
        // Side lake — the old reverse-seep magnet.
        for y in 1..6 {
            let mut lake = Cell::air();
            lake.sat = Sat(255);
            w.set_cell(3, y, lake);
        }
        for y in 6..10 {
            w.set_cell(3, y, Cell::air());
        }
        let water0 = sat_totals(&w).cell_total;
        let lake0: u32 = (1..6).map(|y| w.get_cell(3, y).unwrap().sat.0 as u32).sum();
        let up0 = w.get_cell(4, 6).unwrap().sat.0 as u16
            + (2..=6)
                .map(|y| w.get_cell(5, y).unwrap().sat.0 as u16)
                .sum::<u16>();
        let mut hot = temp_fill(&w, 20.0);
        let (hx, hy) = hot.tile_of(5, 1);
        hot.set_tile_c(hx, hy, 122.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        // A long straw will later treat the packed column + weather lake
        // as relief. Catch the table rise before that dump — including
        // off-cadence leftover ticks so boil cannot steal the river.
        for t in 1..=8 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let up1 = w.get_cell(4, 6).unwrap().sat.0 as u16
            + (2..=6)
                .map(|y| w.get_cell(5, y).unwrap().sat.0 as u16)
                .sum::<u16>();
        let lake1: u32 = (1..6).map(|y| w.get_cell(3, y).unwrap().sat.0 as u32).sum();
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "water-table bump is mass-flat"
        );
        assert!(
            up1 > up0,
            "leftover must raise the packed column, not only dump into the lake (up {up0}→{up1})"
        );
        assert!(
            lake1 <= lake0 + 8,
            "side lake must not swallow the first leftover pulses (lake {lake0}→{lake1})"
        );
    }

    #[test]
    fn leftover_shoves_groundwater_on_off_cadence_ticks() {
        // Seepage runs every tick; boil does not. The standing head must
        // still push on tick 3 or the table never holds.
        let mut w = World::new(231);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..9 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        let mut src = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(src, &w.hydro).max(1);
        src.sat = Sat(cap);
        w.set_cell(5, 1, src);
        let up0: u16 = (2..=6)
            .map(|y| w.get_cell(5, y).unwrap().sat.0 as u16)
            .sum();
        let mut hot = temp_fill(&w, 20.0);
        let (hx, hy) = hot.tile_of(5, 1);
        hot.set_tile_c(hx, hy, 122.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            ..SteamConfig::default()
        };
        w.tick = 3;
        apply_steam(&mut w, &mut hot, &cfg);
        let up1: u16 = (2..=6)
            .map(|y| w.get_cell(5, y).unwrap().sat.0 as u16)
            .sum();
        assert!(
            up1 > up0,
            "off-cadence leftover must still raise the table (up {up0}→{up1})"
        );
    }

    #[test]
    fn leftover_swells_a_wet_hill_through_seepage_and_a_side_lake() {
        // Playtest: saturated aquifer, dry cap, 122 °C core, ocean magnet.
        // Leftover volume must swell the vadose column, not dump into the lake,
        // and the bump must survive seepage ticks between boil pulses.
        let mut w = World::new(233);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 4..13 {
            for y in 0..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(8, 0, Cell::solid(MaterialId::Bedrock));
        let mut aquifer = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(aquifer, &w.hydro).max(1);
        aquifer.sat = Sat(cap);
        for x in 4..13 {
            for y in 1..6 {
                w.set_cell(x, y, aquifer);
            }
        }
        for y in 1..8 {
            let mut lake = Cell::air();
            lake.sat = Sat(255);
            w.set_cell(2, y, lake);
        }
        for y in 8..12 {
            w.set_cell(2, y, Cell::air());
        }
        let vadose0: u16 = (6..=10)
            .map(|y| w.get_cell(8, y).unwrap().sat.0 as u16)
            .sum();
        let lake0: u32 = (1..8).map(|y| w.get_cell(2, y).unwrap().sat.0 as u32).sum();
        let water0 = sat_totals(&w).cell_total;
        let mut hot = temp_fill(&w, 20.0);
        let (hx, hy) = hot.tile_of(8, 2);
        hot.set_tile_c(hx, hy, 122.0);
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=36 {
            w.tick = t;
            crate::apply_seepage(&mut w);
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let vadose1: u16 = (6..=10)
            .map(|y| w.get_cell(8, y).unwrap().sat.0 as u16)
            .sum();
        let lake1: u32 = (1..8).map(|y| w.get_cell(2, y).unwrap().sat.0 as u32).sum();
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "leftover swell is mass-flat"
        );
        assert!(
            vadose1 > vadose0,
            "leftover must swell the dry cap above the wet hill (vadose {vadose0}→{vadose1})"
        );
        assert!(
            lake1 <= lake0 + 40,
            "side lake must not swallow the leftover head (lake {lake0}→{lake1})"
        );
    }

    #[test]
    fn leftover_field_follows_a_sand_vein() {
        let mut w = World::new(227);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 2..12 {
            for y in 0..12 {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        let mut src = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(src, &w.hydro).max(1);
        src.sat = Sat(cap);
        w.set_cell(5, 1, src);
        for y in 2..10 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.pore = 200;
            w.set_cell(8, y, sand);
        }
        // Open the first hop into the vein.
        let mut gate = Cell::solid(MaterialId::Sand);
        gate.pore = 200;
        w.set_cell(6, 2, gate);
        w.set_cell(7, 2, gate);
        let mut temp = temp_fill(&w, 20.0);
        let (hx, hy) = temp.tile_of(5, 1);
        temp.set_tile_c(hx, hy, 122.0);
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &temp, 100.0, 192);
        let (vein, _) = cell_pressure_norm_with_boil(&w, 8, 8, 20.0, 100.0, 192);
        let (tight, _) = cell_pressure_norm_with_boil(&w, 3, 8, 20.0, 100.0, 192);
        assert!(
            vein > tight + 0.05,
            "leftover must follow the sand vein, not smear both flanks ({vein} vs {tight})"
        );
    }

    #[test]
    fn leftover_adjacent_boilers_share_one_zone() {
        // Playtest: a grid of yellow seed lights. Adjacent boiling wet
        // cells are one vessel — same body pressure, not 0.99 dots.
        let mut w = World::new(229);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..10 {
            for y in 1..9 {
                w.set_cell(x, y, wet);
            }
        }
        w.set_cell(5, 0, Cell::solid(MaterialId::Bedrock));
        let mut temp = temp_fill(&w, 20.0);
        for x in 2..10 {
            for y in 1..9 {
                let (hx, hy) = temp.tile_of(x, y);
                temp.set_tile_c(hx, hy, 122.0);
            }
        }
        w.tick = STEAM_EVERY;
        prepare_leftover_pressure(&w, &temp, 100.0, 192);
        let (a, _) = cell_pressure_norm_with_boil(&w, 3, 3, 122.0, 100.0, 192);
        let (b, _) = cell_pressure_norm_with_boil(&w, 8, 7, 122.0, 100.0, 192);
        let mut yellow = 0u32;
        for x in 2..10 {
            for y in 1..9 {
                let (p, _) = cell_pressure_norm_with_boil(&w, x, y, 122.0, 100.0, 192);
                if p > 0.88 {
                    yellow += 1;
                }
            }
        }
        assert!(
            a > 0.35 && b > 0.35,
            "connected boilers must share a visible zone ({a}, {b})"
        );
        assert!(
            (a - b).abs() < 0.16,
            "adjacent leftover must be one vessel, not a seed grid ({a} vs {b})"
        );
        assert_eq!(
            yellow, 0,
            "fresh equal vessel is one hue, not a grid of maxed yellow dots ({yellow} cells >0.88)"
        );
    }

    #[test]
    fn leftover_head_grows_until_relief() {
        // No outlet: the vessel keeps packing. Pulse is a function of release.
        let mut w = World::new(241);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..11 {
            for y in 0..8 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 3..10 {
            for y in 1..7 {
                w.set_cell(x, y, wet);
            }
        }
        let mut hot = temp_fill(&w, 20.0);
        for x in 3..10 {
            for y in 1..7 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            ..SteamConfig::default()
        };
        w.tick = 1;
        apply_steam(&mut w, &mut hot, &cfg);
        let (p0, _) = cell_pressure_norm_with_boil(&w, 6, 3, 122.0, 100.0, 192);
        for t in 2..=8 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let (p1, _) = cell_pressure_norm_with_boil(&w, 6, 3, 122.0, 100.0, 192);
        assert!(
            p1 > p0 + 0.04,
            "sealed leftover head must grow until relief ({p0}→{p1})"
        );
        assert!(
            p1 > 0.72,
            "packed leftover must leave burnt magenta for orange/yellow ({p1})"
        );
        assert!(
            p1 <= 0.92,
            "growing head must stay on the leftover hue ramp ({p1})"
        );
    }

    #[test]
    fn leftover_full_zone_vents_through_loose_to_open_air() {
        // Playtest: 80×50 packed boiling body. No vadose inside the
        // vessel — relief is the cheapest loose path to sky.
        let mut w = World::new(243);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..13 {
            for y in 0..18 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 3..11 {
            for y in 1..8 {
                w.set_cell(x, y, wet);
            }
        }
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.pore = 200;
        let sand_cap = water_capacity_cell(sand, &w.hydro).max(1);
        sand.sat = Sat(sand_cap);
        for y in 8..13 {
            w.set_cell(8, y, sand);
        }
        for y in 13..18 {
            w.set_cell(8, y, Cell::air());
        }
        let water0 = sat_totals(&w).cell_total;
        let air0 = w.get_cell(8, 13).unwrap().sat.0;
        let mut hot = temp_fill(&w, 20.0);
        for x in 3..11 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=20 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let air1 = w.get_cell(8, 13).unwrap().sat.0;
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "loose relief is mass-flat"
        );
        assert!(
            air1 > air0,
            "packed leftover must find the sand chimney and vent ({air0}→{air1})"
        );
    }

    #[test]
    fn leftover_full_zone_walks_a_confined_cavity_to_sky() {
        let mut w = World::new(245);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        // Bedrock mass so the only cheap path is the carved cave.
        for x in 2..13 {
            for y in 0..15 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 3..10 {
            for y in 1..8 {
                w.set_cell(x, y, wet);
            }
        }
        // Roofed pocket that side-opens to sky.
        for &(x, y) in &[(6, 8), (7, 8), (8, 8), (8, 9), (8, 10), (8, 11), (8, 12)] {
            w.set_cell(x, y, Cell::air());
        }
        let water0 = sat_totals(&w).cell_total;
        let mouth0 = w.get_cell(8, 10).unwrap().sat.0;
        let mut hot = temp_fill(&w, 20.0);
        for x in 3..10 {
            for y in 1..8 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=24 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let tunnel: u32 = [(6, 8), (7, 8), (8, 8), (8, 9), (8, 10), (8, 11), (8, 12)]
            .iter()
            .map(|&(x, y)| w.get_cell(x, y).unwrap().sat.0 as u32)
            .sum();
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "cavity relief is mass-flat"
        );
        assert!(
            tunnel > mouth0 as u32,
            "leftover must walk the confined cave toward sky (tunnel {tunnel})"
        );
    }

    #[test]
    fn leftover_full_zone_discharges_into_weather_u_lake() {
        let mut w = World::new(247);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 0..14 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 5..12 {
            for y in 1..6 {
                w.set_cell(x, y, wet);
            }
        }
        // Wide open U — weather lake, not a boiler pocket.
        for x in 1..5 {
            for y in 1..6 {
                let mut lake = Cell::air();
                lake.sat = Sat(255);
                w.set_cell(x, y, lake);
            }
            for y in 6..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        assert!(
            !vessel_is_boiler(&w, 2, 3),
            "open U lake must survive the boiler test"
        );
        let water0 = sat_totals(&w).cell_total;
        let src0: u32 = (1..6).map(|y| w.get_cell(5, y).unwrap().sat.0 as u32).sum();
        let mut hot = temp_fill(&w, 20.0);
        for x in 5..12 {
            for y in 1..6 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=16 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let src1: u32 = (1..6).map(|y| w.get_cell(5, y).unwrap().sat.0 as u32).sum();
        assert_eq!(
            sat_totals(&w).cell_total,
            water0,
            "lake discharge is mass-flat"
        );
        assert!(
            src1 < src0,
            "packed leftover must discharge into the weather lake (src {src0}→{src1})"
        );
    }

    #[test]
    fn leftover_flooded_boiler_is_a_path_not_relief() {
        let mut w = World::new(249);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Stone);
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..11 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 4..9 {
            for y in 1..6 {
                w.set_cell(x, y, wet);
            }
        }
        // Roofed flooded pocket — boiler, not a U-bowl lake.
        for x in 5..8 {
            let mut pool = Cell::air();
            pool.sat = Sat(255);
            w.set_cell(x, 6, pool);
            w.set_cell(x, 7, Cell::solid(MaterialId::Bedrock));
        }
        assert!(
            vessel_is_boiler(&w, 6, 6),
            "flooded closed cavity is a boiler path hop"
        );
        let mut hot = temp_fill(&w, 20.0);
        for x in 4..9 {
            for y in 1..6 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            ..SteamConfig::default()
        };
        w.tick = 1;
        apply_steam(&mut w, &mut hot, &cfg);
        let (p0, _) = cell_pressure_norm_with_boil(&w, 6, 3, 122.0, 100.0, 192);
        for t in 2..=8 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        let (p1, _) = cell_pressure_norm_with_boil(&w, 6, 3, 122.0, 100.0, 192);
        assert!(
            p1 > p0 + 0.04,
            "dumping into a flooded boiler must not count as relief ({p0}→{p1})"
        );
    }

    #[test]
    fn leftover_weather_lake_drops_load_on_edges_not_the_mouth() {
        let mut w = World::new(251);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Limestone);
        wet.pore = 180;
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 0..14 {
            for y in 0..10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 5..11 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
                crate::mineral::add_dissolved(&mut w, x, y, 80);
            }
        }
        for x in 1..5 {
            for y in 1..5 {
                let mut lake = Cell::air();
                lake.sat = Sat(255);
                w.set_cell(x, y, lake);
            }
            for y in 5..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        assert!(!vessel_is_boiler(&w, 3, 2));
        let min0 = mineral_total(&w);
        let mouth_mat0 = w.get_cell(4, 2).unwrap().material;
        let mut hot = temp_fill(&w, 20.0);
        for x in 5..11 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            ..SteamConfig::default()
        };
        for t in 1..=12 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
        }
        assert_eq!(
            mineral_total(&w),
            min0,
            "lake leftover drop is mineral-flat"
        );
        assert_eq!(
            w.get_cell(4, 2).unwrap().material,
            mouth_mat0,
            "weather-lake mouth must stay open water, not a sinter plug"
        );
        crate::mineral::settle_and_precip_standing_load(&mut w);
        assert_eq!(
            w.get_cell(4, 2).unwrap().material,
            MaterialId::Air,
            "standing-lake dump must skip the leftover vent mouth"
        );
        let edge_or_float = dissolved_at(&w, 3, 2)
            + dissolved_at(&w, 4, 3)
            + dissolved_at(&w, 2, 2)
            + dissolved_at(&w, 5, 2);
        assert!(
            edge_or_float > 0
                || w.get_cell(5, 2)
                    .is_some_and(|c| c.material != MaterialId::Air && c.pore < 180)
                || crate::mineral::leftover_lake_vent_skip(&w, 4, 2),
            "load should rim the shore or float into the lake, not sit as a vent sinter"
        );
    }

    #[test]
    fn leftover_dry_air_mouth_does_not_bank_unbounded_load() {
        // Playtest: leftover dest was open-sky Air (sat=0, holds 0) and
        // dissolved just ticked up. Hanging spray must drop on the apron,
        // not bank thousands of units in mid-air. Lake mouths stay open.
        let mut w = World::new(297);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::solid(MaterialId::Limestone);
        wet.pore = 80;
        let cap = water_capacity_cell(wet, &w.hydro).max(1);
        wet.sat = Sat(cap);
        for x in 2..14 {
            for y in 0..48 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 5..10 {
            for y in 1..5 {
                w.set_cell(x, y, wet);
                crate::mineral::add_dissolved(&mut w, x, y, 140);
            }
        }
        let mut gravel = Cell::solid(MaterialId::Gravel);
        gravel.pore = 200;
        for y in 5..39 {
            w.set_cell(6, y, gravel);
        }
        // Hanging leftover dest: Air beside the last gravel, Air below
        // that, stone ledge to the side so the apron has a seat.
        for y in 37..46 {
            w.set_cell(7, y, Cell::air());
        }
        w.set_cell(8, 37, Cell::solid(MaterialId::Stone));
        w.set_cell(8, 38, Cell::air());
        for y in 39..46 {
            w.set_cell(6, y, Cell::air());
        }
        let min0 = mineral_total(&w);
        let mut hot = temp_fill(&w, 20.0);
        for x in 5..10 {
            for y in 1..5 {
                let (hx, hy) = hot.tile_of(x, y);
                hot.set_tile_c(hx, hy, 122.0);
            }
        }
        let cfg = SteamConfig {
            enable_pore_boil: true,
            enable_escape: false,
            phase_expansion_drive: 192,
            boil_point_c: 100.0,
            reverse_seep_hops: 8,
            ..SteamConfig::default()
        };
        for t in 1..=16 {
            w.tick = t;
            apply_steam(&mut w, &mut hot, &cfg);
            crate::mineral::settle_and_precip_standing_load(&mut w);
        }
        assert_eq!(mineral_total(&w), min0, "leftover mouth dump is mineral-flat");
        let mouth_air = [(6, 39), (6, 40), (7, 38), (7, 39), (7, 40), (8, 38)];
        let bank = mouth_air
            .iter()
            .filter(|&&(x, y)| w.get_cell(x, y).is_some_and(|c| c.material == MaterialId::Air))
            .map(|&(x, y)| dissolved_at(&w, x, y))
            .max()
            .unwrap_or(0);
        assert!(
            bank < 200,
            "dry leftover mouth must not bank spray (max {bank})"
        );
        let mid = w.get_cell(6, 20).unwrap();
        assert!(
            mid.material == MaterialId::Gravel || mid.pore > crate::mineral::VENT_PIPE_LUMEN,
            "mid-chimney must stay an open conduit ({:?} pore {})",
            mid.material,
            mid.pore
        );
        let sintered = mouth_air.iter().any(|&(x, y)| {
            w.get_cell(x, y)
                .is_some_and(|c| c.material == MaterialId::Flowstone)
        });
        assert!(
            sintered || bank == 0,
            "spray should leave the hanging dest as sinter, not an air bank"
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
        assert!(
            p > 0.2,
            "dense cavity humidity must read pressure (got {p})"
        );
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
}
