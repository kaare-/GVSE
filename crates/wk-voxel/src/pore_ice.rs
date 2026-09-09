//! Pore ice — freeze saturated rock pores **in place**.
//!
//! Free-surface lakes become [`MaterialId::Ice`] / Snow in [`crate::phase`].
//! That path must not teach rock pores to swap material: frost heave and a
//! world of Ice cells would thrash mass and occupancy. Here the host stays
//! Sand/Stone/etc., `sat` stays on the cell (audit-flat), and a sparse map
//! marks the water as frozen so seepage / throughflow / confined pressure
//! refuse the cell until thaw.
//!
//! See docs/VOXEL_GEYSER.md § P1.

use wk_material::MaterialId;

use crate::cell::{water_capacity_cell, Cell};
use crate::chunk::ChunkCoord;
use crate::grid::World;
use crate::steam::{chunk_overlaps_cold, for_each_tile_cell_where};
use crate::temperature::Temperature;

/// Cadence for the freeze/thaw scan. Matches seepage-class amortisation.
pub const PORE_ICE_EVERY: u64 = 5;

/// Frozen pore sat recorded for `(gx, gy)`, or 0 when liquid / absent.
#[inline]
pub fn pore_ice_at(world: &World, gx: i32, gy: i32) -> u8 {
    let gx = world.wrap_x(gx);
    world.pore_ice.get(&(gx, gy)).copied().unwrap_or(0)
}

/// True when this cell's pore water is frozen in place.
#[inline]
pub fn is_frozen(world: &World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    world.pore_ice.contains_key(&(gx, gy))
}

/// Mark pore water frozen. Host material and `sat` are unchanged.
pub fn freeze_pores(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    if !can_host_pore_ice(cell, &world.hydro) {
        return;
    }
    if cell.sat.0 == 0 {
        return;
    }
    world.pore_ice.insert((gx, gy), cell.sat.0);
}

/// Clear the freeze mark. `sat` is already on the cell — mass stays flat.
pub fn thaw_pores(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    world.pore_ice.remove(&(gx, gy));
}

#[inline]
fn can_host_pore_ice(cell: Cell, hydro: &wk_material::HydroOverrides) -> bool {
    if matches!(
        cell.material,
        MaterialId::Air | MaterialId::Ice | MaterialId::Snow | MaterialId::Bedrock
    ) {
        return false;
    }
    water_capacity_cell(cell, hydro) > 0
}

/// Freeze / thaw saturated pores from the thermal field.
///
/// Only walks wet-pore chunks (freeze) and existing map keys (thaw). Does not
/// touch free-surface Ice/Snow.
pub fn apply_pore_ice(world: &mut World, temp: &Temperature, freeze_point_c: f32) {
    if world.tick % PORE_ICE_EVERY != 0 {
        return;
    }
    // Thaw first so a warm snap clears seals before we re-freeze.
    if !world.pore_ice.is_empty() {
        let keys: Vec<(i32, i32)> = world.pore_ice.keys().copied().collect();
        for (gx, gy) in keys {
            if temp.at_cell(gx, gy) > freeze_point_c {
                thaw_pores(world, gx, gy);
            }
        }
    }

    // Hot wet geothermal chunks stay `has_wet_pores` forever but never
    // freeze — skip them (and warm tiles inside mixed chunks) instead of
    // walking 4096 cells for `temp.at_cell > freeze`.
    let coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(coord, c)| {
            c.has_wet_pores && chunk_overlaps_cold(temp, **coord, freeze_point_c)
        })
        .map(|(k, _)| *k)
        .collect();
    let mut freeze: Vec<(i32, i32, u8)> = Vec::new();
    for coord in coords {
        for_each_tile_cell_where(
            world,
            temp,
            coord,
            |t| t <= freeze_point_c,
            |gx, gy, _t_c, cell| {
                if cell.sat.0 == 0 || !can_host_pore_ice(cell, &world.hydro) {
                    return;
                }
                freeze.push((gx, gy, cell.sat.0));
            },
        );
    }
    for (gx, gy, sat) in freeze {
        // Already frozen: refresh amount if sat changed.
        world.pore_ice.insert((gx, gy), sat);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::sat_totals;
    use crate::cell::Sat;
    use crate::chunk::ChunkCoord;
    use crate::phase::PhaseConfig;
    use crate::rules::{apply_seepage, apply_water_flow, wake_confined_head};
    use wk_material::MaterialId;

    fn cold_temp(world: &World) -> Temperature {
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            0,
            64,
            64,
            world.seed.0,
            64,
            20,
            false,
        );
        for v in t.cells.values_mut() {
            *v = -10.0;
        }
        t
    }

    fn warm_temp(world: &World) -> Temperature {
        let mut t = Temperature::with_world_bounds(
            4,
            0,
            0,
            64,
            64,
            world.seed.0,
            64,
            20,
            false,
        );
        for v in t.cells.values_mut() {
            *v = 12.0;
        }
        t
    }

    fn saturated_sand_pair() -> World {
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let cap = crate::cell::water_capacity(MaterialId::Sand);
        for x in 2..6 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(cap);
            w.set_cell(x, 1, sand);
        }
        // Dry neighbour that would drink if seepage ran.
        let mut dry = Cell::solid(MaterialId::Sand);
        dry.sat = Sat(0);
        w.set_cell(6, 1, dry);
        w
    }

    #[test]
    fn freeze_blocks_seepage_across_saturated_sand() {
        let mut w = saturated_sand_pair();
        let freeze = PhaseConfig::default().freeze_point_c;
        let cold = cold_temp(&w);
        w.tick = PORE_ICE_EVERY;
        apply_pore_ice(&mut w, &cold, freeze);
        assert!(is_frozen(&w, 4, 1), "saturated sand must freeze in place");
        let before = w.get_cell(6, 1).unwrap().sat.0;
        apply_seepage(&mut w);
        assert_eq!(
            w.get_cell(6, 1).unwrap().sat.0,
            before,
            "frozen pores must not seep into a dry neighbour"
        );
    }

    #[test]
    fn thaw_restores_liquid_and_keeps_sat_flat() {
        let mut w = saturated_sand_pair();
        let freeze = PhaseConfig::default().freeze_point_c;
        let cold = cold_temp(&w);
        w.tick = PORE_ICE_EVERY;
        apply_pore_ice(&mut w, &cold, freeze);
        let before = sat_totals(&w);
        assert!(is_frozen(&w, 4, 1));
        let warm = warm_temp(&w);
        w.tick = PORE_ICE_EVERY * 2;
        apply_pore_ice(&mut w, &warm, freeze);
        assert!(!is_frozen(&w, 4, 1), "warm snap must clear pore ice");
        assert_eq!(sat_totals(&w), before, "thaw must not mint or delete sat");
    }

    #[test]
    fn freeze_does_not_replace_host_with_ice_material() {
        let mut w = saturated_sand_pair();
        freeze_pores(&mut w, 4, 1);
        assert_eq!(
            w.get_cell(4, 1).unwrap().material,
            MaterialId::Sand,
            "pore ice must not swap the host material"
        );
        assert!(is_frozen(&w, 4, 1));
    }

    #[test]
    fn freeze_blocks_confined_pressure_across_pores() {
        // Mini confined aquifer: saturated sand under bedrock, shaft at x=2.
        let mut w = World::new(11);
        for cx in 0..2 {
            w.ensure_chunk(ChunkCoord::new(cx, 0));
        }
        let cap = crate::cell::water_capacity(MaterialId::Sand);
        for x in 0..40 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(cap);
            w.set_cell(x, 1, sand);
            w.set_cell(x, 2, Cell::solid(MaterialId::Bedrock));
            for y in 3..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // High standing donor far right.
        for x in 30..38 {
            for y in 3..8 {
                w.set_cell(x, y, Cell::water());
            }
        }
        // Open a 1-wide well at x=2 down to the aquifer.
        w.set_cell(2, 2, Cell::air());
        w.set_cell(2, 1, {
            let mut a = Cell::air();
            a.sat = Sat(40);
            a
        });
        // Freeze the aquifer pores under / beside the shaft.
        for x in 0..10 {
            freeze_pores(&mut w, x, 1);
        }
        let before = w.get_cell(2, 1).unwrap().sat.0;
        // Force confined wake.
        w.tick = 16;
        wake_confined_head(&mut w, None);
        let after = w.get_cell(2, 1).unwrap().sat.0;
        assert_eq!(
            after, before,
            "frozen aquifer pores must not transmit confined pressure (before={before} after={after})"
        );
    }

    #[test]
    fn hot_wet_chunk_skips_freeze_scan() {
        let mut w = saturated_sand_pair();
        let warm = warm_temp(&w);
        w.tick = PORE_ICE_EVERY;
        apply_pore_ice(&mut w, &warm, PhaseConfig::default().freeze_point_c);
        assert!(
            !is_frozen(&w, 4, 1),
            "warm wet pores must not freeze, and the scan must not invent marks"
        );
        assert!(w.pore_ice.is_empty());
    }

    #[test]
    fn freeze_finds_cold_tile_in_mostly_warm_chunk() {
        let mut w = saturated_sand_pair();
        let freeze = PhaseConfig::default().freeze_point_c;
        let mut temp = warm_temp(&w);
        let (hx, hy) = temp.tile_of(4, 1);
        temp.set_tile_c(hx, hy, -8.0);
        w.tick = PORE_ICE_EVERY;
        apply_pore_ice(&mut w, &temp, freeze);
        assert!(
            is_frozen(&w, 4, 1),
            "a cold tile in a warm chunk must still freeze"
        );
        let (hx2, hy2) = temp.tile_of(2, 1);
        assert_ne!((hx, hy), (hx2, hy2), "fixture must span two tiles");
        assert!(
            !is_frozen(&w, 2, 1),
            "warm tiles in the same chunk must stay liquid"
        );
    }

    #[test]
    fn freeze_blocks_throughflow_through_frozen_bed() {
        let mut w = World::new(13);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let cap = crate::cell::water_capacity(MaterialId::Sand);
        for x in 2..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(cap);
            w.set_cell(x, 1, sand);
        }
        // Standing pool on the bed.
        for x in 3..6 {
            w.set_cell(x, 2, Cell::water());
        }
        // Air spring face at the side.
        w.set_cell(8, 1, Cell::air());
        for x in 2..8 {
            freeze_pores(&mut w, x, 1);
        }
        let before = w.get_cell(8, 1).unwrap().sat.0;
        // Surface flow + throughflow (Priority 4) share this pass.
        apply_water_flow(&mut w);
        assert_eq!(
            w.get_cell(8, 1).unwrap().sat.0,
            before,
            "throughflow must not cross a frozen saturated bed"
        );
    }
}
