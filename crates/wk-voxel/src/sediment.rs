//! Suspended fine sediment: the *other* way water moves solid material.
//!
//! Three transport modes now coexist, and keeping them apart is what makes each
//! one behave correctly:
//!
//! | mode | mechanism | drops out when |
//! |------|-----------|----------------|
//! | bedload | `grain::apply_flow_erosion` relocates whole cells | it stops being pushed |
//! | dissolved | `mineral` — carbonate in solution | the water **leaves** or concentrates |
//! | suspended | this module — clay held up by turbulence | the water **slows** |
//!
//! Clay is not dissolved. It is entrained, held up by the flow itself, so it
//! settles in slack water and is filtered out by pore space rather than soaking
//! into it. Treating it as dissolved load would cement mud in place instead of
//! letting it settle, which is the opposite of what builds a delta.
//!
//! Only clay-grade fines suspend. Sand and gravel are too coarse — they travel
//! as bedload, which the grain pass already does. Bentonite is deliberately
//! excluded: an aquitard that washes away is not a seal.

use wk_material::MaterialId;

use crate::cell::{water_capacity_cell, Cell, Sat};
use crate::grid::World;

/// Sediment units in one full cell of fines. One cell of clay entrains to
/// exactly this, and exactly this deposits back as one cell of clay.
pub const SEDIMENT_PER_CELL: u16 = 255;

/// Suspended load one unit of **moving** water can hold.
///
/// Sized so a reasonably deep flow can carry a cell of fines (sat 64 holds
/// exactly one) while a damp film cannot lift anything. A film that could strip
/// a bed and drop it again next tick is the failure mode this guards.
pub const SUSPEND_PER_MOVING_SAT: u16 = 4;

/// Suspended load **slack** water can hold: none.
///
/// Turbulence is the only thing holding fines up, so when the water stops, all
/// of it drops. That is the whole character of suspended load as against
/// dissolved, and it is what builds a delta where a river slows rather than
/// where it dries.
pub const SUSPEND_PER_SLACK_SAT: u16 = 0;

/// Minimum water in a cell before it can hold sediment up at all.
const MIN_CARRIER_SAT: u8 = 24;

/// True for material fine enough to travel in suspension.
///
/// Bentonite is excluded on purpose: it is the aquitard, and a seal that washes
/// away seals nothing.
#[inline]
pub fn is_suspendable(material: MaterialId) -> bool {
    matches!(material, MaterialId::Clay)
}

/// Sediment mass this cell holds as solid material, for the audit.
#[inline]
pub fn cell_sediment(cell: Cell) -> u16 {
    if is_suspendable(cell.material) {
        SEDIMENT_PER_CELL
    } else {
        0
    }
}

pub fn suspended_at(world: &World, gx: i32, gy: i32) -> u16 {
    let gx = world.wrap_x(gx);
    world.suspended.get(&(gx, gy)).copied().unwrap_or(0)
}

pub fn add_suspended(world: &mut World, gx: i32, gy: i32, amount: u16) {
    if amount == 0 {
        return;
    }
    let gx = world.wrap_x(gx);
    let slot = world.suspended.entry((gx, gy)).or_insert(0);
    *slot = slot.saturating_add(amount);
}

/// Remove up to `amount`, returning what was actually taken.
pub fn take_suspended(world: &mut World, gx: i32, gy: i32, amount: u16) -> u16 {
    if amount == 0 {
        return 0;
    }
    let gx = world.wrap_x(gx);
    let Some(slot) = world.suspended.get_mut(&(gx, gy)) else {
        return 0;
    };
    let took = (*slot).min(amount);
    *slot -= took;
    if *slot == 0 {
        world.suspended.remove(&(gx, gy));
    }
    took
}

/// How much suspended load this cell can hold up.
///
/// Zero for anything but open water: pore space **filters** suspension rather
/// than admitting it, which is why muddy water clogs a gravel bed instead of
/// carrying silt down into the aquifer.
pub fn carrying_capacity(world: &World, gx: i32, gy: i32, moving: bool) -> u16 {
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    if cell.material != MaterialId::Air || cell.sat.0 < MIN_CARRIER_SAT {
        return 0;
    }
    let per = if moving {
        SUSPEND_PER_MOVING_SAT
    } else {
        SUSPEND_PER_SLACK_SAT
    };
    (cell.sat.0 as u16).saturating_mul(per)
}

/// Move suspended load with a water transfer, pro rata.
///
/// Mirrors `mineral::carry_with_water`, but refuses to enter pore space: a
/// filtered grain bed is the physically right outcome and it is also what stops
/// silt teleporting into an aquifer.
pub fn carry_with_water(
    world: &mut World,
    from: (i32, i32),
    to: (i32, i32),
    moved_sat: u8,
    donor_sat_before: u8,
) {
    if moved_sat == 0 || donor_sat_before == 0 {
        return;
    }
    let load = suspended_at(world, from.0, from.1);
    if load == 0 {
        return;
    }
    // Pore space filters fines out; they stay behind with the donor.
    let Some(dest) = world.get_cell(to.0, to.1) else {
        return;
    };
    if dest.material != MaterialId::Air {
        return;
    }
    let share = (load as u32 * moved_sat as u32 / donor_sat_before as u32) as u16;
    let moved = take_suspended(world, from.0, from.1, share);
    add_suspended(world, to.0, to.1, moved);
}

/// Entrain a cell of fines into the water above / beside it.
///
/// Atomic, like cementation and for the same reason: a cell has nowhere to bank
/// "40% eroded", so a cell of clay either goes into suspension whole or stays
/// put. Returns true when it was taken.
pub fn entrain_cell(world: &mut World, gx: i32, gy: i32, carrier: (i32, i32)) -> bool {
    let gx = world.wrap_x(gx);
    let Some(cell) = world.get_cell(gx, gy) else {
        return false;
    };
    if !is_suspendable(cell.material) {
        return false;
    }
    // The water has to be able to hold what it picks up, or it would drop it
    // again on the same tick and the pair would flicker.
    if carrying_capacity(world, carrier.0, carrier.1, true) < SEDIMENT_PER_CELL {
        return false;
    }
    // Bed becomes open water, keeping whatever the pore held. Never mint water.
    world.set_cell(
        gx,
        gy,
        Cell {
            material: MaterialId::Air,
            sat: cell.sat,
            ..cell
        },
    );
    add_suspended(world, carrier.0, carrier.1, SEDIMENT_PER_CELL);
    true
}

/// Drop whatever load the water at this cell can no longer hold up.
///
/// Returns the units deposited. Deposition needs a whole cell's worth *and* a
/// seat under it — a floating mud cell is not a thing, and an unsupported one
/// would only fall as debris next tick.
pub fn settle_at(world: &mut World, gx: i32, gy: i32, moving: bool) -> u16 {
    let gx = world.wrap_x(gx);
    let load = suspended_at(world, gx, gy);
    if load == 0 {
        return 0;
    }
    let ceiling = carrying_capacity(world, gx, gy, moving);
    if load <= ceiling {
        return 0;
    }
    if load < SEDIMENT_PER_CELL {
        // Not enough for a cell yet. Hand it downward so fines accumulate at the
        // bed instead of stalling one short forever in mid-column.
        return sink_toward_bed(world, gx, gy, load - ceiling);
    }
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0;
    };
    if cell.material != MaterialId::Air {
        return 0;
    }
    let seated = matches!(
        world.get_cell(gx, gy - 1),
        Some(b) if b.material != MaterialId::Air
    );
    if !seated {
        return sink_toward_bed(world, gx, gy, load);
    }
    let used = take_suspended(world, gx, gy, SEDIMENT_PER_CELL);
    if used < SEDIMENT_PER_CELL {
        // Could not get a full cell after all; put it back rather than deposit
        // a cell we did not pay for.
        add_suspended(world, gx, gy, used);
        return 0;
    }
    let mut mud = Cell::solid(MaterialId::Clay);
    let cap = water_capacity_cell(mud, &world.hydro);
    mud.sat = Sat(cell.sat.0.min(cap));
    // Water the new mud cannot hold is displaced upward, not destroyed.
    let spill = cell.sat.0.saturating_sub(mud.sat.0);
    world.set_cell(gx, gy, mud);
    if spill > 0 {
        push_water_up(world, gx, gy + 1, spill);
    }
    used
}

/// Hand load down the water column toward the bed.
fn sink_toward_bed(world: &mut World, gx: i32, gy: i32, amount: u16) -> u16 {
    let Some(below) = world.get_cell(gx, gy - 1) else {
        return 0;
    };
    if below.material != MaterialId::Air || below.sat.0 < MIN_CARRIER_SAT {
        return 0;
    }
    let moved = take_suspended(world, gx, gy, amount);
    add_suspended(world, gx, gy - 1, moved);
    0
}

/// Push displaced water up the Air column so a deposit never destroys it.
fn push_water_up(world: &mut World, gx: i32, gy: i32, mut spill: u8) {
    let mut y = gy;
    for _ in 0..16 {
        if spill == 0 {
            return;
        }
        let Some(cell) = world.get_cell(gx, y) else {
            return;
        };
        if cell.material != MaterialId::Air {
            return;
        }
        let room = 255u8.saturating_sub(cell.sat.0);
        let put = room.min(spill);
        if put > 0 {
            let mut next = cell;
            next.sat = Sat(cell.sat.0 + put);
            world.set_cell(gx, y, next);
            spill -= put;
        }
        y += 1;
    }
}

/// Entrain fines where water moves, settle them where it slows.
///
/// Both halves have to be one pass over the same cells, because the thing that
/// decides which happens is the same test — whether this water is going
/// anywhere. Splitting them would let a cell entrain on one pass and settle on
/// the next.
///
/// Rides the flow-erosion cadence (every other tick): moving mud is a surface
/// process, but not one that needs resolving every frame.
pub fn apply_suspension(world: &mut World) {
    if world.tick % 2 != 0 {
        return;
    }
    // Needs both free water *and* loose material to have anything to do, which
    // rules out open-water chunks and dry rock alike — only beds and banks
    // qualify.
    let mut coords: Vec<crate::chunk::ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air && c.has_loose)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));

    let ch = crate::chunk::CHUNK_CELLS_H as i32;
    let cw = crate::chunk::CHUNK_CELLS_W as i32;
    // Entrainment happens only at the water/fines interface, so only look at
    // water that actually touches fines. Considering every wet cell instead cost
    // 2.3 ms/tick on the demo world — as much as all of seepage — because a
    // large ocean is overwhelmingly cells with nothing beneath them to lift.
    // Scan for the *fines*, not for the water. Fines are far rarer than water in
    // an ocean world, and each one has at most three faces to check, so the work
    // tracks how much erodible bed there is rather than how much sea.
    let mut exposed: Vec<(i32, i32)> = Vec::new();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        let base_gx = coord.cx * cw;
        let base_gy = coord.cy * ch;
        for ly in 0..ch {
            for lx in 0..cw {
                if !is_suspendable(chunk.get(lx as usize, ly as usize).material) {
                    continue;
                }
                exposed.push((world.wrap_x(base_gx + lx), base_gy + ly));
            }
        }
    }
    for (gx, gy) in exposed {
        // Water above scours the bed; water beside undercuts the bank. Same
        // faces flow erosion uses for bedload — this is the fine-grained half of
        // the same process.
        for (dx, dy) in [(0, 1), (-1, 0), (1, 0)] {
            let cx = world.wrap_x(gx + dx);
            let cy = gy + dy;
            let Some(carrier) = world.get_cell(cx, cy) else {
                continue;
            };
            if carrier.material != MaterialId::Air || carrier.sat.0 < MIN_CARRIER_SAT {
                continue;
            }
            if crate::rules::grain::flow_bias(world, cx, cy, carrier.sat).is_none() {
                continue;
            }
            if entrain_cell(world, gx, gy, (cx, cy)) {
                break;
            }
        }
    }

    // Settling is driven from the load map rather than from a scan: the state is
    // sparse, so the work is proportional to how much mud is actually in transit.
    let carrying: Vec<(i32, i32)> = world.suspended.keys().copied().collect();
    for (gx, gy) in carrying {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if crate::rules::grain::flow_bias(world, gx, gy, cell.sat).is_some() {
            continue;
        }
        settle_at(world, gx, gy, false);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;

    fn pool(seed: u64) -> World {
        let mut w = World::new(seed);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    fn total(w: &World) -> u32 {
        let mut t = 0u32;
        for chunk in w.chunks.values() {
            for cell in &chunk.cells {
                t += cell_sediment(*cell) as u32;
            }
        }
        t + w.suspended.values().map(|&v| v as u32).sum::<u32>()
    }

    #[test]
    fn entraining_clay_conserves_sediment() {
        let mut w = pool(1);
        w.set_cell(4, 1, Cell::solid(MaterialId::Clay));
        w.set_cell(4, 2, Cell::water());
        let before = total(&w);
        assert!(entrain_cell(&mut w, 4, 1, (4, 2)), "flowing water should lift clay");
        assert_eq!(total(&w), before, "entrainment must conserve sediment");
        assert_eq!(w.get_cell(4, 1).unwrap().material, MaterialId::Air);
        assert_eq!(suspended_at(&w, 4, 2), SEDIMENT_PER_CELL);
    }

    #[test]
    fn settling_returns_the_clay_it_took() {
        let mut w = pool(2);
        w.set_cell(4, 1, Cell::water());
        add_suspended(&mut w, 4, 1, SEDIMENT_PER_CELL);
        let before = total(&w);
        // Slack water: no flow bonus, so the ceiling is low and load drops.
        let used = settle_at(&mut w, 4, 1, false);
        assert_eq!(used, SEDIMENT_PER_CELL, "a full cell of load should settle");
        assert_eq!(total(&w), before, "settling must conserve sediment");
        assert_eq!(w.get_cell(4, 1).unwrap().material, MaterialId::Clay);
    }

    #[test]
    fn moving_water_holds_what_slack_water_drops() {
        // The defining property: capacity depends on flow, so slowing is what
        // deposits. If this stops being true, mud stops forming banks.
        let mut w = pool(3);
        w.set_cell(4, 1, Cell::water());
        let moving = carrying_capacity(&w, 4, 1, true);
        let slack = carrying_capacity(&w, 4, 1, false);
        assert!(
            moving > slack,
            "moving water must carry more ({moving} vs {slack})"
        );
    }

    #[test]
    fn pore_space_filters_suspension_instead_of_admitting_it() {
        // Silt must not travel into an aquifer: fines are strained out at the
        // bed. This is also what stops suspension becoming a second, faster
        // route into pore space that dissolved load does not have.
        let mut w = pool(4);
        w.set_cell(4, 2, Cell::water());
        let mut sand = Cell::solid(MaterialId::Sand);
        sand.sat = Sat(crate::cell::water_capacity(MaterialId::Sand));
        w.set_cell(4, 1, sand);
        add_suspended(&mut w, 4, 2, 200);
        carry_with_water(&mut w, (4, 2), (4, 1), 128, 255);
        assert_eq!(
            suspended_at(&w, 4, 1),
            0,
            "pore space must filter fines out, not take them in"
        );
        assert_eq!(suspended_at(&w, 4, 2), 200, "the load stays with the water");
    }

    #[test]
    fn transport_between_open_water_conserves_load() {
        let mut w = pool(5);
        w.set_cell(4, 2, Cell::water());
        w.set_cell(5, 2, Cell::water());
        add_suspended(&mut w, 4, 2, 200);
        carry_with_water(&mut w, (4, 2), (5, 2), 128, 255);
        let moved = suspended_at(&w, 5, 2);
        assert!((99..=101).contains(&moved), "about half should travel, got {moved}");
        assert_eq!(moved + suspended_at(&w, 4, 2), 200, "transport must conserve");
    }

    #[test]
    fn clay_that_cannot_be_held_up_is_left_alone() {
        // A trickle must not lift a whole cell of clay, or a damp film would
        // strip a bed and drop it again on the next tick.
        let mut w = pool(6);
        w.set_cell(4, 1, Cell::solid(MaterialId::Clay));
        let mut trickle = Cell::air();
        trickle.sat = Sat(30);
        w.set_cell(4, 2, trickle);
        assert!(!entrain_cell(&mut w, 4, 1, (4, 2)));
        assert_eq!(w.get_cell(4, 1).unwrap().material, MaterialId::Clay);
    }

    #[test]
    fn water_spilling_over_a_clay_lip_suspends_it_and_conserves_it() {
        // End to end through the real passes: entrain, travel, settle.
        //
        // The geometry is a cascade rather than a flat sheet, because that is
        // what `flow_bias` recognises as moving water — a brim-full sheet has
        // neither a surface gradient nor a lip, so nothing would be entrained
        // and the conservation check would pass without testing anything.
        let mut w = World::new(21);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // A clay bench on the left, open floor to the right: water runs off the
        // lip at x=8.
        for x in 0..9 {
            for y in 1..4 {
                w.set_cell(x, y, Cell::solid(MaterialId::Clay));
            }
        }
        w.set_cell(0, 4, Cell::solid(MaterialId::Bedrock));
        w.set_cell(0, 5, Cell::solid(MaterialId::Bedrock));
        // Standing water on the bench, spilling toward the drop.
        for x in 1..9 {
            w.set_cell(x, 4, Cell::water());
            w.set_cell(x, 5, Cell::water());
        }
        let before = total(&w);
        assert!(before > 0, "the fixture should contain clay");

        let perf = crate::rules::PerfConfig::default();
        let grain = crate::rules::GrainConfig::default();
        let mut peak_load = 0u32;
        for _ in 0..400 {
            crate::rules::tick_with_perf(&mut w, &perf);
            crate::rules::apply_flow_erosion(&mut w, &grain);
            apply_suspension(&mut w);
            peak_load = peak_load.max(w.suspended.values().map(|&v| v as u32).sum());
            assert_eq!(
                total(&w),
                before,
                "entrain -> transport -> settle must conserve sediment every tick"
            );
        }
        // Conservation is trivially true if nothing ever moved, so pin that the
        // spill did in fact pick sediment up.
        assert!(
            peak_load > 0,
            "water running off a clay lip should suspend something"
        );
    }
}
