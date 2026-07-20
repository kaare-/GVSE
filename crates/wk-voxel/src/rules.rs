//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cellular-automaton rules.
//!
//! One rule per tick sub-pass. Rules always read the world at the
//! start of the pass and write cells via [`World::set_cell`] so
//! chunk dirty rectangles stay coherent for whatever runs next.

use std::collections::HashMap;

use wk_material::MaterialId;

use crate::active::{
    clear_all_dirty, partition_checkerboard, plan_active, ActiveChunk,
};
use crate::cell::{is_grain, water_capacity, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::{self, for_each_region_parallel, map_regions_parallel};

/// Resolve the scan plan for a standalone rule call. Uses current
/// dirty rects; if nothing is dirty, falls back to a full-world scan
/// so unit tests that forget an intermediate dirty still work when
/// the chunk exists. Prefer [`tick`], which plans once and clears.
fn regions_for_standalone(world: &World) -> Vec<ActiveChunk> {
    let planned = plan_active(world);
    if !planned.is_empty() {
        return planned;
    }
    // Full scan fallback — only loaded chunks.
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: crate::chunk::Rect::full(),
        })
        .collect()
}

/// Free-surface / pore hydraulic head in cell units:
/// `y + sat / capacity`. Adjacent cells equalise toward matching heads.
pub fn hydraulic_head(gy: i32, sat: Sat, capacity: u8) -> f32 {
    if capacity == 0 {
        return gy as f32;
    }
    gy as f32 + (sat.0 as f32) / (capacity as f32)
}

/// Saturation to move from A → B (positive) or B → A (negative) so the
/// pair's heads meet in the middle. Clamped to available sat / free
/// capacity. Both capacities must be > 0.
fn sat_move_to_equalize_heads(
    sat_a: u8,
    cap_a: u8,
    gy_a: i32,
    sat_b: u8,
    cap_b: u8,
    gy_b: i32,
) -> i32 {
    if cap_a == 0 || cap_b == 0 {
        return 0;
    }
    let ca = cap_a as f32;
    let cb = cap_b as f32;
    let dh = hydraulic_head(gy_a, Sat(sat_a), cap_a) - hydraulic_head(gy_b, Sat(sat_b), cap_b);
    if dh.abs() < 1e-6 {
        return 0;
    }
    // Full pairwise equalisation: m · (1/ca + 1/cb) = dh.
    // Truncate toward zero so 127.5 → 127 (matches integer half-gap
    // for equal-cap Air and avoids creating a sat unit when two
    // neighbours both drain one cell).
    let m_f = dh * ca * cb / (ca + cb);
    let m = if m_f >= 0.0 {
        m_f.floor() as i32
    } else {
        m_f.ceil() as i32
    };
    if m > 0 {
        let free_b = cap_b as i32 - sat_b as i32;
        m.min(sat_a as i32).min(free_b.max(0))
    } else {
        let free_a = cap_a as i32 - sat_a as i32;
        let mag = (-m).min(sat_b as i32).min(free_a.max(0));
        -mag
    }
}

/// Max sat transferred through a porous solid per seepage step,
/// scaled by [`wk_material::MaterialProps::permeability`].
fn seepage_rate(material: MaterialId) -> i32 {
    use wk_material::MaterialRegistry;
    let p = MaterialRegistry::props(material).permeability;
    if p == 0 {
        return 0;
    }
    // Cap at 32 sat-units/tick at permeability 255 (gravel-ish).
    ((p as i32 * 32) / 255).max(1)
}

fn is_porous_solid(material: MaterialId) -> bool {
    material != MaterialId::Air && water_capacity(material) > 0
}

/// How far same-Y lake equalise looks for a drier surface cell / edge.
const SAME_Y_SURFACE_SCAN: i32 = 12;

/// True when the cell below can support a standing free surface
/// (solid ground or a full water column).
fn is_surface_support(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy - 1) {
        Some(b) if b.material != MaterialId::Air => true,
        Some(b) => b.sat.is_full(),
        None => false,
    }
}

/// Plan a single +x standing-surface head equalise for the edge
/// `(gx,gy) — (gx+1,gy)`. Emits at most one transfer, owned by the
/// left endpoint so each edge is solved once per pass.
fn plan_same_y_pairwise_edge(
    world: &World,
    gx: i32,
    gy: i32,
    local: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    let nx = world.wrap_x(gx + 1);
    if nx == gx {
        return;
    }
    if !is_surface_support(world, gx, gy) || !is_surface_support(world, nx, gy) {
        return;
    }
    let Some(left) = world.get_cell(gx, gy) else {
        return;
    };
    let Some(right) = world.get_cell(nx, gy) else {
        return;
    };
    if left.material != MaterialId::Air || right.material != MaterialId::Air {
        return;
    }
    let cap = water_capacity(MaterialId::Air);
    let move_amt = sat_move_to_equalize_heads(left.sat.0, cap, gy, right.sat.0, cap, gy);
    if move_amt > 0 {
        let free = u8::MAX.saturating_sub(right.sat.0) as i32;
        let amt = move_amt.min(left.sat.0 as i32).min(free);
        if amt > 0 {
            local.push(((gx, gy), (nx, gy), amt));
        }
    } else if move_amt < 0 {
        let free = u8::MAX.saturating_sub(left.sat.0) as i32;
        let amt = (-move_amt).min(right.sat.0 as i32).min(free);
        if amt > 0 {
            local.push(((nx, gy), (gx, gy), amt));
        }
    }
}

/// If a cascade outlet lies on the same-Y surface band in direction
/// `dir`, return how much sat to push into the immediate neighbour
/// (steering the free surface toward that outlet). Immediate cascade
/// edges are already handled by priority 2; this extends the pull
/// across up to [`SAME_Y_SURFACE_SCAN`] standing cells.
fn same_y_cascade_pull(
    world: &World,
    gx: i32,
    gy: i32,
    dir: i32,
    cur_sat: u8,
) -> Option<i32> {
    let immediate = world.wrap_x(gx + dir);
    let Some(side) = world.get_cell(immediate, gy) else {
        return None;
    };
    if side.material != MaterialId::Air {
        return None;
    }
    let free_imm = u8::MAX.saturating_sub(side.sat.0) as i32;
    if free_imm == 0 {
        return None;
    }

    // Immediate cascade is priority 2 — skip duplicate dump here.
    let immediate_cascade = matches!(
        world.get_cell(immediate, gy - 1),
        Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
    );
    if immediate_cascade {
        return None;
    }

    let mut x = immediate;
    for _ in 1..SAME_Y_SURFACE_SCAN {
        x = world.wrap_x(x + dir);
        if x == gx {
            break;
        }
        let Some(cell) = world.get_cell(x, gy) else {
            break;
        };
        if cell.material != MaterialId::Air {
            break;
        }
        if !is_surface_support(world, x, gy) {
            if matches!(
                world.get_cell(x, gy - 1),
                Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
            ) {
                return Some(free_imm.min(cur_sat as i32));
            }
            break;
        }
    }
    None
}

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
pub fn apply_gravity_fall_regions(world: &mut World, active: &[ActiveChunk]) {
    for_each_region_parallel(world, active, |ptrs, wrap_width, ac| {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                // SAFETY: ptrs cover this region's pull write-set; see
                // [`crate::parallel`].
                let Some(cur) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy) }) else {
                    continue;
                };
                let cap = water_capacity(cur.material);
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
                if water_capacity(above.material) == 0 {
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

/// Immediate-neighbour priority water flow.
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
///    weep down through the whole stack at seepage rate to the first
///    Air with room on the far side.
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
                // through it at seepage rate (Darcy), reaching the far
                // side. Try straight down and diagonal-down columns.
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
                    let mut ty = gy - 2;
                    let mut rate = seepage_rate(below1.material);
                    let mut target: Option<i32> = None;
                    for _ in 0..24 {
                        let Some(nb) = world.get_cell(nx, ty) else {
                            break;
                        };
                        if nb.material == MaterialId::Air {
                            if u8::MAX.saturating_sub(nb.sat.0) > 0 {
                                target = Some(ty);
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
                        ty -= 1;
                    }
                    if let Some(ny) = target {
                        let amt = rate.min(remaining).max(1);
                        local.push(((gx, gy), (nx, ny), amt));
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
        let cap = water_capacity(cell.material) as i32;
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
                let cap = water_capacity(MaterialId::Air);
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
        let cap_dst = water_capacity(dst.material) as i32;
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
    let local = map_regions_parallel(active, |ac| {
        let mut local: Vec<((i32, i32), (i32, i32), i32)> = Vec::new();
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = world.wrap_x(ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32);
                let Some(a) = world.get_cell(gx, gy) else {
                    continue;
                };
                let cap_a = water_capacity(a.material);
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
                    let cap_b = water_capacity(b.material);
                    if cap_b == 0 {
                        continue;
                    }
                    let a_solid = is_porous_solid(a.material);
                    let b_solid = is_porous_solid(b.material);
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
                        seepage_rate(a.material).min(seepage_rate(b.material))
                    } else if a_solid {
                        seepage_rate(a.material)
                    } else {
                        seepage_rate(b.material)
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

/// One-cell-per-pass grain fall.
///
/// Each `Air` cell **pulls** a granular neighbour from directly above
/// (swap). Whatever water saturation the Air cell had rises into the
/// vacated upper cell, so a grain sinking through water displaces
/// exactly the water it walks through — mass is conserved.
///
/// Pull + bottom-up + checkerboard matches [`apply_gravity_fall`]:
/// one cell per invocation, including across chunk seams.
///
/// V1 kept simple: grains fall through Air *any* saturation and stop
/// on anything else. Density-ordered stacking between grain species
/// (heavy sinks under light) and buoyancy interactions with less-
/// dense fluids are follow-up rules.
pub fn apply_grain_fall(world: &mut World) {
    let regions = regions_for_standalone(world);
    for pass in partition_checkerboard(&regions) {
        apply_grain_fall_regions(world, &pass);
    }
}

/// Grain fall restricted to a pre-planned active set.
pub fn apply_grain_fall_regions(world: &mut World, active: &[ActiveChunk]) {
    for_each_region_parallel(world, active, |ptrs, wrap_width, ac| {
        for y in ac.rect.y0..=ac.rect.y1 {
            let gy = ac.coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in ac.rect.x0..=ac.rect.x1 {
                let gx = ac.coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                // SAFETY: see [`crate::parallel`].
                let Some(cur) = (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy) }) else {
                    continue;
                };
                if cur.material != MaterialId::Air {
                    continue;
                }
                let Some(above) =
                    (unsafe { parallel::get_cell(ptrs, wrap_width, gx, gy + 1) })
                else {
                    continue;
                };
                if !is_grain(above.material) {
                    continue;
                }
                unsafe {
                    parallel::set_cell(ptrs, wrap_width, gx, gy, above);
                    parallel::set_cell(ptrs, wrap_width, gx, gy + 1, cur);
                }
            }
        }
    });
}

/// Rain source parameters for [`apply_rain`].
#[derive(Debug, Clone, Copy)]
pub struct RainConfig {
    /// World-y row where droplets appear.
    pub top_y: i32,
    /// Inclusive `(x0, x1)` world-x range over which rain can fall.
    pub x_range: (i32, i32),
    /// Chance per column per tick of receiving a droplet.
    pub prob_per_col_per_tick: f32,
    /// Sat delta added per droplet (clamped so a cell can't exceed
    /// `u8::MAX`).
    pub droplet_sat: u8,
    /// Salt mixed into the per-column tick hash so callers can run
    /// multiple independent rain streams (mist vs storm) without
    /// them colliding.
    pub seed_salt: u64,
}

impl Default for RainConfig {
    fn default() -> Self {
        Self {
            top_y: 0,
            x_range: (0, 0),
            prob_per_col_per_tick: 0.02,
            droplet_sat: 64,
            seed_salt: 0xC10D,
        }
    }
}

/// Cheap deterministic 32-bit hash → f32 in `[0, 1)` — same mixer
/// used by [`crate::worldgen::continental_surface_y`].
fn hash_prob(seed: u64, gx: i32, tick_no: u64, salt: u64) -> f32 {
    let mut h = seed
        .wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(tick_no.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(gx as u64);
    h ^= h.wrapping_shr(30);
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h.wrapping_shr(27);
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h.wrapping_shr(31);
    (h as u32 as f32) / (u32::MAX as f32 + 1.0)
}

/// Inject climatic rain that **lands on the ground / ocean surface**
/// under each column (cosmetic sky streaks are drawn separately).
///
/// For each column `gx ∈ cfg.x_range`, roll a deterministic
/// pseudo-random probability seeded by `(world.seed, gx, world.tick,
/// cfg.seed_salt)`. When the roll passes `cfg.prob_per_col_per_tick`,
/// deposit `cfg.droplet_sat` via [`deposit_water_on_surface`] scanning
/// down from `cfg.top_y`.
///
/// Determinism: same world.seed + same tick + same config = same
/// droplet placements.
pub fn apply_rain(world: &mut World, cfg: &RainConfig) {
    apply_rain_with_temp(world, cfg, None, None);
}

/// Climatic rain that becomes **snow** when `temp` is at/below freezing
/// and the column frozen budget has room (see [`crate::phase::deposit_precip_on_surface`]).
pub fn apply_rain_with_temp(
    world: &mut World,
    cfg: &RainConfig,
    temp: Option<&crate::temperature::Temperature>,
    phase: Option<&crate::phase::PhaseConfig>,
) {
    let (x0, x1) = cfg.x_range;
    if x0 > x1 {
        return;
    }
    let seed = world.seed.0;
    let tick_no = world.tick;
    for gx in x0..=x1 {
        let roll = hash_prob(seed, gx, tick_no, cfg.seed_salt);
        if roll >= cfg.prob_per_col_per_tick {
            continue;
        }
        let _ = crate::phase::deposit_precip_on_surface(
            world,
            gx,
            cfg.top_y,
            cfg.droplet_sat as f32,
            temp,
            phase,
        );
    }
}

/// True when wet Air is a standing pool / ocean film / land puddle
/// (rests on solid or on near-full water below) — not a mid-air droplet.
pub fn is_standing_water(world: &World, gx: i32, gy: i32) -> bool {
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if cell.material != MaterialId::Air || cell.sat.is_empty() {
        return false;
    }
    match world.get_cell(gx, gy - 1) {
        Some(below) if below.material != MaterialId::Air => true,
        Some(below) => below.sat.0 >= 200,
        None => false,
    }
}

/// Deposit atmospheric water onto the free-air surface under `start_y`.
///
/// Lands just above solid ground or standing water. Deepens existing
/// water columns, but will **not** grow a one-cell film on bare rock
/// into a tall slope wedge (returns 0 when that film is already full
/// so runoff can clear the hillside first).
pub fn deposit_water_on_surface(world: &mut World, gx: i32, start_y: i32, budget: f32) -> f32 {
    if budget <= 0.0 {
        return 0.0;
    }
    let jx = world.wrap_x(gx);
    let mut y = start_y;
    let mut last_free_air_y: Option<i32> = None;
    for _ in 0..128 {
        let Some(cell) = world.get_cell(jx, y) else {
            y -= 1;
            continue;
        };
        if cell.material != MaterialId::Air {
            // Terrain — fill the open air we just left (directly above).
            // Do not spawn water above a solid pillar into empty sky.
            if let Some(ay) = last_free_air_y {
                if ay == y + 1 {
                    if let Some(ac) = world.get_cell(jx, ay) {
                        return fill_air_sat(world, jx, ay, ac, budget);
                    }
                }
            }
            return 0.0;
        }
        if cell.sat.is_full() {
            // Existing water column (wet over wet) may deepen upward.
            // A full film sitting on bare rock must not stack into a wedge.
            let below_is_water = matches!(
                world.get_cell(jx, y - 1),
                Some(b) if b.material == MaterialId::Air && b.sat.0 >= 200
            );
            if below_is_water {
                if let Some(ay) = last_free_air_y {
                    if let Some(ac) = world.get_cell(jx, ay) {
                        return fill_air_sat(world, jx, ay, ac, budget);
                    }
                }
            }
            return 0.0;
        }
        last_free_air_y = Some(y);
        y -= 1;
    }
    0.0
}

fn fill_air_sat(world: &mut World, gx: i32, gy: i32, cell: Cell, budget: f32) -> f32 {
    let free = u8::MAX as f32 - cell.sat.0 as f32;
    let transfer = budget.min(free).max(0.0);
    let u = transfer.round() as i32;
    if u <= 0 {
        return 0.0;
    }
    let new_sat = (cell.sat.0 as i32 + u).clamp(0, u8::MAX as i32) as u8;
    world.set_cell(
        gx,
        gy,
        Cell {
            sat: Sat(new_sat),
            ..cell
        },
    );
    u as f32
}

/// Surface-evaporation parameters for [`apply_evaporation`].
#[derive(Debug, Clone, Copy)]
pub struct EvapConfig {
    /// Sat removed per qualifying tick from each surface cell.
    pub rate_per_tick: u8,
    /// A cell only evaporates when the cell above it is `Air` with
    /// `sat ≤ dry_above_max`. That keeps sub-surface lake cells from
    /// evaporating — only the top exposed water layer loses mass.
    pub dry_above_max: u8,
    /// Only run on ticks where `world.tick % period_ticks == 0`.
    /// Higher values slow the water→humidity pump so basins linger.
    pub period_ticks: u64,
}

impl Default for EvapConfig {
    fn default() -> Self {
        Self {
            rate_per_tick: 1,
            dry_above_max: 200,
            period_ticks: 1,
        }
    }
}

/// Bleed sat out of **standing** surface water (ocean film, puddles).
///
/// A cell qualifies when:
/// - It's `Air` with `sat > 0`.
/// - The cell directly above is `Air` with `sat ≤ cfg.dry_above_max`
///   OR the above chunk isn't loaded (open sky).
/// - It rests on solid ground **or** on wetter standing water below
///   (so mid-air rain / falling droplets are not re-evaporated before
///   they can reach the ground).
///
/// Compute-then-apply so evap is order-independent.
pub fn apply_evaporation(world: &mut World, cfg: &EvapConfig) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let deltas = collect_evap_deltas(world, cfg);
    apply_evap_deltas(world, deltas, None);
}

/// Mass-conservative variant of [`apply_evaporation`]. Instead of
/// deleting sat, the removed mass is deposited into the supplied
/// [`crate::humidity::Humidity`] heatmap at the cell's tile.
pub fn apply_evaporation_into_humidity(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &EvapConfig,
) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let deltas = collect_evap_deltas(world, cfg);
    apply_evap_deltas(world, deltas, Some(humidity));
}

/// True when wet Air is a free surface of a pool / ocean / land film,
/// not a suspended rain droplet with empty sky below it.
fn rests_on_evap_surface(world: &World, gx: i32, gy: i32, cfg: &EvapConfig) -> bool {
    match world.get_cell(gx, gy - 1) {
        None => false,
        Some(below) if below.material != MaterialId::Air => true,
        Some(below) => below.sat.0 > cfg.dry_above_max,
    }
}

fn collect_evap_deltas(world: &mut World, cfg: &EvapConfig) -> HashMap<(i32, i32), i32> {
    let mut deltas: HashMap<(i32, i32), i32> = HashMap::new();
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    for coord in coords {
        let mut still_wet = false;
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cur.material != MaterialId::Air || cur.sat.is_empty() {
                    continue;
                }
                still_wet = true;
                let sky_above = match world.get_cell(gx, gy + 1) {
                    None => true, // above chunk absent → open sky
                    Some(above) => {
                        above.material == MaterialId::Air && above.sat.0 <= cfg.dry_above_max
                    }
                };
                if !sky_above || !rests_on_evap_surface(world, gx, gy, cfg) {
                    continue;
                }
                let mut rate = cfg.rate_per_tick as i32;
                // Orphaned crest film: no Air neighbour anywhere on the
                // surface (same-y or diagonal-down) → evaporate hard so
                // a single ridge pixel doesn't linger for hours.
                if is_orphan_surface_film(world, gx, gy) {
                    rate = (rate * 8).max(4);
                }
                *deltas.entry((gx, gy)).or_insert(0) -= rate;
            }
        }
        if !still_wet {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                chunk.has_wet_air = false;
            }
        }
    }
    deltas
}

/// True when a wet Air cell on solid has no Air neighbour on any of the
/// six surface directions — nothing lateral flow can couple it to.
fn is_orphan_surface_film(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(-1_i32, 0), (1, 0), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if matches!(
            world.get_cell(nx, ny),
            Some(c) if c.material == MaterialId::Air
        ) {
            return false;
        }
    }
    true
}

fn apply_evap_deltas(
    world: &mut World,
    deltas: HashMap<(i32, i32), i32>,
    mut humidity: Option<&mut crate::humidity::Humidity>,
) {
    for ((gx, gy), delta) in deltas {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        let cap = water_capacity(cell.material) as i32;
        let new_sat = (cell.sat.0 as i32 + delta).clamp(0, cap);
        let actually_removed = cell.sat.0 as i32 - new_sat;
        if actually_removed > 0 {
            if let Some(h) = humidity.as_deref_mut() {
                h.add(gx, gy, actually_removed as f32);
            }
        }
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

/// Condensation-rain parameters for [`apply_condensation_rain`].
///
/// The "cloud row" `top_y` is where droplets appear when a humidity
/// tile is wet enough to precipitate. Rain empties a bounded mass
/// from the tile, and the droplet's sat is proportional to the mass
/// removed (clamped by [`u8::MAX`]).
#[derive(Debug, Clone, Copy)]
pub struct CondensationConfig {
    /// World-y row where droplets condense.
    pub top_y: i32,
    /// A tile only rains when its humidity mass is at or above this.
    /// Prevents faint air moisture from immediately raining back.
    pub min_mass_to_rain: f32,
    /// Chance-per-tick that a *saturated* tile rains at all. Actual
    /// per-tick probability scales linearly from 0 at `min_mass_to_rain`
    /// up to `max_prob_per_tick` at `full_mass`.
    pub max_prob_per_tick: f32,
    /// Humidity mass at which precipitation rate hits its cap.
    pub full_mass: f32,
    /// Mass removed from a tile per rain event.
    pub mass_per_droplet: f32,
    /// Salt mixed into the per-tile tick hash.
    pub seed_salt: u64,
}

impl Default for CondensationConfig {
    fn default() -> Self {
        Self {
            top_y: 0,
            min_mass_to_rain: 64.0,
            max_prob_per_tick: 0.4,
            full_mass: 512.0,
            mass_per_droplet: 96.0,
            seed_salt: 0xC10D_BA5E,
        }
    }
}

/// Orographic rain boost — moist air dumps when climbing tall land.
#[derive(Debug, Clone, Copy)]
pub struct OrographicConfig {
    pub seed: u64,
    pub width_cols: i32,
    pub sea_level_y: i32,
    /// Surface must sit at least this many cells above sea to count
    /// as "tall" for forced release.
    pub tall_above_sea: i32,
    /// Ascent (cells) that reaches full probability multiplier.
    pub ascent_scale: f32,
    /// Max multiplier on rain probability when climbing hard.
    pub max_prob_mult: f32,
    /// Extra mass drained per event on a strong orographic hit.
    pub mass_mult: f32,
    /// Prevailing wind sign (+1 = +x) for upwind surface sampling.
    pub wind_sign: i32,
}

impl Default for OrographicConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            width_cols: 256,
            sea_level_y: 0,
            tall_above_sea: 22,
            ascent_scale: 35.0,
            max_prob_mult: 3.0,
            mass_mult: 1.6,
            wind_sign: 1,
        }
    }
}

/// Precipitation feedback: humidity tiles that hold enough
/// atmospheric water probabilistically drop droplets back into the
/// cell grid, draining the tile as they do.
///
/// Rain lands at the tile's centre column, in the cell at
/// `cfg.top_y` — provided that cell is currently `Air`. Sat and
/// tile mass are both bounded so the pass can't create or lose
/// mass beyond what's actually available.
///
/// Deterministic given `(world.seed, tile_coord, world.tick,
/// cfg.seed_salt)`.
pub fn apply_condensation_rain(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &CondensationConfig,
) {
    apply_condensation_rain_with_orographic(world, humidity, cfg, None);
}

/// Like [`apply_condensation_rain`], but moist tiles over tall /
/// upslope terrain rain more readily (orographic dump).
pub fn apply_condensation_rain_with_orographic(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &CondensationConfig,
    oro: Option<&OrographicConfig>,
) {
    apply_condensation_rain_phased(world, humidity, cfg, oro, None, None);
}

/// Condensation precip with optional snow phase (cold tiles).
pub fn apply_condensation_rain_phased(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &CondensationConfig,
    oro: Option<&OrographicConfig>,
    temp: Option<&crate::temperature::Temperature>,
    phase: Option<&crate::phase::PhaseConfig>,
) {
    if cfg.min_mass_to_rain >= cfg.full_mass || cfg.max_prob_per_tick <= 0.0 {
        return;
    }
    let seed = world.seed.0;
    let tick_no = world.tick;
    let tile_cols = humidity.tile_cols;
    // Snapshot tile keys so we can mutate humidity as we go.
    let tiles: Vec<(i32, i32)> = humidity.cells.keys().copied().collect();
    for (hx, hy) in tiles {
        let mass = humidity.at_tile(hx, hy);
        let (prob_mult, mass_mult, min_mass) = match oro {
            Some(o) => orographic_factors(o, hx, tile_cols, cfg.min_mass_to_rain),
            None => (1.0, 1.0, cfg.min_mass_to_rain),
        };
        if mass < min_mass {
            continue;
        }
        // Linear scale from 0 at min_mass to max at full_mass.
        let t = ((mass - min_mass) / (cfg.full_mass - min_mass)).clamp(0.0, 1.0);
        let effective_prob = (cfg.max_prob_per_tick * t * prob_mult).clamp(0.0, 0.95);
        // Hash uses tile coord + tick + salt for per-tile determinism.
        let roll = hash_prob(
            seed,
            hx.wrapping_mul(73_856_093).wrapping_add(hy),
            tick_no,
            cfg.seed_salt,
        );
        if roll >= effective_prob {
            continue;
        }
        // Rain lands on the ground / ocean under the tile centre.
        let centre_gx = hx * tile_cols + tile_cols / 2;
        let take_mass = (cfg.mass_per_droplet * mass_mult).min(mass);
        if take_mass <= 0.0 {
            continue;
        }
        // Cold snow wants a full cell (255); offer at least that when the
        // tile can pay so mountain drizzle becomes pack, not pore water.
        let offer = match (temp, phase) {
            (Some(_), Some(ph)) => take_mass.max(ph.min_budget_to_snow).min(mass),
            _ => take_mass,
        };
        let landed = crate::phase::deposit_precip_on_surface(
            world,
            centre_gx,
            cfg.top_y,
            offer,
            temp,
            phase,
        );
        if landed <= 0.0 {
            continue;
        }
        // Drain the humidity tile by the mass that landed (clamp to tile).
        let entry = humidity.cells.entry((hx, hy)).or_insert(0.0);
        *entry -= landed.min(*entry);
        if *entry < 1e-6 {
            humidity.cells.remove(&(hx, hy));
        }
    }
}

fn orographic_factors(
    oro: &OrographicConfig,
    hx: i32,
    tile_cols: i32,
    base_min_mass: f32,
) -> (f32, f32, f32) {
    use crate::worldgen::continental_surface_y;
    let tc = tile_cols.max(1);
    let gx = hx * tc + tc / 2;
    let sign = if oro.wind_sign >= 0 { 1 } else { -1 };
    let gx_up = gx - sign * tc;
    let s_here = continental_surface_y(oro.seed, gx, oro.sea_level_y, oro.width_cols);
    let s_up = continental_surface_y(oro.seed, gx_up, oro.sea_level_y, oro.width_cols);
    let ascent = (s_here - s_up) as f32;
    let tall = s_here >= oro.sea_level_y + oro.tall_above_sea;
    if !tall && ascent <= 2.0 {
        return (1.0, 1.0, base_min_mass);
    }
    let climb = (ascent / oro.ascent_scale.max(1.0)).clamp(0.0, 1.0);
    // Tall peaks dump readily even without a steep local climb —
    // moist air that makes it inland tends to rain out over high land.
    let tall_f = if tall { 0.65 } else { 0.0 };
    let strength = (climb * 0.7 + tall_f).clamp(0.0, 1.0);
    let prob_mult = 1.0 + strength * (oro.max_prob_mult - 1.0);
    let mass_mult = 1.0 + strength * (oro.mass_mult - 1.0);
    // Tall / climbing air rains from thinner clouds too.
    let min_mass = base_min_mass * (1.0 - 0.55 * strength);
    (prob_mult, mass_mult, min_mass)
}

/// Karst dissolution parameters for [`apply_karst_dissolution`].
#[derive(Debug, Clone, Copy)]
pub struct KarstConfig {
    /// Base probability per tick that a Limestone cell dissolves
    /// *per* wet neighbour it has. Effective probability is
    /// `min(1, prob_per_wet_neighbour × wet_count)`.
    pub prob_per_wet_neighbour: f32,
    /// A neighbouring Air cell counts as "wet" once its sat is at
    /// or above this threshold. Prevents faint rain droplets from
    /// dissolving whole cliffs.
    pub min_wet_neighbour_sat: u8,
    /// Salt mixed into the per-cell tick hash so callers can run
    /// different karst regimes side-by-side.
    pub seed_salt: u64,
}

impl Default for KarstConfig {
    fn default() -> Self {
        // Tuned so a limestone body under constant water exposure
        // dissolves visibly over a few thousand ticks — game-scale,
        // not real karst-formation-scale.
        Self {
            prob_per_wet_neighbour: 0.001,
            min_wet_neighbour_sat: 200,
            seed_salt: 0xCAFE_D155_01F0_D000_u64,
        }
    }
}

/// Karst dissolution: Limestone cells with wet Air neighbours
/// probabilistically dissolve into Air, freeing their pore
/// saturation into the new Air cell.
///
/// Deterministic given `(world.seed, gx, gy, world.tick,
/// cfg.seed_salt)`.
///
/// Compute-then-apply so the sweep order doesn't affect the outcome.
///
/// Chunks without [`Chunk::has_limestone`] are skipped; the flag is
/// sticky on write and cleared here when a scan finds no limestone
/// left (empty sky / pure-stone slabs stay cheap).
pub fn apply_karst_dissolution(world: &mut World, cfg: &KarstConfig) {
    let mut converts: Vec<(i32, i32, Cell)> = Vec::new();
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_limestone)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    let seed = world.seed.0;
    let tick_no = world.tick;

    for coord in coords {
        let mut still_lime = false;
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cur.material != MaterialId::Limestone {
                    continue;
                }
                still_lime = true;
                // Count wet Air neighbours (4-connected).
                let mut wet = 0u32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if let Some(n) = world.get_cell(gx + dx, gy + dy) {
                        if n.material == MaterialId::Air && n.sat.0 >= cfg.min_wet_neighbour_sat {
                            wet += 1;
                        }
                    }
                }
                if wet == 0 {
                    continue;
                }
                let effective_prob =
                    (cfg.prob_per_wet_neighbour * wet as f32).clamp(0.0, 1.0);
                // Bake gy into the hash so cells at different y
                // levels get independent rolls even though tick is
                // shared.
                let roll = hash_prob(
                    seed,
                    gx.wrapping_mul(73_856_093).wrapping_add(gy),
                    tick_no,
                    cfg.seed_salt,
                );
                if roll >= effective_prob {
                    continue;
                }
                // Dissolve — keep whatever pore water this cell held.
                converts.push((
                    gx,
                    gy,
                    Cell {
                        material: MaterialId::Air,
                        sat: cur.sat,
                        flags: cur.flags,
                        _pad: cur._pad,
                    },
                ));
            }
        }
        if !still_lime {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                chunk.has_limestone = false;
            }
        }
    }
    for (gx, gy, cell) in converts {
        world.set_cell(gx, gy, cell);
    }
}

/// How many gravity→surface-flow cycles run inside one [`tick`].
///
/// Several substeps with re-planned dirty halos let ponds level and
/// hill water drain at a liquid pace. On flat shelves where cascade
/// edges are 5-10 cells away, half-gap propagates at ~1 cell/substep,
/// so we need enough substeps to keep up with steady rain.
const FLOW_SUBSTEPS: usize = 12;

/// Advance the sim by one tick.
///
/// Runs the sub-passes in a fixed order:
///
/// 1. **Flow substeps** (×[`FLOW_SUBSTEPS`]): gravity fall, then
///    Air–Air hydraulic-head surface flow (horizontal + diagonal).
///    Each substep re-plans from dirty so water can advance several
///    cells per tick and seek a flat free surface on slopes.
/// 2. Seepage — water soaks into / through porous solids by head,
///    rate-limited by permeability.
/// 3. Grain fall — granular materials sink into the Air cell below.
///
/// Rain and evaporation are **opt-in**: callers wire
/// [`apply_rain`] and [`apply_evaporation`] into their per-frame
/// loop themselves. Scenario tests pass `tick(world)` alone and
/// stay deterministic without weather.
///
/// **Dirty / active chunks.** Each flow substep [`plan_active`]s from
/// dirty rects (halo + neighbour wake), then [`clear_all_dirty`].
/// Writes rebuild dirty for the next substep / tick. A fully settled
/// world plans nothing and the physics passes early-out.
///
/// **Checkerboard.** Gravity and grain run four colour sub-passes
/// (EE → OE → EO → OO); within a colour, regions run on rayon when
/// enabled. Surface flow and seepage scan the same partition (also
/// parallel per colour) but apply from one snapshot so edges are not
/// re-solved mid-rule.
pub fn tick(world: &mut World) {
    for _ in 0..FLOW_SUBSTEPS {
        let active = plan_active(world);
        clear_all_dirty(world);
        if active.is_empty() {
            break;
        }
        let passes = partition_checkerboard(&active);
        for pass in &passes {
            apply_gravity_fall_regions(world, pass);
        }
        apply_water_flow_regions(world, &active);
    }

    // Seepage + grain fall read the same dirty halo the substep loop
    // built. Do NOT clear dirty here: if these passes don't write
    // (e.g. no porous solids, no grains), we still need next tick to
    // re-process the cells the substeps just modified.
    let active = plan_active(world);
    if !active.is_empty() {
        apply_seepage_regions(world, &active);
        let passes = partition_checkerboard(&active);
        for pass in &passes {
            apply_grain_fall_regions(world, pass);
        }
    }

    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use wk_material::MaterialId;

    fn setup_column_world() -> World {
        // One chunk. Row y=0 is a solid Bedrock floor; every other
        // cell is Air (empty).
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..(CHUNK_CELLS_W as i32) {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    #[test]
    fn droplet_falls_one_cell_per_pass() {
        let mut w = setup_column_world();
        w.set_cell(4, 10, Cell::water());
        assert!(w.get_cell(4, 10).unwrap().sat.is_full());
        assert!(w.get_cell(4, 9).unwrap().sat.is_empty());

        apply_gravity_fall(&mut w);
        assert!(w.get_cell(4, 10).unwrap().sat.is_empty());
        assert!(w.get_cell(4, 9).unwrap().sat.is_full());
    }

    #[test]
    fn droplet_stops_on_bedrock() {
        let mut w = setup_column_world();
        w.set_cell(2, 1, Cell::water());
        apply_gravity_fall(&mut w);
        // Bedrock capacity is 0 — no move.
        assert!(w.get_cell(2, 1).unwrap().sat.is_full());
        assert!(w
            .get_cell(2, 0)
            .unwrap()
            .sat
            .is_empty());
    }

    #[test]
    fn resting_column_does_not_compress() {
        let mut w = setup_column_world();
        // Water in y=1..4 (four cells), solid bedrock at y=0.
        for y in 1..=4 {
            w.set_cell(2, y, Cell::water());
        }
        apply_gravity_fall(&mut w);
        // All four cells should still be full — each already sits on
        // full water or bedrock and has nowhere to go.
        for y in 1..=4 {
            assert!(
                w.get_cell(2, y).unwrap().sat.is_full(),
                "y={y} lost water"
            );
        }
    }

    #[test]
    fn lake_bed_sand_wets_clay_and_stone_below_via_tick() {
        // Lake water on sand over clay over stone. Downward pore soak
        // must reach every porous layer (gravity + seepage), not stop
        // at the sand cap.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Contain laterally so free water can't run off the column.
        for x in 3..=5 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=6 {
            w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
            w.set_cell(5, y, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 2, Cell::solid(MaterialId::Clay));
        w.set_cell(4, 3, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 4, Cell::water());
        w.set_cell(4, 5, Cell::water());

        for _ in 0..30 {
            tick(&mut w);
        }

        let sand = w.get_cell(4, 3).unwrap();
        let clay = w.get_cell(4, 2).unwrap();
        let stone = w.get_cell(4, 1).unwrap();
        let sand_cap = water_capacity(MaterialId::Sand);
        let clay_cap = water_capacity(MaterialId::Clay);
        let stone_cap = water_capacity(MaterialId::Stone);
        assert_eq!(sand.sat.0, sand_cap, "sand should saturate");
        assert_eq!(clay.sat.0, clay_cap, "clay under sand must saturate");
        assert_eq!(stone.sat.0, stone_cap, "stone under clay must saturate");
    }

    #[test]
    fn deep_stone_stack_keeps_wetting_after_surface_quiesces() {
        // Reproduce the lake-bed report: sand saturates quickly, then
        // deeper porous stone must keep taking water over many ticks
        // even after the free-surface looks settled.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 3..=5 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=20 {
            w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
            w.set_cell(5, y, Cell::solid(MaterialId::Bedrock));
        }
        for y in 1..=16 {
            w.set_cell(4, y, Cell::solid(MaterialId::Stone));
        }
        w.set_cell(4, 17, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 18, Cell::solid(MaterialId::Sand));
        // Deep lake column above the bed.
        for y in 19..=22 {
            w.set_cell(4, y, Cell::water());
        }

        // After a few ticks the sand cap is wet; the deep stone must
        // not be left dry because dirty planning went quiet.
        for _ in 0..8 {
            tick(&mut w);
        }
        let sand = w.get_cell(4, 18).unwrap().sat.0;
        let sand_cap = water_capacity(MaterialId::Sand);
        assert_eq!(sand, sand_cap, "sand cap should be saturated early");

        for _ in 0..40 {
            tick(&mut w);
        }
        let stone_cap = water_capacity(MaterialId::Stone);
        let deep = w.get_cell(4, 1).unwrap().sat.0;
        let mid = w.get_cell(4, 8).unwrap().sat.0;
        assert_eq!(
            mid, stone_cap,
            "mid-stack stone should saturate (mid={mid})"
        );
        assert_eq!(
            deep, stone_cap,
            "deep stone under the lake bed should saturate (deep={deep})"
        );
    }

    #[test]
    fn water_saturates_porous_solid_up_to_capacity() {
        let mut w = setup_column_world();
        // Sand cell sits above bedrock at y=1; water above at y=2.
        w.set_cell(3, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(3, 2, Cell::water());

        // One pass: as much as fits transfers into the sand up to its
        // porosity capacity.
        apply_gravity_fall(&mut w);
        let sand = w.get_cell(3, 1).unwrap();
        let above = w.get_cell(3, 2).unwrap();
        let sand_cap = water_capacity(MaterialId::Sand);
        assert_eq!(sand.sat.0, sand_cap);
        assert_eq!(above.sat.0, u8::MAX - sand_cap);

        // A second pass: sand is at capacity → no more water moves in.
        apply_gravity_fall(&mut w);
        let sand2 = w.get_cell(3, 1).unwrap();
        let above2 = w.get_cell(3, 2).unwrap();
        assert_eq!(sand2.sat.0, sand_cap);
        assert_eq!(above2.sat.0, u8::MAX - sand_cap);
    }

    #[test]
    fn does_not_leak_through_stone() {
        // Stone porosity is small but > 0. Ensure the pass never over-fills
        // and no water disappears.
        let mut w = World::new(2);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..(CHUNK_CELLS_W as i32) {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 2, Cell::water());
        let cap = water_capacity(MaterialId::Stone);
        let start_mass: i32 =
            w.get_cell(5, 2).unwrap().sat.0 as i32 + w.get_cell(5, 1).unwrap().sat.0 as i32;

        apply_gravity_fall(&mut w);

        let stone = w.get_cell(5, 1).unwrap();
        let above = w.get_cell(5, 2).unwrap();
        assert_eq!(stone.sat.0, cap);
        assert_eq!(above.sat.0 as i32 + stone.sat.0 as i32, start_mass);
    }

    #[test]
    fn droplet_falls_across_chunk_boundary() {
        // Chunk (0, 1) at y=64..127; chunk (0, 0) at y=0..63.
        // Drop a water cell at gy=64 (bottom row of chunk (0,1)),
        // expect it in gy=63 (top row of chunk (0,0)) after one pass.
        let mut w = World::new(3);
        // Instantiate both chunks so `get_cell` returns Some for both
        // sides of the seam.
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.set_cell(7, 64, Cell::water());
        assert!(w.get_cell(7, 64).unwrap().sat.is_full());
        assert!(w.get_cell(7, 63).unwrap().sat.is_empty());

        apply_gravity_fall(&mut w);

        assert!(w.get_cell(7, 64).unwrap().sat.is_empty());
        assert!(w.get_cell(7, 63).unwrap().sat.is_full());
    }

    #[test]
    fn droplet_falls_one_step_across_even_to_odd_seam() {
        // Even-cy above odd-cy (cy=2 → cy=1): pull + checkerboard must
        // still move exactly one cell, not double-step.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.ensure_chunk(ChunkCoord::new(0, 2));
        let seam = 2 * CHUNK_CELLS_H as i32; // 128
        w.set_cell(7, seam, Cell::water());
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(7, seam).unwrap().sat.is_empty());
        assert!(w.get_cell(7, seam - 1).unwrap().sat.is_full());
        assert!(
            w.get_cell(7, seam - 2).unwrap().sat.is_empty(),
            "must not fall two cells in one checkerboard rule"
        );
    }

    #[test]
    fn missing_below_chunk_stops_fall() {
        // Chunk (0, 0) exists; chunk (0, -1) does not. A water cell
        // at gy=0 (bottom of chunk 0,0) has no below chunk — it must
        // stay put rather than pour into the void.
        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::water());
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(1, 0).unwrap().sat.is_full());
        // Below chunk still doesn't exist.
        assert_eq!(w.get_cell(1, -1), None);
    }

    // ------------ lateral spill ------------

    fn setup_air_row(width: i32) -> World {
        // Bedrock floor at y=0, everything above y=0 is Air, in one
        // 64-wide chunk. `width` is how many columns to make bedrock.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..width {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    fn total_sat(world: &World, xs: std::ops::Range<i32>, y: i32) -> i64 {
        xs.map(|x| world.get_cell(x, y).map(|c| c.sat.0 as i64).unwrap_or(0))
            .sum()
    }

    #[test]
    fn spill_equalizes_isolated_pair() {
        // Bedrock walls (cap 0) so the water cell only has one Air
        // neighbour — isolates a single pair. Stone is porous and
        // would participate in seepage, not in spill.
        let mut w = setup_air_row(64);
        w.set_cell(9, 5, Cell::solid(MaterialId::Bedrock));
        w.set_cell(10, 5, Cell::water());
        // (11, 5) starts as default Air with sat 0.
        let start_mass = w.get_cell(10, 5).unwrap().sat.0 as i32;

        apply_lateral_spill(&mut w);

        let l = w.get_cell(10, 5).unwrap().sat.0 as i32;
        let r = w.get_cell(11, 5).unwrap().sat.0 as i32;
        // Head equalisation on equal-cap Air ≡ half the sat gap.
        assert_eq!(l, 255 - 127);
        assert_eq!(r, 127);
        assert_eq!(l + r, start_mass, "mass conserved");
    }

    #[test]
    fn spill_is_symmetric_across_a_single_pass() {
        // Water at gx=10 with dry air on both sides. Rule must feed
        // both neighbours equally — the pair is symmetric.
        let mut w = setup_air_row(64);
        w.set_cell(10, 5, Cell::water());
        apply_lateral_spill(&mut w);
        let left = w.get_cell(9, 5).unwrap().sat.0;
        let right = w.get_cell(11, 5).unwrap().sat.0;
        assert_eq!(left, right, "L/R must be equal");
        assert!(left > 0);
        // Mass conserved across the three cells.
        let total = w.get_cell(9, 5).unwrap().sat.0 as i32
            + w.get_cell(10, 5).unwrap().sat.0 as i32
            + w.get_cell(11, 5).unwrap().sat.0 as i32;
        assert_eq!(total, 255);
    }

    #[test]
    fn spill_conserves_mass_over_a_long_chain() {
        let mut w = setup_air_row(64);
        // Puddle at columns 20..25 (5 water cells), rest dry.
        for x in 20..25 {
            w.set_cell(x, 3, Cell::water());
        }
        let start_mass = total_sat(&w, 0..64, 3);
        for _ in 0..30 {
            apply_lateral_spill(&mut w);
        }
        let end_mass = total_sat(&w, 0..64, 3);
        assert_eq!(start_mass, end_mass, "mass must be preserved");
    }

    #[test]
    fn spill_stops_at_a_solid_wall() {
        let mut w = setup_air_row(64);
        // Impermeable Bedrock wall — spill is Air–Air only.
        w.set_cell(5, 5, Cell::solid(MaterialId::Bedrock));
        w.set_cell(4, 5, Cell::water());
        apply_lateral_spill(&mut w);
        assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Bedrock);
        assert_eq!(w.get_cell(5, 5).unwrap().sat.0, 0);
        assert_eq!(w.get_cell(3, 5).unwrap().sat.0, 127);
        assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 255 - 127);
    }

    #[test]
    fn spill_propagates_one_cell_per_tick() {
        // Full-water cell at x=32 in an otherwise dry row. After N
        // ticks, non-zero sat should reach at least x=32-N and x=32+N.
        let mut w = setup_air_row(64);
        w.set_cell(32, 3, Cell::water());

        for tick_i in 1..=4 {
            apply_lateral_spill(&mut w);
            // Both sides should have some water by tick_i.
            let left = w.get_cell(32 - tick_i, 3).unwrap();
            let right = w.get_cell(32 + tick_i, 3).unwrap();
            assert!(
                left.sat.0 > 0,
                "tick={tick_i} left cell x={} should have water",
                32 - tick_i
            );
            assert!(
                right.sat.0 > 0,
                "tick={tick_i} right cell x={} should have water",
                32 + tick_i
            );
            // The frontier is exactly `tick_i` cells: no water yet
            // one further out.
            if 32 - tick_i - 1 >= 0 {
                assert_eq!(
                    w.get_cell(32 - tick_i - 1, 3).unwrap().sat.0,
                    0,
                    "tick={tick_i} frontier at x={}",
                    32 - tick_i - 1
                );
            }
            if 32 + tick_i + 1 < 64 {
                assert_eq!(
                    w.get_cell(32 + tick_i + 1, 3).unwrap().sat.0,
                    0,
                    "tick={tick_i} frontier at x={}",
                    32 + tick_i + 1
                );
            }
        }
    }

    // ------------ seepage ------------

    #[test]
    fn seepage_wets_adjacent_sand_from_air_water() {
        let mut w = World::new(42);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // Dry sand beside a full water cell.
        w.set_cell(3, 1, Cell::water());
        w.set_cell(4, 1, Cell::solid(MaterialId::Sand));
        let before = w.get_cell(3, 1).unwrap().sat.0 as i32
            + w.get_cell(4, 1).unwrap().sat.0 as i32;
        apply_seepage(&mut w);
        let sand = w.get_cell(4, 1).unwrap();
        let air = w.get_cell(3, 1).unwrap();
        assert!(sand.sat.0 > 0, "sand should take on pore water");
        assert!(air.sat.0 < 255, "air should lose sat to the sand");
        assert_eq!(
            air.sat.0 as i32 + sand.sat.0 as i32,
            before,
            "mass conserved"
        );
        // Rate-limited: one tick can't dump the whole lake into sand.
        let rate = seepage_rate(MaterialId::Sand);
        assert!(sand.sat.0 as i32 <= rate);
    }

    #[test]
    fn seepage_skips_impermeable_bedrock() {
        let mut w = World::new(43);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(2, 1, Cell::water());
        w.set_cell(3, 1, Cell::solid(MaterialId::Bedrock));
        apply_seepage(&mut w);
        assert_eq!(w.get_cell(2, 1).unwrap().sat.0, 255);
        assert_eq!(w.get_cell(3, 1).unwrap().sat.0, 0);
    }

    #[test]
    fn seepage_prefers_lower_head() {
        // Two sand cells on bedrock: left full pores, right dry.
        let mut w = World::new(44);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            for y in 0..4 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        let cap = water_capacity(MaterialId::Sand);
        w.set_cell(5, 2, Cell {
            material: MaterialId::Sand,
            sat: Sat(cap),
            ..Cell::default()
        });
        w.set_cell(6, 2, Cell::solid(MaterialId::Sand));
        apply_seepage(&mut w);
        let l = w.get_cell(5, 2).unwrap().sat.0;
        let r = w.get_cell(6, 2).unwrap().sat.0;
        assert!(r > 0);
        assert!(l < cap);
        assert_eq!(l as i32 + r as i32, cap as i32);
    }

    #[test]
    fn hydraulic_head_ranks_full_air_above_dry_sand() {
        let ha = hydraulic_head(10, Sat::FULL, water_capacity(MaterialId::Air));
        let hs = hydraulic_head(10, Sat::EMPTY, water_capacity(MaterialId::Sand));
        assert!(ha > hs);
    }

    #[test]
    fn spill_crosses_chunk_boundary() {
        // Full water cell at gx=63 in chunk (0, 0); empty air at
        // gx=64 in chunk (1, 0). Stone wall at gx=62 so only the
        // cross-boundary pair contributes.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..128 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(62, 5, Cell::solid(MaterialId::Bedrock));
        w.set_cell(63, 5, Cell::water());
        apply_lateral_spill(&mut w);
        assert_eq!(w.get_cell(63, 5).unwrap().sat.0, 255 - 127);
        assert_eq!(w.get_cell(64, 5).unwrap().sat.0, 127);
        // Mass conserved across the boundary.
        assert_eq!(
            w.get_cell(63, 5).unwrap().sat.0 as i32
                + w.get_cell(64, 5).unwrap().sat.0 as i32,
            255
        );
    }

    #[test]
    fn gravity_only_drains_droplet_over_passes() {
        // Verified without lateral spill so we can assert exact
        // per-tick positions of a single droplet.
        let mut w = setup_column_world();
        w.set_cell(6, 5, Cell::water());

        apply_gravity_fall(&mut w);
        assert!(w.get_cell(6, 4).unwrap().sat.is_full());
        assert!(w.get_cell(6, 5).unwrap().sat.is_empty());
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(6, 3).unwrap().sat.is_full());
        apply_gravity_fall(&mut w);
        apply_gravity_fall(&mut w);
        assert!(
            w.get_cell(6, 1).unwrap().sat.is_full(),
            "water should be resting on bedrock"
        );
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(6, 1).unwrap().sat.is_full());
        assert!(w.get_cell(6, 0).unwrap().sat.is_empty()); // bedrock sat stays 0
    }

    // ------------ grain fall ------------

    #[test]
    fn grain_falls_through_empty_air() {
        let mut w = setup_column_world();
        // Sand at y=5, everything below is empty Air, bedrock at y=0.
        w.set_cell(4, 5, Cell::solid(MaterialId::Sand));
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(4, 4).map(|c| c.material),
            Some(MaterialId::Sand)
        );
        assert_eq!(
            w.get_cell(4, 5).map(|c| c.material),
            Some(MaterialId::Air)
        );
    }

    #[test]
    fn grain_stops_on_competent_rock() {
        let mut w = setup_column_world();
        w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 3, Cell::solid(MaterialId::Sand));
        apply_grain_fall(&mut w);
        // Below Stone is not Air → no swap.
        assert_eq!(w.get_cell(4, 3).unwrap().material, MaterialId::Sand);
        assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Stone);
    }

    #[test]
    fn grain_stops_on_another_grain() {
        let mut w = setup_column_world();
        w.set_cell(4, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 2, Cell::solid(MaterialId::Gravel));
        apply_grain_fall(&mut w);
        // y=1 is Sand (not Air); Gravel at y=2 has nowhere to swap.
        // Sand at y=1 has bedrock at y=0 (not Air), also stays.
        assert_eq!(w.get_cell(4, 1).unwrap().material, MaterialId::Sand);
        assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Gravel);
    }

    #[test]
    fn grain_sinks_through_water_swap_conserves_mass() {
        // Water column at y=1..=4 (all Air with sat=full); sand at
        // y=5. After one grain pass, sand moves to y=4 and the water
        // that was at y=4 rises into y=5.
        let mut w = setup_column_world();
        for y in 1..=4 {
            w.set_cell(4, y, Cell::water());
        }
        w.set_cell(4, 5, Cell::solid(MaterialId::Sand));
        let start_water: i32 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i32)
            .sum();

        apply_grain_fall(&mut w);

        let end_water: i32 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i32)
            .sum();
        assert_eq!(end_water, start_water, "water sat is conserved by swap");
        assert_eq!(w.get_cell(4, 4).unwrap().material, MaterialId::Sand);
        // Sand carries its own sat (0) up... wait, the Air cell's
        // water rises. The newly-vacated cell at y=5 receives the
        // sat that was in the old below-cell (y=4 water full).
        assert_eq!(w.get_cell(4, 5).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(4, 5).unwrap().sat.is_full());
    }

    #[test]
    fn grain_falls_across_chunk_boundary() {
        // Sand at gy=64 (chunk (0,1) local (7,0)); Air at gy=63
        // (chunk (0,0) local (7,63)). Sand should end at gy=63.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.set_cell(7, 64, Cell::solid(MaterialId::Sand));
        assert_eq!(
            w.get_cell(7, 64).unwrap().material,
            MaterialId::Sand
        );
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(7, 63).unwrap().material,
            MaterialId::Sand,
            "grain should have crossed the seam"
        );
        assert_eq!(
            w.get_cell(7, 64).unwrap().material,
            MaterialId::Air,
            "vacated cell above must be Air"
        );
    }

    #[test]
    fn grain_falls_one_cell_per_pass_through_empty_column() {
        // Multi-pass check that grain fall obeys the 1 cell / pass rule.
        let mut w = setup_column_world();
        w.set_cell(20, 10, Cell::solid(MaterialId::Sand));
        for expected in (1..=9).rev() {
            apply_grain_fall(&mut w);
            assert_eq!(
                w.get_cell(20, expected).map(|c| c.material),
                Some(MaterialId::Sand),
                "grain should be at y={expected}"
            );
            assert_eq!(
                w.get_cell(20, expected + 1).map(|c| c.material),
                Some(MaterialId::Air)
            );
        }
        // One more pass: bedrock below at y=0, no swap.
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(20, 1).unwrap().material,
            MaterialId::Sand
        );
    }

    // ------------ rain ------------

    fn setup_sky_row(y: i32) -> World {
        // Chunk with bedrock floor so climatic rain lands on the surface
        // (y=1), scanning down from the sky row.
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        assert!((0..CHUNK_CELLS_H as i32).contains(&y));
        for x in 0..CHUNK_CELLS_W as i32 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    #[test]
    fn rain_is_deterministic_for_seed_and_tick() {
        let mut a = setup_sky_row(30);
        let mut b = setup_sky_row(30);
        let cfg = RainConfig {
            top_y: 30,
            x_range: (0, 63),
            prob_per_col_per_tick: 0.5,
            droplet_sat: 32,
            seed_salt: 0xF00,
        };
        apply_rain(&mut a, &cfg);
        apply_rain(&mut b, &cfg);
        for x in 0..64 {
            assert_eq!(
                a.get_cell(x, 1).map(|c| c.sat.0),
                b.get_cell(x, 1).map(|c| c.sat.0),
                "identical worlds must produce identical rain (x={x})"
            );
        }
    }

    #[test]
    fn rain_respects_x_range() {
        let mut w = setup_sky_row(30);
        let cfg = RainConfig {
            top_y: 30,
            x_range: (5, 20),
            prob_per_col_per_tick: 1.0, // always
            droplet_sat: 40,
            seed_salt: 1,
        };
        apply_rain(&mut w, &cfg);
        for x in 0..64 {
            let sat = w.get_cell(x, 1).unwrap().sat.0;
            let sky = w.get_cell(x, 30).unwrap().sat.0;
            assert_eq!(sky, 0, "rain must not hang in the sky at x={x}");
            if (5..=20).contains(&x) {
                assert!(sat > 0, "x={x} in range should have rain on the ground");
            } else {
                assert_eq!(sat, 0, "x={x} outside range should stay dry");
            }
        }
    }

    #[test]
    fn rain_droplet_saturates_at_full() {
        let mut w = setup_sky_row(30);
        w.set_cell(3, 1, Cell::water()); // surface already full
        let cfg = RainConfig {
            top_y: 30,
            x_range: (3, 3),
            prob_per_col_per_tick: 1.0,
            droplet_sat: 40,
            seed_salt: 2,
        };
        apply_rain(&mut w, &cfg);
        // Full film on bare rock does not stack into a wedge.
        assert_eq!(w.get_cell(3, 1).unwrap().sat.0, u8::MAX);
        assert_eq!(w.get_cell(3, 2).unwrap().sat.0, 0);
    }

    #[test]
    fn rain_skips_non_air_cells() {
        let mut w = setup_sky_row(30);
        // Buried column of stone — no free air above a solid landing.
        for y in 1..=30 {
            w.set_cell(10, y, Cell::solid(MaterialId::Stone));
        }
        let cfg = RainConfig {
            top_y: 30,
            x_range: (10, 10),
            prob_per_col_per_tick: 1.0,
            droplet_sat: 40,
            seed_salt: 3,
        };
        apply_rain(&mut w, &cfg);
        assert_eq!(w.get_cell(10, 30).unwrap().material, MaterialId::Stone);
        assert_eq!(w.get_cell(10, 30).unwrap().sat.0, 0);
    }

    #[test]
    fn surface_flow_drains_hill_film_diagonally() {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 3, Cell::water());
        apply_water_flow(&mut w);
        assert!(
            w.get_cell(4, 3).unwrap().sat.0 < u8::MAX,
            "hill film should drain by head"
        );
        assert!(
            w.get_cell(5, 2).unwrap().sat.0 > 0,
            "water should move diagonally downhill"
        );
    }

    #[test]
    fn surface_flow_levels_diagonal_slope_wedge() {
        // Packed staircase wedge — the "gaffa tape" failure mode.
        // Head equalisation across diagonals must flatten it downhill.
        let mut w = World::new(21);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // Rising slope solid under a diagonal water wedge.
        for x in 2..10 {
            for y in 1..=(x - 1) {
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
            w.set_cell(x, x, Cell::water());
        }
        let high_before: i32 = (6..10)
            .map(|x| w.get_cell(x, x).unwrap().sat.0 as i32)
            .sum();
        assert!(high_before > 500);
        for _ in 0..40 {
            tick(&mut w);
        }
        let high_after: i32 = (6..10)
            .map(|x| w.get_cell(x, x).map(|c| c.sat.0 as i32).unwrap_or(0))
            .sum();
        assert!(
            high_after < high_before / 4,
            "wedge crest should empty (before={high_before} after={high_after})"
        );
        let pool: i32 = (0..6)
            .flat_map(|x| (1..=4).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum();
        assert!(pool > 400, "water should pool at the foot (got {pool})");
    }

    #[test]
    fn beach_film_drains_into_ocean_not_inland() {
        let mut w = World::new(19);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for x in 0..6 {
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            w.set_cell(x, 2, Cell::water());
        }
        for x in 6..12 {
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            w.set_cell(x, 2, Cell::solid(MaterialId::Sand));
        }
        for x in 8..12 {
            w.set_cell(x, 3, Cell::solid(MaterialId::Sand));
        }
        w.set_cell(6, 3, Cell::water());
        for _ in 0..8 {
            tick(&mut w);
        }
        assert_eq!(
            w.get_cell(6, 3).unwrap().sat.0,
            0,
            "beach film should leave the sand"
        );
        assert_eq!(
            w.get_cell(7, 3).unwrap().sat.0,
            0,
            "must not climb inland up the beach"
        );
        // Film may sit one cell seaward or soak into sand — either is
        // fine; the failure mode was climbing inland.
        let inland_high: i32 = (8..12)
            .map(|x| w.get_cell(x, 4).map(|c| c.sat.0 as i32).unwrap_or(0))
            .sum();
        assert_eq!(inland_high, 0, "no water above the inland sand terrace");
    }

    // NOTE: A previous test forced physics into a quiescent state via
    // `clear_all_dirty` and expected water to still drain. That was
    // testing the retired `remount_unbalanced_surface_water` bandaid.
    // In practice, physics only quiesces when no cell has moved for a
    // full tick; any new write (rain, cloud downpour, editor spawn)
    // rebuilds the dirty rect and re-wakes flow. The artificial
    // clear-then-idle case is intentionally not supported.

    #[test]
    fn beach_slope_rain_does_not_perch_on_shelves() {
        // A monotonic sand slope descending to open Air on the left.
        // Rain deposits water at every column. Every wet cell has sand
        // below and diagonal-down sand — old flow trapped water as
        // staircase perched pools. Diagonal throughflow lets it seep
        // down the slope until it reaches open Air / ocean.
        let mut w = World::new(31);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..30 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // Slope rises 1 cell per column from x=8..=22 (crest at 22, y=15).
        // Left of x=8 is open Air over bedrock (the "sea").
        for x in 8..=22 {
            let top = x - 7; // 1..=15
            for y in 1..=top {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        for x in 23..30 {
            for y in 1..=15 {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        // Saturate all sand so throughflow is the only drain path.
        let cap_sand = crate::cell::water_capacity(MaterialId::Sand);
        for x in 8..30 {
            for y in 1..=15 {
                if let Some(c) = w.get_cell(x, y) {
                    if c.material == MaterialId::Sand {
                        w.set_cell(x, y, Cell { sat: Sat(cap_sand), ..c });
                    }
                }
            }
        }
        // Rain deposit: full sat on every shelf surface cell along the slope.
        for x in 8..=22 {
            let top = x - 7;
            w.set_cell(x, top + 1, Cell::water());
        }
        for _ in 0..80 {
            tick(&mut w);
        }
        // Water on the slope should be nearly gone.
        let mut perched = 0;
        for x in 8..=22 {
            for y in 1..=15 {
                let Some(c) = w.get_cell(x, y) else { continue };
                if c.material == MaterialId::Air && c.sat.0 >= 128 {
                    perched += 1;
                }
            }
        }
        // Sea pool at x=0..=7, y=1..=3 should have caught the drained mass.
        let sea: i32 = (0..=7)
            .flat_map(|x| (1..=3).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum();
        assert!(
            perched <= 3,
            "slope should drain (perched={perched}, sea_sat={sea})"
        );
        assert!(sea > 500, "sea should catch drainage (sea_sat={sea})");
    }

    #[test]
    fn user_scenario_water_equilibrates_across_flat_shelf_and_cascades() {
        // The user's mental model:
        // - Rain drops water on an Air cell above an impermeable block.
        // - Immediate neighbours also sit above impermeable → water
        //   spreads (averaged) across them.
        // - One neighbour is an "air-above-air" cascade edge → water
        //   there falls, opening space for more sideways flow.
        //
        // World layout (y-up):
        //   x:  8 9 10 11 12 13
        //   y=51 . . .  .  .  .   (Air, dry)
        //   y=50 . . W  .  .  .   (Air; W = rain drop)
        //   y=49 # # #  #  .  .   (Bedrock shelf ending at 11; 12+ is Air)
        //   y=48 . . .  .  .  .   (Air below the shelf edge)
        //   y=0  # # #  #  #  #   (Bedrock floor)
        let mut w = World::new(2);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..32 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for x in 8..=11 {
            w.set_cell(x, 49, Cell::solid(MaterialId::Bedrock));
        }
        // Rain drop at (10, 50).
        w.set_cell(10, 50, Cell::water());

        // One tick's water flow should already start cascading right
        // (x=11,50 has Air below at y=49? no, x=11,49 is bedrock. So
        // cascade edge is at x=12,50 whose below x=12,49 is Air).
        // Water at x=10,50 first goes right one cell per substep, then
        // falls off the shelf.
        for _ in 0..20 {
            tick(&mut w);
        }

        // No water should have climbed left onto more bedrock shelf.
        assert!(
            w.get_cell(10, 50).unwrap().sat.0 < 8,
            "source cell must nearly empty (got sat={})",
            w.get_cell(10, 50).unwrap().sat.0
        );
        // Water should have cascaded off the shelf. Some sits on the
        // shelf (equilibrated across x=6..12), some falls into the
        // right chasm at x=12+. Whichever way it goes, it must not
        // climb inland uphill (there's no uphill to climb here — just
        // the flat bedrock shelf x=8..=11 and the left/right chasms).
        let landed_right: i32 = (12..32)
            .flat_map(|x| (0..=48).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum();
        let landed_left: i32 = (0..8)
            .flat_map(|x| (0..=48).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum();
        assert!(
            landed_right + landed_left >= 150,
            "water should cascade off the shelf edge (right={landed_right} left={landed_left})"
        );
    }

    #[test]
    fn rain_on_descending_shore_cascades_into_ocean_pool() {
        // Mimics the visible image: shore descends left, ocean at bottom
        // left. Rain drops water at shelf-top cells. Water must cascade
        // diagonally down the shore into the ocean pool in a few ticks —
        // not accumulate as terraced puddles.
        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..100 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // Slope: shore top rises from y=1 at x=5 to y=15 at x=20 (rise=1/col).
        for x in 5..=20 {
            let top = x - 4; // 1..=16
            for y in 1..=top {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        // High plateau for x=21..40 (top=17).
        for x in 21..40 {
            for y in 1..=17 {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        // Ocean pool below sea level at x=0..4 (deep).
        for x in 0..5 {
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
        }
        // Rain falls at three shelf-top cells along the slope.
        w.set_cell(10, 7, Cell::water());   // shelf top at x=10 is y=6
        w.set_cell(15, 12, Cell::water());  // shelf top at x=15 is y=11
        w.set_cell(20, 17, Cell::water());  // shelf top at x=20 is y=16

        // Run enough ticks for cascade to reach ocean.
        for _ in 0..30 {
            tick(&mut w);
        }

        // No water should remain on the sand shelves at the deposit
        // heights (perched pool test).
        let perched_10 = w.get_cell(10, 7).unwrap().sat.0;
        let perched_15 = w.get_cell(15, 12).unwrap().sat.0;
        let perched_20 = w.get_cell(20, 17).unwrap().sat.0;
        assert!(
            perched_10 < 32 && perched_15 < 32 && perched_20 < 32,
            "shelf cells should drain (got {perched_10}, {perched_15}, {perched_20})"
        );

        // Ocean gained some, sand absorbed some (seepage), and total
        // mass is conserved.
        let mut total = 0i64;
        for x in 0..40 {
            for y in 0..30 {
                if let Some(c) = w.get_cell(x, y) {
                    total += c.sat.0 as i64;
                }
            }
        }
        // Baseline sand had 0 sat; ocean had 5*5*255=6375; rain added 3*255=765.
        // Sand can absorb up to 15*15*180 ≈ 40k, so mass may sit in sand.
        assert!(total >= 6375 + 765 - 50, "mass roughly conserved (total={total})");
    }

    #[test]
    fn continuous_rain_on_flat_shelf_drains_via_cascade_edge() {
        // Flat sand shelf 8 cells wide. Left of shelf is a cliff (Air).
        // Right of shelf is inland (more sand higher).
        // Rain sat=8 falls on every shelf cell every tick.
        // With cascade at the left edge, water shouldn't stack up.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..40 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // Cliff: at x=0..=9 shore top is Y=1 (Air above). At x=10..=17
        // shore top is y=10 (flat shelf 8 cells wide). At x=18+ higher.
        for x in 10..=17 {
            for y in 1..=10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        for x in 18..30 {
            for y in 1..=15 {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }

        // Track max sat seen on the shelf over 40 ticks of rain.
        let mut max_shelf_sat: u8 = 0;
        for _t in 0..40 {
            // Rain deposit: 8 sat on each shelf cell (10..=17) at y=11.
            for x in 10..=17 {
                let cell = w.get_cell(x, 11).unwrap();
                if cell.material == MaterialId::Air {
                    let new_sat = (cell.sat.0 as i32 + 8).min(255) as u8;
                    w.set_cell(x, 11, Cell { sat: Sat(new_sat), ..cell });
                }
            }
            tick(&mut w);
            for x in 10..=17 {
                let c = w.get_cell(x, 11).unwrap();
                if c.sat.0 > max_shelf_sat {
                    max_shelf_sat = c.sat.0;
                }
            }
        }
        // At steady state, water on the shelf should be low because
        // cascade at x=10 dumps it off the cliff on each substep.
        assert!(
            max_shelf_sat < 200,
            "shelf water should drain via cascade edge (max seen: {max_shelf_sat})"
        );
    }

    #[test]
    fn continuous_rain_on_stepped_shore_does_not_pool_on_shelves() {
        // Realistic shore: descending in 2-cell-wide steps (like a
        // staircase). Rain falls on every shelf. Water must cascade
        // down step by step, not accumulate as terrace pools.
        //
        // Terrain (top view of tops):
        //   x=  8 9 10 11 12 13 14 15 16 17
        //   top y=1 1  3  3  5  5  7  7  9  9
        //
        // Ocean at x < 8. Each shelf is 2 cells wide, drop of 2y.
        let mut w = World::new(6);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..30 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let tops = [(8, 1), (9, 1), (10, 3), (11, 3), (12, 5), (13, 5), (14, 7), (15, 7), (16, 9), (17, 9)];
        for &(x, top) in &tops {
            for y in 1..=top {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
        // Ocean pool at x=0..=7, up to y=1.
        for x in 0..=7 {
            w.set_cell(x, 1, Cell::water());
        }

        let mut max_shelf: u8 = 0;
        for _t in 0..40 {
            // Rain 6 sat per tick on each shelf-top-air cell.
            for &(x, top) in &tops {
                let y = top + 1;
                let cell = w.get_cell(x, y).unwrap();
                if cell.material == MaterialId::Air {
                    let new_sat = (cell.sat.0 as i32 + 6).min(255) as u8;
                    w.set_cell(x, y, Cell { sat: Sat(new_sat), ..cell });
                }
            }
            tick(&mut w);
            for &(x, top) in &tops {
                let y = top + 1;
                let c = w.get_cell(x, y).unwrap();
                if c.sat.0 > max_shelf {
                    max_shelf = c.sat.0;
                }
            }
        }
        // Steady-state shelf sat should stay low.
        assert!(
            max_shelf < 128,
            "stepped-shore shelves should keep draining (max shelf sat: {max_shelf})"
        );
    }


    #[test]
    fn same_y_equalize_flattens_stepped_lake_surface() {
        // Free-surface terrace inside a closed basin (solid shores):
        //
        //   y=3: # W W . . . . #
        //   y=2: # W W W W W W #
        //   y=1: # # # # # # # #
        //
        // Same-Y equalise should spread the step across the row so the
        // surface is no longer a hard cliff of full cells.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..10 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
        }
        // Basin walls.
        w.set_cell(0, 2, Cell::solid(MaterialId::Bedrock));
        w.set_cell(0, 3, Cell::solid(MaterialId::Bedrock));
        w.set_cell(9, 2, Cell::solid(MaterialId::Bedrock));
        w.set_cell(9, 3, Cell::solid(MaterialId::Bedrock));
        for x in 1..9 {
            w.set_cell(x, 2, Cell::water());
        }
        w.set_cell(2, 3, Cell::water());
        w.set_cell(3, 3, Cell::water());

        for _ in 0..30 {
            tick(&mut w);
        }

        let row: Vec<u8> = (1..9).map(|x| w.get_cell(x, 3).unwrap().sat.0).collect();
        let max = *row.iter().max().unwrap();
        let min = *row.iter().min().unwrap();
        let sum: i32 = row.iter().map(|&s| s as i32).sum();
        assert_eq!(sum, 510, "mass on the free surface must be conserved: {row:?}");
        assert!(
            max < 220,
            "terrace must thin out across the lake (row={row:?})"
        );
        assert!(
            min > 20,
            "dry gaps on the free surface must fill in (row={row:?})"
        );
        assert!(
            (max as i32) - (min as i32) < 80,
            "same-Y surface should be close to level (row={row:?})"
        );
    }

    #[test]
    fn solid_staircase_film_drains_left_into_lower_pool() {
        // Geometry from the user's first image (impermeable sand):
        //
        //   y=3:  .  .  .  w  .     <- thin film on higher step (THE STUCK PIXEL)
        //   y=2:  d  P  P  #  .     <- drop | pool2(2) | pool3(3)? wait
        //
        // Simpler staircase matching the description:
        //   y=3: . . . W .
        //   y=2: D # P P #
        //   y=1: # # # # #
        //   y=0: ###########
        //
        // W at (3,3) on sand (3,2). Diagonal-down left is sand (2,2).
        // Same-row left (2,3) is Air above sand (2,2) — corner cell.
        // From (2,3), diagonal-down left (1,2) is pool Air P.
        // Drop D at (0,2) is lower basin.
        //
        // Expected: W → (2,3) → dump into P → eventually into D.
        // Must NOT sit for 100 ticks.
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
        }
        // Sand step face + upper terrace.
        w.set_cell(2, 2, Cell::solid(MaterialId::Bedrock)); // step face
        w.set_cell(3, 2, Cell::solid(MaterialId::Bedrock)); // under W
        w.set_cell(4, 2, Cell::solid(MaterialId::Bedrock));
        // Lower terrace floor under pool/drop.
        w.set_cell(0, 1, Cell::solid(MaterialId::Bedrock));
        w.set_cell(1, 1, Cell::solid(MaterialId::Bedrock));
        // (1,2) and (0,2) are Air — the lower pool/drop level.
        // Seed a little water in the pool so it's "occupied" like the image.
        w.set_cell(1, 2, Cell { material: MaterialId::Air, sat: Sat(200), flags: Default::default(), _pad: 0 });
        // The stuck higher film.
        w.set_cell(3, 3, Cell::water());

        for _ in 0..30 {
            tick(&mut w);
        }

        let stuck = w.get_cell(3, 3).unwrap().sat.0;
        let corner = w.get_cell(2, 3).unwrap().sat.0;
        let pool = w.get_cell(1, 2).unwrap().sat.0;
        let drop = w.get_cell(0, 2).unwrap().sat.0;
        assert!(
            stuck < 8,
            "higher-step film must drain (stuck={stuck} corner={corner} pool={pool} drop={drop})"
        );
        assert!(
            (pool as i32) + (drop as i32) >= 200,
            "water must reach lower level (pool={pool} drop={drop})"
        );
    }

    #[test]
    fn impermeable_shore_cascades_off_within_seconds() {
        // Simulates user's setup: sand set to impermeable (no seepage
        // or throughflow). Uses Bedrock terrain to model this without
        // touching global overrides (which race with other tests).
        //
        // Shore descends left, has a 6-cell flat plateau at top, then
        // rises again. Rain hits every cell along the shore surface.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..60 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for x in 8..=13 {
            for y in 1..=(x - 7) {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 14..=19 {
            for y in 1..=6 {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        for x in 20..=25 {
            let top = 6 + (x - 19);
            for y in 1..=top {
                w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
            }
        }
        let surface_cells: Vec<(i32, i32)> = (8..=25)
            .map(|x| {
                let mut top_y = 0;
                for y in 1..30 {
                    if let Some(c) = w.get_cell(x, y) {
                        if c.material == MaterialId::Bedrock {
                            top_y = y;
                        }
                    }
                }
                (x, top_y + 1)
            })
            .collect();

        let mut max_plateau: u8 = 0;
        for _t in 0..60 {
            for &(x, y) in &surface_cells {
                let cell = w.get_cell(x, y).unwrap();
                if cell.material == MaterialId::Air {
                    let new_sat = (cell.sat.0 as i32 + 5).min(255) as u8;
                    w.set_cell(x, y, Cell { sat: Sat(new_sat), ..cell });
                }
            }
            tick(&mut w);
            // Only assert plateau cells drain. Beach edge pools by design.
            for x in 14..=19 {
                let c = w.get_cell(x, 7).unwrap();
                if c.sat.0 > max_plateau {
                    max_plateau = c.sat.0;
                }
            }
        }
        assert!(
            max_plateau < 128,
            "impermeable plateau must keep draining (max sat: {max_plateau})"
        );
    }

    #[test]
    fn tick_drains_hill_mound_instead_of_stalling() {
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for x in 0..8 {
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
            w.set_cell(x, 2, Cell::solid(MaterialId::Stone));
        }
        for x in 8..16 {
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
        }
        for y in 3..=6 {
            for x in 3..6 {
                w.set_cell(x, y, Cell::water());
            }
        }
        let mass_high = |w: &World| -> i32 {
            (3..6)
                .flat_map(|x| (3..=6).map(move |y| (x, y)))
                .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
                .sum()
        };
        let before = mass_high(&w);
        assert!(before > 1000);
        for _ in 0..12 {
            tick(&mut w);
        }
        let after = mass_high(&w);
        assert!(
            after < before / 2,
            "mound should mostly leave the high step (before={before} after={after})"
        );
        let low: i32 = (8..16)
            .flat_map(|x| (1..=5).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum();
        assert!(low > 200, "water should pool on the lower step (got {low})");
    }

    #[test]
    fn lone_ridge_pixel_drains_via_throughflow_or_evap() {
        // A single wet Air on a sand crest with sand on both flanks
        // (no Air neighbours). Historic sticky-water case: gravity +
        // seepage stopped at sand porosity, leaving the pixel forever.
        let mut w = World::new(13);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // Sand pyramid, crest at (6, 3).
        for x in 3..10 {
            for y in 1..=3 {
                if (x as i32 - 6).abs() <= (3 - y) {
                    w.set_cell(x, y, Cell::solid(MaterialId::Sand));
                }
            }
        }
        // Saturate the pyramid sand fully so gravity + seepage would stop.
        let cap_sand = crate::cell::water_capacity(MaterialId::Sand);
        for x in 3..10 {
            for y in 1..=3 {
                if let Some(c) = w.get_cell(x, y) {
                    if c.material == MaterialId::Sand {
                        w.set_cell(x, y, Cell {
                            sat: Sat(cap_sand),
                            ..c
                        });
                    }
                }
            }
        }
        w.set_cell(6, 4, Cell::water()); // lone wet Air on crest
        let cfg = EvapConfig {
            rate_per_tick: 1,
            dry_above_max: 200,
            period_ticks: 1,
        };
        for _ in 0..200 {
            tick(&mut w);
            apply_evaporation(&mut w, &cfg);
        }
        let stuck = w.get_cell(6, 4).unwrap().sat.0;
        assert!(
            stuck < 8,
            "lone ridge pixel should drain (throughflow + orphan evap), got sat={stuck}"
        );
    }

    #[test]
    fn surface_flow_moves_single_sat_droplet_off_ridge() {
        // Force-1 trickle: sat=1 with drier Air neighbours must move —
        // the head equalizer's floor truncated 0.5 to zero and left
        // droplets stuck. Prefer downhill; mass is preserved.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let mut c = Cell::air();
        c.sat = Sat(1);
        w.set_cell(4, 3, c);
        apply_water_flow(&mut w);
        let src = w.get_cell(4, 3).unwrap().sat.0 as i32;
        let mass: i32 = (0..8)
            .flat_map(|x| (1..=4).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum();
        assert_eq!(src, 0, "lone droplet must leave the source cell");
        assert_eq!(mass, 1, "mass must be preserved (got {mass})");
    }

    // ------------ evaporation ------------

    #[test]
    fn evap_removes_from_surface_water_only() {
        // Water column at gy=1..=5, dry air above. Only the topmost
        // wet cell (gy=5) has dry Air above and should lose sat.
        let mut w = setup_column_world();
        for y in 1..=5 {
            w.set_cell(4, y, Cell::water());
        }
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w, &cfg);
        for y in 1..=4 {
            assert!(
                w.get_cell(4, y).unwrap().sat.is_full(),
                "sub-surface cell y={y} should not evaporate"
            );
        }
        // Top wet cell lost a tiny bit.
        let top = w.get_cell(4, 5).unwrap().sat.0;
        assert!(top < u8::MAX);
        assert!(top >= u8::MAX - cfg.rate_per_tick);
    }

    #[test]
    fn evap_drains_a_droplet_to_zero_over_time() {
        // Surface film on bedrock should tick down to zero over many passes.
        let mut w = setup_column_world();
        let mut c = Cell::air();
        c.sat = Sat(20);
        w.set_cell(4, 1, c);
        let cfg = EvapConfig {
            rate_per_tick: 5,
            dry_above_max: 200,
            period_ticks: 1,
        };
        for _ in 0..10 {
            apply_evaporation(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(w.get_cell(4, 1).unwrap().sat.0, 0);
    }

    #[test]
    fn evap_skips_airborne_rain_droplets() {
        // Wet Air with empty sky below is falling rain — must not
        // re-evaporate before gravity can land it.
        let mut w = setup_column_world();
        let mut c = Cell::air();
        c.sat = Sat(80);
        w.set_cell(4, 10, c);
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w, &cfg);
        assert_eq!(
            w.get_cell(4, 10).unwrap().sat.0,
            80,
            "airborne rain must survive evaporation"
        );
    }

    #[test]
    fn evap_leaves_dry_cells_alone() {
        let mut w = setup_column_world();
        // Air at y=5 with sat=0. No writes should occur.
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w, &cfg);
        assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 0);
    }

    #[test]
    fn evap_into_humidity_conserves_mass() {
        // Sat leaving cells lands as humidity mass. Sum should stay
        // constant across a single evap pass.
        use crate::humidity::Humidity;
        let mut w = setup_column_world();
        for y in 1..=5 {
            w.set_cell(4, y, Cell::water());
        }
        let mut h = Humidity::new(4);
        let cfg = EvapConfig {
            rate_per_tick: 3,
            dry_above_max: 200,
            period_ticks: 1,
        };
        let cell_sat_before: i64 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i64)
            .sum();
        let hum_before = h.total_mass();

        apply_evaporation_into_humidity(&mut w, &mut h, &cfg);

        let cell_sat_after: i64 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i64)
            .sum();
        let hum_after = h.total_mass();
        assert!(cell_sat_after < cell_sat_before, "some water must have left");
        let removed = (cell_sat_before - cell_sat_after) as f32;
        let gained = hum_after - hum_before;
        assert!(
            (removed - gained).abs() < 1e-3,
            "removed sat ({removed}) should equal humidity gain ({gained})"
        );
    }

    #[test]
    fn evap_into_humidity_matches_bare_evap_cell_state() {
        // The cell-side effect must be identical to `apply_evaporation`
        // — humidity routing is purely an additive record of the
        // removed mass, not a different eligibility rule.
        use crate::humidity::Humidity;
        let mut w_bare = setup_column_world();
        let mut w_hum = setup_column_world();
        for y in 1..=5 {
            w_bare.set_cell(4, y, Cell::water());
            w_hum.set_cell(4, y, Cell::water());
        }
        let mut h = Humidity::new(4);
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w_bare, &cfg);
        apply_evaporation_into_humidity(&mut w_hum, &mut h, &cfg);
        for y in 1..=5 {
            assert_eq!(
                w_bare.get_cell(4, y).map(|c| c.sat.0),
                w_hum.get_cell(4, y).map(|c| c.sat.0),
                "cell y={y} should evaporate identically"
            );
        }
    }

    // ------------ karst dissolution ------------

    fn setup_limestone_world() -> World {
        // Chunk (0, 0). Solid Limestone at y=1..=10, Bedrock at y=0,
        // Air above.
        let mut w = World::new(999);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..CHUNK_CELLS_W as i32 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Limestone));
            }
        }
        w
    }

    #[test]
    fn dry_limestone_never_dissolves() {
        let mut w = setup_limestone_world();
        // No wet neighbours anywhere — just dry Air above.
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 1,
        };
        for _ in 0..50 {
            apply_karst_dissolution(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        // No cell converted.
        for x in 0..(CHUNK_CELLS_W as i32) {
            for y in 1..=10 {
                assert_eq!(
                    w.get_cell(x, y).unwrap().material,
                    MaterialId::Limestone,
                    "dry limestone at ({x},{y}) must not dissolve"
                );
            }
        }
    }

    #[test]
    fn wet_limestone_eventually_dissolves() {
        let mut w = setup_limestone_world();
        // Put water full on top of a specific limestone cell.
        w.set_cell(10, 11, Cell::water());
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 42,
        };
        // With prob 1.0 the top-most limestone under the puddle
        // should convert on the first tick.
        apply_karst_dissolution(&mut w, &cfg);
        let after = w.get_cell(10, 10).unwrap();
        assert_eq!(after.material, MaterialId::Air, "wet limestone must dissolve");
    }

    #[test]
    fn karst_is_deterministic_for_seed_and_tick() {
        let mut a = setup_limestone_world();
        let mut b = setup_limestone_world();
        // Same puddle placement on both.
        for x in 5..=15 {
            a.set_cell(x, 11, Cell::water());
            b.set_cell(x, 11, Cell::water());
        }
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 0.5,
            min_wet_neighbour_sat: 200,
            seed_salt: 7,
        };
        for _ in 0..10 {
            apply_karst_dissolution(&mut a, &cfg);
            apply_karst_dissolution(&mut b, &cfg);
            a.tick = a.tick.wrapping_add(1);
            b.tick = b.tick.wrapping_add(1);
        }
        for x in 0..(CHUNK_CELLS_W as i32) {
            for y in 1..=10 {
                assert_eq!(
                    a.get_cell(x, y).map(|c| c.material),
                    b.get_cell(x, y).map(|c| c.material),
                    "seed-determinism failed at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn karst_ignores_non_limestone_solids() {
        // Stone cell adjacent to water — should never dissolve.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 6, Cell::water());
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 3,
        };
        for _ in 0..20 {
            apply_karst_dissolution(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Stone);
    }

    // ------------ condensation rain ------------

    fn setup_cloud_world() -> (World, crate::humidity::Humidity) {
        let mut w = World::new(21);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..CHUNK_CELLS_W as i32 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let h = crate::humidity::Humidity::new(4);
        (w, h)
    }

    fn ground_sat_sum(w: &World) -> i64 {
        (0..CHUNK_CELLS_W as i32)
            .map(|x| w.get_cell(x, 1).map(|c| c.sat.0 as i64).unwrap_or(0))
            .sum()
    }

    #[test]
    fn condensation_never_rains_from_a_dry_tile() {
        let (mut w, mut h) = setup_cloud_world();
        // No humidity anywhere. Rain must not appear.
        let cfg = CondensationConfig {
            top_y: 30,
            ..CondensationConfig::default()
        };
        for _ in 0..20 {
            apply_condensation_rain(&mut w, &mut h, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        for x in 0..CHUNK_CELLS_W as i32 {
            assert_eq!(w.get_cell(x, 1).unwrap().sat.0, 0);
            assert_eq!(w.get_cell(x, 30).unwrap().sat.0, 0);
        }
    }

    #[test]
    fn condensation_rains_when_tile_is_wet() {
        let (mut w, mut h) = setup_cloud_world();
        // Humidity over tile covering gx centre 2; rain lands on ground.
        h.add(1, 30, 1000.0);
        let cfg = CondensationConfig {
            top_y: 30,
            max_prob_per_tick: 1.0, // guaranteed to rain
            ..CondensationConfig::default()
        };
        apply_condensation_rain(&mut w, &mut h, &cfg);
        let landed = w.get_cell(2, 1).unwrap();
        assert!(
            landed.sat.0 > 0,
            "cloud with 1000 mass should have rained on the ground (got sat={})",
            landed.sat.0
        );
        assert_eq!(w.get_cell(2, 30).unwrap().sat.0, 0, "sky row stays dry");
    }

    #[test]
    fn condensation_is_mass_conservative() {
        let (mut w, mut h) = setup_cloud_world();
        // Spread humidity across a few tiles.
        h.add(1, 30, 400.0);
        h.add(6, 30, 300.0);
        h.add(11, 30, 250.0);
        let total_before = h.total_mass();
        let world_sat_before = ground_sat_sum(&w);

        let cfg = CondensationConfig {
            top_y: 30,
            max_prob_per_tick: 1.0,
            ..CondensationConfig::default()
        };
        for _ in 0..5 {
            apply_condensation_rain(&mut w, &mut h, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }

        let total_after = h.total_mass();
        let world_sat_after = ground_sat_sum(&w);
        let humidity_lost = total_before - total_after;
        let world_gained = (world_sat_after - world_sat_before) as f32;
        assert!(
            (humidity_lost - world_gained).abs() < 1.5,
            "humidity_lost={humidity_lost}, world_gained={world_gained} — mass must balance"
        );
    }

    #[test]
    fn condensation_is_deterministic_for_seed_and_tick() {
        let (mut w1, mut h1) = setup_cloud_world();
        let (mut w2, mut h2) = setup_cloud_world();
        for tile in [(1, 30), (6, 30), (11, 30)] {
            h1.add(tile.0, tile.1, 400.0);
            h2.add(tile.0, tile.1, 400.0);
        }
        let cfg = CondensationConfig {
            top_y: 30,
            max_prob_per_tick: 0.7,
            seed_salt: 12345,
            ..CondensationConfig::default()
        };
        for _ in 0..10 {
            apply_condensation_rain(&mut w1, &mut h1, &cfg);
            apply_condensation_rain(&mut w2, &mut h2, &cfg);
            w1.tick = w1.tick.wrapping_add(1);
            w2.tick = w2.tick.wrapping_add(1);
        }
        for x in 0..CHUNK_CELLS_W as i32 {
            assert_eq!(
                w1.get_cell(x, 1).map(|c| c.sat.0),
                w2.get_cell(x, 1).map(|c| c.sat.0),
                "world state must be deterministic at x={x}"
            );
        }
        assert_eq!(h1.total_mass(), h2.total_mass());
    }

    #[test]
    fn condensation_skips_non_air_landing_cell() {
        let (mut w, mut h) = setup_cloud_world();
        h.add(1, 30, 1000.0);
        // Solid column — nowhere for surface rain to land.
        for y in 1..=30 {
            w.set_cell(2, y, Cell::solid(MaterialId::Stone));
        }
        let mass_before = h.total_mass();
        let cfg = CondensationConfig {
            top_y: 30,
            max_prob_per_tick: 1.0,
            ..CondensationConfig::default()
        };
        apply_condensation_rain(&mut w, &mut h, &cfg);
        assert_eq!(w.get_cell(2, 30).unwrap().material, MaterialId::Stone);
        assert_eq!(h.total_mass(), mass_before);
    }

    #[test]
    fn orographic_boost_rains_thinner_clouds_over_tall_land() {
        use crate::worldgen::WorldgenParams;
        let p = WorldgenParams::default();
        // Find a tile whose centre is well above sea (mountain belt).
        let mut tall_hx = None;
        let tc = 4;
        for hx in 0..(p.width_cols / tc) {
            let gx = hx * tc + tc / 2;
            let s = crate::worldgen::continental_surface_y(
                p.seed,
                gx,
                p.sea_level_y,
                p.width_cols,
            );
            if s >= p.sea_level_y + 22 {
                tall_hx = Some(hx);
                break;
            }
        }
        let tall_hx = tall_hx.expect("worldgen should have tall land");
        // Landing column is the tile centre (matches condensation deposit).
        let centre_gx = tall_hx * tc + tc / 2;
        let surface = crate::worldgen::continental_surface_y(
            p.seed,
            centre_gx,
            p.sea_level_y,
            p.width_cols,
        );
        let mut w = World::new(p.seed);
        // Terrain under the mountain column so rain can land.
        for y in [surface, surface + 1, 40] {
            w.ensure_chunk(ChunkCoord::new(
                centre_gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        w.set_cell(centre_gx, surface, Cell::solid(MaterialId::Stone));
        for y in (surface + 1)..=40 {
            w.set_cell(centre_gx, y, Cell::air());
        }
        let sky = surface + 12;
        for y in (surface + 1)..=sky {
            w.ensure_chunk(ChunkCoord::new(
                centre_gx.div_euclid(CHUNK_CELLS_W as i32),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
            w.set_cell(centre_gx, y, Cell::air());
        }
        let mut h = crate::humidity::Humidity::new(tc);
        // Thin cloud — below default min_mass_to_rain (64) but above
        // orographic-reduced threshold on tall peaks.
        // Add at tile centre so humidity key matches landing column.
        h.add(centre_gx, sky, 50.0);
        let cfg = CondensationConfig {
            top_y: sky,
            min_mass_to_rain: 64.0,
            max_prob_per_tick: 1.0,
            full_mass: 120.0,
            mass_per_droplet: 24.0,
            ..CondensationConfig::default()
        };
        let oro = OrographicConfig {
            seed: p.seed,
            width_cols: p.width_cols,
            sea_level_y: p.sea_level_y,
            tall_above_sea: 22,
            wind_sign: 1,
            ..OrographicConfig::default()
        };
        let before = h.total_mass();
        // Without oro: should not rain (mass 50 < 64).
        for _ in 0..40 {
            apply_condensation_rain(&mut w, &mut h, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(h.total_mass(), before, "thin cloud should not rain flat");
        // With oro over tall land: should dump within a few dozen ticks.
        for _ in 0..40 {
            apply_condensation_rain_with_orographic(&mut w, &mut h, &cfg, Some(&oro));
            w.tick = w.tick.wrapping_add(1);
            if h.total_mass() < before {
                break;
            }
        }
        assert!(
            h.total_mass() < before,
            "orographic rain should drain thin mountain clouds (mass {} → {})",
            before,
            h.total_mass()
        );
    }

    #[test]
    fn karst_low_sat_neighbour_does_not_dissolve() {
        // Air cell above limestone has sat below threshold → no
        // dissolution.
        let mut w = setup_limestone_world();
        let mut wet_ish = Cell::air();
        wet_ish.sat = Sat(50); // below threshold 200
        w.set_cell(10, 11, wet_ish);
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 4,
        };
        for _ in 0..10 {
            apply_karst_dissolution(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(
            w.get_cell(10, 10).unwrap().material,
            MaterialId::Limestone,
            "damp-but-not-wet neighbour must not dissolve karst"
        );
    }

    #[test]
    fn settled_column_goes_quiescent() {
        // Droplet falls down a one-cell-wide bedrock shaft so lateral
        // spill can't keep the row alive forever. Once it rests, the
        // dirty plan empties and physics early-outs.
        let mut w = setup_column_world();
        for y in 1..16 {
            w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
            w.set_cell(5, y, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(4, 8, Cell::water());
        for _ in 0..20 {
            tick(&mut w);
        }
        // Consume any residual dirty from the last write.
        tick(&mut w);
        assert!(
            plan_active(&w).is_empty(),
            "settled world must plan no active chunks"
        );
        // Find where the droplet rested (flow substeps can leave a
        // thin film split across a couple of floor cells).
        let mut rested = None;
        for y in 1..8 {
            if let Some(c) = w.get_cell(4, y) {
                if c.sat.0 > 0 {
                    rested = Some((4, y, c.sat.0));
                    break;
                }
            }
        }
        let (rx, ry, sat_before) = rested.expect("droplet should rest in the shaft");
        tick(&mut w);
        assert_eq!(w.get_cell(rx, ry).unwrap().sat.0, sat_before);
        assert!(plan_active(&w).is_empty());
    }

    #[test]
    fn tick_runs_gravity_then_spill_and_conserves_mass() {
        // Full tick pass: flow substeps drop the droplet several rows
        // and spread it sideways. Total sat is unchanged.
        let mut w = setup_column_world();
        w.set_cell(30, 5, Cell::water());
        let start_mass = 255i64;

        tick(&mut w);
        let after_mass: i64 = (0..64i32)
            .flat_map(|x| (0..64i32).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i64))
            .sum();
        assert_eq!(after_mass, start_mass, "tick must conserve total sat");

        // With FLOW_SUBSTEPS gravity passes, the droplet leaves y=5.
        assert!(w.get_cell(30, 5).unwrap().sat.is_empty());
        // Some water should exist on the floor or just above it, spread
        // across neighbouring columns.
        let wet_near_floor: i32 = (28..=32)
            .flat_map(|x| (1..=4).map(move |y| (x, y)))
            .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.sat.0 > 0).unwrap_or(false))
            .count() as i32;
        assert!(
            wet_near_floor >= 2,
            "droplet should fall and spread (wet cells={wet_near_floor})"
        );
    }

    #[test]
    fn quiescent_lake_still_evaporates() {
        // Dirty-rect physics can go idle while a lake remains. Evap
        // must keep bleeding surface water via the wet-air occupancy
        // flag — not only when the chunk is dirty.
        let mut w = setup_column_world();
        w.set_cell(4, 1, Cell::water());
        clear_all_dirty(&mut w);
        assert!(plan_active(&w).is_empty());
        assert!(w.chunks[&ChunkCoord::new(0, 0)].has_wet_air);

        let mut h = crate::humidity::Humidity::new(4);
        let cfg = EvapConfig {
            rate_per_tick: 5,
            dry_above_max: 200,
            period_ticks: 1,
        };
        apply_evaporation_into_humidity(&mut w, &mut h, &cfg);
        assert!(
            w.get_cell(4, 1).unwrap().sat.0 < u8::MAX,
            "surface water must evaporate even when physics is quiescent"
        );
        assert!(h.total_mass() > 0.0);
    }

    #[test]
    fn karst_skips_chunks_without_limestone_flag() {
        let mut w = setup_column_world();
        // Wet air only — no limestone. Flag stays false; pass is a no-op.
        w.set_cell(4, 2, Cell::water());
        assert!(!w.chunks[&ChunkCoord::new(0, 0)].has_limestone);
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 1,
            seed_salt: 1,
        };
        apply_karst_dissolution(&mut w, &cfg);
        assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Air);
    }

    #[test]
    fn parallel_tick_matches_serial_on_multi_chunk_fixture() {
        // Two-by-two chunk slab with water + sand so gravity, spill,
        // seepage, and grain all fire across several colours.
        fn build() -> World {
            let mut w = World::new(42);
            for cx in 0..2 {
                for cy in 0..2 {
                    w.ensure_chunk(ChunkCoord::new(cx, cy));
                }
            }
            for x in 0..128 {
                w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            }
            w.set_cell(10, 40, Cell::water());
            w.set_cell(70, 90, Cell::water());
            w.set_cell(20, 50, Cell::solid(MaterialId::Sand));
            w.set_cell(90, 100, Cell::solid(MaterialId::Sand));
            w.set_cell(11, 1, Cell::solid(MaterialId::Sand));
            w
        }

        crate::parallel::set_parallel_enabled(false);
        let mut serial = build();
        for _ in 0..30 {
            tick(&mut serial);
        }

        crate::parallel::set_parallel_enabled(true);
        let mut parallel = build();
        for _ in 0..30 {
            tick(&mut parallel);
        }

        for cx in 0..2 {
            for cy in 0..2 {
                let coord = ChunkCoord::new(cx, cy);
                let a = serial.chunks.get(&coord).expect("serial chunk");
                let b = parallel.chunks.get(&coord).expect("parallel chunk");
                assert_eq!(
                    a.cells, b.cells,
                    "parallel tick diverged from serial at {coord:?}"
                );
            }
        }
        // Leave the process default (parallel on) for later tests.
        crate::parallel::set_parallel_enabled(true);
    }
}
