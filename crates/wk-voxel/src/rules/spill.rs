//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Same-row Air–Air spill (test helper; not used by tick).

use std::collections::HashMap;

use wk_material::MaterialId;

use crate::active::{partition_checkerboard, ActiveChunk};
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::{map_regions_parallel};

use super::head::sat_move_to_equalize_heads;
use super::plan::regions_for_standalone;

/// Same-row Air–Air head equalisation only.
///
/// **Test helper** — not called by [`tick`]. Production surface flow is
/// [`apply_water_flow`] (diagonal-down, cascade, same-Y equalise,
/// throughflow). See `docs/VOXEL_WATER.md`.
pub fn apply_lateral_spill(world: &mut World) {
    let regions = regions_for_standalone(world);
    apply_lateral_spill_regions(world, &regions);
}

/// Lateral spill restricted to a pre-planned active set.
pub fn apply_lateral_spill_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    let mut deltas: HashMap<(i32, i32), i32> = HashMap::new();
    for pass in partition_checkerboard(active) {
        accumulate_lateral_spill_deltas(world, &pass, &mut deltas);
    }
    for ((gx, gy), delta) in deltas {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        let cap = water_capacity_with(cell.material, &world.hydro) as i32;
        let new_sat = (cell.sat.0 as i32 + delta).clamp(0, cap);
        world.set_cell(
            gx,
            gy,
            Cell {
                sat: Sat(new_sat as u8),
                ..cell
            },
        );
    }
}

fn accumulate_lateral_spill_deltas(
    world: &World,
    active: &[ActiveChunk],
    deltas: &mut HashMap<(i32, i32), i32>,
) {
    let hydro = world.hydro;
    let local = map_regions_parallel(active, |ac| {
        let mut local: HashMap<(i32, i32), i32> = HashMap::new();
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let left_x = world.wrap_x(gx);
                let right_x = world.wrap_x(gx + 1);
                if left_x == right_x {
                    continue;
                }
                let Some(left) = world.get_cell(left_x, gy) else {
                    continue;
                };
                if left.material != MaterialId::Air {
                    continue;
                }
                let Some(right) = world.get_cell(right_x, gy) else {
                    continue;
                };
                if right.material != MaterialId::Air {
                    continue;
                }
                let cap = water_capacity_with(MaterialId::Air, &hydro);
                let move_amt = sat_move_to_equalize_heads(
                    left.sat.0, cap, gy, right.sat.0, cap, gy,
                );
                if move_amt == 0 {
                    continue;
                }
                *local.entry((left_x, gy)).or_insert(0) -= move_amt;
                *local.entry((right_x, gy)).or_insert(0) += move_amt;
            }
        }
        local
    });
    for map in local {
        for (k, v) in map {
            *deltas.entry(k).or_insert(0) += v;
        }
    }
}
