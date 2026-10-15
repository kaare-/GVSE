//! Dig F3 — Lab seam organism / corpse handoff.
//!
//! See [`docs/HALO_SHARD.md`]. Owner deletes locally, delivers once with an
//! idempotency id; peer injects under pop / habit caps. Un-ACKed parcels
//! stay in-flight and re-spawn on the owner after
//! [`SEAM_LIFE_IN_FLIGHT_TIMEOUT`] exchanges.

use serde::{Deserialize, Serialize};

use crate::fasthash::FxHashSet;
use crate::organism::{habit_of, Atom, Corpse, OrganismStore, SpawnFail};
use crate::seam::{edge_x_for, SeamSide};
use crate::World;

/// Exchanges without ACK before the owner re-spawns the parcel locally.
pub const SEAM_LIFE_IN_FLIGHT_TIMEOUT: u32 = 60;
/// Cap remembered receive ids (idempotency ring).
const SEEN_LIFE_CAP: usize = 256;
/// Max life parcels per exchange (Lab smoke).
pub const SEAM_LIFE_MAX_PER_EXCHANGE: usize = 8;
/// Hold after import so the peer face does not bounce life back next tick.
pub const SEAM_LIFE_IMPORT_HOLD_TICKS: u16 = 30;

/// Wire / backlog unit for one living atom or corpse.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SeamLifeParcel {
    pub id: u64,
    pub kind: SeamLifeKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SeamLifeKind {
    Atom(Atom),
    Corpse(Corpse),
}

impl SeamLifeParcel {
    pub fn anchor_gx(&self) -> i32 {
        match &self.kind {
            SeamLifeKind::Atom(a) => a.gx,
            SeamLifeKind::Corpse(c) => c.gx,
        }
    }
}

/// Dig F3 bookkeeping on [`crate::seam::SeamState`] (not saved).
#[derive(Debug, Clone, Default)]
pub struct SeamLifeState {
    pub next_id: u64,
    pub out_west: Vec<SeamLifeParcel>,
    pub out_east: Vec<SeamLifeParcel>,
    pub in_flight_west: Vec<(SeamLifeParcel, u32)>,
    pub in_flight_east: Vec<(SeamLifeParcel, u32)>,
    /// ACKs for peer life to send on the next Exchange (dialer side).
    pub pending_ack_west: Vec<u64>,
    pub pending_ack_east: Vec<u64>,
    pub seen_ids: FxHashSet<u64>,
    seen_order: Vec<u64>,
}

impl SeamLifeState {
    fn next_id(&mut self) -> u64 {
        let id = self.next_id.max(1);
        self.next_id = id.wrapping_add(1).max(1);
        id
    }

    fn remember_seen(&mut self, id: u64) {
        if !self.seen_ids.insert(id) {
            return;
        }
        self.seen_order.push(id);
        while self.seen_order.len() > SEEN_LIFE_CAP {
            if let Some(old) = self.seen_order.first().copied() {
                self.seen_order.remove(0);
                self.seen_ids.remove(&old);
            } else {
                break;
            }
        }
    }
}

/// Collect atoms/corpses on the open seam face into outbound + in-flight.
pub fn export_life_at_seam(
    world: &mut World,
    store: &mut OrganismStore,
    side: SeamSide,
) -> Vec<SeamLifeParcel> {
    let Some(cfg) = world.seam.config else {
        return Vec::new();
    };
    let open = match side {
        SeamSide::West => cfg.open_west,
        SeamSide::East => cfg.open_east,
    };
    if !open || world.wrap_width.is_some() {
        return Vec::new();
    }
    let edge_x = edge_x_for(cfg, side);
    let mut exported = Vec::new();

    // Live atoms (scan backward for swap_remove).
    let mut i = store.atoms.len();
    while i > 0 {
        i -= 1;
        if exported.len() >= SEAM_LIFE_MAX_PER_EXCHANGE {
            break;
        }
        if store.atoms[i].seam_hold_ticks > 0 {
            store.atoms[i].seam_hold_ticks =
                store.atoms[i].seam_hold_ticks.saturating_sub(1);
            continue;
        }
        if store.atoms[i].gx != edge_x {
            continue;
        }
        let atom = store.atoms.swap_remove(i);
        let id = world.seam.life.next_id();
        let parcel = SeamLifeParcel {
            id,
            kind: SeamLifeKind::Atom(atom),
        };
        push_in_flight(world, side, parcel.clone());
        exported.push(parcel);
    }

    // Parked atoms with anchor on the face.
    let mut i = store.parked.len();
    while i > 0 {
        i -= 1;
        if exported.len() >= SEAM_LIFE_MAX_PER_EXCHANGE {
            break;
        }
        if store.parked[i].seam_hold_ticks > 0 {
            store.parked[i].seam_hold_ticks =
                store.parked[i].seam_hold_ticks.saturating_sub(1);
            continue;
        }
        if store.parked[i].gx != edge_x {
            continue;
        }
        let atom = store.parked.swap_remove(i);
        let id = world.seam.life.next_id();
        let parcel = SeamLifeParcel {
            id,
            kind: SeamLifeKind::Atom(atom),
        };
        push_in_flight(world, side, parcel.clone());
        exported.push(parcel);
    }

    // Corpses.
    let mut i = store.corpses.len();
    while i > 0 {
        i -= 1;
        if exported.len() >= SEAM_LIFE_MAX_PER_EXCHANGE {
            break;
        }
        if store.corpses[i].seam_hold_ticks > 0 {
            store.corpses[i].seam_hold_ticks =
                store.corpses[i].seam_hold_ticks.saturating_sub(1);
            continue;
        }
        if store.corpses[i].gx != edge_x {
            continue;
        }
        let corpse = store.corpses.swap_remove(i);
        let id = world.seam.life.next_id();
        let parcel = SeamLifeParcel {
            id,
            kind: SeamLifeKind::Corpse(corpse),
        };
        push_in_flight(world, side, parcel.clone());
        exported.push(parcel);
    }

    let mut i = store.parked_corpses.len();
    while i > 0 {
        i -= 1;
        if exported.len() >= SEAM_LIFE_MAX_PER_EXCHANGE {
            break;
        }
        if store.parked_corpses[i].seam_hold_ticks > 0 {
            store.parked_corpses[i].seam_hold_ticks =
                store.parked_corpses[i].seam_hold_ticks.saturating_sub(1);
            continue;
        }
        if store.parked_corpses[i].gx != edge_x {
            continue;
        }
        let corpse = store.parked_corpses.swap_remove(i);
        let id = world.seam.life.next_id();
        let parcel = SeamLifeParcel {
            id,
            kind: SeamLifeKind::Corpse(corpse),
        };
        push_in_flight(world, side, parcel.clone());
        exported.push(parcel);
    }

    match side {
        SeamSide::West => world.seam.life.out_west = exported.clone(),
        SeamSide::East => world.seam.life.out_east = exported.clone(),
    }
    exported
}

fn push_in_flight(world: &mut World, side: SeamSide, parcel: SeamLifeParcel) {
    let slot = match side {
        SeamSide::West => &mut world.seam.life.in_flight_west,
        SeamSide::East => &mut world.seam.life.in_flight_east,
    };
    slot.push((parcel, 0));
}

/// Drain outbound life for the wire (clears the out bucket; in-flight stays).
pub fn take_life_outbound(world: &mut World, side: SeamSide) -> Vec<SeamLifeParcel> {
    match side {
        SeamSide::West => std::mem::take(&mut world.seam.life.out_west),
        SeamSide::East => std::mem::take(&mut world.seam.life.out_east),
    }
}

/// Apply peer ACK ids — drop matching in-flight tombstones.
pub fn ack_life_ids(world: &mut World, side: SeamSide, ids: &[u64]) {
    if ids.is_empty() {
        return;
    }
    let set: FxHashSet<u64> = ids.iter().copied().collect();
    let slot = match side {
        SeamSide::West => &mut world.seam.life.in_flight_west,
        SeamSide::East => &mut world.seam.life.in_flight_east,
    };
    slot.retain(|(p, _)| !set.contains(&p.id));
}

/// Age in-flight; re-spawn parcels that timed out (neighbor down / lost ACK).
pub fn tick_life_in_flight(world: &mut World, store: &mut OrganismStore, side: SeamSide) {
    let slot = match side {
        SeamSide::West => &mut world.seam.life.in_flight_west,
        SeamSide::East => &mut world.seam.life.in_flight_east,
    };
    let mut keep = Vec::new();
    let mut revive = Vec::new();
    for (parcel, age) in slot.drain(..) {
        let age = age + 1;
        if age >= SEAM_LIFE_IN_FLIGHT_TIMEOUT {
            revive.push(parcel);
        } else {
            keep.push((parcel, age));
        }
    }
    *slot = keep;
    for parcel in revive {
        let _ = inject_life_local(store, parcel);
    }
}

fn inject_life_local(store: &mut OrganismStore, parcel: SeamLifeParcel) -> Result<(), SpawnFail> {
    match parcel.kind {
        SeamLifeKind::Atom(mut atom) => {
            if store.living_count() >= store.atom_cap() {
                return Err(SpawnFail::PopCap);
            }
            let habit = habit_of(&atom);
            if !store.habit_has_room(habit, 0) {
                return Err(SpawnFail::HabitCap(habit));
            }
            atom.seam_hold_ticks = SEAM_LIFE_IMPORT_HOLD_TICKS;
            store.atoms.push(atom);
            Ok(())
        }
        SeamLifeKind::Corpse(mut corpse) => {
            corpse.seam_hold_ticks = SEAM_LIFE_IMPORT_HOLD_TICKS;
            store.corpses.push(corpse);
            Ok(())
        }
    }
}

/// Inject peer life onto our seam face. Returns ids successfully applied
/// (including idempotent duplicates already seen).
pub fn apply_life_inbound(
    world: &mut World,
    store: &mut OrganismStore,
    side: SeamSide,
    parcels: Vec<SeamLifeParcel>,
) -> Vec<u64> {
    let Some(cfg) = world.seam.config else {
        return Vec::new();
    };
    let edge_x = edge_x_for(cfg, side);
    let mut acked = Vec::new();
    for mut parcel in parcels {
        if world.seam.life.seen_ids.contains(&parcel.id) {
            acked.push(parcel.id);
            continue;
        }
        // Remap anchor onto our open face; modules stay body-local.
        match &mut parcel.kind {
            SeamLifeKind::Atom(a) => a.gx = edge_x,
            SeamLifeKind::Corpse(c) => c.gx = edge_x,
        }
        match inject_life_local(store, parcel.clone()) {
            Ok(()) => {
                world.seam.life.remember_seen(parcel.id);
                acked.push(parcel.id);
            }
            Err(_) => {
                // Habit/pop full — leave unacked so owner can revive.
            }
        }
    }
    world.seam.silent_ticks = 0;
    acked
}

/// Queue ACK ids for the next outbound Exchange (after applying peer life).
pub fn stash_pending_acks(world: &mut World, side: SeamSide, ids: Vec<u64>) {
    if ids.is_empty() {
        return;
    }
    let slot = match side {
        SeamSide::West => &mut world.seam.life.pending_ack_west,
        SeamSide::East => &mut world.seam.life.pending_ack_east,
    };
    slot.extend(ids);
}

/// Take pending ACK ids for the wire.
pub fn take_pending_acks(world: &mut World, side: SeamSide) -> Vec<u64> {
    match side {
        SeamSide::West => std::mem::take(&mut world.seam.life.pending_ack_west),
        SeamSide::East => std::mem::take(&mut world.seam.life.pending_ack_east),
    }
}

/// In-process Lab smoke: west east-face ↔ east west-face life.
pub fn exchange_seam_life(
    west: &mut World,
    west_orgs: &mut OrganismStore,
    east: &mut World,
    east_orgs: &mut OrganismStore,
) {
    tick_life_in_flight(west, west_orgs, SeamSide::East);
    tick_life_in_flight(east, east_orgs, SeamSide::West);

    let to_east = {
        export_life_at_seam(west, west_orgs, SeamSide::East);
        take_life_outbound(west, SeamSide::East)
    };
    let to_west = {
        export_life_at_seam(east, east_orgs, SeamSide::West);
        take_life_outbound(east, SeamSide::West)
    };

    let ack_e = apply_life_inbound(east, east_orgs, SeamSide::West, to_east);
    let ack_w = apply_life_inbound(west, west_orgs, SeamSide::East, to_west);
    ack_life_ids(west, SeamSide::East, &ack_e);
    ack_life_ids(east, SeamSide::West, &ack_w);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use crate::chunk::CHUNK_CELLS_W;
    use crate::organism::{Atom, ModuleId};
    use crate::seam::attach_open_seam;
    use wk_material::MaterialId;

    fn slab_pair() -> (World, World) {
        let w = CHUNK_CELLS_W as i32;
        let mut west = World::new(1);
        let mut east = World::new(2);
        for x in 0..w {
            west.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            east.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            west.set_cell(x, 1, Cell::air());
            east.set_cell(x, 1, Cell::air());
        }
        attach_open_seam(&mut west, w, SeamSide::East);
        attach_open_seam(&mut east, w, SeamSide::West);
        (west, east)
    }

    fn floater_at(gx: i32, gy: i32) -> Atom {
        Atom::from_body(gx, gy, 8.0, vec![(0, 0, ModuleId::Nucleus)])
    }

    #[test]
    fn atom_crosses_seam_once() {
        let (mut west, mut east) = slab_pair();
        let mut west_orgs = OrganismStore::new();
        let mut east_orgs = OrganismStore::new();
        let edge = CHUNK_CELLS_W as i32 - 1;
        west_orgs.atoms.push(floater_at(edge, 1));
        assert_eq!(west_orgs.living_count(), 1);
        assert_eq!(east_orgs.living_count(), 0);

        exchange_seam_life(&mut west, &mut west_orgs, &mut east, &mut east_orgs);

        assert_eq!(west_orgs.living_count(), 0);
        assert_eq!(east_orgs.living_count(), 1);
        assert_eq!(east_orgs.atoms[0].gx, 0);
        assert!(west.seam.life.in_flight_east.is_empty());
        assert!(east_orgs.atoms[0].seam_hold_ticks > 0);

        // Second exchange must not bounce / duplicate (import hold).
        exchange_seam_life(&mut west, &mut west_orgs, &mut east, &mut east_orgs);
        assert_eq!(east_orgs.living_count(), 1);
        assert_eq!(west_orgs.living_count(), 0);
    }

    #[test]
    fn habit_cap_blocks_and_timeout_revives() {
        let (mut west, mut east) = slab_pair();
        let mut west_orgs = OrganismStore::new();
        let mut east_orgs = OrganismStore::new();
        // Fill east atom habit soft-cap with floaters away from the face.
        let cap = crate::organism::HABIT_SOFT_CAP_ATOMS;
        for i in 0..cap {
            east_orgs.atoms.push(floater_at(2 + (i as i32 % 10), 1));
        }
        assert!(!east_orgs.habit_has_room(crate::organism::HabitKind::Atom, 0));

        let edge = CHUNK_CELLS_W as i32 - 1;
        west_orgs.atoms.push(floater_at(edge, 1));
        export_life_at_seam(&mut west, &mut west_orgs, SeamSide::East);
        let outbound = take_life_outbound(&mut west, SeamSide::East);
        assert_eq!(outbound.len(), 1);
        let acked = apply_life_inbound(&mut east, &mut east_orgs, SeamSide::West, outbound);
        assert!(acked.is_empty(), "habit-full peer must not ACK");
        assert_eq!(west.seam.life.in_flight_east.len(), 1);

        // Age past timeout → revive on west.
        for _ in 0..SEAM_LIFE_IN_FLIGHT_TIMEOUT {
            tick_life_in_flight(&mut west, &mut west_orgs, SeamSide::East);
        }
        assert_eq!(west_orgs.living_count(), 1);
        assert!(west.seam.life.in_flight_east.is_empty());
    }
}
