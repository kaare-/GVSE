//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Vertical gravity fall for free water.

use wk_material::MaterialId;

use crate::active::{partition_checkerboard, ActiveChunk};
use crate::cell::{water_capacity_cell, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::FxHashSet;
use crate::grid::World;
use crate::parallel::for_each_region_parallel;

use super::head::{seepage_rate_cell, seepage_uptake_rate_cell};
use super::plan::regions_for_standalone;

/// Bottom-up single-step gravity fall for water saturation.
///
/// Each cell **pulls** water from the cell directly above into its
/// own free capacity. Behaviour:
///
/// - Traversal is **bottom-up** within each chunk (`y = 0 → H-1`),
///   matching Petri Purho's Noita rule: a column of water on solid
///   ground stays put (each cell is already full or impermeable),
///   while a lone droplet migrates down exactly one cell per
///   invocation (the destination pulls, then its own turn as a
///   source has already passed).
/// - **Pull (not push)** so checkerboard sub-passes stay one-step
///   across chunk seams: the lower chunk owns the write into the
///   destination and drains the upper neighbour, which is always a
///   different colour and therefore not concurrent.
/// - **Cross-chunk**: own + `cy + 1` via chunk-local indexing (same
///   write-set as [`parallel::pull_write_coords`]). Missing above
///   chunks yield no move.
/// - **Air → porous solid** for a walled pond, or a stacked lake column
///   with standing wet Air (`sat ≥ 160`) on both sides. Shore / weir /
///   open-surge faces stay in Air; seepage soaks those beds. (Requiring
///   neighbours to be `sat == 255` left near-full open basins dry.)
/// - **Pore water in solids never freefalls.** Solid→solid and solid→Air
///   sat moves belong to seepage (Darcy). Dumping a full cell of pore
///   water down a soil column each gravity pass looked like powder
///   draining through the mountain.
///
/// This is intentionally the simplest possible fall model — one cell
/// per invocation, no lateral spread, no density swap. Free-fall
/// acceleration and the density-swap rule are follow-up PRs.
pub fn apply_gravity_fall(world: &mut World) {
    let regions = regions_for_standalone(world);
    // One snapshot for both checkerboard colours — rebuilding the key
    // set per colour was 2× the HashSet walk on every standalone call.
    let loaded = water_load_index(world);
    for pass in partition_checkerboard(&regions) {
        apply_gravity_fall_regions_loaded(world, &pass, &loaded);
    }
}

/// Cells that currently carry dissolved mineral or suspended silt.
///
/// Gravity's hot loop cannot touch the sparse maps (raw chunk pointers),
/// so callers snapshot this once and reuse it across checkerboard colours
/// / flow substeps. FxHash — leftover SipHash as karst ages (`diss`
/// 1k → 10k). Same cells. Load that moves during the tick lags until
/// the next snapshot — geology-OK; the stream mineral test runs 120 ticks.
pub(crate) fn water_load_index(world: &World) -> FxHashSet<(i32, i32)> {
    if world.dissolved.is_empty() && world.suspended.is_empty() {
        return FxHashSet::default();
    }
    world
        .dissolved
        .keys()
        .copied()
        .chain(world.suspended.keys().copied())
        .collect()
}

/// Gravity fall restricted to a pre-planned active set (see [`plan_active`]).
///
/// One checkerboard colour at a time — callers that already hold a
/// full plan should wrap with [`partition_checkerboard`]. Regions in
/// the same colour run on rayon when [`parallel::parallel_enabled`].
///
/// Per-column source probe: if no mobile sat sits in `(y0, y1+1]` the
/// column cannot pull this pass — skip it. (A stricter "free+sat" probe
/// regressed on Super-Server shores — more probe reads, little skip.)
/// Odd-step gravity cadence is unchanged.
///
/// Hot path reads/writes the region's chunk (and `cy+1` at the top
/// seam) by local index — same ~10× win as flow/seepage vs per-cell
/// HashMap `get_cell`/`set_cell`.
pub fn apply_gravity_fall_regions(world: &mut World, active: &[ActiveChunk]) {
    let loaded = water_load_index(world);
    apply_gravity_fall_regions_loaded(world, active, &loaded);
}

/// Gravity fall with a prebuilt load index — see [`water_load_index`].
pub(crate) fn apply_gravity_fall_regions_loaded(
    world: &mut World,
    active: &[ActiveChunk],
    loaded: &FxHashSet<(i32, i32)>,
) {
    let hydro = world.hydro;
    // Gravity only pulls free water. Plant-dirty dry land under dry sky
    // is leftover — dest can still receive a fall from `cy+1`, so skip
    // only when this chunk *and* the chunk above have no wet Air.
    // Occupancy is the source of truth.
    // Dissolved / suspended load has to ride these transfers, but the hot
    // loop runs over raw chunk pointers and cannot touch the sparse maps.
    // Collect the moves and apply them after. Once karst has emitted *any*
    // load, `dissolved` stays non-empty for the rest of the soak — so the
    // skip must be per-source, not "is the map empty". Otherwise every
    // lake drip takes a mutex and two HashMap lookups forever.
    let track_load = !loaded.is_empty();
    let load_moves: std::sync::Mutex<Vec<((i32, i32), (i32, i32), u8, u8)>> =
        std::sync::Mutex::new(Vec::new());
    for_each_region_parallel(world, active, |ptrs, wrap_width, ac| {
        let base_gx = ac.coord.cx * CHUNK_CELLS_W as i32;
        let wrap = |gx: i32| match wrap_width {
            Some(w) if w > 0 => gx.rem_euclid(w),
            _ => gx,
        };
        let gx_of = |lx: u8| wrap(base_gx + lx as i32);
        let Some(own) = ptrs.get(&ac.coord) else {
            return;
        };
        let above_ptr = ptrs.get(&ChunkCoord::new(ac.coord.cx, ac.coord.cy + 1));
        {
            // SAFETY: occupancy flags are read-only here; write set is
            // the checkerboard colour already granted to this closure.
            let dest_wet = unsafe { (*own).has_wet_air };
            let above_wet = above_ptr
                .map(|p| unsafe { (*p).has_wet_air })
                .unwrap_or(false);
            if !dest_wet && !above_wet {
                return;
            }
        }

        // SAFETY: `own` / `above_ptr` come from [`parallel::pull_write_coords`]
        // for this checkerboard colour (disjoint write sets).
        let read_xy = |lx: u8, ly: i32| -> Option<Cell> {
            if ly < 0 {
                return None;
            }
            let ly_u = ly as usize;
            if ly_u < CHUNK_CELLS_H {
                Some(unsafe { (*own).get(lx as usize, ly_u) })
            } else if let Some(p) = above_ptr {
                // Read into cy+1 (not only the seam cell). stacked_air /
                // open_surge at a bed contact on y=CHUNK_CELLS_H-1 need
                // neighbours in the chunk above; returning None there
                // made every seam lake look like an open surge.
                let local = ly_u - CHUNK_CELLS_H;
                if local < CHUNK_CELLS_H {
                    Some(unsafe { (*p).get(lx as usize, local) })
                } else {
                    None
                }
            } else {
                None
            }
        };
        let write_xy = |lx: u8, ly: i32, cell: Cell| {
            if ly < 0 {
                return;
            }
            let ly_u = ly as usize;
            if ly_u < CHUNK_CELLS_H {
                unsafe {
                    (*own).set(lx as usize, ly_u, cell);
                }
            } else if ly_u == CHUNK_CELLS_H {
                // Gravity only pulls one cell; seam write stays at local y=0.
                if let Some(p) = above_ptr {
                    unsafe {
                        (*p).set(lx as usize, 0, cell);
                    }
                }
            }
        };
        let mobile_cap = |cell: Cell| -> u8 {
            if cell.material == MaterialId::Air {
                u8::MAX
            } else {
                water_capacity_cell(cell, &hydro)
            }
        };
        // Walled pond, or stacked lake away from an immediate dry face.
        // An 8-cell escape scan left dry beds under most of a pond shore.
        let walled_air = |lx: u8, ly_src: i32| -> bool {
            let solid = |nlx: i32| -> bool {
                if nlx < 0 || nlx >= CHUNK_CELLS_W as i32 {
                    return true;
                }
                match read_xy(nlx as u8, ly_src) {
                    None => true,
                    Some(c) => c.material != MaterialId::Air,
                }
            };
            solid(lx as i32 - 1) && solid(lx as i32 + 1)
        };
        let stacked_air = |lx: u8, ly_src: i32| -> bool {
            matches!(
                read_xy(lx, ly_src + 1),
                Some(c) if c.material == MaterialId::Air && c.sat.0 >= 160
            )
        };
        let open_surge_face = |lx: u8, ly_src: i32| -> bool {
            // Lake interior = standing wet Air on both sides. Requiring
            // sat==255 treated near-full open basins as surge and left
            // beds dry; sat≥160 matches the standing-water threshold.
            let standing = |nlx: i32| -> bool {
                if nlx < 0 || nlx >= CHUNK_CELLS_W as i32 {
                    // Unknown chunk seam: do not invent a dry face.
                    return true;
                }
                matches!(
                    read_xy(nlx as u8, ly_src),
                    Some(c) if c.material == MaterialId::Air && c.sat.0 >= 160
                )
            };
            !standing(lx as i32 - 1) || !standing(lx as i32 + 1)
        };
        // Slope runoff: if free water can still fall diagonal-down or
        // cascade off an edge, do not gravity-drink the bed. Thick
        // blobs on hillsides look "settled" (wet neighbours + stack)
        // and used to soak in place like jelly instead of draining.
        let downhill_air_escape = |lx: u8, ly_src: i32| -> bool {
            for dx in [-1_i32, 1] {
                let nlx = lx as i32 + dx;
                if nlx < 0 || nlx >= CHUNK_CELLS_W as i32 {
                    continue;
                }
                let n = nlx as u8;
                // Diagonal-down into Air with room.
                if let Some(diag) = read_xy(n, ly_src - 1) {
                    if diag.material == MaterialId::Air && !diag.sat.is_full() {
                        return true;
                    }
                }
                // Side Air sitting above Air with room (cascade edge).
                if let Some(side) = read_xy(n, ly_src) {
                    if side.material == MaterialId::Air {
                        if let Some(below) = read_xy(n, ly_src - 1) {
                            if below.material == MaterialId::Air && !below.sat.is_full() {
                                return true;
                            }
                        }
                    }
                }
            }
            false
        };
        ac.for_each_x(|x| {
            let mut any_mobile = false;
            ac.for_each_y_in_col(x, |y| {
                if any_mobile {
                    return;
                }
                let Some(above) = read_xy(x, y as i32 + 1) else {
                    return;
                };
                if above.sat.is_empty() {
                    return;
                }
                if mobile_cap(above) == 0 {
                    return;
                }
                any_mobile = true;
            });
            if !any_mobile {
                return;
            }
            // Sliding window: after processing y, cell y+1 is the next
            // destination — reuse the post-pull (or untouched) above.
            // A y gap resets the window (re-read); dilated bits are
            // contiguous around each write.
            let mut next_cur: Option<Cell> = None;
            let mut last_y: Option<u8> = None;
            ac.for_each_y_in_col(x, |y| {
                if last_y.is_some_and(|p| y != p.saturating_add(1)) {
                    next_cur = None;
                }
                last_y = Some(y);
                let cur = match next_cur.take() {
                    Some(c) => c,
                    None => match read_xy(x, y as i32) {
                        Some(c) => c,
                        None => return,
                    },
                };
                let cap = mobile_cap(cur);
                if cap == 0 {
                    return;
                }
                let free = cap.saturating_sub(cur.sat.0);
                if free == 0 {
                    return;
                }
                let Some(above) = read_xy(x, y as i32 + 1) else {
                    return;
                };
                if above.sat.is_empty() || mobile_cap(above) == 0 {
                    next_cur = Some(above);
                    return;
                }
                // Walled ponds and stacked lake interiors infiltrate the
                // bed. Open surge / sheet faces stay in Air — seepage
                // splash-wets those. `read_xy` must see into cy+1 so a
                // bed at y=63 under water at y=64 can detect stacked
                // water at y=65 (chunk seam). Slope blobs that can still
                // cascade downhill must not gravity-drink their seat.
                if cur.material != MaterialId::Air && above.material == MaterialId::Air {
                    let src_y = y as i32 + 1;
                    let settled_stack = stacked_air(x, src_y) && !open_surge_face(x, src_y);
                    if downhill_air_escape(x, src_y) {
                        next_cur = Some(above);
                        return;
                    }
                    if !walled_air(x, src_y) && !settled_stack {
                        next_cur = Some(above);
                        return;
                    }
                    // Cell-aware: infiltration under a lake is the dominant way
                    // water enters the ground, so reading a material average
                    // here made the wetting front uniform no matter how the
                    // pore field varied underneath it.
                    let rate = if above.sat.0 >= 160 {
                        seepage_rate_cell(cur, &hydro)
                    } else {
                        seepage_uptake_rate_cell(cur, &hydro, cap)
                    };
                    if rate <= 0 {
                        next_cur = Some(above);
                        return;
                    }
                    let move_amt = (above.sat.0 as i32).min(free as i32).min(rate) as u8;
                    if move_amt == 0 {
                        next_cur = Some(above);
                        return;
                    }
                    let new_above = Cell {
                        sat: Sat(above.sat.0 - move_amt),
                        ..above
                    };
                    let new_cur = Cell {
                        sat: Sat(cur.sat.0 + move_amt),
                        ..cur
                    };
                    write_xy(x, y as i32 + 1, new_above);
                    write_xy(x, y as i32, new_cur);
                    // Infiltrating water takes its mineral load into the ground.
                    if track_load && loaded.contains(&(gx_of(x), y as i32 + 1)) {
                        load_moves.lock().unwrap().push((
                            (gx_of(x), y as i32 + 1),
                            (gx_of(x), y as i32),
                            move_amt,
                            above.sat.0,
                        ));
                    }
                    next_cur = Some(new_above);
                    return;
                }

                // Free water falls only through Air. Pore water in solids
                // is seepage's job — otherwise a wet soil cell dumps its
                // entire sat into the cell below every gravity pass
                // (powder freefall through the mountain).
                if cur.material != MaterialId::Air || above.material != MaterialId::Air {
                    next_cur = Some(above);
                    return;
                }

                let move_amt = above.sat.0.min(free);
                if move_amt == 0 {
                    next_cur = Some(above);
                    return;
                }
                let new_above = Cell {
                    sat: Sat(above.sat.0 - move_amt),
                    ..above
                };
                let new_cur = Cell {
                    sat: Sat(cur.sat.0 + move_amt),
                    ..cur
                };
                write_xy(x, y as i32 + 1, new_above);
                write_xy(x, y as i32, new_cur);
                if track_load && loaded.contains(&(gx_of(x), y as i32 + 1)) {
                    load_moves.lock().unwrap().push((
                        (gx_of(x), y as i32 + 1),
                        (gx_of(x), y as i32),
                        move_amt,
                        above.sat.0,
                    ));
                }
                next_cur = Some(new_above);
            });
        });
    });
    if track_load {
        for (from, to, moved, donor_before) in load_moves.into_inner().unwrap() {
            crate::mineral::carry_with_water(world, from, to, moved, donor_before);
            crate::sediment::carry_with_water(world, from, to, moved, donor_before);
        }
    }
}
