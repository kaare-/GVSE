//! Sparse free-water temperature ledger (Phase 3 slice A).
//!
//! `World.water_temp` stores °C for **Air sat only**. Absent key ⇒ inherit
//! the coarse tile via [`Temperature::at_cell`]. Heat-only: never invents
//! or moves `sat`. Mix on free-water transfers; clear when `sat → 0`.
//! Pore-water T is deferred.

use wk_material::MaterialId;

use crate::grid::World;
use crate::temperature::Temperature;

/// Effective free-water temperature at `(gx, gy)`.
///
/// Sparse hit wins; otherwise tile inherit. Not meaningful for solids /
/// pore water (callers should not treat pore seats as carrying this ledger).
#[inline]
pub fn water_temp_at(world: &World, temp: &Temperature, gx: i32, gy: i32) -> f32 {
    let gx = world.wrap_x(gx);
    if let Some(&t) = world.water_temp.get(&(gx, gy)) {
        return t;
    }
    temp.at_cell(gx, gy)
}

/// Write an explicit free-water temperature (°C). No sat change.
#[inline]
pub fn set_water_temp(world: &mut World, gx: i32, gy: i32, t_c: f32) {
    let gx = world.wrap_x(gx);
    if t_c.is_finite() {
        world.water_temp.insert((gx, gy), t_c);
    }
}

/// Drop the sparse key (next read inherits the tile).
#[inline]
pub fn clear_water_temp(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    world.water_temp.remove(&(gx, gy));
}

/// Clear when the cell is Air with `sat == 0` (or missing).
#[inline]
pub fn clear_water_temp_if_dry(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    let dry = world
        .get_cell(gx, gy)
        .map(|c| c.material != MaterialId::Air || c.sat.0 == 0)
        .unwrap_or(true);
    if dry {
        world.water_temp.remove(&(gx, gy));
    }
}

/// Mass-weighted mix of free-water T when `moved` sat lands in Air `to`.
///
/// Call **after** sat writes. `donor_sat_before` is the donor's sat before
/// the transfer. Donor pore seats contribute tile inherit (no pore ledger).
/// Clears the donor key when Air sat hits 0. Heat-only — never touches sat.
pub fn mix_water_temp_on_transfer(
    world: &mut World,
    temp: &Temperature,
    from: (i32, i32),
    to: (i32, i32),
    moved: u8,
    donor_sat_before: u8,
) {
    if moved == 0 || donor_sat_before == 0 {
        return;
    }
    let to_x = world.wrap_x(to.0);
    let from_x = world.wrap_x(from.0);
    let Some(dest) = world.get_cell(to_x, to.1) else {
        return;
    };
    if dest.material != MaterialId::Air {
        // Pore-water T deferred — do not stamp the ledger into rock.
        clear_water_temp_if_dry(world, from_x, from.1);
        return;
    }
    let dest_sat_after = dest.sat.0;
    if dest_sat_after == 0 {
        clear_water_temp(world, to_x, to.1);
        clear_water_temp_if_dry(world, from_x, from.1);
        return;
    }
    let dest_sat_before = dest_sat_after.saturating_sub(moved);
    let donor_explicit = world.water_temp.contains_key(&(from_x, from.1));
    let dest_explicit = world.water_temp.contains_key(&(to_x, to.1));
    let donor_t = water_temp_at(world, temp, from_x, from.1);
    // Tile inherit at dest (even when sat_before==0 — empty Air still has a tile).
    let dest_inherit = water_temp_at(world, temp, to_x, to.1);
    // Stay sparse when both sides only inherit the same tile °C.
    if !donor_explicit && !dest_explicit && (donor_t - dest_inherit).abs() < 0.05 {
        clear_water_temp_if_dry(world, from_x, from.1);
        return;
    }
    let mixed = if dest_sat_before == 0 {
        // Empty seat filling: arriving parcel defines T.
        donor_t
    } else {
        let dest_t = dest_inherit;
        let w_d = dest_sat_before as f32;
        let w_m = moved as f32;
        (dest_t * w_d + donor_t * w_m) / (w_d + w_m)
    };
    set_water_temp(world, to_x, to.1, mixed);
    clear_water_temp_if_dry(world, from_x, from.1);
}

/// Test / debug: number of explicit free-water T keys.
#[inline]
pub fn water_temp_len(world: &World) -> usize {
    world.water_temp.len()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::sat_totals;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;
    use crate::temperature::Temperature;

    fn cold_field(temp_c: f32) -> Temperature {
        let mut t = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 32, false);
        t.config.base_temp_c = temp_c;
        t.config.water_convect_bias = 0.0;
        for v in t.cells.values_mut() {
            *v = temp_c;
        }
        t
    }

    #[test]
    fn absent_key_inherits_tile() {
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut cell = Cell::air();
        cell.sat = Sat(200);
        w.set_cell(2, 2, cell);
        let temp = cold_field(-10.0);
        assert!(
            (water_temp_at(&w, &temp, 2, 2) - (-10.0)).abs() < 1e-3,
            "absent ledger must inherit tile"
        );
        assert_eq!(water_temp_len(&w), 0);
    }

    #[test]
    fn mix_and_clear_on_sat_transfer() {
        let mut w = World::new(2);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let temp = cold_field(-20.0);

        let mut hot = Cell::air();
        hot.sat = Sat(100);
        w.set_cell(4, 5, hot);
        set_water_temp(&mut w, 4, 5, 80.0);

        let mut cold = Cell::air();
        cold.sat = Sat(100);
        w.set_cell(4, 4, cold);
        // Dest inherits cold tile until mix writes.

        // Simulate transfer: move 100 sat from (4,5) → (4,4).
        let mut src = w.get_cell(4, 5).unwrap();
        src.sat = Sat(0);
        w.set_cell(4, 5, src);
        let mut dst = w.get_cell(4, 4).unwrap();
        dst.sat = Sat(200);
        w.set_cell(4, 4, dst);

        let before = sat_totals(&w).cell_total;
        mix_water_temp_on_transfer(&mut w, &temp, (4, 5), (4, 4), 100, 100);
        assert_eq!(
            sat_totals(&w).cell_total,
            before,
            "heat helpers must not invent sat"
        );
        assert!(
            !w.water_temp.contains_key(&(4, 5)),
            "donor sat→0 must clear"
        );
        let mixed = water_temp_at(&w, &temp, 4, 4);
        // 100 @ −20 inherit + 100 @ 80 → ~30 °C
        assert!(
            (mixed - 30.0).abs() < 0.5,
            "mass-weighted mix expected ~30, got {mixed}"
        );
        assert!(
            (mixed - temp.at_cell(4, 4)).abs() > 5.0,
            "mixed free water must differ from cold tile"
        );
    }

    #[test]
    fn hot_dump_retains_t_vs_cold_tile() {
        // Same sequence gravity / leftover mouth use after sat writes:
        // hot free water lands in empty Air under a uniformly cold tile.
        let mut w = World::new(3);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(8, 2, Cell::solid(wk_material::MaterialId::Stone));
        let mut below = Cell::air();
        below.sat = Sat(0);
        w.set_cell(8, 3, below);
        let mut hot = Cell::air();
        hot.sat = Sat(180);
        w.set_cell(8, 4, hot);
        set_water_temp(&mut w, 8, 4, 90.0);

        let temp = cold_field(-15.0);
        let tracked0 = sat_totals(&w).cell_total;

        let moved = 180u8;
        let mut src = w.get_cell(8, 4).unwrap();
        src.sat = Sat(0);
        w.set_cell(8, 4, src);
        let mut dst = w.get_cell(8, 3).unwrap();
        dst.sat = Sat(moved);
        w.set_cell(8, 3, dst);
        mix_water_temp_on_transfer(&mut w, &temp, (8, 4), (8, 3), moved, moved);

        assert_eq!(
            sat_totals(&w).cell_total,
            tracked0,
            "water_temp dump must stay TRACKED-flat"
        );
        let t_water = water_temp_at(&w, &temp, 8, 3);
        let t_tile = temp.at_cell(8, 3);
        assert!(
            (t_water - t_tile).abs() > 20.0,
            "hot dump cell T ({t_water}) must briefly ≠ cold tile ({t_tile})"
        );
        assert!(
            t_water > 50.0,
            "arriving parcel should stay warm, got {t_water}"
        );
        assert!(
            !w.water_temp.contains_key(&(8, 4)),
            "empty donor must clear"
        );
    }

    #[test]
    fn clear_helper_drops_key() {
        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        set_water_temp(&mut w, 1, 1, 12.0);
        assert_eq!(water_temp_len(&w), 1);
        clear_water_temp(&mut w, 1, 1);
        assert_eq!(water_temp_len(&w), 0);
    }
}
