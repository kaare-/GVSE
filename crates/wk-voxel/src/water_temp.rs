//! Sparse free-water temperature ledger (Phase 3).
//!
//! `World.water_temp` stores °C for **Air sat only**. Absent key ⇒ inherit
//! the coarse tile via [`Temperature::at_cell`]. Heat-only: never invents
//! or moves `sat`. Mix on free-water transfers; clear when `sat → 0`.
//! Pore-water T is deferred.
//!
//! Slice C: leftover mouth dumps **always** stamp hot free-water T (same-tile
//! inherit must not stay sparse), then soft-cool toward tile/air so skin
//! couple cannot instantly wipe sub-zero discharge.

use wk_material::MaterialId;

use crate::grid::World;
use crate::temperature::Temperature;

/// Soft cool rate toward tile ambient per cool pass (≪ skin couple quench).
pub const WATER_TEMP_SOFT_COOL_RATE: f32 = 0.06;
/// Drop sparse key when within this many °C of ambient (back to inherit).
const WATER_TEMP_COOL_CLEAR_EPS: f32 = 0.75;

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

/// Leftover / reverse-seep mouth dump into Air: always stamp free-water T.
///
/// Same-tile pore→Air dumps would otherwise stay sparse (donor inherit ==
/// dest inherit) and then track the skin-cooled tile instantly. Heat-only.
pub fn mix_mouth_water_temp_on_transfer(
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
    // Capture donor before any clear — pore seats use hot tile inherit.
    let donor_t = water_temp_at(world, temp, from_x, from.1);
    let dest_t = if dest_sat_before == 0 {
        // Empty seat: arriving parcel defines T (ignore cold tile inherit).
        donor_t
    } else if world.water_temp.contains_key(&(to_x, to.1)) {
        water_temp_at(world, temp, to_x, to.1)
    } else {
        // Standing cold film already there — mix parcel into inherit.
        temp.at_cell(to_x, to.1)
    };
    let mixed = if dest_sat_before == 0 {
        donor_t
    } else {
        let w_d = dest_sat_before as f32;
        let w_m = moved as f32;
        (dest_t * w_d + donor_t * w_m) / (w_d + w_m)
    };
    // Always write — no same-tile sparse skip (Slice C).
    set_water_temp(world, to_x, to.1, mixed);
    clear_water_temp_if_dry(world, from_x, from.1);
}

/// Ease explicit free-water T toward the cell's tile ambient (soft cool).
///
/// Runs after the coarse thermal step so ambient already includes air↔water
/// skin couple. Rate ≪ skin quench — hot mouth discharge stays warm enough
/// to matter in sub-zero air. Heat-only; clears dry / near-ambient keys.
pub fn cool_water_temp_toward_ambient(
    world: &mut World,
    temp: &Temperature,
    rate: f32,
) {
    if world.water_temp.is_empty() {
        return;
    }
    let a = rate.clamp(0.0, 1.0);
    if a < 1e-6 {
        return;
    }
    let keys: Vec<(i32, i32)> = world.water_temp.keys().copied().collect();
    for (gx, gy) in keys {
        let Some(cell) = world.get_cell(gx, gy) else {
            world.water_temp.remove(&(gx, gy));
            continue;
        };
        if cell.material != MaterialId::Air || cell.sat.0 == 0 {
            world.water_temp.remove(&(gx, gy));
            continue;
        }
        let Some(&t) = world.water_temp.get(&(gx, gy)) else {
            continue;
        };
        // Ambient = tile (skin-coupled for watery surfaces). Soft toward it.
        let ambient = temp.at_cell(gx, gy);
        let next = t + (ambient - t) * a;
        if (next - ambient).abs() < WATER_TEMP_COOL_CLEAR_EPS {
            world.water_temp.remove(&(gx, gy));
        } else {
            world.water_temp.insert((gx, gy), next);
        }
    }
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

    #[test]
    fn mouth_dump_stamps_hot_same_tile_vs_mix_skip() {
        // Pore→Air in one coarse tile: ordinary mix stays sparse; mouth must stamp.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut pore = Cell::solid(wk_material::MaterialId::Limestone);
        pore.sat = Sat(120);
        w.set_cell(4, 1, pore);
        w.set_cell(4, 2, Cell::air());

        let mut temp = cold_field(-18.0);
        let (hx, hy) = temp.tile_of(4, 1);
        assert_eq!(
            temp.tile_of(4, 2),
            (hx, hy),
            "fixture must share one thermal tile"
        );
        temp.set_tile_c(hx, hy, 95.0);

        let tracked0 = sat_totals(&w).cell_total;
        let moved = 80u8;
        let donor_before = 120u8;

        // Sat write (same sequence reverse_push uses before the helper).
        let mut src = w.get_cell(4, 1).unwrap();
        src.sat = Sat(donor_before - moved);
        w.set_cell(4, 1, src);
        let mut dst = w.get_cell(4, 2).unwrap();
        dst.sat = Sat(moved);
        w.set_cell(4, 2, dst);

        // Ordinary mix stays sparse (same inherit).
        mix_water_temp_on_transfer(&mut w, &temp, (4, 1), (4, 2), moved, donor_before);
        assert!(
            !w.water_temp.contains_key(&(4, 2)),
            "baseline mix must stay sparse on same-tile inherit"
        );

        // Mouth helper always stamps while the seat still reads hot.
        mix_mouth_water_temp_on_transfer(&mut w, &temp, (4, 1), (4, 2), moved, donor_before);
        assert_eq!(sat_totals(&w).cell_total, tracked0);
        assert!(
            w.water_temp.contains_key(&(4, 2)),
            "mouth dump must stamp explicit water_temp"
        );
        // Skin couple snaps the shared tile cold; sparse free-water keeps heat.
        temp.set_tile_c(hx, hy, -18.0);
        let t_water = water_temp_at(&w, &temp, 4, 2);
        let t_tile = temp.at_cell(4, 2);
        assert!(
            (t_water - t_tile).abs() > 40.0,
            "hot mouth stamp ({t_water}) must ≠ cold tile ({t_tile})"
        );
        assert!(t_water > 50.0, "stamped discharge must stay hot, got {t_water}");
    }

    #[test]
    fn soft_cool_retains_hot_vs_cold_tile() {
        // Skin couple would snap the tile; soft cool only eases water_temp.
        let mut w = World::new(6);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut film = Cell::air();
        film.sat = Sat(200);
        w.set_cell(3, 3, film);
        set_water_temp(&mut w, 3, 3, 90.0);

        let temp = cold_field(-20.0);
        let tracked0 = sat_totals(&w).cell_total;
        let t0 = water_temp_at(&w, &temp, 3, 3);
        for _ in 0..8 {
            cool_water_temp_toward_ambient(&mut w, &temp, WATER_TEMP_SOFT_COOL_RATE);
        }
        assert_eq!(sat_totals(&w).cell_total, tracked0, "soft cool is heat-only");
        let t1 = water_temp_at(&w, &temp, 3, 3);
        let t_tile = temp.at_cell(3, 3);
        assert!(
            t1 < t0 - 1.0,
            "soft cool must ease toward ambient ({t0} → {t1})"
        );
        assert!(
            (t1 - t_tile).abs() > 30.0,
            "after soft cool, discharge ({t1}) must still ≠ cold tile ({t_tile})"
        );
        assert!(
            w.water_temp.contains_key(&(3, 3)),
            "explicit key must survive early soft cool (not instant wipe)"
        );
    }

    #[test]
    fn soft_cool_clears_near_ambient() {
        let mut w = World::new(7);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut film = Cell::air();
        film.sat = Sat(100);
        w.set_cell(2, 2, film);
        let temp = cold_field(-5.0);
        set_water_temp(&mut w, 2, 2, -4.5);
        cool_water_temp_toward_ambient(&mut w, &temp, 0.5);
        assert!(
            !w.water_temp.contains_key(&(2, 2)),
            "near-ambient free water must drop back to inherit"
        );
    }
}
