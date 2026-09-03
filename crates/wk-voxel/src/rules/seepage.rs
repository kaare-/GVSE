//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Permeability-limited pore soak.

use wk_material::{HydroOverrides, MaterialId};

use crate::active::ActiveChunk;
use crate::cell::{permeability_cell, water_capacity_cell, Cell, Sat};
use crate::chunk::{ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W, STANDING_AIR_SAT};
use crate::fasthash::FxHashSet;
use crate::grid::World;
use crate::parallel::map_regions_parallel;

use super::head::{
    is_porous_cell, sat_move_to_equalize_heads, seepage_conduct_rate_cells, seepage_rate_cell,
    seepage_fire_odds_cell, seepage_uptake_rate_cell,
};
use super::plan::{regions_for_standalone, regions_lake_bed_loaded};

/// Odds a *full* throughput through limestone opens the aperture one step.
///
/// The response is **squared** in throughput above
/// [`crate::mineral::APERTURE_MIN_THROUGHPUT`], so this number is not a uniform
/// erosion rate — it sets how sharply flow focuses. At this value a cell
/// carrying an ordinary seepage step opens slowly (rock feels hard), while one
/// carrying several times that opens more than an order of magnitude faster and
/// becomes a pipe. Raising it erodes everything and loses the channels; lowering
/// it freezes the rock.
const APERTURE_GROWTH_SCALE: f32 = 12.0;
/// Salt for the deterministic aperture-growth roll.
const APERTURE_SEED_SALT: u64 = 0xA9E5_7075_0BE0_1111;

/// Rows of seam-coupled seepage on the **lower** chunk (below the face).
/// A shallow strip left saturated shelves (y=62|63 full, y=61 dry).
const SEAM_SEEPAGE_DEPTH_LO: i32 = 16;
/// Upper-chunk strip stays shallow — a deep strip pulled pond water
/// sideways at y=70 and stalled infiltration in unit tests.
const SEAM_SEEPAGE_DEPTH_HI: i32 = 4;

/// Walk down a porous column from `start_y` through saturated cells and
/// re-dirty the first unsaturated pore (the wetting front). Stops at
/// impermeable / void. Mirrors the standing-water lake-bed walk.
fn touch_downward_pore_front(
    world: &World,
    hydro: &wk_material::HydroOverrides,
    gx: i32,
    start_y: i32,
    touches: &mut Vec<(i32, i32)>,
) {
    let mut yy = start_y;
    for _ in 0..(CHUNK_CELLS_H * 2) {
        if yy < 0 {
            break;
        }
        let Some(cell) = world.get_cell(gx, yy) else {
            break;
        };
        if !is_porous_cell(cell, hydro) {
            break;
        }
        let cap = water_capacity_cell(cell, hydro);
        if cap == 0 {
            break;
        }
        if cell.sat.0 < cap {
            touches.push((gx, yy));
            break;
        }
        yy -= 1;
    }
}

/// Quiet free-surface lakes stop dirty-tracking once water looks settled,
/// leaving beds (and deep pore stacks under a wet sand cap) bone-dry.
/// Re-dirty unsaturated porous cells that still have a standing-water or
/// wetter-pore neighbour so seepage / gravity keep infiltrating.
///
/// Runs every tick: cost is only the wet-chunk scan, and once beds are
/// saturated the touch set is empty (steady lakes stay quiet). Standing
/// water without an unsaturated front uses the standing-air y band so
/// dry sky in shore / mid-ocean surface chunks is leftover.
///
/// Also walks **down from standing wet Air** so beds that live in the
/// chunk below (y=63 under water at y=64) are woken — `has_wet_air` alone
/// never visits that dry cy-1 chunk.
pub fn wake_lake_bed_pores(world: &mut World) {
    let hydro = world.hydro;
    let regions = regions_lake_bed_loaded(world);
    let mut touches: Vec<(i32, i32)> = Vec::new();
    let mut standing_updates: Vec<(ChunkCoord, bool)> = Vec::new();
    let mut unsat_updates: Vec<(ChunkCoord, bool)> = Vec::new();
    for ac in &regions {
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            continue;
        };
        let base_gx = ac.coord.cx * CHUNK_CELLS_W as i32;
        let base_gy = ac.coord.cy * CHUNK_CELLS_H as i32;
        let mut any_standing = false;
        let mut any_unsat = false;
        // Quiet standing-only chunks: dry sky / buried rock is leftover.
        // Unsaturated pores can sit anywhere, so those chunks keep the
        // full rect. The downward walk from standing still reaches the
        // bed in the chunk below.
        let (scan_y0, scan_y1, standing_only) =
            if !chunk.has_unsaturated_pores {
                match chunk.standing_band_y(ac.rect) {
                    Some((lo, hi)) => (lo, hi, true),
                    None => (ac.rect.y0, ac.rect.y1, false),
                }
            } else {
                (ac.rect.y0, ac.rect.y1, false)
            };
        for y in scan_y0..=scan_y1 {
            let ly = y as usize;
            let gy = base_gy + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let lx = x as usize;
                let cell = chunk.get(lx, ly);
                let gx = world.wrap_x(base_gx + x as i32);

                // Standing water → touch unsaturated porous below / beside
                // (crosses the horizontal chunk seam into cy-1).
                // Walk only through *saturated* pores; stop at the first
                // unsaturated cell (the wetting front). Walking through dry
                // cells marked the whole crust dirty and looked like
                // groundwater "teleporting" under a dry gap.
                if cell.material == MaterialId::Air && cell.sat.0 >= STANDING_AIR_SAT {
                    any_standing = true;
                    // Mid-ocean / lake interior: below is more water, not a
                    // bed. One chunk-local peek replaces a get_cell walk.
                    let below = if ly > 0 {
                        Some(chunk.get(lx, ly - 1))
                    } else {
                        world.get_cell(gx, gy - 1)
                    };
                    if let Some(below) = below {
                        if is_porous_cell(below, &hydro) {
                            let cap = water_capacity_cell(below, &hydro);
                            if cap > 0 && below.sat.0 < cap {
                                touches.push((gx, gy - 1));
                            } else if cap > 0 {
                                let mut yy = gy - 2;
                                for _ in 0..(CHUNK_CELLS_H * 2) {
                                    let Some(b) = world.get_cell(gx, yy) else {
                                        break;
                                    };
                                    if !is_porous_cell(b, &hydro) {
                                        break;
                                    }
                                    let cap = water_capacity_cell(b, &hydro);
                                    if cap == 0 {
                                        break;
                                    }
                                    if b.sat.0 < cap {
                                        touches.push((gx, yy));
                                        break;
                                    }
                                    yy -= 1;
                                }
                            }
                        }
                    }
                    for dx in [-1_i32, 1] {
                        let nlx = lx as i32 + dx;
                        let nx = world.wrap_x(gx + dx);
                        let n = if nlx >= 0 && nlx < CHUNK_CELLS_W as i32 {
                            Some(chunk.get(nlx as usize, ly))
                        } else {
                            world.get_cell(nx, gy)
                        };
                        if let Some(n) = n {
                            if is_porous_cell(n, &hydro) {
                                let cap = water_capacity_cell(n, &hydro);
                                if cap > 0 && n.sat.0 < cap {
                                    touches.push((nx, gy));
                                }
                            }
                        }
                    }
                }

                if !is_porous_cell(cell, &hydro) {
                    continue;
                }
                let cap = water_capacity_cell(cell, &hydro);
                if cap == 0 {
                    continue;
                }
                if cell.sat.0 < cap {
                    any_unsat = true;
                }
                if cell.sat.0 >= cap {
                    continue;
                }
                let mut feed = false;
                if let Some(above) = world.get_cell(gx, gy + 1) {
                    if above.material == MaterialId::Air && above.sat.0 >= 160 {
                        feed = true;
                    } else if is_porous_cell(above, &hydro)
                        && above.sat.0 > cell.sat.0
                    {
                        feed = true;
                    }
                }
                if !feed {
                    for dx in [-1_i32, 1] {
                        let nx = world.wrap_x(gx + dx);
                        if matches!(
                            world.get_cell(nx, gy),
                            Some(n) if n.material == MaterialId::Air && n.sat.0 >= 160
                        ) {
                            feed = true;
                            break;
                        }
                    }
                }
                if feed {
                    touches.push((gx, gy));
                    // Keep the wetting front below this cell awake — seam
                    // rows that only equalise horizontally never nudged the
                    // chunk under (playtest 160k tick shelf).
                    if gy > 0 {
                        touch_downward_pore_front(world, &hydro, gx, gy - 1, &mut touches);
                    }
                }
            }
        }
        standing_updates.push((ac.coord, any_standing));
        if !standing_only {
            unsat_updates.push((ac.coord, any_unsat));
        }
    }
    for (coord, any_standing) in standing_updates {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if any_standing {
                chunk.has_standing_air = true;
            } else {
                // Partial-rect scan: do not shrink the y band. Only
                // clear when this walk saw no standing water at all.
                chunk.clear_standing_air();
            }
        }
    }
    for (coord, any_unsat) in unsat_updates {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            chunk.has_unsaturated_pores = any_unsat;
        }
    }
    for (gx, gy) in touches {
        world.touch_dirty(gx, gy);
    }
}

/// Re-dirty pore faces across vertical chunk seams (y=63|64, 127|128, …).
///
/// Underground sat can equilibrate inside each chunk then go quiet while
/// a sharp step remains on the cy boundary — the sat heatmap shows that
/// as a horizontal shelf. Wet-air wake never visits dry cy neighbours
/// that only hold pore water, so we couple a **band** of seam rows
/// explicitly (not only the two face cells).
pub fn wake_vertical_chunk_seam_pores(world: &mut World) {
    let hydro = world.hydro;
    let ch = CHUNK_CELLS_H as i32;
    let cw = CHUNK_CELLS_W as i32;
    let mut touches: Vec<(i32, i32)> = Vec::new();
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        let above = ChunkCoord::new(coord.cx, coord.cy + 1);
        if !world.chunks.contains_key(&above) {
            continue;
        }
        // Face at top of this chunk / bottom of cy+1, plus one row of halo
        // so lateral shore fronts crossing the seam stay coupled.
        let y_lo = coord.cy * ch + (ch - 1);
        let y_hi = y_lo + 1;
        let band = [y_lo - 1, y_lo, y_hi, y_hi + 1];
        let base_gx = coord.cx * cw;
        let Some(lo_chunk) = world.chunks.get(&coord) else {
            continue;
        };
        let Some(hi_chunk) = world.chunks.get(&above) else {
            continue;
        };
        for lx in 0..cw {
            let gx = world.wrap_x(base_gx + lx);
            let lo = lo_chunk.get(lx as usize, (ch - 1) as usize);
            let hi = hi_chunk.get(lx as usize, 0);
            let lo_pore = is_porous_cell(lo, &hydro);
            let hi_pore = is_porous_cell(hi, &hydro);
            let lo_air = lo.material == MaterialId::Air && lo.sat.0 >= 160;
            let hi_air = hi.material == MaterialId::Air && hi.sat.0 >= 160;
            if !((lo_pore || lo_air) && (hi_pore || hi_air)) {
                continue;
            }
            let lo_cap = water_capacity_cell(lo, &hydro);
            let hi_cap = water_capacity_cell(hi, &hydro);
            let lo_room = lo_pore && lo_cap > 0 && lo.sat.0 < lo_cap;
            let hi_room = hi_pore && hi_cap > 0 && hi.sat.0 < hi_cap;
            let lo_wet = lo.sat.0 > 0;
            let hi_wet = hi.sat.0 > 0;
            let lo_full = lo_pore && lo_cap > 0 && lo.sat.0 >= lo_cap;
            let hi_full = hi_pore && hi_cap > 0 && hi.sat.0 >= hi_cap;
            if !(lo_wet || hi_wet || lo_air || hi_air) {
                continue;
            }
            if !(lo_room || hi_room || lo_air || hi_air || lo_full || hi_full) {
                continue;
            }
            for &yy in &band {
                if yy < 0 {
                    continue;
                }
                let Some(c) = world.get_cell(gx, yy) else {
                    continue;
                };
                if is_porous_cell(c, &hydro) {
                    let cap = water_capacity_cell(c, &hydro);
                    if cap > 0 && (c.sat.0 < cap || c.sat.0 > 0) {
                        touches.push((gx, yy));
                    }
                } else if c.material == MaterialId::Air && c.sat.0 > 0 {
                    touches.push((gx, yy));
                }
            }
            // Moisture above the seam must keep driving the column below —
            // horizontal equalisation along y=63|64 stalls without this.
            if lo_pore && (hi_wet || hi_air || lo_full) {
                touch_downward_pore_front(world, &hydro, gx, y_lo, &mut touches);
            } else if lo_wet && lo_pore {
                touch_downward_pore_front(world, &hydro, gx, y_lo - 1, &mut touches);
            }
        }
    }
    for (gx, gy) in touches {
        world.touch_dirty(gx, gy);
    }
}

/// Minimal active regions covering vertical chunk seams.
///
/// Used every tick so quiet-EO seepage cadence does not leave pore water
/// shelved at y=63|64 for thousands of ticks while the row above keeps
/// equalising horizontally.
pub fn seam_seepage_regions(world: &World) -> Vec<ActiveChunk> {
    use std::collections::HashMap;
    let ch = CHUNK_CELLS_H as i32;
    let cw = CHUNK_CELLS_W as i32;
    let depth_lo = SEAM_SEEPAGE_DEPTH_LO.min(ch);
    let depth_hi = SEAM_SEEPAGE_DEPTH_HI.min(ch);
    let mut map: HashMap<ChunkCoord, Rect> = HashMap::new();
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        let above = ChunkCoord::new(coord.cx, coord.cy + 1);
        if !world.chunks.contains_key(&above) {
            continue;
        }
        // Only the columns that are actually coupled across the face need the
        // band. Emitting it for every chunk pair at full width made this the
        // most expensive pass in the simulation (17 ms/call on the stress
        // world) while most seams are dry sky or dry rock with nothing to move.
        let Some(span) = seam_coupled_span(world, coord, above) else {
            continue;
        };
        let strip_lo = Rect {
            x0: span.0,
            y0: (ch - depth_lo).max(0) as u8,
            x1: span.1,
            y1: (ch - 1) as u8,
        };
        let strip_hi = Rect {
            x0: span.0,
            y0: 0,
            x1: span.1,
            y1: (depth_hi - 1).min(ch - 1) as u8,
        };
        map.entry(coord)
            .and_modify(|r| *r = merge_seam_rect(*r, strip_lo))
            .or_insert(strip_lo);
        map.entry(above)
            .and_modify(|r| *r = merge_seam_rect(*r, strip_hi))
            .or_insert(strip_hi);
    }
    let mut out: Vec<ActiveChunk> = map
        .into_iter()
        .map(|(coord, rect)| ActiveChunk { coord, rect })
        .collect();
    out.sort_by(|a, b| a.coord.cy.cmp(&b.coord.cy).then(a.coord.cx.cmp(&b.coord.cx)));
    out
}

/// Order transfers so a donor feeds its **best-conducting** face first.
///
/// This is the competitive half of vein formation, and it was missing. Aperture
/// growth amplifies whatever carries flow, but nothing starved the alternatives,
/// so preference plus enough time still equalised: every path stayed viable and
/// the front advanced as a broad uniform wedge no matter how strongly the
/// permeable route was favoured. A real conduit captures its neighbours' water
/// and they dry out — that is what makes the structure stable instead of
/// transient.
///
/// The apply loop already clamps each transfer by the donor's *live* saturation,
/// so a donor with limited water cannot satisfy every face. Which face loses was
/// decided by neighbour coordinate, with ties serving the **smallest** transfer
/// first — so the weakest face was fed and the conduit went hungry.
///
/// Serving the largest transfer first makes the conduit win, because `amt` is
/// already conductance-limited: the best-conducting face is the largest one.
/// Nothing about mass changes — the same water moves, it just goes down the
/// path that can carry it.
///
/// Deterministic: the amount ordering is total, and destination breaks ties.
fn serve_best_faces_first(xfers: &mut [((i32, i32), (i32, i32), i32)]) {
    xfers.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(b.2.cmp(&a.2))
            .then(a.1.cmp(&b.1))
    });
}

/// Local `x` span of the columns where water could actually cross this seam,
/// or `None` when the face is inert.
///
/// Deliberately more permissive than the per-column predicate in
/// [`wake_vertical_chunk_seam_pores`]: it asks only that both sides can hold
/// or pass water and that one of them has some, so it can never exclude a
/// column that the wake would have coupled.
fn seam_coupled_span(world: &World, lower: ChunkCoord, upper: ChunkCoord) -> Option<(u8, u8)> {
    let ch = CHUNK_CELLS_H as i32;
    let cw = CHUNK_CELLS_W as usize;
    let lo_chunk = world.chunks.get(&lower)?;
    let hi_chunk = world.chunks.get(&upper)?;
    // Sticky occupancy: a seam with no water on either side has nothing to do.
    let any_water = lo_chunk.has_wet_pores
        || lo_chunk.has_wet_air
        || hi_chunk.has_wet_pores
        || hi_chunk.has_wet_air;
    if !any_water {
        return None;
    }
    let hydro = world.hydro;
    let mut lo_x: Option<usize> = None;
    let mut hi_x = 0usize;
    for lx in 0..cw {
        let lo = lo_chunk.get(lx, (ch - 1) as usize);
        let hi = hi_chunk.get(lx, 0);
        let lo_open = is_porous_cell(lo, &hydro) || lo.material == MaterialId::Air;
        let hi_open = is_porous_cell(hi, &hydro) || hi.material == MaterialId::Air;
        if !(lo_open && hi_open) {
            continue;
        }
        if lo.sat.0 == 0 && hi.sat.0 == 0 {
            continue;
        }
        lo_x = Some(lo_x.unwrap_or(lx));
        hi_x = lx;
    }
    let lo_x = lo_x?;
    Some((lo_x as u8, hi_x as u8))
}

/// True when a diagonal face is a genuine shortcut through tighter material —
/// that is, when a vein runs diagonally.
///
/// Without diagonals a diagonal vein cannot conduct along its own axis: water
/// has to zigzag through the two corner cells between, so the vein is throttled
/// to *their* permeability and only grid-aligned veins conduct. That is a real
/// artifact, but adding diagonal faces everywhere is not the fix — in
/// homogeneous rock it just gives every cell eight faces instead of four and
/// roughly doubles drainage, which is a global retune of the water model and
/// works against water lingering in permeable layers at all.
///
/// So the face opens only where the anisotropy actually bites: both ends more
/// permeable than either corner. In homogeneous material the corners match the
/// ends and nothing opens, leaving tuned behaviour untouched.
#[allow(clippy::too_many_arguments)]
fn diagonal_is_a_shortcut(
    world: &World,
    read: &impl Fn(i32, i32, i32, i32) -> Option<Cell>,
    hydro: &HydroOverrides,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    dx: i32,
    dy: i32,
) -> bool {
    let a = match read(lx, ly, gx, gy) {
        Some(c) => c,
        None => return false,
    };
    let bx = world.wrap_x(gx + dx);
    let b = match read(lx + dx, ly + dy, bx, gy + dy) {
        Some(c) => c,
        None => return false,
    };
    // The two cells the orthogonal zigzag would have to pass through.
    let corner_h = read(lx + dx, ly, bx, gy);
    let corner_v = read(lx, ly + dy, gx, gy + dy);
    let ends = permeability_cell(a, hydro).min(permeability_cell(b, hydro));
    let corners = corner_h
        .map(|c| permeability_cell(c, hydro))
        .unwrap_or(0)
        .max(corner_v.map(|c| permeability_cell(c, hydro)).unwrap_or(0));
    ends > corners
}

fn merge_seam_rect(a: Rect, b: Rect) -> Rect {
    Rect {
        x0: a.x0.min(b.x0),
        y0: a.y0.min(b.y0),
        x1: a.x1.max(b.x1),
        y1: a.y1.max(b.y1),
    }
}

/// Cross-seam pore coupling every tick (not cadence-gated).
pub fn apply_seepage_seam_coupling(world: &mut World) {
    let regions = seam_seepage_regions(world);
    if regions.is_empty() {
        return;
    }
    apply_seepage_regions(world, &regions);
}

/// Permeability-limited soak: water moves from wet cells into
/// adjacent porous solids (and between porous solids) down the
/// hydraulic-head gradient.
///
/// This is what makes a puddle wet the sand beach instead of only
/// skating across Air. Rate is capped by the solid's permeability
/// so gravel drinks fast and clay / stone drink slowly.
///
/// Transfers are planned from a snapshot, then applied in a stable
/// order with live capacity checks so mass is conserved exactly.
pub fn apply_seepage(world: &mut World) {
    let regions = regions_for_standalone(world);
    apply_seepage_regions(world, &regions);
}

/// Seepage restricted to a pre-planned active set.
///
/// Single compute-then-apply scan (same snapshot rule as spill / flow —
/// checkerboard is not required for a read-only accumulate).
pub fn apply_seepage_regions(world: &mut World, active: &[ActiveChunk]) {
    apply_seepage_regions_ex(world, active, false)
}

/// Seepage restricted to the **surface contact** only (`contact_only`).
///
/// Percolation *inside* materials is a geological-timescale process and is
/// cadence-gated, but the interface between fast surface water and slow
/// groundwater must stay correct every tick or the top layers show the
/// wrong saturation (and so the wrong deposition). This runs the
/// `Air ↔ porous solid` faces — infiltration and pore weep — and skips
/// peer `pore ↔ pore` conduction, which is the deep, slow part.
pub fn apply_seepage_contact_regions(world: &mut World, active: &[ActiveChunk]) {
    apply_seepage_regions_ex(world, active, true)
}

pub fn apply_seepage_regions_ex(
    world: &mut World,
    active: &[ActiveChunk],
    contact_only: bool,
) {
    if active.is_empty() {
        return;
    }
    // (from, to, amt) with amt > 0.
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    accumulate_seepage_xfers_ex(world, active, &mut xfers, contact_only);
    // Nothing dissolved anywhere means load transport and precipitation have
    // nothing to do. Snapshot keys once into FxHash: after karst the map
    // stays non-empty, and most seepage sources still carry nothing.
    // SipHash contains on every transfer was leftover as `diss` grows.
    let mut loaded: FxHashSet<(i32, i32)> = if world.dissolved.is_empty() {
        FxHashSet::default()
    } else {
        world.dissolved.keys().copied().collect()
    };

    // Apply in a stable order. Each transfer re-reads live sat so a
    // source drained by an earlier xfer simply sends less — every
    // individual move conserves mass exactly.
    serve_best_faces_first(&mut xfers);
    for (from, to, amt) in xfers {
        let Some(src) = world.get_cell(from.0, from.1) else {
            continue;
        };
        let Some(dst) = world.get_cell(to.0, to.1) else {
            continue;
        };
        let cap_dst = water_capacity_cell(dst, &world.hydro) as i32;
        if cap_dst == 0 {
            continue;
        }
        let free = cap_dst - dst.sat.0 as i32;
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
        // Mineral bookkeeping. Both guards matter: these run per transfer on a
        // hot path, and the checks the loop already has in hand (is there any
        // load at all, is the receiver even soluble) skip the common case
        // without a map lookup or a cell re-read.
        if !loaded.is_empty() && loaded.contains(&(from.0, from.1)) {
            // Dissolved mineral travels with the water that carries it, so karst
            // load reaches an outlet instead of sitting where the rock dissolved.
            crate::mineral::carry_with_water(world, from, to, amt as u8, src.sat.0);
            // The brake: load above what the receiving water can hold cements
            // back into the pore space. A conduit that keeps flowing stays open;
            // one that stalls or concentrates seals itself with flowstone.
            crate::mineral::precipitate_at(world, to.0, to.1);
            // Later xfers in this apply must see the load that just arrived.
            loaded.insert((to.0, to.1));
        }
        // Throughput widens the aperture it passed through. This is what turns
        // a preferential path into a conduit: more flow opens the rock, opener
        // rock carries more flow. Below the yield threshold the call is a
        // no-op — skip the cell re-read.
        if amt as u8 > crate::mineral::APERTURE_MIN_THROUGHPUT
            && crate::mineral::is_soluble_rock(dst.material)
        {
            crate::mineral::widen_aperture(
                world,
                to.0,
                to.1,
                amt as u8,
                APERTURE_GROWTH_SCALE,
                APERTURE_SEED_SALT,
            );
        }
    }
}

fn accumulate_seepage_xfers_ex(
    world: &World,
    active: &[ActiveChunk],
    xfers: &mut Vec<((i32, i32), (i32, i32), i32)>,
    contact_only: bool,
) {
    // Right, up, and both diagonals. Every cell in the region is visited, so
    // each face is handled exactly once from its lower-left end.
    //
    // The diagonals exist because without them a vein cannot conduct along its
    // own axis: water had to zigzag through the matrix cells between, so a
    // diagonal fracture was throttled to matrix permeability while a
    // grid-aligned one ran at its own. Channels could only ever form along the
    // world axes.
    const OFFSETS: [(i32, i32); 4] = [(1, 0), (0, 1), (1, 1), (1, -1)];
    // Diagonal faces are √2 further apart, so the same head drives less flux.
    // 181/256 ≈ 1/√2. Without it diagonal conduction reads as *faster* than
    // straight, which is worse than having no diagonals at all.
    const DIAGONAL_NUM: i32 = 181;
    // Decorrelates the fractional-rate gate from other hashed decisions.
    const SEEPAGE_FIRE_SALT: u64 = 0x5EE9_0DD5;
    const DIAGONAL_DEN: i32 = 256;
    let hydro = world.hydro;
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    // Sticky occupancy: a chunk that has never held wet Air or a wet
    // pore has no face that can move water. The flow halo still includes
    // dry rock / empty sky from gravity and body writes; walking those
    // was the leftover seepage cost on a tall world. Bootstrap (no flags
    // set yet) keeps every region so a legacy save cannot skip a wet
    // chunk whose flags were never stamped.
    let any_water = world
        .chunks
        .values()
        .any(|c| c.has_wet_air || c.has_wet_pores);
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        // Chunk-local reads — same pattern as water_flow (~10× vs HashMap).
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            return local;
        };
        if any_water && !chunk.has_wet_air && !chunk.has_wet_pores {
            return local;
        }
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
                let a = chunk.get(x as usize, y as usize);
                let cap_a = water_capacity_cell(a, &hydro);
                if cap_a == 0 {
                    continue;
                }
                let a_solid = is_porous_cell(a, &hydro);
                // Lake / rain interior: Air whose +x / +y faces are also Air
                // cannot infiltrate or weep. Diagonals are pore-only. Skip
                // before the neighbour loop so a 64×64 pond does not pay
                // head math on every cell.
                if !a_solid {
                    let right = read(lx + 1, ly, world.wrap_x(gx + 1), gy);
                    let up = read(lx, ly + 1, gx, gy + 1);
                    let airish = |c: Option<Cell>| match c {
                        Some(n) => n.material == MaterialId::Air && !is_porous_cell(n, &hydro),
                        None => true,
                    };
                    if airish(right) && airish(up) {
                        continue;
                    }
                }
                // Air–Air / impermeable–impermeable edges are no-ops —
                // check materials before head math. Dominates rainy
                // ocean shore halos.
                for (dx, dy) in OFFSETS {
                    let diagonal = dx != 0 && dy != 0;
                    // Diagonals only ever carry pore↔pore conduction, which the
                    // contact pass skips by definition — discard them before
                    // paying for the neighbour read.
                    if contact_only && diagonal {
                        continue;
                    }
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy + dy;
                    if dx != 0 && nx == gx {
                        continue;
                    }
                    let Some(b) = read(lx + dx, ly + dy, nx, ny) else {
                        continue;
                    };
                    let b_solid = is_porous_cell(b, &hydro);
                    if !a_solid && !b_solid {
                        continue;
                    }
                    // Contact pass: only the surface interface, not the slow
                    // percolation between two buried pores.
                    if contact_only && a_solid && b_solid {
                        continue;
                    }
                    let cap_b = water_capacity_cell(b, &hydro);
                    if cap_b == 0 {
                        continue;
                    }
                    // Quiet saturated table: both pores full → no room
                    // either way. Skip before the fire-odds roll and head
                    // math. Air faces still run (infiltration / weep).
                    if a_solid && b_solid && a.sat.0 >= cap_a && b.sat.0 >= cap_b {
                        continue;
                    }
                    // Diagonals only carry pore↔pore conduction, which is the
                    // anisotropy they were added to fix. Surface infiltration
                    // and weep keep their orthogonal faces: those rules are
                    // tuned against a free surface, and giving a lake bed four
                    // more faces to force-fill through would change how fast
                    // ponds soak away, which is not what this is for.
                    if diagonal
                        && (!(a_solid && b_solid)
                            || !diagonal_is_a_shortcut(world, &read, &hydro, gx, gy, lx, ly, dx, dy))
                    {
                        continue;
                    }
                    // Fractional conduction: an edge fires with odds that make its
                    // *average* rate exactly proportional to permeability. Integer
                    // rates alone crushed stone's whole 5..40 range into 1..2, so
                    // pore variation never reached the water — three stone cells at
                    // permeability 5, 10 and 23 all held 5-6 sat in playtest.
                    if a_solid && b_solid {
                        let odds = seepage_fire_odds_cell(a, &hydro)
                            .min(seepage_fire_odds_cell(b, &hydro));
                        if odds <= 0.0 {
                            continue;
                        }
                        if odds < 1.0 {
                            let roll = crate::rules::hash_prob(
                                world.seed.0,
                                gx.wrapping_mul(73_856_093).wrapping_add(gy),
                                world.tick,
                                SEEPAGE_FIRE_SALT,
                            );
                            if roll >= odds {
                                continue;
                            }
                        }
                    }
                    let mut move_amt = sat_move_to_equalize_heads(
                        a.sat.0, cap_a, gy, b.sat.0, cap_b, ny,
                    );
                    // Capillary retention: pore water may only drive *down* by
                    // the amount above the donor's field capacity. Below that
                    // it is held against gravity and stays put.
                    //
                    // This replaces a flat 10% residual film. Retention is what
                    // stops every cell draining into the one below, which was
                    // making the only stable state a saturated wedge growing up
                    // from bedrock regardless of the pore field. It also gives
                    // each material its character: clay perches a table, gravel
                    // lets water straight through.
                    if a_solid && b_solid && move_amt != 0 {
                        let downward = if move_amt > 0 {
                            gy > ny // a → b and a is higher
                        } else {
                            ny > gy // b → a and b is higher
                        };
                        if downward {
                            let donor = if move_amt > 0 { a } else { b };
                            let mobile = crate::cell::drainable_sat_cell(donor, &hydro) as i32;
                            if mobile <= 0 {
                                move_amt = 0;
                            } else if move_amt > 0 {
                                move_amt = move_amt.min(mobile);
                            } else {
                                move_amt = move_amt.max(-mobile);
                            }
                        }
                    }
                    // Standing free water on a settled bed keeps infiltrating.
                    // Skip the force-fill when the Air is still runoff (open
                    // face / downhill escape) so hillside blobs drain instead
                    // of vanishing into the seat like jelly.
                    if dy == 1
                        && a_solid
                        && b.material == MaterialId::Air
                        && b.sat.0 >= 160
                        && !standing_air_is_runoff(world, &read, nx, ny, lx + dx, ly + dy)
                    {
                        let free = cap_a.saturating_sub(a.sat.0) as i32;
                        move_amt = -(b.sat.0 as i32).min(free);
                    }
                    // Standing pond / lake side face → bank soak (same idea).
                    if dx != 0 {
                        if a_solid
                            && b.material == MaterialId::Air
                            && b.sat.0 >= 160
                            && !standing_air_is_runoff(world, &read, nx, ny, lx + dx, ly + dy)
                        {
                            let free = cap_a.saturating_sub(a.sat.0) as i32;
                            if free > 0 {
                                move_amt = -(b.sat.0 as i32).min(free);
                            }
                        } else if !a_solid
                            && a.material == MaterialId::Air
                            && a.sat.0 >= 160
                            && b_solid
                            && !standing_air_is_runoff(world, &read, gx, gy, lx, ly)
                        {
                            let free = cap_b.saturating_sub(b.sat.0) as i32;
                            if free > 0 {
                                move_amt = (a.sat.0 as i32).min(free);
                            }
                        }
                    }
                    // Pore spring into buried / enclosed Air. Head equalise
                    // alone stalls once cavity air is a bit wetter (fraction)
                    // than a depleted wall — dug voids stayed empty inside
                    // blue groundwater. Surface sheet films stay splash-capped
                    // below; here we force wet pores to keep weeping.
                    if a_solid
                        && b.material == MaterialId::Air
                        && !b.sat.is_full()
                        && a.sat.0 > 0
                        && !is_surface_sheet_air(
                            world, &read, nx, ny, lx + dx, ly + dy, &b,
                        )
                    {
                        let free = u8::MAX.saturating_sub(b.sat.0) as i32;
                        let weep = (a.sat.0 as i32).min(free);
                        if weep > 0 {
                            move_amt = move_amt.max(weep);
                        }
                    } else if b_solid
                        && a.material == MaterialId::Air
                        && !a.sat.is_full()
                        && b.sat.0 > 0
                        && !is_surface_sheet_air(world, &read, gx, gy, lx, ly, &a)
                    {
                        let free = u8::MAX.saturating_sub(a.sat.0) as i32;
                        let weep = (b.sat.0 as i32).min(free);
                        if weep > 0 {
                            move_amt = -weep;
                        }
                    }
                    if move_amt == 0 {
                        continue;
                    }
                    let rate = if a_solid && b_solid {
                        // Peer pores: drier side limits conduction (wetting
                        // front). Full vertical min-perm piped a residual
                        // film to bedrock and left a "dry" mid gap under
                        // hill dumps — looked like teleported groundwater.
                        seepage_conduct_rate_cells(a, cap_a, b, cap_b, &hydro)
                    } else if move_amt > 0 {
                        // A → B: infiltrating into B, or A weeping into Air.
                        if b_solid {
                            if a.material == MaterialId::Air && a.sat.0 >= 160 {
                                if standing_air_is_runoff(world, &read, gx, gy, lx, ly) {
                                    seepage_uptake_rate_cell(b, &hydro, cap_b)
                                } else {
                                    seepage_rate_cell(b, &hydro)
                                }
                            } else {
                                seepage_uptake_rate_cell(b, &hydro, cap_b)
                            }
                        } else {
                            seepage_rate_cell(a, &hydro)
                        }
                    } else {
                        // B → A: infiltrating into A, or B weeping into Air.
                        if a_solid {
                            if b.material == MaterialId::Air && b.sat.0 >= 160 {
                                if standing_air_is_runoff(world, &read, nx, ny, lx + dx, ly + dy)
                                {
                                    seepage_uptake_rate_cell(a, &hydro, cap_a)
                                } else {
                                    seepage_rate_cell(a, &hydro)
                                }
                            } else {
                                seepage_uptake_rate_cell(a, &hydro, cap_a)
                            }
                        } else {
                            seepage_rate_cell(b, &hydro)
                        }
                    };
                    // Longer path across a diagonal face.
                    let rate = if diagonal {
                        ((rate * DIAGONAL_NUM) / DIAGONAL_DEN).max(1)
                    } else {
                        rate
                    };
                    // Fully saturated faces weep faster into open Air
                    // (cliff springs) — still permeability-capped, but
                    // not stuck at 1 sat/tick for tight stone.
                    let rate = {
                        let a_full = a_solid && a.sat.0 >= cap_a;
                        let b_full = b_solid && b.sat.0 >= cap_b;
                        let into_air = (a_full && !b_solid) || (b_full && !a_solid);
                        if into_air {
                            (rate * 3).clamp(1, 16)
                        } else {
                            rate
                        }
                    };
                    // Open surface sheet films only splash-wet a dry bank so
                    // overland flow does not vanish into pores. Enclosed /
                    // buried Air (cavities, caves) takes a full pore weep —
                    // otherwise groundwater never fills dug voids.
                    let rate = {
                        let air_solid = a_solid != b_solid;
                        if !air_solid {
                            rate
                        } else {
                            let air = if a_solid { &b } else { &a };
                            let standing_face = air.material == MaterialId::Air && air.sat.0 >= 160;
                            if standing_face {
                                rate
                            } else {
                                let air_lx = if a_solid { lx + dx } else { lx };
                                let air_ly = if a_solid { ly + dy } else { ly };
                                let air_gx = if a_solid { nx } else { gx };
                                let air_gy = if a_solid { ny } else { gy };
                                if air.material == MaterialId::Air
                                    && is_surface_sheet_air(
                                        world, &read, air_gx, air_gy, air_lx, air_ly, air,
                                    )
                                {
                                    rate.min(SHEET_FACE_SPLASH)
                                } else {
                                    rate
                                }
                            }
                        }
                    };
                    if rate <= 0 {
                        continue;
                    }
                    if move_amt > 0 {
                        local.push(((gx, gy), (nx, ny), move_amt.min(rate)));
                    } else {
                        local.push(((nx, ny), (gx, gy), (-move_amt).min(rate)));
                    }
                }
            }
        }
        local
    });
    for mut v in local {
        xfers.append(&mut v);
    }
}

/// Splash into a column face / flowing bed — not a full pore fill.
const SHEET_FACE_SPLASH: i32 = 4;

/// True when standing Air can still cascade / drain as overland flow.
///
/// Only cascade edges and diagonal-down Air count. A same-Y open Air
/// neighbour used to mark calm lake-shore cells as "runoff", which
/// skipped bank force-fill and left sawtooth seepage fingers into the
/// hill on the U heatmap.
fn standing_air_is_runoff(
    world: &World,
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
) -> bool {
    for dx in [-1_i32, 1] {
        let nx = world.wrap_x(gx + dx);
        let nlx = lx + dx;
        // Cascade edge: side Air sitting above Air with room.
        if let Some(side) = read(nlx, ly, nx, gy) {
            if side.material == MaterialId::Air {
                if let Some(below) = read(nlx, ly - 1, nx, gy - 1) {
                    if below.material == MaterialId::Air && !below.sat.is_full() {
                        return true;
                    }
                }
            }
        }
        // Diagonal-down into Air with room (shelf / slope drain).
        match read(nlx, ly - 1, nx, gy - 1) {
            Some(c) if c.material == MaterialId::Air && !c.sat.is_full() => return true,
            _ => {}
        }
    }
    false
}

fn air_has_dry_escape(
    world: &World,
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
) -> bool {
    for dx in [-1_i32, 1] {
        let nx = world.wrap_x(gx + dx);
        match read(lx + dx, ly, nx, gy) {
            Some(c) if c.material == MaterialId::Air && c.sat.0 < 32 => return true,
            _ => {}
        }
    }
    false
}

/// Surface overland film: thin Air resting on solid/full water with a dry
/// side escape. Buried cavity Air (open space under a roof) is **not** a
/// sheet — pores must weep into it at full rate.
fn is_surface_sheet_air(
    world: &World,
    read: &dyn Fn(i32, i32, i32, i32) -> Option<Cell>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    air: &Cell,
) -> bool {
    if air.material != MaterialId::Air || air.sat.0 >= 160 {
        return false;
    }
    let on_support = matches!(
        read(lx, ly - 1, gx, gy - 1),
        Some(b) if b.material != MaterialId::Air || b.sat.is_full()
    );
    if !on_support {
        return false;
    }
    air_has_dry_escape(world, read, gx, gy, lx, ly)
}

/// Re-dirty wet porous faces that can still weep into Air with room.
///
/// Quiet groundwater next to a dug cavity otherwise drops out of dirty
/// tracking and never fills the void (playtest: empty circle in blue sat).
pub fn wake_pore_weep_into_air(world: &mut World) {
    let hydro = world.hydro;
    let ch = CHUNK_CELLS_H as i32;
    let cw = CHUNK_CELLS_W as i32;
    let mut touches: Vec<(i32, i32)> = Vec::new();
    let mut clear_pores: Vec<ChunkCoord> = Vec::new();
    let mut air_updates: Vec<(ChunkCoord, bool)> = Vec::new();
    let mut unsat_updates: Vec<(ChunkCoord, bool)> = Vec::new();
    const DIRS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_pores)
        .map(|(&coord, _)| coord)
        .collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        // Buried crust (no Air at all) can only weep on the perimeter —
        // an interior pore cannot face Air. Surface / cavity chunks keep
        // the full scan. Neighbour reads stay chunk-local when they can.
        let open_air = chunk.has_open_air;
        let base_gx = coord.cx * cw;
        let base_gy = coord.cy * ch;
        let mut still_wet = false;
        let mut any_air = false;
        let mut any_unsat = false;
        for y in 0..CHUNK_CELLS_H {
            for x in 0..CHUNK_CELLS_W {
                let cell = chunk.get(x, y);
                if cell.material == MaterialId::Air {
                    any_air = true;
                }
                if is_porous_cell(cell, &hydro) {
                    if cell.sat.0 > 0 {
                        still_wet = true;
                    }
                    let cap = water_capacity_cell(cell, &hydro);
                    if cap > 0 && cell.sat.0 < cap {
                        any_unsat = true;
                    }
                }
                if !open_air
                    && x != 0
                    && x + 1 != CHUNK_CELLS_W
                    && y != 0
                    && y + 1 != CHUNK_CELLS_H
                {
                    continue;
                }
                if !is_porous_cell(cell, &hydro) {
                    continue;
                }
                let cap = water_capacity_cell(cell, &hydro);
                // Need a meaningful donor — residual film only skipped.
                if cap == 0 || cell.sat.0 <= 2 {
                    continue;
                }
                let gx = world.wrap_x(base_gx + x as i32);
                let gy = base_gy + y as i32;
                let mut face = false;
                for (dx, dy) in DIRS {
                    let lx = x as i32 + dx;
                    let ly = y as i32 + dy;
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy + dy;
                    let n = if lx >= 0 && lx < cw && ly >= 0 && ly < ch {
                        Some(chunk.get(lx as usize, ly as usize))
                    } else {
                        world.get_cell(nx, ny)
                    };
                    let Some(n) = n else {
                        continue;
                    };
                    if n.material == MaterialId::Air && !n.sat.is_full() {
                        touches.push((nx, ny));
                        face = true;
                    }
                }
                if face {
                    touches.push((gx, gy));
                    // Recharge halo: wake wet pore neighbours so the
                    // aquifer can keep feeding the spring face.
                    for (dx, dy) in DIRS {
                        let lx = x as i32 + dx;
                        let ly = y as i32 + dy;
                        let nx = world.wrap_x(gx + dx);
                        let ny = gy + dy;
                        let n = if lx >= 0 && lx < cw && ly >= 0 && ly < ch {
                            Some(chunk.get(lx as usize, ly as usize))
                        } else {
                            world.get_cell(nx, ny)
                        };
                        let Some(n) = n else {
                            continue;
                        };
                        if is_porous_cell(n, &hydro) && n.sat.0 > 0 {
                            touches.push((nx, ny));
                        }
                    }
                }
            }
        }
        if !still_wet {
            clear_pores.push(coord);
        }
        air_updates.push((coord, any_air));
        unsat_updates.push((coord, any_unsat));
    }
    for (gx, gy) in touches {
        world.touch_dirty(gx, gy);
    }
    for coord in clear_pores {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            chunk.has_wet_pores = false;
        }
    }
    for (coord, any_air) in air_updates {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            chunk.has_open_air = any_air;
        }
    }
    for (coord, any_unsat) in unsat_updates {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            chunk.has_unsaturated_pores = any_unsat;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A donor with limited water must feed its conduit, not its flanks.
    ///
    /// The apply loop clamps every transfer by the donor's live saturation, so
    /// ordering *is* allocation: whatever is served last gets whatever is left,
    /// which is often nothing. Before this, order came from neighbour coordinate
    /// with ties serving the smallest transfer first — so a weak face could take
    /// the water a well-conducting one needed, and no path ever starved.
    #[test]
    fn a_donor_feeds_its_best_conducting_face_first() {
        let donor = (10, 5);
        let weak = (9, 5);
        let strong = (11, 5);
        let mut xfers = vec![(donor, weak, 2), (donor, strong, 30)];
        serve_best_faces_first(&mut xfers);
        assert_eq!(
            xfers[0].1, strong,
            "the largest (best-conducting) transfer must be served first"
        );

        // Order of arrival must not matter.
        let mut other = vec![(donor, strong, 30), (donor, weak, 2)];
        serve_best_faces_first(&mut other);
        assert_eq!(other, xfers, "allocation must not depend on insertion order");
    }

    #[test]
    fn allocation_is_deterministic_when_faces_tie() {
        // Equal conductance has to resolve the same way every run, or seepage
        // stops being reproducible for a seed.
        let donor = (4, 4);
        let mut a = vec![(donor, (5, 4), 7), (donor, (4, 5), 7)];
        let mut b = vec![(donor, (4, 5), 7), (donor, (5, 4), 7)];
        serve_best_faces_first(&mut a);
        serve_best_faces_first(&mut b);
        assert_eq!(a, b, "tied faces must order deterministically");
    }

    #[test]
    fn donors_stay_grouped() {
        // The apply loop reads live saturation per transfer; grouping by donor is
        // what makes "served first" mean anything.
        let mut xfers = vec![
            ((2, 2), (3, 2), 5),
            ((1, 1), (2, 1), 9),
            ((2, 2), (2, 3), 40),
            ((1, 1), (1, 2), 1),
        ];
        serve_best_faces_first(&mut xfers);
        assert_eq!(xfers[0].0, (1, 1));
        assert_eq!(xfers[1].0, (1, 1));
        assert_eq!(xfers[2].0, (2, 2));
        assert_eq!(xfers[3].0, (2, 2));
        assert_eq!(xfers[2].2, 40, "biggest first within a donor");
    }
}
