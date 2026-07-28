//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Priority surface water flow (cascade, equalise, throughflow).

use std::collections::HashMap;

use wk_material::MaterialId;

use crate::active::{partition_checkerboard, ActiveChunk};
use crate::cell::{water_capacity, Cell, Sat};
use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::{map_regions_parallel};

use super::head::{
    is_porous_solid, plan_same_y_pairwise_edge,
    same_y_cascade_pull, seepage_rate,
};
use super::plan::regions_for_standalone;

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
///    level instead of terracing / checkerboarding.
/// 4. **Throughflow** — if below is a stack of saturated porous cells,
///    weep at seepage rate to the nearest opening: a **side Air face**
///    (cliff / spring) or Air below the stack.
///
/// Vertical bulk fall stays in [`apply_gravity_fall`] (pull-based).
/// Porous soak stays in [`apply_seepage`]. Mass is preserved by greedy
/// per-source distribution when multiple targets contend for one cell.
pub fn apply_water_flow(world: &mut World) {
    let regions = regions_for_standalone(world);
    apply_water_flow_regions(world, &regions);
}

/// Priority water flow restricted to a pre-planned active set.
pub fn apply_water_flow_regions(world: &mut World, active: &[ActiveChunk]) {
    if active.is_empty() {
        return;
    }
    let mut xfers: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
    for pass in partition_checkerboard(active) {
        accumulate_water_flow_xfers(world, &pass, &mut xfers);
    }

    if xfers.is_empty() {
        return;
    }

    // Greedy per-source distribution so total planned outflow never
    // exceeds a cell's sat (mass conservation with multiple targets).
    let mut by_source: HashMap<(i32, i32), Vec<usize>> = HashMap::new();
    for (i, (from, _, _)) in xfers.iter().enumerate() {
        by_source.entry(*from).or_default().push(i);
    }
    for (from, mut ixs) in by_source {
        let Some(src) = world.get_cell(from.0, from.1) else {
            continue;
        };
        let mut budget = src.sat.0 as i32;
        // Priority order was already baked in by push order; sort stable
        // by original index so priority 1 (diag-down) wins the budget.
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
    for (from, to, amt) in xfers {
        if amt <= 0 {
            continue;
        }
        let Some(src) = world.get_cell(from.0, from.1) else {
            continue;
        };
        let Some(dst) = world.get_cell(to.0, to.1) else {
            continue;
        };
        if src.material != MaterialId::Air || dst.material != MaterialId::Air {
            continue;
        }
        let free = u8::MAX as i32 - dst.sat.0 as i32;
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

fn accumulate_water_flow_xfers(
    world: &World,
    active: &[ActiveChunk],
    xfers: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    let tick_flip = (world.tick & 1) == 0;
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = world.wrap_x(ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32);
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cur.material != MaterialId::Air {
                    continue;
                }
                // Below tells us whether we're on a "surface" (below is
                // solid or full water) or falling (below is Air with room).
                let below_cell = world.get_cell(gx, gy - 1);
                let on_surface = match below_cell {
                    None => false,
                    Some(b) => b.material != MaterialId::Air || b.sat.is_full(),
                };

                // Dry standing Air still owns the +x equalise edge so a
                // wet neighbour can pour into it (otherwise wet→dry
                // never ran when the dry cell was the left endpoint).
                // Keep this — skipping dry cells regressed shelf cascade
                // / hill-drain feel in the water suite.
                if cur.sat.is_empty() {
                    if on_surface {
                        plan_same_y_pairwise_edge(world, gx, gy, &mut local);
                    }
                    continue;
                }

                let mut remaining = cur.sat.0 as i32;
                // Randomize L/R per cell so water doesn't bias one way.
                let flip = tick_flip ^ (((gx + gy) & 1) == 0);
                let dirs = if flip { [-1_i32, 1] } else { [1_i32, -1] };

                // --- Priority 1: diagonal-down into Air with room ---
                // Shelf edge: (dx, y-1) is Air, so water can fall there.
                for dx in dirs {
                    if remaining == 0 {
                        break;
                    }
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy - 1;
                    let Some(dst) = world.get_cell(nx, ny) else {
                        continue;
                    };
                    if dst.material != MaterialId::Air {
                        continue;
                    }
                    let free = u8::MAX.saturating_sub(dst.sat.0) as i32;
                    if free == 0 {
                        continue;
                    }
                    let move_amt = remaining.min(free);
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
                        let Some(side) = world.get_cell(nx, gy) else {
                            continue;
                        };
                        if side.material != MaterialId::Air {
                            continue;
                        }
                        let side_below = world.get_cell(nx, gy - 1);
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
                        let move_amt = remaining.min(free);
                        local.push(((gx, gy), (nx, gy), move_amt));
                        remaining -= move_amt;
                    }

                    // --- Priority 3a: same-Y cascade pull ---
                    // If a cascade outlet sits further along the surface
                    // band, push toward it so lake terraces fall into the
                    // lower reach.
                    for dx in dirs {
                        if remaining == 0 {
                            break;
                        }
                        let Some(want) =
                            same_y_cascade_pull(world, gx, gy, dx, cur.sat.0) else {
                            continue;
                        };
                        let tx = world.wrap_x(gx + dx);
                        let Some(side) = world.get_cell(tx, gy) else {
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
                }

                // --- Priority 3b: pairwise +x standing equalise ---
                // Always, even if cascade dumped everything — the edge
                // may still need the reverse transfer from a wetter +x.
                if on_surface {
                    plan_same_y_pairwise_edge(world, gx, gy, &mut local);
                }

                if remaining == 0 || !on_surface {
                    continue;
                }

                // --- Priority 4: throughflow through saturated porous ---
                // Real physics: water pressed on saturated soil flows
                // through it at seepage rate (Darcy). Exit at the first
                // opening: a side Air face (cliff / spring) or Air below
                // the stack — not only the bottom.
                let mut placed = false;
                for dx in [0_i32, -1, 1] {
                    if placed {
                        break;
                    }
                    let nx = world.wrap_x(gx + dx);
                    let Some(below1) = world.get_cell(nx, gy - 1) else {
                        continue;
                    };
                    if !is_porous_solid(below1.material) {
                        continue;
                    }
                    let cap1 = water_capacity(below1.material);
                    if below1.sat.0 < cap1 {
                        continue; // gravity + seepage handle unsaturated
                    }
                    let mut rate = seepage_rate(below1.material);
                    // Prefer the shallowest exit so mid-cliff springs beat
                    // a deep toe drain when both are open.
                    let mut best: Option<(i32, i32, i32)> = None; // depth, tx, ty
                    let mut depth = 1i32;
                    let mut ty = gy - 1;
                    for _ in 0..24 {
                        let Some(nb) = world.get_cell(nx, ty) else {
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
                        if !is_porous_solid(nb.material) {
                            break;
                        }
                        let cap = water_capacity(nb.material);
                        if nb.sat.0 < cap {
                            break;
                        }
                        rate = rate.min(seepage_rate(nb.material));
                        // Side springs: open Air beside this saturated cell.
                        for sdx in [-1_i32, 1] {
                            let sx = world.wrap_x(nx + sdx);
                            if sx == nx {
                                continue;
                            }
                            let Some(side) = world.get_cell(sx, ty) else {
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
                    }
                    if let Some((_d, tx, ty)) = best {
                        let amt = rate.min(remaining).max(1);
                        local.push(((gx, gy), (tx, ty), amt));
                        placed = true;
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
