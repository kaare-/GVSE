//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Permeability-limited pore soak.

use wk_material::MaterialId;

use crate::active::ActiveChunk;
use crate::cell::{water_capacity_with, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::map_regions_parallel;

use super::head::{
    is_porous_solid_with, sat_move_to_equalize_heads, seepage_conduct_rate_with, seepage_rate_with,
    seepage_uptake_rate_with,
};
use super::plan::{regions_for_standalone, regions_wet_loaded};

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
/// that only hold pore water, so we couple the seam rows explicitly.
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
        let y_lo = coord.cy * ch + (ch - 1);
        let y_hi = y_lo + 1;
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
            // Any cross-seam moisture that can still move.
            let lo_cap = water_capacity_with(lo.material, &hydro);
            let hi_cap = water_capacity_with(hi.material, &hydro);
            let lo_room = lo_pore && lo_cap > 0 && lo.sat.0 < lo_cap;
            let hi_room = hi_pore && hi_cap > 0 && hi.sat.0 < hi_cap;
            let lo_wet = lo.sat.0 > 0;
            let hi_wet = hi.sat.0 > 0;
            if (lo_wet || hi_wet || lo_air || hi_air) && (lo_room || hi_room) {
                if lo_room {
                    touches.push((gx, y_lo));
                }
                if hi_room {
                    touches.push((gx, y_hi));
                }
            }
        }
    }
    for (gx, gy) in touches {
        world.touch_dirty(gx, gy);
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
                    // into a drier neighbour when the donor is meaningfully
                    // wet. Otherwise a residual film pipes to bedrock and
                    // pools there while the mid-column stays "dry".
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
                            // Donor must be ≥ ~30% full to advance the front.
                            if cap_d == 0 || (sat_d as i32) * 10 < (cap_d as i32) * 3 {
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
                    // Open flowing films only splash-wet a dry bank so the
                    // leading edge does not vanish into pores. Standing
                    // pond / lake faces (and settled beds) soak at the
                    // full uptake rate — sides and bottoms both recharge.
                    let rate = {
                        let air_solid = a_solid != b_solid;
                        if !air_solid {
                            rate
                        } else {
                            let air = if a_solid { &b } else { &a };
                            let standing_face = air.material == MaterialId::Air && air.sat.0 >= 160;
                            if standing_face {
                                rate
                            } else if dx != 0 {
                                rate.min(SHEET_FACE_SPLASH)
                            } else {
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

/// True when standing Air still has a runoff path: open (non-full) Air
/// neighbour, or diagonal/cascade downhill Air with room. Hillside blobs
/// must shed along the slope instead of soaking their seat at full perm.
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
        match read(nlx, ly, nx, gy) {
            Some(c) if c.material == MaterialId::Air && !c.sat.is_full() => return true,
            _ => {}
        }
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
