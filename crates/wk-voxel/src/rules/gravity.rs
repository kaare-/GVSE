//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Vertical gravity fall for free water.

use crate::active::{partition_checkerboard, ActiveChunk};
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::{self, for_each_region_parallel};

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
/// - **Cross-chunk**: dispatch through [`World::get_cell`] /
///   [`World::set_cell`]. Missing above/below chunks yield no move.
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
pub fn apply_gravity_fall_regions(world: &mut World, active: &[ActiveChunk]) {
    let hydro = world.hydro;
    for_each_region_parallel(world, active, |ptrs, wrap_width, ac| {
        let base_gx = ac.coord.cx * CHUNK_CELLS_W as i32;
        let base_gy = ac.coord.cy * CHUNK_CELLS_H as i32;
        for x in ac.rect.x0..=ac.rect.x1 {
            let gx = base_gx + x as i32;
            let mut any_mobile = false;
            for y in ac.rect.y0..=ac.rect.y1 {
                let gy = base_gy + y as i32;
                // SAFETY: ptrs cover own + cy+1; see [`crate::parallel`].
                let Some(above) =
                    (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy + 1) })
                else {
                    continue;
                };
                if above.sat.is_empty() {
                    continue;
                }
                if water_capacity_with(above.material, &hydro) == 0 {
                    continue;
                }
                any_mobile = true;
                break;
            }
            if !any_mobile {
                continue;
            }
            for y in ac.rect.y0..=ac.rect.y1 {
                let gy = base_gy + y as i32;
                let Some(cur) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy) }) else {
                    continue;
                };
                let cap = water_capacity_with(cur.material, &hydro);
                if cap == 0 {
                    continue;
                }
                let free = cap.saturating_sub(cur.sat.0);
                if free == 0 {
                    continue;
                }
                let Some(above) =
                    (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy + 1) })
                else {
                    continue;
                };
                if above.sat.is_empty() {
                    continue;
                }
                if water_capacity_with(above.material, &hydro) == 0 {
                    continue;
                }
                let move_amt = above.sat.0.min(free);
                if move_amt == 0 {
                    continue;
                }
                unsafe {
                    parallel::set_cell(
                        ptrs,
                        wrap_width,
                        gx,
                        gy + 1,
                        Cell {
                            sat: Sat(above.sat.0 - move_amt),
                            ..above
                        },
                    );
                    parallel::set_cell(
                        ptrs,
                        wrap_width,
                        gx,
                        gy,
                        Cell {
                            sat: Sat(cur.sat.0 + move_amt),
                            ..cur
                        },
                    );
                }
            }
        }
    });
}
