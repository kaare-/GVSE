//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Hydraulic head and shared surface/seepage helpers.

use wk_material::{HydroOverrides, MaterialId, MaterialRegistry};

use crate::cell::{permeability_cell, water_capacity_cell, water_capacity_with, Cell, Sat};
use crate::chunk::{Chunk, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Free-surface / pore hydraulic head in cell units:
/// `y + sat / capacity`. Adjacent cells equalise toward matching heads.
pub(crate) fn hydraulic_head(gy: i32, sat: Sat, capacity: u8) -> f32 {
    if capacity == 0 {
        return gy as f32;
    }
    gy as f32 + (sat.0 as f32) / (capacity as f32)
}

/// Saturation to move from A → B (positive) or B → A (negative) so the
/// pair's heads meet in the middle. Clamped to available sat / free
/// capacity. Both capacities must be > 0.
pub(crate) fn sat_move_to_equalize_heads(
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
pub(crate) fn seepage_rate_with(material: MaterialId, hydro: &HydroOverrides) -> i32 {
    let p = MaterialRegistry::props_with(material, hydro).permeability;
    if p == 0 {
        return 0;
    }
    // Cap at 32 sat-units/tick at permeability 255 (gravel-ish).
    ((p as i32 * 32) / 255).max(1)
}

#[inline]
pub(crate) fn seepage_rate_cell(cell: Cell, hydro: &HydroOverrides) -> i32 {
    let p = permeability_cell(cell, hydro);
    if p == 0 {
        return 0;
    }
    ((p as i32 * 32) / 255).max(1)
}

/// Permeability that yields exactly one sat-unit per pass in
/// [`seepage_rate_cell`]. Below this the rate floors at 1 and stops resolving.
pub(crate) const SEEPAGE_RATE_QUANTUM: i32 = 255 / 32 + 1;

/// How many seepage passes a cell waits between transfers, for permeabilities
/// too low for [`seepage_rate_cell`] to express.
///
/// That rate floors at one sat-unit per pass, so **every** material below
/// permeability 8 conducted identically: clay (10), flowstone (12) and
/// bentonite (1) all came out at 1. The whole tight end of the spectrum was
/// quantised into a single value, which made a real aquitard impossible to
/// express and flattened the pore variation on tight rock that the fracture
/// tail is supposed to produce.
///
/// Striding recovers the resolution without fractional sat: permeability 1
/// transfers one unit every 8 passes, which is exactly 1/8 of permeability 8.
#[inline]
pub(crate) fn seepage_stride_cell(cell: Cell, hydro: &HydroOverrides) -> u32 {
    let p = permeability_cell(cell, hydro) as i32;
    if p <= 0 {
        return u32::MAX;
    }
    if p >= SEEPAGE_RATE_QUANTUM {
        1
    } else {
        (SEEPAGE_RATE_QUANTUM / p).max(1) as u32
    }
}

/// Surface / top-layer infiltration into a porous cell this step.
///
/// Bone-dry ground takes only a trickle so most free water can run past
/// (overland / sheet flow). As the contact cell wets, uptake climbs toward
/// the full [`seepage_rate_with`] — like a wetting front opening pores.
/// Full cells take nothing more (`free == 0`).
///
/// Peer solid↔solid flow uses [`seepage_conduct_rate_with`] instead.
pub(crate) fn seepage_uptake_rate_with(
    material: MaterialId,
    hydro: &HydroOverrides,
    sat: u8,
    cap: u8,
) -> i32 {
    let base = seepage_rate_with(material, hydro);
    if base <= 0 || cap == 0 {
        return 0;
    }
    let free = cap.saturating_sub(sat) as i32;
    if free <= 0 {
        return 0;
    }
    // Wetness fraction with a small dry kick so sat=0 still seeps ~1/8 rate
    // instead of stalling forever as a perfect seal.
    //   sat=0     → ~base/8
    //   sat=cap/2 → ~base/2
    //   sat→cap   → →base (clamped by free)
    let kick = (cap as i32 / 8).max(1);
    let scaled = (base * (sat as i32 + kick)) / (cap as i32 + kick);
    scaled.max(1).min(free).min(base)
}

pub(crate) fn seepage_uptake_rate_cell(cell: Cell, hydro: &HydroOverrides, cap: u8) -> i32 {
    let base = seepage_rate_cell(cell, hydro);
    if base <= 0 || cap == 0 {
        return 0;
    }
    let free = cap.saturating_sub(cell.sat.0) as i32;
    if free <= 0 {
        return 0;
    }
    let kick = (cap as i32 / 8).max(1);
    let scaled = (base * (cell.sat.0 as i32 + kick)) / (cap as i32 + kick);
    scaled.max(1).min(free).min(base)
}

/// Solid↔solid pore conduction: permeability × relative wetness.
///
/// The drier neighbour is the bottleneck — bone-dry underground paths
/// crawl, while a saturated pair runs at the slower material's full
/// [`seepage_rate_with`]. Same dry-kick curve as surface uptake so dry
/// sand does not flash-equalise an aquifer next to wet sand.
pub(crate) fn seepage_conduct_rate_with(
    mat_a: MaterialId,
    hydro: &HydroOverrides,
    sat_a: u8,
    cap_a: u8,
    mat_b: MaterialId,
    sat_b: u8,
    cap_b: u8,
) -> i32 {
    let base = seepage_rate_with(mat_a, hydro).min(seepage_rate_with(mat_b, hydro));
    if base <= 0 || cap_a == 0 || cap_b == 0 {
        return 0;
    }
    let kick_a = (cap_a as i32 / 8).max(1);
    let kick_b = (cap_b as i32 / 8).max(1);
    let wa = sat_a as i32 + kick_a;
    let wb = sat_b as i32 + kick_b;
    let ca = cap_a as i32 + kick_a;
    let cb = cap_b as i32 + kick_b;
    // base * min(wa/ca, wb/cb)
    let num = (wa * cb).min(wb * ca);
    let den = ca * cb;
    let scaled = (base * num) / den;
    scaled.max(1).min(base)
}

pub(crate) fn seepage_conduct_rate_cells(
    cell_a: Cell,
    cap_a: u8,
    cell_b: Cell,
    cap_b: u8,
    hydro: &HydroOverrides,
) -> i32 {
    let base = seepage_rate_cell(cell_a, hydro).min(seepage_rate_cell(cell_b, hydro));
    if base <= 0 || cap_a == 0 || cap_b == 0 {
        return 0;
    }
    let kick_a = (cap_a as i32 / 8).max(1);
    let kick_b = (cap_b as i32 / 8).max(1);
    let wa = cell_a.sat.0 as i32 + kick_a;
    let wb = cell_b.sat.0 as i32 + kick_b;
    let ca = cap_a as i32 + kick_a;
    let cb = cap_b as i32 + kick_b;
    let num = (wa * cb).min(wb * ca);
    let den = ca * cb;
    ((base * num) / den).max(1).min(base)
}

pub(crate) fn is_porous_solid_with(material: MaterialId, hydro: &HydroOverrides) -> bool {
    material != MaterialId::Air && water_capacity_with(material, hydro) > 0
}

#[inline]
pub(crate) fn is_porous_cell(cell: Cell, hydro: &HydroOverrides) -> bool {
    cell.material != MaterialId::Air && water_capacity_cell(cell, hydro) > 0
}

/// How far same-Y lake equalise looks for a drier surface cell / edge.
pub(crate) const SAME_Y_SURFACE_SCAN: i32 = 12;

/// True when the cell below can support a standing free surface
/// (solid ground or a full water column).
pub(crate) fn is_surface_support(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy - 1) {
        Some(b) if b.material != MaterialId::Air => true,
        Some(b) => b.sat.is_full(),
        None => false,
    }
}

/// Plan a single +x standing-surface head equalise for the edge
/// `(gx,gy) — (gx+1,gy)`. Emits at most one transfer, owned by the
/// left endpoint so each edge is solved once per pass.
///
/// `max_move` soft-caps the transfer so cascade-pull + equalise cannot
/// empty a cell into its neighbour in one hop (jagged 255|0 fronts).
///
/// `chunk`/`(lx,ly)` are an optional chunk-local read cache; when set,
/// neighbour probes that fall inside the chunk skip `world.get_cell`
/// (~10× cheaper) — see `get_cell_microbench`.
pub(crate) fn plan_same_y_pairwise_edge_in(
    world: &World,
    chunk: Option<(&Chunk, i32, i32)>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    max_move: i32,
    local: &mut Vec<((i32, i32), (i32, i32), i32)>,
) {
    if max_move <= 0 {
        return;
    }
    let nx = world.wrap_x(gx + 1);
    if nx == gx {
        return;
    }
    let read = |ax: i32, ay: i32, cx: i32, cy: i32| -> Option<Cell> {
        if let Some((chunk, _bx, _by)) = chunk {
            if cx >= 0 && cx < CHUNK_CELLS_W as i32 && cy >= 0 && cy < CHUNK_CELLS_H as i32 {
                return Some(chunk.get(cx as usize, cy as usize));
            }
        }
        world.get_cell(ax, ay)
    };
    // is_surface_support checks (x, y-1). Chunk-local when in range.
    let left_below = read(gx, gy - 1, lx, ly - 1);
    let left_support = matches!(
        left_below,
        Some(b) if b.material != MaterialId::Air || b.sat.is_full()
    );
    if !left_support {
        return;
    }
    let right_below = read(nx, gy - 1, lx + 1, ly - 1);
    let right_support = matches!(
        right_below,
        Some(b) if b.material != MaterialId::Air || b.sat.is_full()
    );
    if !right_support {
        return;
    }
    let Some(left) = read(gx, gy, lx, ly) else {
        return;
    };
    let Some(right) = read(nx, gy, lx + 1, ly) else {
        return;
    };
    if left.material != MaterialId::Air || right.material != MaterialId::Air {
        return;
    }
    let cap = water_capacity_with(MaterialId::Air, &world.hydro);
    let move_amt = sat_move_to_equalize_heads(left.sat.0, cap, gy, right.sat.0, cap, gy);
    if move_amt > 0 {
        let free = u8::MAX.saturating_sub(right.sat.0) as i32;
        let amt = move_amt.min(left.sat.0 as i32).min(free).min(max_move);
        if amt > 0 {
            local.push(((gx, gy), (nx, gy), amt));
        }
    } else if move_amt < 0 {
        let free = u8::MAX.saturating_sub(left.sat.0) as i32;
        let amt = (-move_amt).min(right.sat.0 as i32).min(free).min(max_move);
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
///
/// Pull only the head excess over the neighbour (half the delta) —
/// dumping *all* remaining sat fought pairwise equalize and piled
/// 1–2 cell shore spikes forever on otherwise flat lakes.
pub(crate) fn same_y_cascade_pull(
    world: &World,
    gx: i32,
    gy: i32,
    dir: i32,
    cur_sat: u8,
) -> Option<i32> {
    same_y_cascade_pull_in(world, None, gx, gy, 0, 0, dir, cur_sat)
}

/// [`same_y_cascade_pull`] with an optional chunk-local read cache
/// (same contract as [`plan_same_y_pairwise_edge_in`]).
pub(crate) fn same_y_cascade_pull_in(
    world: &World,
    chunk: Option<(&Chunk, i32, i32)>,
    gx: i32,
    gy: i32,
    lx: i32,
    ly: i32,
    dir: i32,
    cur_sat: u8,
) -> Option<i32> {
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let read = |ax: i32, ay: i32, cx: i32, cy: i32| -> Option<Cell> {
        if let Some((chunk, _bx, _by)) = chunk {
            if cx >= 0 && cx < cw && cy >= 0 && cy < ch {
                return Some(chunk.get(cx as usize, cy as usize));
            }
        }
        world.get_cell(ax, ay)
    };

    let immediate = world.wrap_x(gx + dir);
    let Some(side) = read(immediate, gy, lx + dir, ly) else {
        return None;
    };
    if side.material != MaterialId::Air {
        return None;
    }
    let free_imm = u8::MAX.saturating_sub(side.sat.0) as i32;
    if free_imm == 0 {
        return None;
    }
    // Already flat with the neighbour — leave equalize alone.
    let head = cur_sat.saturating_sub(side.sat.0);
    if head <= 1 {
        return None;
    }
    let pull = ((head / 2) as i32).max(1);

    // Immediate cascade is priority 2 — skip duplicate dump here.
    let immediate_cascade = matches!(
        read(immediate, gy - 1, lx + dir, ly - 1),
        Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
    );
    if immediate_cascade {
        return None;
    }

    let mut x = immediate;
    let mut clx = lx + dir;
    for _ in 1..SAME_Y_SURFACE_SCAN {
        x = world.wrap_x(x + dir);
        clx += dir;
        if x == gx {
            break;
        }
        let Some(cell) = read(x, gy, clx, ly) else {
            break;
        };
        // Thin floating Organic is a soft lid — look past it so cascade
        // pull still levels water around shore mats (otherwise free
        // surfaces comb against organic dams).
        if cell.material == MaterialId::Organic {
            let soft_lid = !cell.is_waterlogged_organic()
                && matches!(
                    read(x, gy - 1, clx, ly - 1),
                    Some(b) if b.material == MaterialId::Air && b.sat.0 >= 200
                );
            if soft_lid {
                continue;
            }
            break;
        }
        if cell.material != MaterialId::Air {
            break;
        }
        let below = read(x, gy - 1, clx, ly - 1);
        let supported = matches!(
            below,
            Some(b) if b.material != MaterialId::Air || b.sat.is_full()
        );
        if !supported {
            if matches!(
                below,
                Some(b) if b.material == MaterialId::Air && !b.sat.is_full()
            ) {
                return Some(free_imm.min(pull).min(cur_sat as i32));
            }
            break;
        }
    }
    None
}
