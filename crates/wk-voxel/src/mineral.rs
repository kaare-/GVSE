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
/// `sat × SOLUBILITY_PER_SAT / 16` units.
///
/// This has to be generous, and 4 was far too tight. Pore water in rock has a
/// small `sat` — a near-saturated LooseLimestone cell holds ~27 — so a ceiling
/// of `27 × 4 / 16 = 6` meant load precipitated almost the instant it entered
/// rock. A 160 k-tick soak showed cells carrying 15–24 units against a ceiling
/// of 6, cementing their own aperture shut (`pore` 86 → 46 over ~1800 ticks).
/// The brake was beating the growth everywhere, so no conduit could survive and
/// caves never developed.
///
/// At 24 a pore cell of that wetness carries ~40, comfortably above what
/// dissolution puts into it, so load **travels** and only drops where water is
/// actually lost — evaporating at an outlet, or depressurising at a spring.
/// That is the intended shape: deposits at discharge points, not cement in the
/// aquifer.
pub const SOLUBILITY_PER_SAT: u16 = 24;

/// Precipitate that fills an Air cell once fully occluded.
pub const DEPOSIT_MATERIAL: MaterialId = MaterialId::Flowstone;

/// Pore steps one precipitation event may close (keeps cementing gradual).
pub const PRECIPITATE_MAX_STEP: u16 = 8;

/// Solubility that [`widen_aperture`]'s scale is expressed against (limestone).
/// Other materials open proportionally slower.
const LIMESTONE_SOLUBILITY_REF: f32 = 40.0;

/// Yield of non-carbonate competent rock under throughput, on the same scale as
/// carbonate `solubility`.
///
/// Abrasion, not dissolution. Silicate rock does widen a fracture that carries
/// water, just far more slowly, and it releases **no dissolved load** because
/// grinding rock produces *suspended* sediment — a species the sim does not
/// model yet. Stone is outside the mineral ledger entirely
/// ([`is_soluble_rock`]), so widening it unbalances nothing; the material it
/// loses simply is not tracked until suspended load exists.
const MECHANICAL_ABRASION_REF: f32 = 4.0;

/// Throughput a transfer must exceed before competent rock yields at all.
///
/// A critical-shear threshold: it is what makes continuous limestone and stone
/// *hard*, so erosion happens only along the few paths that actually carry
/// flow instead of everywhere the rock happens to be wet.
pub const APERTURE_MIN_THROUGHPUT: u8 = 8;

/// How much less an artesian outlet can hold than water still at depth.
///
/// Water arriving under pressure gives up a share of its load as it
/// depressurises, so a rising spring builds a mound instead of carrying its
/// mineral away to wherever it eventually evaporates.
const ARTESIAN_CEILING_DIVISOR: u16 = 4;

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

/// Mineral still held as solid in this cell.
///
/// Scales with how open the cell is: `pore` is the aperture, so a porous rock
/// cell genuinely contains less rock than a dense one. Because
/// [`MINERAL_PER_CELL`] is 255, **one pore step is exactly one load unit** —
/// widening releases 1, occluding consumes 1, and the audit balances without
/// any scaling factor.
#[inline]
pub fn cell_mineral(cell: Cell) -> u16 {
    if !is_soluble_rock(cell.material) {
        return 0;
    }
    MINERAL_PER_CELL - cell.pore as u16
}

/// Emit the mineral freed by dissolving a cell of soluble rock into its water.
///
/// Takes the cell as it was *before* conversion: a cell already widened toward
/// full aperture has released most of its mineral incrementally and must not
/// emit a second full cell's worth.
pub fn emit_from_dissolved_rock(world: &mut World, gx: i32, gy: i32, was: Cell) {
    let remaining = cell_mineral(was);
    if remaining == 0 {
        return;
    }
    add_dissolved(world, gx, gy, remaining);
}

/// Widen a soluble cell's aperture by the water passing through it.
///
/// This is the self-amplifying half of vein formation: throughput opens the
/// aperture, a wider aperture conducts and stores more, so more water comes
/// through. `pore` *is* the aperture state — no separate flux counter — and it
/// only ever increases here, so it can never strand saturation above a
/// shrinking capacity. Precipitation ([`precipitate_at`]) is the brake.
///
/// Deliberately probabilistic and slow: geology, not a frame-scale effect.
/// Deterministic given `(seed, position, tick)` like the rest of karst.
///
/// Returns true when the cell opened fully and dissolved away.
pub fn widen_aperture(
    world: &mut World,
    gx: i32,
    gy: i32,
    throughput: u8,
    scale: f32,
    seed_salt: u64,
) -> bool {
    if throughput == 0 || scale <= 0.0 {
        return false;
    }
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    // Carbonate dissolves and silicate abrades, but both widen under flow, so
    // both belong here — only the rate and the ledger differ.
    if !crate::cell::is_competent_rock(cell.material) || cell.pore == u8::MAX {
        return false;
    }
    // Below a threshold flow, competent rock simply does not yield. Without a
    // threshold every wetted cell erodes a little and the result is uniform
    // widening — a slightly more porous aquifer rather than pipes.
    if throughput <= APERTURE_MIN_THROUGHPUT {
        return false;
    }
    let solubility = {
        let s = MaterialRegistry::base_props(cell.material).solubility;
        if s > 0 {
            s as f32
        } else {
            MECHANICAL_ABRASION_REF
        }
    };
    let over = (throughput - APERTURE_MIN_THROUGHPUT) as f32
        / (255 - APERTURE_MIN_THROUGHPUT) as f32;
    // **Superlinear** in throughput. This is what channelizes: a cell carrying
    // twice the water opens roughly four times faster, so a small head start
    // compounds into a conduit while its neighbours stay effectively solid.
    // A linear response spread the erosion evenly instead.
    let p = scale * over * over * (solubility / LIMESTONE_SOLUBILITY_REF);
    if p <= 0.0 {
        return false;
    }
    let roll = crate::rules::hash_prob(
        world.seed.0,
        gx.wrapping_mul(73_856_093).wrapping_add(gy),
        world.tick,
        seed_salt,
    );
    if roll >= p.min(1.0) {
        return false;
    }
    // One pore step releases exactly one unit of mineral.
    let mut next = cell;
    next.pore = cell.pore.saturating_add(1);
    if next.pore == u8::MAX {
        // Fully open: the cement is gone. For a clastic rock that leaves the
        // grains it was holding together, not a void — dissolving sandstone
        // must not delete the sand.
        let freed = cell_mineral(cell);
        let keep = cell.sat;
        let becomes = loose_parent(cell.material).unwrap_or(MaterialId::Air);
        world.set_cell(
            gx,
            gy,
            Cell {
                material: becomes,
                sat: keep,
                ..cell
            },
        );
        add_dissolved(world, gx, gy, freed);
        return true;
    }
    world.set_cell(gx, gy, next);
    // Only carbonate puts anything into solution. Abraded silicate releases
    // nothing, which is consistent because it is not in the ledger either.
    if cell_mineral(cell) > 0 {
        add_dissolved(world, gx, gy, 1);
    }
    false
}

/// The rock a loose sediment becomes when carbonate cements its grains.
///
/// Loose material cannot hold a channel: repose and grain settle destroy any
/// void the moment it opens, so conduits could only ever form in competent rock
/// and never in the near-surface layer where water actually runs. Cementing is
/// what gives near-surface channels somewhere to persist, and it closes the loop
/// — water deposits mineral, sediment sets, set rock holds a void, the void
/// becomes a conduit, the conduit concentrates flow.
///
/// Carbonate rubble needs no new material: cemented limestone gravel *is*
/// limestone. Soil and organics are deliberately absent — they rot rather than
/// set, and the fungi and compost paths already cover that.
#[inline]
pub fn cemented_form(material: MaterialId) -> Option<MaterialId> {
    match material {
        MaterialId::Sand => Some(MaterialId::Sandstone),
        MaterialId::Gravel | MaterialId::LooseRock => Some(MaterialId::Conglomerate),
        MaterialId::LooseLimestone => Some(MaterialId::Limestone),
        _ => None,
    }
}

/// The sediment a cemented rock returns to when its cement dissolves away.
///
/// A clastic rock must not dissolve to a void: only the carbonate matrix is
/// soluble, and the grains it was holding together stay exactly where they were.
/// Without this, dissolving sandstone would delete the sand.
#[inline]
pub fn loose_parent(material: MaterialId) -> Option<MaterialId> {
    match material {
        MaterialId::Sandstone => Some(MaterialId::Sand),
        MaterialId::Conglomerate => Some(MaterialId::Gravel),
        _ => None,
    }
}

/// Minimum load before precipitation sets a bed rather than just sitting in it.
///
/// Cementation is one atomic step because there is nowhere to bank a partial
/// amount: an insoluble sediment carries no mineral in the ledger
/// ([`cell_mineral`]), so there is no such thing as half-cemented sand. The
/// resulting rock *is* soluble, so everything after the first step is ordinary
/// pore occlusion.
pub const CEMENT_MIN_LOAD: u16 = 32;

/// Cement a loose sediment cell with the mineral its water is carrying.
///
/// Conserves exactly: the load taken becomes the new cell's mineral content,
/// because `cell_mineral` of the cemented rock is `MINERAL_PER_CELL - pore` and
/// `pore` is set to the complement of what was consumed. A lightly cemented bed
/// is therefore genuinely porous, and further precipitation tightens it through
/// the normal occlusion path.
fn cement_cell(world: &mut World, gx: i32, gy: i32, excess: u16) -> u16 {
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    let Some(into) = cemented_form(cell.material) else {
        return 0;
    };
    if excess < CEMENT_MIN_LOAD {
        return 0;
    }
    let want = excess.min(MINERAL_PER_CELL);
    let used = take_dissolved(world, gx, gy, want);
    if used == 0 {
        return 0;
    }
    let mut next = cell;
    next.material = into;
    // Exact: mineral consumed == mineral now held as cement.
    next.pore = (MINERAL_PER_CELL - used.min(MINERAL_PER_CELL)) as u8;
    // Tightening pore can drop capacity below the water already present.
    let cap = water_capacity_cell(next, &world.hydro);
    let spill = next.sat.0.saturating_sub(cap);
    next.sat = Sat(next.sat.0.min(cap));
    world.set_cell(gx, gy, next);
    if spill > 0 {
        push_water_up(world, gx, gy + 1, spill);
    }
    used
}

/// Rock that carries mineral mass for the audit: **carbonate only**.
///
/// Driven purely by the material's `solubility`, which already says exactly
/// this — limestone and flowstone at 40, silicate stone at 0. The hardcoded
/// `| Stone` that used to be here was the whole problem: silicate rock
/// dissolved into the same load that precipitates as flowstone, so the sim was
/// quietly converting granite into carbonate. Stone erodes *mechanically*
/// instead (see [`widen_aperture`] and surface flow erosion), which is both
/// simpler and what actually happens.
#[inline]
pub fn is_soluble_rock(material: MaterialId) -> bool {
    MaterialRegistry::base_props(material).solubility > 0
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
    let ceiling = carrying_capacity(world, gx, gy);
    precipitate_over(world, gx, gy, ceiling)
}

/// Precipitate on **depressurisation** at an artesian discharge.
///
/// Water forced up a confined path is under pressure; at the outlet it
/// depressurises and can hold far less in solution, so a share of the load
/// drops even though nothing evaporated. This is what puts a travertine mound
/// at a rising spring rather than a flat stain where the water later dries.
pub fn precipitate_artesian(world: &mut World, gx: i32, gy: i32) -> u16 {
    let ceiling = carrying_capacity(world, gx, gy) / ARTESIAN_CEILING_DIVISOR;
    precipitate_over(world, gx, gy, ceiling)
}

/// Shared core: drop whatever load exceeds `ceiling`.
fn precipitate_over(world: &mut World, gx: i32, gy: i32, ceiling: u16) -> u16 {
    let gx = world.wrap_x(gx);
    let load = dissolved_at(world, gx, gy);
    if load == 0 {
        return 0;
    }
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
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

    // Loose sediment: set the grains rather than trickling into a pore space
    // the ledger cannot account for. `occlude_pore` refuses insoluble material,
    // so without this the load simply stayed in solution forever.
    if cemented_form(cell.material).is_some() {
        return cement_cell(world, gx, gy, excess);
    }

    // Cement into this cell's own pore space. One unit of load closes exactly
    // one pore step — the reverse of `widen_aperture`, which is what lets a
    // conduit seal again.
    if cell.material != MaterialId::Air {
        return occlude_pore(world, gx, gy, excess);
    }
    // An outlet is open Air, so there is no pore here to cement. Deposit onto
    // the floor beneath instead — that is where travertine actually forms, and
    // it means a discharge builds up immediately rather than banking a mobile
    // load that the next transfer can carry away again.
    occlude_pore(world, gx, gy - 1, excess)
}

/// Cement `excess` load into a soluble cell's pore space, one unit per step.
///
/// Only soluble rock qualifies: its mineral is what
/// [`crate::audit::mineral_total`] counts, so occluding anything else would
/// consume load without the solid gaining it.
fn occlude_pore(world: &mut World, gx: i32, gy: i32, excess: u16) -> u16 {
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    if !is_soluble_rock(cell.material) || cell.pore == 0 {
        return 0;
    }
    let step = excess.min(cell.pore as u16).min(PRECIPITATE_MAX_STEP) as u8;
    if step == 0 {
        return 0;
    }
    // Load is banked on the cell that held the water, which for a floor deposit
    // is the cell above.
    let used = take_dissolved(world, gx, gy, step as u16);
    let used = if used == 0 {
        take_dissolved(world, gx, gy + 1, step as u16)
    } else {
        used
    };
    if used == 0 {
        return 0;
    }
    let mut next = cell;
    next.pore = cell.pore.saturating_sub(used.min(u8::MAX as u16) as u8);
    // Shrinking pore can drop capacity below current sat. Shed the excess
    // upward rather than letting the audit see a loss.
    let cap = water_capacity_cell(next, &world.hydro);
    let spill = next.sat.0.saturating_sub(cap);
    next.sat = Sat(next.sat.0.min(cap));
    world.set_cell(gx, gy, next);
    if spill > 0 {
        push_water_up(world, gx, gy + 1, spill);
    }
    used
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
        let rock = Cell::solid(MaterialId::Limestone);
        w.set_cell(4, 1, rock);
        // Caller converts, then reports the cell as it was.
        w.set_cell(4, 1, Cell::air());
        emit_from_dissolved_rock(&mut w, 4, 1, rock);
        assert_eq!(dissolved_at(&w, 4, 1), cell_mineral(rock));
    }

    #[test]
    fn a_widened_cell_does_not_emit_a_second_full_load() {
        // A cell most of the way to full aperture has already released its
        // mineral one step at a time; dissolving it must only free the rest.
        let mut w = bed(6);
        let mut worn = Cell::solid(MaterialId::Limestone);
        worn.pore = 200;
        w.set_cell(4, 1, worn);
        w.set_cell(4, 1, Cell::air());
        emit_from_dissolved_rock(&mut w, 4, 1, worn);
        assert_eq!(
            dissolved_at(&w, 4, 1),
            MINERAL_PER_CELL - 200,
            "only the mineral still held may be released"
        );
    }

    #[test]
    fn one_pore_step_is_one_mineral_unit() {
        // The identity the whole ledger rests on: widening by a step releases
        // exactly one unit, so total mineral is unchanged.
        let mut w = bed(7);
        let mut rock = Cell::solid(MaterialId::Limestone);
        rock.pore = 100;
        rock.sat = Sat(30);
        w.set_cell(4, 1, rock);
        let before = crate::audit::mineral_total(&w);
        // Force the roll to pass with a certainty-scale call.
        let opened = widen_aperture(&mut w, 4, 1, 255, 1000.0, 1);
        assert!(!opened, "one step should not fully dissolve a fresh cell");
        assert_eq!(
            w.get_cell(4, 1).unwrap().pore,
            101,
            "aperture should open by one step"
        );
        assert_eq!(
            crate::audit::mineral_total(&w),
            before,
            "widening must conserve mineral: rock lost 1, load gained 1"
        );
    }

    #[test]
    fn full_aperture_dissolves_the_cell_and_conserves() {
        let mut w = bed(8);
        let mut rock = Cell::solid(MaterialId::Limestone);
        rock.pore = 253;
        w.set_cell(4, 1, rock);
        let before = crate::audit::mineral_total(&w);
        let mut opened = false;
        for _ in 0..8 {
            if widen_aperture(&mut w, 4, 1, 255, 1000.0, 2) {
                opened = true;
                break;
            }
            w.tick += 1;
        }
        assert!(opened, "a nearly-open cell should dissolve away");
        assert_eq!(
            w.get_cell(4, 1).unwrap().material,
            MaterialId::Air,
            "fully opened rock becomes void"
        );
        assert_eq!(
            crate::audit::mineral_total(&w),
            before,
            "dissolving the last of a cell must conserve mineral"
        );
    }

    /// Fraction of attempts that open a cell, over many deterministic rolls.
    fn open_rate(throughput: u8, scale: f32) -> f32 {
        let mut hits = 0u32;
        let trials = 400u32;
        for t in 0..trials {
            let mut w = bed(41);
            let mut rock = Cell::solid(MaterialId::Limestone);
            rock.pore = 100;
            w.set_cell(4, 1, rock);
            w.tick = t as u64;
            if widen_aperture(&mut w, 4, 1, throughput, scale, 7) {
                hits += 1;
            } else if w.get_cell(4, 1).unwrap().pore > 100 {
                hits += 1;
            }
        }
        hits as f32 / trials as f32
    }

    #[test]
    fn rock_does_not_yield_below_the_flow_threshold() {
        // Continuous limestone must be *hard*: wetted rock that carries only a
        // trickle stays solid, so erosion cannot spread out into a uniformly
        // more porous aquifer.
        assert_eq!(
            open_rate(APERTURE_MIN_THROUGHPUT, 1000.0),
            0.0,
            "at or below the threshold nothing dissolves, however long it sits"
        );
        assert_eq!(open_rate(1, 1000.0), 0.0);
    }

    #[test]
    fn erosion_is_superlinear_so_flow_focuses_into_channels() {
        // The channelizing property: doubling the water through a cell must more
        // than double how fast it opens, so a small head start compounds into a
        // pipe while neighbours stay effectively solid. A linear response would
        // widen everything evenly.
        let lo = open_rate(40, 6.0);
        let hi = open_rate(80, 6.0);
        assert!(lo > 0.0, "precondition: the low rate is not zero");
        assert!(
            hi > lo * 3.0,
            "2x throughput should open >3x faster (lo={lo:.3} hi={hi:.3})"
        );
    }

    #[test]
    fn artesian_discharge_drops_load_that_would_otherwise_stay_dissolved() {
        // Same cell, same load, same water: at depth it stays in solution, at a
        // depressurised outlet it drops. That difference is the mound.
        let build = || {
            let mut w = bed(11);
            // Soluble floor — travertine cements onto the rock at the outlet.
            let mut floor = Cell::solid(MaterialId::Limestone);
            floor.pore = 120;
            w.set_cell(4, 1, floor);
            let mut c = Cell::air();
            c.sat = Sat(200);
            w.set_cell(4, 2, c);
            // Load just inside what pressurised water can carry.
            let ceiling = (200u16 * SOLUBILITY_PER_SAT) / 16;
            add_dissolved(&mut w, 4, 2, ceiling);
            w
        };
        let mut confined = build();
        let mut discharged = build();
        assert_eq!(
            precipitate_at(&mut confined, 4, 2),
            0,
            "water still under pressure holds its load"
        );
        assert!(
            precipitate_artesian(&mut discharged, 4, 2) > 0,
            "depressurising at an outlet must drop part of the load"
        );
        assert!(
            discharged.get_cell(4, 1).unwrap().pore < 120,
            "the mineral should cement onto the rock at the outlet"
        );
        assert_eq!(
            crate::audit::mineral_total(&discharged),
            crate::audit::mineral_total(&confined),
            "artesian precipitation must conserve mineral"
        );
    }

    #[test]
    fn precipitation_closes_the_aperture_it_opened() {
        // Deposition is the brake on aperture growth: load cements pore shut,
        // one unit per step, and the ledger stays flat.
        let mut w = bed(9);
        let mut rock = Cell::solid(MaterialId::Limestone);
        rock.pore = 120;
        rock.sat = Sat(4); // little water, so the load is over the ceiling
        w.set_cell(4, 1, rock);
        add_dissolved(&mut w, 4, 1, 64);
        let before = crate::audit::mineral_total(&w);
        let used = precipitate_at(&mut w, 4, 1);
        assert!(used > 0, "excess load should cement into the pore space");
        let after = w.get_cell(4, 1).unwrap();
        assert!(
            after.pore < 120,
            "precipitation should tighten the aperture (pore {} -> {})",
            120,
            after.pore
        );
        assert_eq!(
            crate::audit::mineral_total(&w),
            before,
            "precipitation must conserve mineral"
        );
    }

    /// Total mineral: what the audit counts, rock plus load.
    fn mineral_here(w: &World, gx: i32, gy: i32) -> u16 {
        cell_mineral(w.get_cell(gx, gy).unwrap()) + dissolved_at(w, gx, gy)
    }

    #[test]
    fn cementing_sand_conserves_the_mineral_it_consumes() {
        // Loose sediment carries no mineral in the ledger, so cementation has
        // to be one atomic step: the load consumed becomes the new rock's
        // cement content exactly, with `pore` set to the complement.
        let mut w = bed(7);
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(crate::cell::water_capacity(MaterialId::Sand));
        w.set_cell(4, 1, sand);
        add_dissolved(&mut w, 4, 1, 120);
        let before = mineral_here(&w, 4, 1);

        let used = precipitate_over(&mut w, 4, 1, 0);
        assert!(used > 0, "a loaded sand bed should cement");
        let after = w.get_cell(4, 1).unwrap();
        assert_eq!(
            after.material,
            MaterialId::Sandstone,
            "cemented sand is sandstone"
        );
        assert_eq!(
            mineral_here(&w, 4, 1),
            before,
            "cementation must conserve mineral"
        );
        // Lightly cemented means genuinely porous, so it still carries water.
        assert!(
            after.pore > 0,
            "a partly cemented bed should keep pore space, got {}",
            after.pore
        );
    }

    #[test]
    fn a_thin_load_leaves_sand_loose() {
        let mut w = bed(8);
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(crate::cell::water_capacity(MaterialId::Sand));
        w.set_cell(4, 1, sand);
        add_dissolved(&mut w, 4, 1, CEMENT_MIN_LOAD - 1);
        precipitate_over(&mut w, 4, 1, 0);
        assert_eq!(
            w.get_cell(4, 1).unwrap().material,
            MaterialId::Sand,
            "a trace of mineral should not set a whole cell of sand"
        );
    }

    #[test]
    fn dissolving_sandstone_returns_sand_not_a_void() {
        // Only the carbonate matrix is soluble. If a clastic rock opened to Air
        // the grains it was holding together would simply be deleted.
        let mut w = bed(9);
        let mut rock = Cell::solid(MaterialId::Sandstone);
        rock.pore = u8::MAX - 1;
        w.set_cell(4, 1, rock);
        let opened = widen_aperture(&mut w, 4, 1, 255, 1000.0, 0);
        assert!(opened, "a nearly-open cell should finish opening");
        assert_eq!(
            w.get_cell(4, 1).unwrap().material,
            MaterialId::Sand,
            "dissolved sandstone must leave its sand behind"
        );
    }

    #[test]
    fn cemented_rock_is_competent_so_it_can_hold_a_void() {
        // The whole reason for cementing sediment: loose material cannot hold a
        // channel, because repose destroys any void the moment it opens.
        for m in [MaterialId::Sandstone, MaterialId::Conglomerate] {
            assert!(
                crate::cell::is_competent_rock(m),
                "{m:?} must be competent or it cannot hold a conduit open"
            );
            assert!(
                !crate::cell::is_grain(m),
                "{m:?} must not repose like loose sediment"
            );
        }
    }
}
