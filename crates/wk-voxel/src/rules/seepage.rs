//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Permeability-limited pore soak.

use crate::active::{partition_checkerboard, ActiveChunk};
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::{map_regions_parallel};

use super::head::{is_porous_solid_with, sat_move_to_equalize_heads, seepage_rate_with};
use super::plan::regions_for_standalone;

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
/// Checkerboard scan + single apply (same snapshot rule as spill).
pub fn apply_seepage_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    // (from, to, amt) with amt > 0.
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    for pass in partition_checkerboard(active) {
        accumulate_seepage_xfers(world, &pass, &mut xfers);
    }

    // Apply in a stable order. Each transfer re-reads live sat so a
    // source drained by an earlier xfer simply sends less — every
    // individual move conserves mass exactly.
    xfers.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then(a.1.cmp(&b.1))
            .then(a.2.cmp(&b.2))
    });
    for (from, to, amt) in xfers {
        let Some(src) = world.get_cell(from.0, from.1) else {
            continue;
        };
        let Some(dst) = world.get_cell(to.0, to.1) else {
            continue;
        };
        let cap_dst = water_capacity_with(dst.material, &world.hydro) as i32;
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
    }
}

fn accumulate_seepage_xfers(
    world: &World,
    active: &[ActiveChunk],
    xfers: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    const OFFSETS: [(i32, i32); 2] = [(1, 0), (0, 1)];
    let hydro = world.hydro;
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = world.wrap_x(ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32);
                let Some(a) = world.get_cell(gx, gy) else {
                    continue;
                };
                let cap_a = water_capacity_with(a.material, &hydro);
                if cap_a == 0 {
                    continue;
                }
                for (dx, dy) in OFFSETS {
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy + dy;
                    if dx != 0 && nx == gx {
                        continue;
                    }
                    let Some(b) = world.get_cell(nx, ny) else {
                        continue;
                    };
                    let cap_b = water_capacity_with(b.material, &hydro);
                    if cap_b == 0 {
                        continue;
                    }
                    let a_solid = is_porous_solid_with(a.material, &hydro);
                    let b_solid = is_porous_solid_with(b.material, &hydro);
                    if !a_solid && !b_solid {
                        continue;
                    }
                    let move_amt = sat_move_to_equalize_heads(
                        a.sat.0, cap_a, gy, b.sat.0, cap_b, ny,
                    );
                    if move_amt == 0 {
                        continue;
                    }
                    let rate = if a_solid && b_solid {
                        seepage_rate_with(a.material, &hydro).min(seepage_rate_with(b.material, &hydro))
                    } else if a_solid {
                        seepage_rate_with(a.material, &hydro)
                    } else {
                        seepage_rate_with(b.material, &hydro)
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
