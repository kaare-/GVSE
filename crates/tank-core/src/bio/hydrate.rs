//! Phase 4 — hydration, root co-occupancy, air-side habitability
//! (`cell-language.md` §h.1).

use wk_material::MaterialId;

use crate::bio::store::{CellStore, NONE, NONE16, OWNER_BIAS};
use crate::bio::{GenomeStore, ModeCoeffs};
use crate::cell::Cell;
use crate::consts::{
    HABITABLE_H, HYDRATION_CONTACT, HYDRATION_DROP, HYDRATION_DROP_HARD, MOIST_SAT, ROOT_BOUND_PIXELS,
};
use crate::grid::PhysicsGrid;
use crate::humidity::{is_air_cell, is_moist_substrate, is_water_cell, substrate_moisture};
use crate::pixels::{CoarseField, PixelLayer};

use super::minerals;
use super::step::pixel_to_phys;

/// Sand / soil a root may co-occupy.
#[inline]
pub fn is_rootable(c: Cell) -> bool {
    matches!(c.id(), MaterialId::Sand | MaterialId::Soil | MaterialId::Organic)
        && substrate_moisture(c) >= MOIST_SAT
}

/// Whether a pixel is a habitable seat for a cell with the given coeffs.
/// Free water always; moist air / moist substrate for any cell; moist
/// sand/soil co-occupancy only for UPTAKE(water) ≥ 2.
pub fn seat_ok(
    grid: &PhysicsGrid,
    coarse: &CoarseField,
    pixels: &PixelLayer,
    nx: i32,
    ny: i32,
    mc: &ModeCoeffs,
) -> bool {
    if nx < 0 || ny < 0 || nx >= pixels.w as i32 || ny >= pixels.h as i32 {
        return false;
    }
    let idx = (ny as usize) * (pixels.w as usize) + nx as usize;
    if pixels.owner[idx] != 0 {
        return false;
    }
    let vis_h = grid.visible_h() as u32;
    let (vx, vy) = pixel_to_phys(nx as u16, ny as u16, vis_h);
    let seat = grid.get_visible(vx, vy);
    if is_water_cell(seat) {
        return true;
    }
    if is_air_cell(seat) {
        let h = coarse
            .humidity
            .get(minerals::idx_of_pixel(coarse, nx as u16, ny as u16))
            .map_or(0, |&q| (q >> 8) as u8);
        return h >= HABITABLE_H;
    }
    // Solid: only roots enter moist sand/soil.
    mc.uptake_water >= 2 && is_rootable(seat)
}

/// Source hydration of a cell from its seat (before bond conduction).
pub fn source_hydration(
    grid: &PhysicsGrid,
    coarse: &CoarseField,
    cell_px: u16,
    cell_py: u16,
    uptake_water: u8,
) -> u8 {
    let vis_h = grid.visible_h() as u32;
    let (vx, vy) = pixel_to_phys(cell_px, cell_py, vis_h);
    let seat = grid.get_visible(vx, vy);
    if is_water_cell(seat) {
        return 255;
    }
    let ci = minerals::idx_of_pixel(coarse, cell_px, cell_py);
    let air_h = coarse.humidity.get(ci).map_or(0, |&q| (q >> 8) as u8);
    let moist = if !seat.is_open() {
        substrate_moisture(seat)
    } else {
        // Moist air counts as contact with a wet surface when humid enough.
        air_h
    };
    if moist < MOIST_SAT && air_h < HABITABLE_H {
        return 0;
    }
    if uptake_water >= 1 {
        255
    } else if moist >= MOIST_SAT || air_h >= HABITABLE_H {
        HYDRATION_CONTACT
    } else {
        0
    }
}

/// Recompute every live cell's hydration: source from the seat, then
/// conduct along division bonds (BFS by depth within each organism).
pub fn refresh(cells: &mut CellStore, genomes: &GenomeStore, grid: &PhysicsGrid, coarse: &CoarseField) {
    let ids = cells.live_ids();
    for id in ids {
        let Some(c) = cells.get(id).copied() else {
            continue;
        };
        let uptake = genomes.coeffs(c.genotype, c.mode).uptake_water;
        let src = source_hydration(grid, coarse, c.px, c.py, uptake);
        if let Some(cell) = cells.get_mut(id) {
            cell.hydration = src;
        }
    }
    // Conduct parent → child along division bonds (glue does not).
    let orgs: Vec<u32> = cells.iter_orgs().map(|(id, _)| id).collect();
    for org_id in orgs {
        let Some(o) = cells.org(org_id).cloned() else {
            continue;
        };
        // Slots are stored in BFS order (parent before children).
        for (si, s) in o.slots.iter().enumerate() {
            if s.cell == NONE || s.parent_slot == NONE16 {
                continue;
            }
            let Some(ps) = o.slots.get(s.parent_slot as usize) else {
                continue;
            };
            if ps.cell == NONE {
                continue;
            }
            let Some(parent) = cells.get(ps.cell).copied() else {
                continue;
            };
            let drop = if genomes.coeffs(parent.genotype, parent.mode).wall_h >= 128 {
                HYDRATION_DROP_HARD
            } else {
                HYDRATION_DROP
            };
            let conducted = parent.hydration.saturating_sub(drop);
            if let Some(child) = cells.get_mut(s.cell) {
                if conducted > child.hydration {
                    child.hydration = conducted;
                }
            }
            let _ = si;
        }
    }
}

/// Count root pixels per visible physics cell; return those with ≥
/// `ROOT_BOUND_PIXELS` (bound sand) and the set of organism ids that
/// have at least one such pixel (anchored bodies).
pub fn bound_cells(
    cells: &CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &PixelLayer,
) -> (Vec<(i32, i32)>, Vec<u32>) {
    let vis_h = grid.visible_h() as u32;
    let mut counts: Vec<((i32, i32), u8, u32)> = Vec::new();
    for (id, c) in cells.iter_alive() {
        if genomes.coeffs(c.genotype, c.mode).uptake_water < 2 {
            continue;
        }
        let (vx, vy) = pixel_to_phys(c.px, c.py, vis_h);
        let seat = grid.get_visible(vx, vy);
        if seat.is_open() {
            continue;
        }
        if !is_rootable(seat) && !is_moist_substrate(seat) {
            // Already rooted into sand that may have dried a little —
            // still count if the owner pixel sits on sand/soil.
            if !matches!(seat.id(), MaterialId::Sand | MaterialId::Soil | MaterialId::Organic) {
                continue;
            }
        }
        let idx = (c.py as usize) * (pixels.w as usize) + c.px as usize;
        if pixels.owner.get(idx).copied().unwrap_or(0) != id + OWNER_BIAS {
            continue;
        }
        match counts.iter_mut().find(|(k, _, _)| *k == (vx, vy)) {
            Some((_, n, org)) => {
                *n = n.saturating_add(1);
                *org = c.org;
            }
            None => counts.push(((vx, vy), 1, c.org)),
        }
    }
    let mut bound = Vec::new();
    let mut anchored = Vec::new();
    for ((vx, vy), n, org) in counts {
        if n >= ROOT_BOUND_PIXELS {
            bound.push((vx, vy));
            if !anchored.contains(&org) {
                anchored.push(org);
            }
        }
    }
    (bound, anchored)
}

/// True when the organism has a bound root (static against slosh).
pub fn is_anchored(anchored: &[u32], org: u32) -> bool {
    anchored.contains(&org)
}
