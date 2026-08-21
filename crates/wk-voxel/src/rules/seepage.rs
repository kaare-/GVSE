//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Permeability-limited pore soak.

use wk_material::MaterialId;

use crate::active::ActiveChunk;
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::map_regions_parallel;

use super::head::{
    is_porous_solid_with, sat_move_to_equalize_heads, seepage_conduct_rate_with, seepage_rate_with,
    seepage_uptake_rate_with,
};
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
/// Single compute-then-apply scan (same snapshot rule as spill / flow —
/// checkerboard is not required for a read-only accumulate).
pub fn apply_seepage_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    // (from, to, amt) with amt > 0.
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    accumulate_seepage_xfers(world, active, &mut xfers);

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
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        // Chunk-local reads — same pattern as water_flow (~10× vs HashMap).
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
                let a = chunk.get(x as usize, y as usize);
                let cap_a = water_capacity_with(a.material, &hydro);
                if cap_a == 0 {
                    continue;
                }
                let a_solid = is_porous_solid_with(a.material, &hydro);
                // Air–Air / impermeable–impermeable edges are no-ops —
                // check materials before head math. Dominates rainy
                // ocean shore halos.
                for (dx, dy) in OFFSETS {
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy + dy;
                    if dx != 0 && nx == gx {
                        continue;
                    }
                    let Some(b) = read(lx + dx, ly + dy, nx, ny) else {
                        continue;
                    };
                    let b_solid = is_porous_solid_with(b.material, &hydro);
                    if !a_solid && !b_solid {
                        continue;
                    }
                    let cap_b = water_capacity_with(b.material, &hydro);
                    if cap_b == 0 {
                        continue;
                    }
                    let mut move_amt = sat_move_to_equalize_heads(
                        a.sat.0, cap_a, gy, b.sat.0, cap_b, ny,
                    );
                    // A persistent water column keeps infiltrating its bed
                    // toward pore capacity. The pairwise head formula alone
                    // stalls partially wet (~1/3 full in worldgen lakes).
                    // Keep this in seepage so a moving deep surge only wets
                    // the bed at the material permeability rate; gravity must
                    // never empty the whole pore capacity in one pull.
                    if dy == 1
                        && a_solid
                        && b.material == MaterialId::Air
                        && !b.sat.is_empty()
                        && matches!(
                            read(lx + dx, ly + dy + 1, nx, ny + 1),
                            Some(above)
                                if above.material == MaterialId::Air && above.sat.0 >= 160
                        )
                    {
                        let free = cap_a.saturating_sub(a.sat.0) as i32;
                        move_amt = -(b.sat.0 as i32).min(free);
                    }
                    if move_amt == 0 {
                        continue;
                    }
                    let rate = if a_solid && b_solid {
                        // Peer pores: drier side limits conduction.
                        seepage_conduct_rate_with(
                            a.material, &hydro, a.sat.0, cap_a, b.material, b.sat.0, cap_b,
                        )
                    } else if move_amt > 0 {
                        // A → B: infiltrating into B, or A weeping into Air.
                        if b_solid {
                            seepage_uptake_rate_with(b.material, &hydro, b.sat.0, cap_b)
                        } else {
                            seepage_rate_with(a.material, &hydro)
                        }
                    } else {
                        // B → A: infiltrating into A, or B weeping into Air.
                        if a_solid {
                            seepage_uptake_rate_with(a.material, &hydro, a.sat.0, cap_a)
                        } else {
                            seepage_rate_with(b.material, &hydro)
                        }
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
                    // A column face is not a bucket. Lateral Air→solid
                    // (and a flowing sheet on a bed) only splash-wet.
                    let rate = {
                        let air_solid = a_solid != b_solid;
                        if !air_solid {
                            rate
                        } else if dx != 0 {
                            rate.min(SHEET_FACE_SPLASH)
                        } else {
                            let air = if a_solid { &b } else { &a };
                            let air_lx = if a_solid { lx + dx } else { lx };
                            let air_gx = if a_solid { nx } else { gx };
                            let air_gy = if a_solid { ny } else { gy };
                            let air_ly = if a_solid { ly + dy } else { ly };
                            if air.material == MaterialId::Air
                                && air_has_dry_escape(
                                    world, &read, air_gx, air_gy, air_lx, air_ly,
                                )
                            {
                                rate.min(SHEET_FACE_SPLASH)
                            } else {
                                rate
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
