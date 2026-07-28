//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Hydraulic head and shared surface/seepage helpers.

use wk_material::{HydroOverrides, MaterialId, MaterialRegistry};

use crate::cell::{water_capacity_with, Sat};
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

pub(crate) fn is_porous_solid_with(material: MaterialId, hydro: &HydroOverrides) -> bool {
    material != MaterialId::Air && water_capacity_with(material, hydro) > 0
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
pub(crate) fn plan_same_y_pairwise_edge(
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
    let cap = water_capacity_with(MaterialId::Air, &world.hydro);
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
    // Already flat with the neighbour — leave equalize alone.
    let head = cur_sat.saturating_sub(side.sat.0);
    if head <= 1 {
        return None;
    }
    let pull = ((head / 2) as i32).max(1);

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
                return Some(free_imm.min(pull).min(cur_sat as i32));
            }
            break;
        }
    }
    None
}
