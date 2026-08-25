//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Dissolved mineral load — the other half of karst.
//!
//! Dissolution used to delete rock outright, which made karst the one process
//! in the sim that did not conserve its own material. Here rock becomes
//! **load** carried by the water, the load rides water transfers, and it
//! **precipitates** where the water can no longer hold it. That closes the
//! transport loop that builds tufa terraces, flowstone, and spring mounds.
//!
//! Conserved quantity: `rock cells × MINERAL_PER_CELL + Σ dissolved load`.
//! See [`crate::audit::mineral_total`] and docs/VOXEL_GROUNDWATER_VEINS.md.

use wk_material::{MaterialId, MaterialRegistry};

use crate::cell::{water_capacity_cell, Cell, Sat};
use crate::grid::World;

/// Load units produced by dissolving one full cell of soluble rock.
///
/// Also the amount that must accumulate in one place to deposit a cell back.
/// 255 keeps a dissolve→deposit round trip exact in `u16` arithmetic.
pub const MINERAL_PER_CELL: u16 = 255;

/// Load one unit of water can hold before the excess precipitates.
///
/// Sets the concentration ceiling: a cell with `sat` can carry
/// `sat × SOLUBILITY_PER_SAT / 16` units. Deliberately generous — the intent is
/// that load travels with flowing water and drops at an *outlet*, not that it
/// precipitates a few cells from where it dissolved.
pub const SOLUBILITY_PER_SAT: u16 = 4;

/// Precipitate that fills an Air cell once fully occluded.
pub const DEPOSIT_MATERIAL: MaterialId = MaterialId::Limestone;

/// Dissolved load carried by the water in this cell.
#[inline]
pub fn dissolved_at(world: &World, gx: i32, gy: i32) -> u16 {
    let gx = world.wrap_x(gx);
    world.dissolved.get(&(gx, gy)).copied().unwrap_or(0)
}

/// Add load to a cell, saturating.
pub fn add_dissolved(world: &mut World, gx: i32, gy: i32, add: u16) {
    if add == 0 {
        return;
    }
    let gx = world.wrap_x(gx);
    let e = world.dissolved.entry((gx, gy)).or_insert(0);
    *e = e.saturating_add(add);
}

/// Remove up to `want` load, returning what was taken.
pub fn take_dissolved(world: &mut World, gx: i32, gy: i32, want: u16) -> u16 {
    if want == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let Some(e) = world.dissolved.get_mut(&(gx, gy)) else {
        return 0;
    };
    let taken = (*e).min(want);
    *e -= taken;
    if *e == 0 {
        world.dissolved.remove(&(gx, gy));
    }
    taken
}

/// How much load a cell's current water can hold in solution.
#[inline]
pub fn carrying_capacity(world: &World, gx: i32, gy: i32) -> u16 {
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    (cell.sat.0 as u16).saturating_mul(SOLUBILITY_PER_SAT) / 16
}

/// Move load with water: when `moved` of `donor_sat_before` leaves a cell, the
/// same share of its dissolved load goes along.
///
/// Called from the water passes so load follows the flow instead of sitting
/// where the rock happened to dissolve.
pub fn carry_with_water(
    world: &mut World,
    from: (i32, i32),
    to: (i32, i32),
    moved: u8,
    donor_sat_before: u8,
) {
    if moved == 0 || donor_sat_before == 0 {
        return;
    }
    let load = dissolved_at(world, from.0, from.1);
    if load == 0 {
        return;
    }
    // Pro rata, rounding down so transport can never mint load.
    let share = ((load as u32 * moved as u32) / donor_sat_before as u32) as u16;
    let taken = take_dissolved(world, from.0, from.1, share);
    add_dissolved(world, to.0, to.1, taken);
}

/// Emit the mineral freed by dissolving one cell of `material` into its water.
///
/// The caller has already converted the cell; this is the bookkeeping that
/// keeps the mineral total flat.
pub fn emit_from_dissolved_rock(world: &mut World, gx: i32, gy: i32, material: MaterialId) {
    if !is_soluble_rock(material) {
        return;
    }
    add_dissolved(world, gx, gy, MINERAL_PER_CELL);
}

/// Rock that carries mineral mass for the audit.
#[inline]
pub fn is_soluble_rock(material: MaterialId) -> bool {
    MaterialRegistry::base_props(material).solubility > 0
        || matches!(material, MaterialId::Limestone | MaterialId::Stone)
}

/// Precipitate load a cell's water can no longer hold.
///
/// Two triggers, both "the water left or shrank":
///
/// - **Evaporation / drainage** — water gone, so the whole load drops. This is
///   what builds a mound at a spring outlet.
/// - **Concentration** — load above the carrying ceiling drops.
///
/// Deposition first occludes pore space in a neighbouring solid (raising its
/// `pore` toward full is the reverse of aperture growth), and once a full
/// cell's worth has accumulated in open Air, mints a [`DEPOSIT_MATERIAL`] cell.
/// Returns units of load consumed into solid.
pub fn precipitate_at(world: &mut World, gx: i32, gy: i32) -> u16 {
    let gx = world.wrap_x(gx);
    let load = dissolved_at(world, gx, gy);
    if load == 0 {
        return 0;
    }
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    let ceiling = carrying_capacity(world, gx, gy);
    if load <= ceiling {
        return 0;
    }
    let excess = load - ceiling;

    // A full cell of mineral in open Air becomes rock.
    if cell.material == MaterialId::Air && excess >= MINERAL_PER_CELL {
        // Only seat a deposit with something under it — floating flowstone is
        // not a thing, and an unsupported mint would just fall as debris.
        let seated = matches!(
            world.get_cell(gx, gy - 1),
            Some(b) if b.material != MaterialId::Air
        );
        if seated {
            let used = take_dissolved(world, gx, gy, MINERAL_PER_CELL);
            let mut deposit = Cell::solid(DEPOSIT_MATERIAL);
            // Fresh precipitate is dense: start at the tight end of the range.
            deposit.pore = 0;
            // Keep whatever water fits; the rest stays as free load-free water
            // above, handled by the normal passes.
            let cap = water_capacity_cell(deposit, &world.hydro);
            deposit.sat = Sat(cell.sat.0.min(cap));
            world.set_cell(gx, gy, deposit);
            return used;
        }
    }

    // Otherwise tighten this cell's own pore space (cements the aperture shut).
    if cell.material != MaterialId::Air && cell.pore > 0 {
        let step = ((excess / 16).max(1)).min(cell.pore as u16) as u8;
        let used = take_dissolved(world, gx, gy, step as u16 * 16);
        let mut next = cell;
        next.pore = cell.pore.saturating_sub(step);
        // Shrinking pore can drop capacity below current sat. Shed the excess
        // upward rather than letting the audit see a loss.
        let cap = water_capacity_cell(next, &world.hydro);
        let spill = next.sat.0.saturating_sub(cap);
        next.sat = Sat(next.sat.0.min(cap));
        world.set_cell(gx, gy, next);
        if spill > 0 {
            push_water_up(world, gx, gy + 1, spill);
        }
        return used;
    }
    0
}

/// Park shed water in the first Air cell with room above `gy`.
fn push_water_up(world: &mut World, gx: i32, gy: i32, mut amount: u8) {
    for dy in 0..8 {
        if amount == 0 {
            return;
        }
        let y = gy + dy;
        let Some(mut c) = world.get_cell(gx, y) else {
            return;
        };
        if c.material != MaterialId::Air {
            continue;
        }
        let room = u8::MAX - c.sat.0;
        let put = room.min(amount);
        if put > 0 {
            c.sat = Sat(c.sat.0 + put);
            world.set_cell(gx, y, c);
            amount -= put;
        }
    }
}

/// Drop the entire load of a cell whose water has left (evaporation, drainage).
///
/// Unlike [`precipitate_at`] this ignores the concentration ceiling: there is
/// no water left to hold anything.
pub fn precipitate_dry_cell(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    let load = dissolved_at(world, gx, gy);
    if load == 0 {
        return;
    }
    let Some(cell) = world.get_cell(gx, gy) else {
        return;
    };
    if cell.sat.0 > 0 {
        return;
    }
    if load < MINERAL_PER_CELL {
        // Not enough for a cell yet — leave it banked so repeated wet/dry
        // cycles at the same outlet can build up to a deposit.
        return;
    }
    if cell.material == MaterialId::Air {
        let seated = matches!(
            world.get_cell(gx, gy - 1),
            Some(b) if b.material != MaterialId::Air
        );
        if !seated {
            return;
        }
        let _ = take_dissolved(world, gx, gy, MINERAL_PER_CELL);
        let mut deposit = Cell::solid(DEPOSIT_MATERIAL);
        deposit.pore = 0;
        world.set_cell(gx, gy, deposit);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;

    fn bed(seed: u64) -> World {
        let mut w = World::new(seed);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    #[test]
    fn load_rides_water_pro_rata() {
        let mut w = bed(1);
        w.set_cell(4, 2, Cell::water());
        add_dissolved(&mut w, 4, 2, 200);
        // Half the donor's water leaves.
        carry_with_water(&mut w, (4, 2), (5, 2), 128, 255);
        let moved = dissolved_at(&w, 5, 2);
        assert!(
            (99..=101).contains(&moved),
            "about half the load should travel, got {moved}"
        );
        assert_eq!(
            moved + dissolved_at(&w, 4, 2),
            200,
            "transport must conserve load"
        );
    }

    #[test]
    fn transport_never_mints_load() {
        let mut w = bed(2);
        w.set_cell(4, 2, Cell::water());
        add_dissolved(&mut w, 4, 2, 7);
        for _ in 0..32 {
            carry_with_water(&mut w, (4, 2), (5, 2), 1, 255);
        }
        assert!(
            dissolved_at(&w, 4, 2) + dissolved_at(&w, 5, 2) <= 7,
            "rounding must never create load"
        );
    }

    #[test]
    fn dry_outlet_deposits_a_cell() {
        let mut w = bed(3);
        // Dry, seated Air cell holding a full cell's worth of mineral.
        w.set_cell(4, 1, Cell::air());
        add_dissolved(&mut w, 4, 1, MINERAL_PER_CELL);
        precipitate_dry_cell(&mut w, 4, 1);
        assert_eq!(
            w.get_cell(4, 1).unwrap().material,
            DEPOSIT_MATERIAL,
            "an evaporated outlet should leave a mineral deposit"
        );
        assert_eq!(dissolved_at(&w, 4, 1), 0, "deposit consumes the load");
    }

    #[test]
    fn unsupported_load_does_not_mint_floating_rock() {
        let mut w = bed(4);
        w.set_cell(4, 6, Cell::air()); // nothing beneath
        add_dissolved(&mut w, 4, 6, MINERAL_PER_CELL);
        precipitate_dry_cell(&mut w, 4, 6);
        assert_eq!(
            w.get_cell(4, 6).unwrap().material,
            MaterialId::Air,
            "flowstone must not form in mid-air"
        );
    }

    #[test]
    fn dissolving_rock_emits_its_mineral() {
        let mut w = bed(5);
        w.set_cell(4, 1, Cell::solid(MaterialId::Limestone));
        // Caller converts, then reports.
        w.set_cell(4, 1, Cell::air());
        emit_from_dissolved_rock(&mut w, 4, 1, MaterialId::Limestone);
        assert_eq!(dissolved_at(&w, 4, 1), MINERAL_PER_CELL);
    }
}
