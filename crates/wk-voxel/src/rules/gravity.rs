//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Vertical gravity fall for free water.

use wk_material::MaterialId;

use crate::active::{partition_checkerboard, ActiveChunk};
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::for_each_region_parallel;

use super::head::seepage_uptake_rate_with;
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
///   with full wet Air on both sides. Shore / weir / open-surge faces
///   stay in Air; seepage splash-wets those beds.
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
    for pass in partition_checkerboard(&regions) {
        apply_gravity_fall_regions(world, &pass);
    }
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
    let hydro = world.hydro;
    for_each_region_parallel(world, active, |ptrs, _wrap_width, ac| {
        let Some(own) = ptrs.get(&ac.coord) else {
            return;
        };
        let above_ptr = ptrs.get(&ChunkCoord::new(ac.coord.cx, ac.coord.cy + 1));

        // SAFETY: `own` / `above_ptr` come from [`parallel::pull_write_coords`]
        // for this checkerboard colour (disjoint write sets).
        let read_xy = |lx: u8, ly: i32| -> Option<Cell> {
            if ly < 0 {
                return None;
            }
            let ly_u = ly as usize;
            if ly_u < CHUNK_CELLS_H {
                Some(unsafe { (*own).get(lx as usize, ly_u) })
            } else if ly_u == CHUNK_CELLS_H {
                let p = above_ptr?;
                Some(unsafe { (*p).get(lx as usize, 0) })
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
                if let Some(p) = above_ptr {
                    unsafe {
                        (*p).set(lx as usize, 0, cell);
                    }
                }
            }
        };
        let mobile_cap = |m: MaterialId| -> u8 {
            if m == MaterialId::Air {
                u8::MAX
            } else {
                water_capacity_with(m, &hydro)
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
            // Lake interior = full wet Air on both sides. Anything else
            // (dry Air, partial film, or a solid weir face) is a surge /
            // shore column and must not gravity-drink its bed.
            let full_wet = |nlx: i32| -> bool {
                if nlx < 0 || nlx >= CHUNK_CELLS_W as i32 {
                    // Unknown chunk seam: do not invent a dry face.
                    return true;
                }
                matches!(
                    read_xy(nlx as u8, ly_src),
                    Some(c) if c.material == MaterialId::Air && c.sat.is_full()
                )
            };
            !full_wet(lx as i32 - 1) || !full_wet(lx as i32 + 1)
        };
        for x in ac.rect.x0..=ac.rect.x1 {
            let mut any_mobile = false;
            for y in ac.rect.y0..=ac.rect.y1 {
                let Some(above) = read_xy(x, y as i32 + 1) else {
                    continue;
                };
                if above.sat.is_empty() {
                    continue;
                }
                if mobile_cap(above.material) == 0 {
                    continue;
                }
                any_mobile = true;
                break;
            }
            if !any_mobile {
                continue;
            }
            // Sliding window: after processing y, cell y+1 is the next
            // destination — reuse the post-pull (or untouched) above.
            let mut next_cur: Option<Cell> = None;
            for y in ac.rect.y0..=ac.rect.y1 {
                let cur = match next_cur.take() {
                    Some(c) => c,
                    None => match read_xy(x, y as i32) {
                        Some(c) => c,
                        None => continue,
                    },
                };
                let cap = mobile_cap(cur.material);
                if cap == 0 {
                    continue;
                }
                let free = cap.saturating_sub(cur.sat.0);
                if free == 0 {
                    continue;
                }
                let Some(above) = read_xy(x, y as i32 + 1) else {
                    continue;
                };
                if above.sat.is_empty() || mobile_cap(above.material) == 0 {
                    next_cur = Some(above);
                    continue;
                }
                // Walled ponds and stacked lake interiors infiltrate the
                // bed on a wetting curve — bone-dry beds take a trickle so
                // free water can still move; wet beds drink faster.
                if cur.material != MaterialId::Air && above.material == MaterialId::Air {
                    let src_y = y as i32 + 1;
                    let settled_stack = stacked_air(x, src_y) && !open_surge_face(x, src_y);
                    if !walled_air(x, src_y) && !settled_stack {
                        next_cur = Some(above);
                        continue;
                    }
                    let rate = seepage_uptake_rate_with(cur.material, &hydro, cur.sat.0, cap);
                    if rate <= 0 {
                        next_cur = Some(above);
                        continue;
                    }
                    let move_amt = (above.sat.0 as i32).min(free as i32).min(rate) as u8;
                    if move_amt == 0 {
                        next_cur = Some(above);
                        continue;
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
                    next_cur = Some(new_above);
                    continue;
                }

                // Free water falls only through Air. Pore water in solids
                // is seepage's job — otherwise a wet soil cell dumps its
                // entire sat into the cell below every gravity pass
                // (powder freefall through the mountain).
                if cur.material != MaterialId::Air || above.material != MaterialId::Air {
                    next_cur = Some(above);
                    continue;
                }

                let move_amt = above.sat.0.min(free);
                if move_amt == 0 {
                    next_cur = Some(above);
                    continue;
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
                next_cur = Some(new_above);
            }
        }
    });
}
