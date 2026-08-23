//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Permeability-limited pore soak.

use wk_material::MaterialId;

use crate::active::ActiveChunk;
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::map_regions_parallel;

use super::head::{
    is_porous_solid_with, sat_move_to_equalize_heads, seepage_conduct_rate_with, seepage_rate_with,
    seepage_uptake_rate_with,
};
use super::plan::{regions_for_standalone, regions_wet_loaded};

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
        if !is_porous_solid_with(cell.material, hydro) {
            break;
        }
        let cap = water_capacity_with(cell.material, hydro);
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
/// saturated the touch set is empty (steady lakes stay quiet).
///
/// Also walks **down from standing wet Air** so beds that live in the
/// chunk below (y=63 under water at y=64) are woken — `has_wet_air` alone
/// never visits that dry cy-1 chunk.
pub fn wake_lake_bed_pores(world: &mut World) {
    let hydro = world.hydro;
    let regions = regions_wet_loaded(world);
    let mut touches: Vec<(i32, i32)> = Vec::new();
    for ac in &regions {
        let Some(chunk) = world.chunks.get(&ac.coord) else {
            continue;
        };
        let base_gx = ac.coord.cx * CHUNK_CELLS_W as i32;
        let base_gy = ac.coord.cy * CHUNK_CELLS_H as i32;
        for y in ac.rect.y0..=ac.rect.y1 {
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
                if cell.material == MaterialId::Air && cell.sat.0 >= 160 {
                    let mut yy = gy - 1;
                    for _ in 0..(CHUNK_CELLS_H * 2) {
                        let Some(below) = world.get_cell(gx, yy) else {
                            break;
                        };
                        if !is_porous_solid_with(below.material, &hydro) {
                            break;
                        }
                        let cap = water_capacity_with(below.material, &hydro);
                        if cap == 0 {
                            break;
                        }
                        if below.sat.0 < cap {
                            touches.push((gx, yy));
                            break;
                        }
                        yy -= 1;
                    }
                    for dx in [-1_i32, 1] {
                        let nx = world.wrap_x(gx + dx);
                        if let Some(n) = world.get_cell(nx, gy) {
                            if is_porous_solid_with(n.material, &hydro) {
                                let cap = water_capacity_with(n.material, &hydro);
                                if cap > 0 && n.sat.0 < cap {
                                    touches.push((nx, gy));
                                }
                            }
                        }
                    }
                }

                if !is_porous_solid_with(cell.material, &hydro) {
                    continue;
                }
                let cap = water_capacity_with(cell.material, &hydro);
                if cap == 0 || cell.sat.0 >= cap {
                    continue;
                }
                let mut feed = false;
                if let Some(above) = world.get_cell(gx, gy + 1) {
                    if above.material == MaterialId::Air && above.sat.0 >= 160 {
                        feed = true;
                    } else if is_porous_solid_with(above.material, &hydro)
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
        for lx in 0..cw {
            let gx = world.wrap_x(base_gx + lx);
            let Some(lo) = world.get_cell(gx, y_lo) else {
                continue;
            };
            let Some(hi) = world.get_cell(gx, y_hi) else {
                continue;
            };
            let lo_pore = is_porous_solid_with(lo.material, &hydro);
            let hi_pore = is_porous_solid_with(hi.material, &hydro);
            let lo_air = lo.material == MaterialId::Air && lo.sat.0 >= 160;
            let hi_air = hi.material == MaterialId::Air && hi.sat.0 >= 160;
            if !((lo_pore || lo_air) && (hi_pore || hi_air)) {
                continue;
            }
            let lo_cap = water_capacity_with(lo.material, &hydro);
            let hi_cap = water_capacity_with(hi.material, &hydro);
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
                if is_porous_solid_with(c.material, &hydro) {
                    let cap = water_capacity_with(c.material, &hydro);
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
        let strip_lo = Rect {
            x0: 0,
            y0: (ch - depth_lo).max(0) as u8,
            x1: (cw - 1) as u8,
            y1: (ch - 1) as u8,
        };
        let strip_hi = Rect {
            x0: 0,
            y0: 0,
            x1: (cw - 1) as u8,
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
                    // Wetting-front plug: pore water may only drive *down*
                    // into a drier neighbour when the donor is more than a
                    // residual film. A 30% gate left shore shelves stuck at
                    // sat≈cap/4 on the U heatmap (playtest stone 5/20).
                    if a_solid && b_solid && move_amt != 0 {
                        let downward = if move_amt > 0 {
                            gy > ny // a → b and a is higher
                        } else {
                            ny > gy // b → a and b is higher
                        };
                        if downward {
                            let (sat_d, cap_d) = if move_amt > 0 {
                                (a.sat.0, cap_a)
                            } else {
                                (b.sat.0, cap_b)
                            };
                            // Residual film only (≤10% / sat≤2) — blocks the
                            // old bedrock pipe, still lets groundwater crawl.
                            if cap_d == 0
                                || sat_d <= 2
                                || (sat_d as i32) * 10 < (cap_d as i32)
                            {
                                move_amt = 0;
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
                        seepage_conduct_rate_with(
                            a.material, &hydro, a.sat.0, cap_a, b.material, b.sat.0, cap_b,
                        )
                    } else if move_amt > 0 {
                        // A → B: infiltrating into B, or A weeping into Air.
                        if b_solid {
                            if a.material == MaterialId::Air && a.sat.0 >= 160 {
                                if standing_air_is_runoff(world, &read, gx, gy, lx, ly) {
                                    seepage_uptake_rate_with(b.material, &hydro, b.sat.0, cap_b)
                                } else {
                                    seepage_rate_with(b.material, &hydro)
                                }
                            } else {
                                seepage_uptake_rate_with(b.material, &hydro, b.sat.0, cap_b)
                            }
                        } else {
                            seepage_rate_with(a.material, &hydro)
                        }
                    } else {
                        // B → A: infiltrating into A, or B weeping into Air.
                        if a_solid {
                            if b.material == MaterialId::Air && b.sat.0 >= 160 {
                                if standing_air_is_runoff(world, &read, nx, ny, lx + dx, ly + dy)
                                {
                                    seepage_uptake_rate_with(a.material, &hydro, a.sat.0, cap_a)
                                } else {
                                    seepage_rate_with(a.material, &hydro)
                                }
                            } else {
                                seepage_uptake_rate_with(a.material, &hydro, a.sat.0, cap_a)
                            }
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
    const DIRS: [(i32, i32); 4] = [(-1, 0), (1, 0), (0, -1), (0, 1)];
    for (&coord, chunk) in &world.chunks {
        let base_gx = coord.cx * cw;
        let base_gy = coord.cy * ch;
        for y in 0..CHUNK_CELLS_H {
            for x in 0..CHUNK_CELLS_W {
                let cell = chunk.get(x, y);
                if !is_porous_solid_with(cell.material, &hydro) {
                    continue;
                }
                let cap = water_capacity_with(cell.material, &hydro);
                // Need a meaningful donor — residual film only skipped.
                if cap == 0 || cell.sat.0 <= 2 {
                    continue;
                }
                let gx = world.wrap_x(base_gx + x as i32);
                let gy = base_gy + y as i32;
                let mut face = false;
                for (dx, dy) in DIRS {
                    let nx = world.wrap_x(gx + dx);
                    let ny = gy + dy;
                    let Some(n) = world.get_cell(nx, ny) else {
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
                        let nx = world.wrap_x(gx + dx);
                        let ny = gy + dy;
                        let Some(n) = world.get_cell(nx, ny) else {
                            continue;
                        };
                        if is_porous_solid_with(n.material, &hydro) && n.sat.0 > 0 {
                            touches.push((nx, ny));
                        }
                    }
                }
            }
        }
    }
    for (gx, gy) in touches {
        world.touch_dirty(gx, gy);
    }
}
