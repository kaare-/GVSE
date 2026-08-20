//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Priority surface water flow (cascade, equalise, throughflow).

use std::collections::{HashMap, HashSet, VecDeque};

use wk_material::MaterialId;

use crate::active::ActiveChunk;
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::map_regions_parallel;

use super::head::{
    hydraulic_head, is_porous_solid_with, plan_same_y_pairwise_edge_in, same_y_cascade_pull_in,
    seepage_rate_with,
};
use super::plan::{regions_for_standalone, regions_wet_loaded};

/// Priority water flow.
///
/// For each wet `Air` cell (compute-then-apply so the pass is
/// order-independent), pick the best target in this order:
///
/// 1. **Diagonal-down Air with room** — dump as much sat as fits.
/// 2. **Immediate side is a cascade edge** — the side neighbour is Air
///    with an Air-with-room directly below it (the water we push there
///    will fall next tick). Dump all we can.
/// 3. **Same-Y surface equalise** — scan up to [`SAME_Y_SURFACE_SCAN`]
///    standing cells for a cascade outlet and push toward it; then
///    pairwise head-equalise each +x standing edge so wide lake tops
///    level instead of terracing / checkerboarding. **Organic wash-through**
///    punches standing water through Organic mats into Air beyond.
/// 4. **Throughflow** — if below is a stack of saturated porous cells,
///    weep at seepage rate to the nearest opening: a **side Air face**
///    (cliff / spring) or Air below the stack.
/// 5. **Confined upward head** — Air-with-room on a full wet column pulls
///    from the connected free-surface donor (bedrock pipes / communicating
///    vessels). See [`wake_confined_head`].
///
/// Vertical bulk fall stays in [`apply_gravity_fall`] (pull-based).
/// Porous soak stays in [`apply_seepage`]. Mass is preserved by greedy
/// per-source distribution when multiple targets contend for one cell.
pub fn apply_water_flow(world: &mut World) {
    let regions = regions_for_standalone(world);
    apply_water_flow_regions(world, &regions);
}

/// How often a quiet world re-scans confined head across loaded chunks.
/// Was 8 (~1–2 ms amortized on Super-Server); 16 keeps pipes honest
/// enough without eating the quiet-world physics budget.
const CONFINED_HEAD_WAKE_EVERY: u64 = 16;
/// Max sat moved per confined free-surface → rising-column transfer.
const CONFINED_HEAD_RATE: i32 = 32;
/// Cap BFS size when walking a pressure-connected wet-Air body.
const CONFINED_HEAD_BFS_LIMIT: usize = 8192;

/// Priority water flow restricted to a pre-planned active set.
///
/// Full priorities including throughflow + confined in **one** commit
/// (shared source budgets — unit tests / [`PerfConfig::full_feel`]).
/// The FPS tick path uses [`apply_water_flow_regions_ex`] without those,
/// then runs throughflow/confined once after the substep loop.
pub fn apply_water_flow_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    accumulate_water_flow_xfers(world, active, &mut xfers, true);
    accumulate_confined_upward_xfers(world, active, &mut xfers);
    commit_air_sat_xfers(world, &mut xfers);
}

/// Like [`apply_water_flow_regions`], optionally including Priority-4
/// throughflow in the same scan.
pub(crate) fn apply_water_flow_regions_ex(
    world: &mut World,
    active: &[ActiveChunk],
    include_throughflow: bool,
) {
    if active.is_empty() {
        return;
    }
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    accumulate_water_flow_xfers(world, active, &mut xfers, include_throughflow);
    commit_air_sat_xfers(world, &mut xfers);
}

/// Priority-4 throughflow only (saturated porous → spring / toe).
///
/// Called once per tick after the surface-flow substep loop so rainy
/// beaches don't pay the deep stack walk on every even substep.
pub(crate) fn apply_throughflow_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    accumulate_throughflow_xfers(world, active, &mut xfers);
    commit_air_sat_xfers(world, &mut xfers);
}

/// Confined upward equalisation for a planned active set (once/tick).
pub(crate) fn apply_confined_upward_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    accumulate_confined_upward_xfers(world, active, &mut xfers);
    commit_air_sat_xfers(world, &mut xfers);
}

/// Periodic full-chunk confined-head wake (communicating vessels).
///
/// Must not use dirty-halo planning — ocean evaporation keeps surface
/// cells dirty forever and would starve a quiet pipe shaft.
pub fn wake_confined_head(world: &mut World) {
    if world.tick % CONFINED_HEAD_WAKE_EVERY != 0 {
        return;
    }
    // Wet-air sticky chunks only — dry sky/stone cannot host a pipe.
    let regions = regions_wet_loaded(world);
    apply_confined_upward_regions(world, &regions);
}

fn commit_air_sat_xfers(
    world: &mut World,
    xfers: &mut [((i32, i32), (i32, i32), i32)],
) {
    if xfers.is_empty() {
        return;
    }
    let mut by_source: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, (from, _, _)) in xfers.iter().enumerate() {
        by_source.entry(*from).or_default().push(i);
    }
    for (from, mut ixs) in by_source {
        let Some(src) = world.get_cell(from.0, from.1) else {
            continue;
        };
        let mut budget = src.sat.0 as i32;
        ixs.sort();
        for i in ixs {
            let want = xfers[i].2;
            if budget <= 0 || want <= 0 {
                xfers[i].2 = 0;
                continue;
            }
            let give = want.min(budget);
            xfers[i].2 = give;
            budget -= give;
        }
    }
    xfers.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
    for (from, to, amt) in xfers.iter().copied() {
        if amt <= 0 {
            continue;
        }
        let Some(src) = world.get_cell(from.0, from.1) else {
            continue;
        };
        let Some(dst) = world.get_cell(to.0, to.1) else {
            continue;
        };
        if src.material != MaterialId::Air {
            continue;
        }
        let free = if dst.material == MaterialId::Air {
            u8::MAX as i32 - dst.sat.0 as i32
        } else {
            let cap = water_capacity_with(dst.material, &world.hydro) as i32;
            if cap == 0 {
                continue;
            }
            cap - dst.sat.0 as i32
        };
        let amt = amt.min(src.sat.0 as i32).min(free.max(0));
        if amt <= 0 {
            continue;
        }
        world.set_cell(
            from.0,
            from.1,
            Cell {
                sat: Sat(src.sat.0 - amt as u8),
                ..src
            },
        );
        world.set_cell(
            to.0,
            to.1,
            Cell {
                sat: Sat(dst.sat.0 + amt as u8),
                ..dst
            },
        );
    }
}

/// True when wet Air at `(gx,gy)` has room (or solid/sky) above — a
/// free-surface candidate for confined-head donation.
fn is_air_free_surface(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy + 1) {
        None => true,
        Some(a) if a.material != MaterialId::Air => true,
        Some(a) => !a.sat.is_full(),
    }
}

/// True when both horizontal neighbours are non-Air (or world edge).
fn is_walled_column(world: &World, gx: i32, gy: i32) -> bool {
    let wall = |x: i32| match world.get_cell(world.wrap_x(x), gy) {
        None => true,
        Some(c) => c.material != MaterialId::Air,
    };
    wall(gx - 1) && wall(gx + 1)
}

/// True when both horizontal neighbours are Air (open lake / ocean top).
/// 1-wide and 2-wide shafts have at least one solid side and return false.
fn open_air_both_sides(world: &World, gx: i32, gy: i32) -> bool {
    let air = |x: i32| {
        matches!(
            world.get_cell(world.wrap_x(x), gy),
            Some(c) if c.material == MaterialId::Air
        )
    };
    air(gx - 1) && air(gx + 1)
}

/// Whether a rising cell may pull from a connected free-surface donor.
///
/// - Donor at a **higher row**: always allow (1-wide or 2-wide shafts /
///   ocean head). Open lakes almost always donate on the same row, so
///   they stay with same-Y equalise.
/// - Same-row finish: require a fully walled 1-wide column.
fn allows_confined_rise(world: &World, gx: i32, gy: i32, donor_y: i32) -> bool {
    if donor_y > gy {
        return true;
    }
    is_walled_column(world, gx, gy)
}

#[derive(Clone, Copy)]
struct PressureBody {
    max_head: f32,
    donor: (i32, i32),
}

fn consider_pressure_donor(
    x: i32,
    y: i32,
    sat: Sat,
    cap: u8,
    best_head: &mut f32,
    best_donor: &mut (i32, i32),
) {
    let h = hydraulic_head(y, sat, cap);
    if h > *best_head {
        *best_head = h;
        *best_donor = (x, y);
    }
}

/// Walk up a contiguous full-wet Air column from `(x,y)`, mark visited,
/// enqueue lateral neighbours, and record the free-surface donor at the
/// top. Avoids flood-filling an entire deep ocean just to learn the
/// surface head of a column we already touched.
fn climb_full_air_column(
    world: &World,
    x: i32,
    y_start: i32,
    cap: u8,
    queue: &mut VecDeque<(i32, i32)>,
    visited: &mut HashSet<(i32, i32)>,
    best_head: &mut f32,
    best_donor: &mut (i32, i32),
) {
    let mut y = y_start;
    loop {
        let Some(c) = world.get_cell(x, y) else {
            break;
        };
        if c.material != MaterialId::Air || !c.sat.is_full() {
            break;
        }
        if !visited.insert((x, y)) {
            break;
        }
        // Lateral pressure links at this depth.
        for dx in [-1_i32, 1] {
            let nx = world.wrap_x(x + dx);
            if let Some(n) = world.get_cell(nx, y) {
                if n.material == MaterialId::Air && n.sat.is_full() {
                    if !visited.contains(&(nx, y)) {
                        queue.push_back((nx, y));
                    }
                } else if n.material == MaterialId::Air && n.sat.0 > 0 {
                    consider_pressure_donor(nx, y, n.sat, cap, best_head, best_donor);
                }
            }
        }
        if is_air_free_surface(world, x, y) {
            consider_pressure_donor(x, y, c.sat, cap, best_head, best_donor);
        }
        match world.get_cell(x, y + 1) {
            Some(above)
                if above.material == MaterialId::Air && above.sat.is_full() =>
            {
                y += 1;
            }
            Some(above) if above.material == MaterialId::Air && above.sat.0 > 0 => {
                consider_pressure_donor(x, y + 1, above.sat, cap, best_head, best_donor);
                break;
            }
            _ => break,
        }
    }
}

/// Pressure-connected body through **full** wet Air, starting at a full
/// seed. Free-surface head is the max `hydraulic_head` among full cells
/// that have room above and among adjacent partial wet-Air cells.
///
/// Deep oceans are handled by climbing each touched column to its free
/// surface instead of visiting every submerged cell; once a free surface
/// higher than the seed column is found we stop (enough to drive a rise).
fn pressure_body_from_full(
    world: &World,
    seed_x: i32,
    seed_y: i32,
    cache: &mut HashMap<(i32, i32), PressureBody>,
) -> Option<PressureBody> {
    if let Some(&body) = cache.get(&(seed_x, seed_y)) {
        return Some(body);
    }
    let seed = world.get_cell(seed_x, seed_y)?;
    if seed.material != MaterialId::Air || !seed.sat.is_full() {
        return None;
    }

    let cap = water_capacity_with(MaterialId::Air, &world.hydro);
    let head_eps = 1.0 / (cap as f32);
    // Require a free surface at least one full cell above the seed
    // before early-out. The shaft's own rising film sits just above the
    // seed and must not stop the search before we reach the reservoir.
    let early_exit_head = (seed_y + 2) as f32;
    let mut queue = VecDeque::new();
    let mut visited: HashSet<(i32, i32)> = HashSet::new();
    queue.push_back((seed_x, seed_y));

    let mut best_donor = (seed_x, seed_y);
    let mut best_head = hydraulic_head(seed_y, seed.sat, cap);

    while let Some((x, y)) = queue.pop_front() {
        if visited.len() > CONFINED_HEAD_BFS_LIMIT {
            break;
        }
        if visited.contains(&(x, y)) {
            continue;
        }
        let Some(c) = world.get_cell(x, y) else {
            continue;
        };
        if c.material != MaterialId::Air || !c.sat.is_full() {
            continue;
        }
        // Climb this column for its free-surface head, then continue
        // laterally via the climb's enqueued neighbours + downward link.
        climb_full_air_column(
            world,
            x,
            y,
            cap,
            &mut queue,
            &mut visited,
            &mut best_head,
            &mut best_donor,
        );
        // Downward continuity (pipe floors / deeper basins).
        if let Some(below) = world.get_cell(x, y - 1) {
            if below.material == MaterialId::Air && below.sat.is_full() {
                if !visited.contains(&(x, y - 1)) {
                    queue.push_back((x, y - 1));
                }
            } else if below.material == MaterialId::Air && below.sat.0 > 0 {
                consider_pressure_donor(x, y - 1, below.sat, cap, &mut best_head, &mut best_donor);
            }
        }
        // Reservoir / far free surface found — stop before flood-filling
        // the ocean. (Local shaft film is below early_exit_head.)
        if best_head > early_exit_head + head_eps {
            break;
        }
    }

    let body = PressureBody {
        max_head: best_head,
        donor: best_donor,
    };
    for key in &visited {
        cache.insert(*key, body);
    }
    // Seed may equal best when the climb marked it; always cache seed.
    cache.insert((seed_x, seed_y), body);
    Some(body)
}

/// Plan transfers from a connected free-surface donor into Air cells
/// that sit on a full wet column and still have room (rising pipe /
/// shaft surfaces). Mass leaves the high reservoir surface so the
/// confined column stays full — gravity cannot undo the rise.
fn accumulate_confined_upward_xfers(
    world: &World,
    active: &[ActiveChunk],
    xfers: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    let mut cache: HashMap<(i32, i32), PressureBody> = HashMap::new();
    let cap = water_capacity_with(MaterialId::Air, &world.hydro);
    let head_eps = 1.0 / (cap as f32);

    for ac in active {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = world.wrap_x(ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32);
                let Some(dst) = world.get_cell(gx, gy) else {
                    continue;
                };
                if dst.material != MaterialId::Air || dst.sat.is_full() {
                    continue;
                }
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                // Rising column: must sit on a full wet-Air cell that
                // can transmit confined pressure.
                if below.material != MaterialId::Air || !below.sat.is_full() {
                    continue;
                }
                // Open ocean/lake tops: both lateral neighbours are Air and
                // the cell is already a free surface. Confined same-Y is a
                // no-op (`allows_confined_rise`), but `pressure_body_from_full`
                // still BFS-climbs the body — dominant cost on rainy shores.
                // Keep shafts (any solid side) and buried/lid cells.
                if is_air_free_surface(world, gx, gy) && open_air_both_sides(world, gx, gy) {
                    continue;
                }
                let Some(body) =
                    pressure_body_from_full(world, gx, gy - 1, &mut cache)
                else {
                    continue;
                };
                let dst_head = hydraulic_head(gy, dst.sat, cap);
                if body.max_head <= dst_head + head_eps {
                    continue;
                }
                let (dx, dy) = body.donor;
                if dx == gx && dy == gy {
                    continue;
                }
                // Same-Y open lakes → same-Y equalise; higher donor row
                // (ocean / far reservoir) may rise even in a 2-wide shaft.
                if !allows_confined_rise(world, gx, gy, dy) {
                    continue;
                }
                let Some(donor) = world.get_cell(dx, dy) else {
                    continue;
                };
                if donor.material != MaterialId::Air || donor.sat.0 == 0 {
                    continue;
                }
                let free = cap as i32 - dst.sat.0 as i32;
                let dh_sat = ((body.max_head - dst_head) * cap as f32).floor() as i32;
                let amt = CONFINED_HEAD_RATE
                    .min(free)
                    .min(donor.sat.0 as i32)
                    .min(dh_sat.max(1));
                if amt > 0 {
                    xfers.push(((dx, dy), (gx, gy), amt));
                }
            }
        }
    }
}

fn accumulate_water_flow_xfers(
    world: &World,
    active: &[ActiveChunk],
    xfers: &mut Vec<((i32, i32), (i32, i32), i32)>,
    include_throughflow: bool,
) {
    let tick_flip = (world.tick & 1) == 0;
    let hydro = world.hydro;
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        // Cache the source chunk once. Inner probes read chunk-local when
        // they stay inside the region's chunk; only edge probes fall back
        // to `world.get_cell` (wrap + HashMap). Measured 10× cheaper.
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            return local;
        };
        let base_gx = ac.coord.cx * cw;
        let base_gy = ac.coord.cy * ch;
        // Read (gx, gy) via chunk when (lx, ly) is in-chunk, else world.
        let read = |lx: i32, ly: i32, gx: i32, gy: i32| -> Option<Cell> {
            if lx >= 0 && lx < cw && ly >= 0 && ly < ch {
                Some(chunk.get(lx as usize, ly as usize))
            } else {
                world.get_cell(gx, gy)
            }
        };
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = base_gy + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let lx = x as i32;
                let ly = y as i32;
                // Source cell — always in-chunk under the rect bounds.
                let cur = chunk.get(x as usize, y as usize);
                if cur.material != MaterialId::Air {
                    continue;
                }
                let gx = world.wrap_x(base_gx + lx);
                // Ocean-body fast path: a full-sat Air with a full-sat Air
                // directly above is a **buried** water cell, not a free
                // surface — unless it has an open face. A solid weir or
                // dry / partial Air (the next hillside column) must run:
                // treating only solids as faces left a huge bump against
                // dry Air with a hairline film on top.
                if cur.sat.is_full() {
                    let above = read(lx, ly + 1, gx, gy + 1);
                    if matches!(above, Some(a) if a.material == MaterialId::Air && a.sat.is_full())
                    {
                        let left = read(lx - 1, ly, world.wrap_x(gx - 1), gy);
                        let right = read(lx + 1, ly, world.wrap_x(gx + 1), gy);
                        let closed = |c: Option<Cell>| {
                            matches!(c, Some(n) if n.material == MaterialId::Air && n.sat.is_full())
                        };
                        if closed(left) && closed(right) {
                            continue;
                        }
                    }
                }
                // Below tells us whether we're on a "surface" (below is
                // solid or full water) or falling (below is Air with room).
                let below_cell = read(lx, ly - 1, gx, gy - 1);
                let on_surface = match below_cell {
                    None => false,
                    Some(b) => b.material != MaterialId::Air || b.sat.is_full(),
                };

                // Calm free surface: full sat on full water/solid below
                // with full-sat Air on both sides — no cascade, equalise,
                // or throughflow work. Open-lake tops after the buried skip.
                if cur.sat.is_full() && on_surface {
                    let left = read(lx - 1, ly, world.wrap_x(gx - 1), gy);
                    let right = read(lx + 1, ly, world.wrap_x(gx + 1), gy);
                    let calm = |c: Option<Cell>| {
                        matches!(c, Some(n) if n.material == MaterialId::Air && n.sat.is_full())
                    };
                    if calm(left) && calm(right) {
                        continue;
                    }
                }

                // Dry standing Air still owns the +x equalise edge so a
                // wet neighbour can pour into it (otherwise wet→dry
                // never ran when the dry cell was the left endpoint).
                // Keep this — skipping dry cells regressed shelf cascade
                // / hill-drain feel in the water suite.
                if cur.sat.is_empty() {
                    if on_surface {
                        plan_same_y_pairwise_edge_in(
                            world,
                            Some((chunk, base_gx, base_gy)),
                            gx,
                            gy,
                            lx,
                            ly,
                            &mut local,
                        );
                    }
                    continue;
                }

                let mut remaining = cur.sat.0 as i32;
                // Randomize L/R per cell so water doesn't bias one way.
                let flip = tick_flip ^ (((gx + gy) & 1) == 0);
                let dirs = if flip { [-1_i32, 1] } else { [1_i32, -1] };
                let depth = wet_stack_depth(&read, gx, gy, lx, ly, cur.sat.0);
                // Trickles crawl; a full film / stacked dump keeps a fat front.
                let step = sheet_step_cap(depth);
                let drain = drain_step_cap(depth);

                // --- Priority 1: diagonal-down into Air with room ---
                // Shelf edge: (dx, y-1) is Air, so water can fall there.
                for dx in dirs {
                    if remaining == 0 {
                        break;
                    }
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy - 1;
                    let Some(dst) = read(lx + dx, ly - 1, nx, ny) else {
                        continue;
                    };
                    if dst.material != MaterialId::Air {
                        continue;
                    }
                    let free = u8::MAX.saturating_sub(dst.sat.0) as i32;
                    if free == 0 {
                        continue;
                    }
                    let move_amt = remaining.min(free).min(drain);
                    local.push(((gx, gy), (nx, ny), move_amt));
                    remaining -= move_amt;
                }

                if remaining > 0 && on_surface {
                    // --- Priority 2: immediate side is a cascade edge ---
                    // side is Air AND (side, y-1) is Air with room → water
                    // dumped there falls next tick. Move all we can.
                    for dx in dirs {
                        if remaining == 0 {
                            break;
                        }
                        let nx = world.wrap_x(gx + dx);
                        let Some(side) = read(lx + dx, ly, nx, gy) else {
                            continue;
                        };
                        if side.material != MaterialId::Air {
                            continue;
                        }
                        let side_below = read(lx + dx, ly - 1, nx, gy - 1);
                        let cascade_edge = matches!(
                            side_below,
                            Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
                        );
                        if !cascade_edge {
                            continue;
                        }
                        let free = u8::MAX.saturating_sub(side.sat.0) as i32;
                        if free == 0 {
                            continue;
                        }
                        let move_amt = remaining.min(free).min(step);
                        local.push(((gx, gy), (nx, gy), move_amt));
                        remaining -= move_amt;
                    }

                    // --- Priority 3a: same-Y cascade pull ---
                    // If a cascade outlet sits further along the surface
                    // band, push toward it so lake terraces fall into the
                    // lower reach. Chunk-local look-ahead (same cache as
                    // pairwise equalise) — up to SAME_Y_SURFACE_SCAN
                    // world.get_cell calls was hot on shore bands.
                    for dx in dirs {
                        if remaining == 0 {
                            break;
                        }
                        let Some(want) = same_y_cascade_pull_in(
                            world,
                            Some((chunk, base_gx, base_gy)),
                            gx,
                            gy,
                            lx,
                            ly,
                            dx,
                            cur.sat.0,
                        ) else {
                            continue;
                        };
                        let tx = world.wrap_x(gx + dx);
                        let Some(side) = read(lx + dx, ly, tx, gy) else {
                            continue;
                        };
                        if side.material != MaterialId::Air {
                            continue;
                        }
                        let free = u8::MAX.saturating_sub(side.sat.0) as i32;
                        let move_amt = remaining.min(free).min(want);
                        if move_amt > 0 {
                            local.push(((gx, gy), (tx, gy), move_amt));
                            remaining -= move_amt;
                        }
                    }

                    // --- Priority 3b: wash through Organic dams ---
                    // Organic is a sponge — standing water punches through
                    // to Air beyond instead of pooling forever behind mats.
                    if remaining > 0 {
                        remaining = plan_organic_wash_through(
                            world,
                            &read,
                            gx,
                            gy,
                            lx,
                            ly,
                            dirs,
                            remaining,
                            &mut local,
                        );
                    }

                    // --- Priority 3c: porous-face dividend ---
                    // Facing a dry/porous column: absorb a material-rate
                    // share into the contact cell; any leftover may spill
                    // over the crest only when this column already has
                    // water at/above that crest height (hydraulic head).
                    if remaining > 0 {
                        remaining = plan_porous_face_dividend(
                            world,
                            &read,
                            gx,
                            gy,
                            lx,
                            ly,
                            dirs,
                            remaining,
                            step,
                            &mut local,
                        );
                    }
                }

                // --- Priority 3b: pairwise +x standing equalise ---
                // Always, even if cascade dumped everything — the edge
                // may still need the reverse transfer from a wetter +x.
                if on_surface {
                    plan_same_y_pairwise_edge_in(
                        world,
                        Some((chunk, base_gx, base_gy)),
                        gx,
                        gy,
                        lx,
                        ly,
                        &mut local,
                    );
                }

                if remaining == 0 || !on_surface || !include_throughflow {
                    continue;
                }

                plan_throughflow_from_cell(
                    world,
                    &read,
                    &hydro,
                    gx,
                    gy,
                    lx,
                    ly,
                    remaining,
                    &mut local,
                );
            }
        }
        local
    });
    for mut v in local {
        xfers.append(&mut v);
    }
}

/// Throughflow-only scan (Priority 4) — once per tick from [`tick`].
fn accumulate_throughflow_xfers(
    world: &World,
    active: &[ActiveChunk],
    xfers: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    let hydro = world.hydro;
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            return local;
        };
        let base_gx = ac.coord.cx * cw;
        let base_gy = ac.coord.cy * ch;
        let read = |lx: i32, ly: i32, gx: i32, gy: i32| -> Option<Cell> {
            if lx >= 0 && lx < cw && ly >= 0 && ly < ch {
                Some(chunk.get(lx as usize, ly as usize))
            } else {
                world.get_cell(gx, gy)
            }
        };
        for y in ac.rect.y0..=ac.rect.y1 {
            let ly = y as i32;
            let gy = base_gy + ly;
            for x in ac.rect.x0..=ac.rect.x1 {
                let lx = x as i32;
                let gx = world.wrap_x(base_gx + lx);
                let cur = chunk.get(x as usize, y as usize);
                if cur.material != MaterialId::Air || cur.sat.is_empty() {
                    continue;
                }
                // Same calm / buried skips as the main flow scan.
                if cur.sat.is_full() {
                    let above = read(lx, ly + 1, gx, gy + 1);
                    if matches!(above, Some(a) if a.material == MaterialId::Air && a.sat.is_full())
                    {
                        continue;
                    }
                }
                let below_cell = read(lx, ly - 1, gx, gy - 1);
                let on_surface = match below_cell {
                    None => false,
                    Some(b) => b.material != MaterialId::Air || b.sat.is_full(),
                };
                if !on_surface {
                    continue;
                }
                if cur.sat.is_full() {
                    let left = read(lx - 1, ly, world.wrap_x(gx - 1), gy);
                    let right = read(lx + 1, ly, world.wrap_x(gx + 1), gy);
                    let calm = |c: Option<Cell>| {
                        matches!(c, Some(n) if n.material == MaterialId::Air && n.sat.is_full())
                    };
                    if calm(left) && calm(right) {
                        continue;
                    }
                }
                plan_throughflow_from_cell(
                    world,
                    &read,
                    &hydro,
                    gx,
                    gy,
                    lx,
                    ly,
                    cur.sat.0 as i32,
                    &mut local,
                );
            }
        }
        local
    });
    for mut v in local {
        xfers.append(&mut v);
    }
}

/// How many stacked wet-Air cells sit at/above this source (1 = a film).
fn wet_stack_depth(
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    sat: u8,
) -> i32 {
    if sat < 96 {
        return 0;
    }
    let mut d = 1;
    for dy in 1..=5 {
        match read(lx, ly + dy, gx, gy + dy) {
            Some(c) if c.material == MaterialId::Air && c.sat.0 >= 160 => d += 1,
            _ => break,
        }
    }
    d
}

/// Per-pass sat cap for same-Y / overtop / skate-over-dry.
///
/// Depth 0 is a trickle (`sat < 96`) and crawls. A full film or a
/// stacked dump must keep a fat front — a 40-sat cap turned a
/// hilltop dump into a hairline over dry ground.
fn sheet_step_cap(depth: i32) -> i32 {
    match depth {
        0 => 40,
        1 => 180,
        2 => 220,
        _ => 255,
    }
}

/// Per-pass sat cap for diagonal-down drain.
fn drain_step_cap(depth: i32) -> i32 {
    match depth {
        0 => 64,
        1 => 200,
        2 => 240,
        _ => 255,
    }
}

/// Porous-face dividend: soak by material, spill leftover only with head.
///
/// Facing a dry/porous solid (not Air/Organic):
/// 1. Absorb up to the material seepage rate into the contact cell.
/// 2. Leftover may spill into Air at the column crest — only when *this*
///    source cell sits at or above that crest (`gy >= crest_y`). Mass and
///    `step` (speed proxy) cap how much passes; a pond film below a berm
///    never invents head to climb the hillside.
fn plan_porous_face_dividend(
    world: &World,
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    dirs: [i32; 2],
    mut remaining: i32,
    step: i32,
    local: &mut Vec<((i32, i32), (i32, i32), i32)>,
) -> i32 {
    for dx in dirs {
        if remaining <= 0 {
            break;
        }
        let nx = world.wrap_x(gx + dx);
        let Some(side) = read(lx + dx, ly, nx, gy) else {
            continue;
        };
        if side.material == MaterialId::Air || side.material == MaterialId::Organic {
            continue;
        }
        let cap = water_capacity_with(side.material, &world.hydro) as i32;
        if cap <= 0 {
            continue;
        }

        // 1) Material dividend — absorb this much into the column per tick.
        let soak_rate = seepage_rate_with(side.material, &world.hydro);
        let free = (cap - side.sat.0 as i32).max(0);
        let soak = remaining.min(free).min(soak_rate);
        if soak > 0 {
            local.push(((gx, gy), (nx, gy), soak));
            remaining -= soak;
        }
        if remaining <= 0 {
            break;
        }

        // 2) Crest Air above the solid face in that column.
        let mut crest: Option<(i32, i32, i32, i32)> = None; // nx, ny, nlx, nly
        for dy in 1..=8 {
            let Some(above) = read(lx + dx, ly + dy, nx, gy + dy) else {
                break;
            };
            if above.material != MaterialId::Air {
                continue;
            }
            crest = Some((nx, gy + dy, lx + dx, ly + dy));
            break;
        }
        let Some((cx, cy, clx, cly)) = crest else {
            continue;
        };

        // Head: only water that is already at/above the crest overflows.
        // (Do not use free-surface scan — that lets ponds stair-climb.)
        if gy < cy {
            continue;
        }

        let Some(crest_cell) = read(clx, cly, cx, cy) else {
            continue;
        };
        let free_air = u8::MAX.saturating_sub(crest_cell.sat.0) as i32;
        // Overflow share: leftover mass, capped by step (speed).
        let overflow = remaining.min(free_air).min(step.max(soak_rate));
        if overflow > 0 {
            local.push(((gx, gy), (cx, cy), overflow));
            remaining -= overflow;
        }
    }
    remaining
}

/// Max Organic cells water may punch through in one wash.
const ORGANIC_WASH_SPAN: i32 = 8;
/// Cap sat moved through Organic per source cell per pass (still aggressive).
const ORGANIC_WASH_RATE: i32 = 96;

/// Standing water washes through a span of Organic into Air beyond.
///
/// Organic is a sponge / mat, not a masonry dam — without this, shore
/// mounds seal basins into sticky perched pools.
fn plan_organic_wash_through(
    world: &World,
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    dirs: [i32; 2],
    mut remaining: i32,
    local: &mut Vec<((i32, i32), (i32, i32), i32)>,
) -> i32 {
    for dx in dirs {
        if remaining <= 0 {
            break;
        }
        let mut x = world.wrap_x(gx + dx);
        let mut clx = lx + dx;
        let Some(first) = read(clx, ly, x, gy) else {
            continue;
        };
        if first.material != MaterialId::Organic {
            continue;
        }
        let mut span = 0i32;
        let mut exit: Option<(i32, i32, i32, i32)> = None; // nx, ny, free, prefer
        loop {
            span += 1;
            if span > ORGANIC_WASH_SPAN {
                break;
            }
            let nx = world.wrap_x(x + dx);
            let nlx = clx + dx;
            let Some(c) = read(nlx, ly, nx, gy) else {
                break;
            };
            if c.material == MaterialId::Organic {
                x = nx;
                clx = nlx;
                continue;
            }
            if c.material != MaterialId::Air {
                break;
            }
            let free = u8::MAX.saturating_sub(c.sat.0) as i32;
            let below = read(nlx, ly - 1, nx, gy - 1);
            let cascade = matches!(
                below,
                Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
            );
            if free > 0 {
                let prefer = if cascade { 2 } else { 1 };
                exit = Some((nx, gy, free, prefer));
            }
            // Also allow dumping into Air directly below the far Organic
            // face (wash down the lee side).
            if let Some(b) = below {
                if b.material == MaterialId::Air {
                    let bfree = u8::MAX.saturating_sub(b.sat.0) as i32;
                    if bfree > 0 {
                        let bp = if !b.sat.is_full() { 3 } else { 1 };
                        if exit.map(|e| bp > e.3).unwrap_or(true) {
                            exit = Some((nx, gy - 1, bfree, bp));
                        }
                    }
                }
            }
            break;
        }
        if let Some((tx, ty, free, prefer)) = exit {
            let rate = if prefer >= 2 {
                remaining
            } else {
                ORGANIC_WASH_RATE
            };
            let amt = remaining.min(free).min(rate);
            if amt > 0 {
                local.push(((gx, gy), (tx, ty), amt));
                remaining -= amt;
            }
        }
    }
    remaining
}

fn plan_throughflow_from_cell(
    world: &World,
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    hydro: &wk_material::HydroOverrides,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    remaining: i32,
    local: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    // --- Priority 4: throughflow through saturated porous ---
    // Real physics: water pressed on saturated soil flows through it at
    // seepage rate (Darcy). Exit at the first opening: a side Air face
    // (cliff / spring) or Air below the stack — not only the bottom.
    let mut remaining = remaining;
    let mut placed = false;
    for dx in [0_i32, -1, 1] {
        if placed || remaining <= 0 {
            break;
        }
        let nx = world.wrap_x(gx + dx);
        let Some(below1) = read(lx + dx, ly - 1, nx, gy - 1) else {
            continue;
        };
        if !is_porous_solid_with(below1.material, hydro) {
            continue;
        }
        let cap1 = water_capacity_with(below1.material, hydro);
        if below1.sat.0 < cap1 {
            continue; // gravity + seepage handle unsaturated
        }
        let mut rate = seepage_rate_with(below1.material, hydro);
        // Prefer the shallowest exit so mid-cliff springs beat a deep toe.
        let mut best: Option<(i32, i32, i32)> = None; // depth, tx, ty
        let mut depth = 1i32;
        let mut ty = gy - 1;
        let mut lty = ly - 1;
        for _ in 0..24 {
            let Some(nb) = read(lx + dx, lty, nx, ty) else {
                break;
            };
            if nb.material == MaterialId::Air {
                if u8::MAX.saturating_sub(nb.sat.0) > 0 {
                    let cand = (depth, nx, ty);
                    if best.map(|b| cand < b).unwrap_or(true) {
                        best = Some(cand);
                    }
                }
                break;
            }
            if !is_porous_solid_with(nb.material, hydro) {
                break;
            }
            let cap = water_capacity_with(nb.material, hydro);
            if nb.sat.0 < cap {
                break;
            }
            rate = rate.min(seepage_rate_with(nb.material, hydro));
            for sdx in [-1_i32, 1] {
                let sx = world.wrap_x(nx + sdx);
                if sx == nx {
                    continue;
                }
                let Some(side) = read(lx + dx + sdx, lty, sx, ty) else {
                    continue;
                };
                if side.material != MaterialId::Air {
                    continue;
                }
                if u8::MAX.saturating_sub(side.sat.0) == 0 {
                    continue;
                }
                let cand = (depth, sx, ty);
                if best.map(|b| cand < b).unwrap_or(true) {
                    best = Some(cand);
                }
            }
            depth += 1;
            ty -= 1;
            lty -= 1;
        }
        if let Some((_d, tx, ty)) = best {
            let amt = rate.min(remaining).max(1);
            local.push(((gx, gy), (tx, ty), amt));
            remaining -= amt;
            placed = true;
        }
    }
}
