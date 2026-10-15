//! Living cells and organisms with a realised map (§b.5).
//!
//! An organism is the set of cells connected by division bonds plus the
//! slots they grew into. Slots outlive their cells so a parent can regrow
//! a lost part; a subtree cut off from the seed re-roots as a new
//! organism; a brood daughter detaches when her gate fires.
#![allow(clippy::too_many_arguments)]

use serde::{Deserialize, Serialize};
use crate::bio::genome::GenomeStore;
use crate::bio::program::Gate;
use crate::consts::MAX_SLOTS;
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;

pub const EMPTY_OWNER: u32 = 0;
/// Owner ids for living cells start at 1 (index into CellStore + 1).
pub const OWNER_BIAS: u32 = 1;
pub const NONE: u32 = u32::MAX;
pub const NONE16: u16 = u16::MAX;

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct CellRec {
    pub px: u16,
    pub py: u16,
    /// Current realised size in pixels (grows toward the mode shape).
    pub w: u8,
    pub h: u8,
    pub genotype: u32,
    pub org: u32,
    pub slot: u16,
    pub mode: u8,
    /// Metabolic age in activity ticks (idling cells age slower).
    pub age: u16,
    /// Sub-tick ageing accumulator (Q8).
    pub age_acc: u8,
    /// Fixed-point glucose store (`consts::FP` = 1 g).
    pub energy: i32,
    /// 0..=255 composite stress.
    pub stress: u8,
    pub damage: u8,
    pub waste: u8,
    pub heading: u8,
    /// Index into the active child list (which child entry is next).
    pub child_idx: u8,
    /// Remaining repeats on the current child entry.
    pub child_reps: u8,
    /// Hop count from the organism seed along division bonds.
    pub depth: u8,
    /// Division-bond parent cell id (`NONE` = seed).
    pub parent_cell: u32,
    /// GLUE attachment to another organism's cell (`NONE` = none).
    pub glue_to: u32,
    pub glue_strength: u8,
    /// Bonded daughter waiting for her detach gate (egg / brood).
    pub brood: bool,
    pub detach_gate: Gate,
    /// Received signal amplitude this tick.
    pub sig: u8,
    /// Ticks of white division flash left.
    pub flash: u8,
    /// Cilia drive state (running vs tumbling).
    pub running: bool,
    /// Stuck as front-pane film.
    pub film: bool,
    pub alive: bool,
}

impl CellRec {
    fn blank() -> Self {
        Self {
            px: 0,
            py: 0,
            w: 1,
            h: 1,
            genotype: 0,
            org: 0,
            slot: 0,
            mode: 0,
            age: 0,
            age_acc: 0,
            energy: 0,
            stress: 0,
            damage: 0,
            waste: 0,
            heading: 2,
            child_idx: 0,
            child_reps: 0,
            depth: 0,
            parent_cell: NONE,
            glue_to: NONE,
            glue_strength: 0,
            brood: false,
            detach_gate: Gate::always(),
            sig: 0,
            flash: 0,
            running: true,
            film: false,
            alive: false,
        }
    }

    pub fn area(&self) -> i32 {
        self.w as i32 * self.h as i32
    }
}

/// One grown position of a body (§b.5 realised map).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Slot {
    /// Living cell in this slot, or `NONE` when dead/shed (regrowable).
    pub cell: u32,
    pub rel_x: i16,
    pub rel_y: i16,
    pub mode: u8,
    pub parent_slot: u16,
    pub bond: u8,
    pub heading: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OrgRec {
    pub seed: u32,
    pub generation: u32,
    pub alive: bool,
    pub origin_x: i32,
    pub origin_y: i32,
    pub slots: Vec<Slot>,
    pub n_alive: u16,
    /// Σ full build cost of alive slots (pace law body cost).
    pub body_cost_fp: i32,
    /// Birth tick, for inspect.
    pub born_tick: u64,
}

#[derive(Debug, Clone)]
pub struct CellStore {
    cells: Vec<CellRec>,
    free: Vec<u32>,
    orgs: Vec<OrgRec>,
    org_free: Vec<u32>,
    live: usize,
}

impl CellStore {
    pub fn new() -> Self {
        Self {
            cells: Vec::new(),
            free: Vec::new(),
            orgs: Vec::new(),
            org_free: Vec::new(),
            live: 0,
        }
    }

    pub fn len(&self) -> usize {
        self.live
    }

    pub fn is_empty(&self) -> bool {
        self.live == 0
    }

    pub fn capacity_ids(&self) -> u32 {
        self.cells.len() as u32
    }

    pub fn get(&self, id: u32) -> Option<&CellRec> {
        self.cells.get(id as usize).filter(|c| c.alive)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut CellRec> {
        self.cells.get_mut(id as usize).filter(|c| c.alive)
    }

    pub fn org(&self, org: u32) -> Option<&OrgRec> {
        self.orgs.get(org as usize).filter(|o| o.alive)
    }

    pub fn org_mut(&mut self, org: u32) -> Option<&mut OrgRec> {
        self.orgs.get_mut(org as usize).filter(|o| o.alive)
    }

    pub fn iter_alive(&self) -> impl Iterator<Item = (u32, &CellRec)> {
        self.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.alive)
            .map(|(i, c)| (i as u32, c))
    }

    pub fn iter_orgs(&self) -> impl Iterator<Item = (u32, &OrgRec)> {
        self.orgs
            .iter()
            .enumerate()
            .filter(|(_, o)| o.alive)
            .map(|(i, o)| (i as u32, o))
    }

    pub fn live_ids(&self) -> Vec<u32> {
        self.iter_alive().map(|(id, _)| id).collect()
    }

    fn alloc_cell(&mut self) -> u32 {
        if let Some(id) = self.free.pop() {
            id
        } else {
            let id = self.cells.len() as u32;
            self.cells.push(CellRec::blank());
            id
        }
    }

    fn alloc_org(&mut self, seed: u32, generation: u32, ox: i32, oy: i32, tick: u64) -> u32 {
        let rec = OrgRec {
            seed,
            generation,
            alive: true,
            origin_x: ox,
            origin_y: oy,
            slots: Vec::new(),
            n_alive: 0,
            body_cost_fp: 0,
            born_tick: tick,
        };
        if let Some(id) = self.org_free.pop() {
            self.orgs[id as usize] = rec;
            id
        } else {
            let id = self.orgs.len() as u32;
            self.orgs.push(rec);
            id
        }
    }

    fn first_reps(genomes: &GenomeStore, genotype: u32, mode: u8) -> u8 {
        let g = genomes.genome(genotype);
        let m = g.mode(mode);
        if m.n_children > 0 {
            m.children[0].repeat
        } else {
            0
        }
    }

    /// Spawn a new organism: one 1×1 cell in `mode` at `(px, py)`.
    pub fn spawn(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        px: u16,
        py: u16,
        genotype: u32,
        mode: u8,
        generation: u32,
        energy: i32,
        heading: u8,
        tick: u64,
        seed: u64,
    ) -> Option<u32> {
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if idx >= pixels.owner.len() || pixels.owner[idx] != EMPTY_OWNER {
            return None;
        }
        let cell_id = self.alloc_cell();
        let org_id = self.alloc_org(cell_id, generation, px as i32, py as i32, tick);
        // Bodies grow on the 4 cardinal headings (daughters stay
        // edge-adjacent); a new organism turns ±90° at random.
        let jitter = (hash_u64(seed, cell_id as u64, genotype as u64, 0xC1) % 3) as u8;
        let heading = heading & 6;
        let heading = match jitter {
            0 => heading.wrapping_add(6) % 8,
            1 => heading,
            _ => heading.wrapping_add(2) % 8,
        };
        let reps = Self::first_reps(genomes, genotype, mode);
        let build = genomes.coeffs(genotype, mode).build_fp;
        self.cells[cell_id as usize] = CellRec {
            px,
            py,
            genotype,
            org: org_id,
            slot: 0,
            mode,
            energy,
            heading,
            child_reps: reps,
            alive: true,
            ..CellRec::blank()
        };
        let org = &mut self.orgs[org_id as usize];
        org.slots.push(Slot {
            cell: cell_id,
            rel_x: 0,
            rel_y: 0,
            mode,
            parent_slot: NONE16,
            bond: 0,
            heading,
        });
        org.n_alive = 1;
        org.body_cost_fp = build;
        claim_rect(pixels, px, py, 1, 1, cell_id + OWNER_BIAS);
        genomes.on_birth(genotype);
        self.live += 1;
        Some(cell_id)
    }

    /// Spawn a bonded body part (or brood daughter) into the parent's organism.
    pub fn spawn_bonded(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        parent: u32,
        px: u16,
        py: u16,
        mode: u8,
        bond: u8,
        heading: u8,
        energy: i32,
        brood: Option<Gate>,
    ) -> Option<u32> {
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if idx >= pixels.owner.len() || pixels.owner[idx] != EMPTY_OWNER {
            return None;
        }
        let p = *self.get(parent)?;
        let org_id = p.org;
        {
            let org = self.orgs.get(org_id as usize)?;
            if !org.alive || org.slots.len() >= MAX_SLOTS {
                return None;
            }
        }
        let genotype = p.genotype;
        let cell_id = self.alloc_cell();
        let reps = Self::first_reps(genomes, genotype, mode);
        let build = genomes.coeffs(genotype, mode).build_fp;
        let org = &mut self.orgs[org_id as usize];
        let slot = org.slots.len() as u16;
        org.slots.push(Slot {
            cell: cell_id,
            rel_x: (px as i32 - org.origin_x) as i16,
            rel_y: (py as i32 - org.origin_y) as i16,
            mode,
            parent_slot: p.slot,
            bond,
            heading,
        });
        org.n_alive += 1;
        org.body_cost_fp += build;
        self.cells[cell_id as usize] = CellRec {
            px,
            py,
            genotype,
            org: org_id,
            slot,
            mode,
            energy,
            heading,
            child_reps: reps,
            depth: p.depth.saturating_add(1),
            parent_cell: parent,
            brood: brood.is_some(),
            detach_gate: brood.unwrap_or(Gate::always()),
            alive: true,
            ..CellRec::blank()
        };
        claim_rect(pixels, px, py, 1, 1, cell_id + OWNER_BIAS);
        genomes.on_birth(genotype);
        self.live += 1;
        Some(cell_id)
    }

    /// Regrow a dead slot (§b.5): same mode, bond and heading as before.
    pub fn regrow_slot(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        org_id: u32,
        slot_idx: u16,
        energy: i32,
    ) -> Option<u32> {
        let (slot, origin_x, origin_y, parent_cell, genotype) = {
            let org = self.org(org_id)?;
            let slot = *org.slots.get(slot_idx as usize)?;
            if slot.cell != NONE || slot.parent_slot == NONE16 {
                return None;
            }
            let parent_cell = org.slots.get(slot.parent_slot as usize)?.cell;
            if parent_cell == NONE {
                return None;
            }
            let genotype = self.get(parent_cell)?.genotype;
            (slot, org.origin_x, org.origin_y, parent_cell, genotype)
        };
        let px = origin_x + slot.rel_x as i32;
        let py = origin_y + slot.rel_y as i32;
        if px < 0 || py < 0 || px >= pixels.w as i32 || py >= pixels.h as i32 {
            return None;
        }
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if pixels.owner[idx] != EMPTY_OWNER {
            return None;
        }
        let parent_depth = self.get(parent_cell)?.depth;
        let cell_id = self.alloc_cell();
        let reps = Self::first_reps(genomes, genotype, slot.mode);
        let build = genomes.coeffs(genotype, slot.mode).build_fp;
        self.cells[cell_id as usize] = CellRec {
            px: px as u16,
            py: py as u16,
            genotype,
            org: org_id,
            slot: slot_idx,
            mode: slot.mode,
            energy,
            heading: slot.heading,
            child_reps: reps,
            depth: parent_depth.saturating_add(1),
            parent_cell,
            alive: true,
            ..CellRec::blank()
        };
        let org = &mut self.orgs[org_id as usize];
        org.slots[slot_idx as usize].cell = cell_id;
        org.n_alive += 1;
        org.body_cost_fp += build;
        claim_rect(pixels, px as u16, py as u16, 1, 1, cell_id + OWNER_BIAS);
        genomes.on_birth(genotype);
        self.live += 1;
        Some(cell_id)
    }

    /// First dead slot whose parent is `parent` (regrowth candidate).
    pub fn dead_child_slot(&self, parent: u32) -> Option<(u32, u16)> {
        let p = self.get(parent)?;
        let org = self.org(p.org)?;
        org.slots
            .iter()
            .enumerate()
            .find(|(_, s)| s.cell == NONE && s.parent_slot == p.slot)
            .map(|(i, _)| (p.org, i as u16))
    }

    pub fn org_slot_count(&self, org: u32) -> u8 {
        self.org(org)
            .map(|o| o.n_alive.min(255) as u8)
            .unwrap_or(0)
    }

    pub fn org_body_cost_fp(&self, org: u32) -> i32 {
        self.org(org).map(|o| o.body_cost_fp).unwrap_or(0)
    }

    pub fn org_generation(&self, org: u32) -> u32 {
        self.orgs
            .get(org as usize)
            .map(|o| o.generation)
            .unwrap_or(0)
    }

    pub fn glue_attachments(&self) -> u32 {
        self.cells
            .iter()
            .filter(|c| c.alive && c.glue_to != NONE)
            .count() as u32
    }

    /// Kin attached by GLUE to any cell of this organism (distinct organisms).
    pub fn glued_kin(&self, org: u32) -> u32 {
        let mut seen: Vec<u32> = Vec::new();
        for c in self.cells.iter().filter(|c| c.alive && c.org == org) {
            if c.glue_to != NONE {
                if let Some(o) = self.get(c.glue_to) {
                    if o.org != org && !seen.contains(&o.org) {
                        seen.push(o.org);
                    }
                }
            }
        }
        for c in self.cells.iter().filter(|c| c.alive && c.org != org) {
            if c.glue_to != NONE {
                if let Some(o) = self.get(c.glue_to) {
                    if o.org == org && !seen.contains(&c.org) {
                        seen.push(c.org);
                    }
                }
            }
        }
        seen.len() as u32
    }

    /// Kill a cell. Its slot stays (regrowable); a cut-off subtree
    /// re-roots as a new organism (§b.5 fragments). Returns the ids of the
    /// new fragment organisms.
    pub fn kill(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        id: u32,
        to_film: bool,
        tick: u64,
    ) -> Vec<u32> {
        let mut fragments = Vec::new();
        let Some(cell) = self.cells.get(id as usize).copied() else {
            return fragments;
        };
        if !cell.alive {
            return fragments;
        }
        self.cells[id as usize].alive = false;
        self.live -= 1;
        release_rect(pixels, cell.px, cell.py, cell.w, cell.h, id + OWNER_BIAS);
        let idx = (cell.py as usize) * (pixels.w as usize) + cell.px as usize;
        if to_film && idx < pixels.film.len() {
            pixels.film[idx] = pixels.film[idx].saturating_add(180).min(220);
        }
        // Read the build cost before the genotype can fold on extinction.
        let build = genomes.coeffs(cell.genotype, cell.mode).build_fp;
        genomes.on_death(cell.genotype);
        // Break glue bonds pointing at this cell.
        for c in self.cells.iter_mut() {
            if c.alive && c.glue_to == id {
                c.glue_to = NONE;
                c.glue_strength = 0;
            }
        }
        let org_id = cell.org;
        let (child_slots, seed_died) = {
            let Some(o) = self.orgs.get_mut(org_id as usize) else {
                self.free.push(id);
                return fragments;
            };
            if let Some(s) = o.slots.get_mut(cell.slot as usize) {
                s.cell = NONE;
            }
            o.n_alive = o.n_alive.saturating_sub(1);
            o.body_cost_fp = (o.body_cost_fp - build).max(0);
            let child_slots: Vec<u16> = o
                .slots
                .iter()
                .enumerate()
                .filter(|(_, s)| s.parent_slot == cell.slot && s.cell != NONE)
                .map(|(i, _)| i as u16)
                .collect();
            (child_slots, o.seed == id)
        };
        // Fragments re-root downstream of the break. Slots are removed
        // from the old body once, after every subtree has been copied.
        let mut moved: Vec<u16> = Vec::new();
        for cs in child_slots {
            if let Some((new_org, members)) =
                self.split_subtree(genomes, org_id, cs, false, tick)
            {
                fragments.push(new_org);
                moved.extend(members);
            }
        }
        if !moved.is_empty() {
            self.remove_slots(org_id, &moved);
            self.refresh_body_cost(genomes, org_id);
        }
        if seed_died {
            // Everything downstream has re-rooted; the old body is gone.
            self.retire_org(org_id);
        } else if let Some(o) = self.orgs.get_mut(org_id as usize) {
            if o.n_alive == 0 {
                o.alive = false;
                o.slots = Vec::new();
                self.org_free.push(org_id);
            }
        }
        self.free.push(id);
        fragments
    }

    fn retire_org(&mut self, org_id: u32) {
        if let Some(o) = self.orgs.get_mut(org_id as usize) {
            if o.alive {
                o.alive = false;
                o.slots = Vec::new();
                o.n_alive = 0;
                self.org_free.push(org_id);
            }
        }
    }

    /// Detach a brood daughter: her subtree becomes a new organism of the
    /// next generation. Returns the new organism id.
    pub fn detach(&mut self, genomes: &GenomeStore, id: u32, tick: u64) -> Option<u32> {
        let cell = *self.get(id)?;
        if cell.parent_cell == NONE {
            return None;
        }
        let (new_org, members) = self.split_subtree(genomes, cell.org, cell.slot, true, tick)?;
        self.remove_slots(cell.org, &members);
        self.refresh_body_cost(genomes, cell.org);
        Some(new_org)
    }

    /// Move the subtree rooted at `root_slot` out of `org_id` into a new
    /// organism. `next_gen` bumps the generation (reproduction) and clears
    /// the brood flag; fragments keep the generation.
    fn split_subtree(
        &mut self,
        genomes: &GenomeStore,
        org_id: u32,
        root_slot: u16,
        next_gen: bool,
        tick: u64,
    ) -> Option<(u32, Vec<u16>)> {
        let (members, generation, root_cell, ox, oy) = {
            let o = self.orgs.get(org_id as usize)?;
            if !o.alive {
                return None;
            }
            let root = o.slots.get(root_slot as usize)?;
            if root.cell == NONE {
                return None;
            }
            let rc = *self.get(root.cell)?;
            // BFS over slots by parent_slot.
            let mut members: Vec<u16> = vec![root_slot];
            let mut i = 0;
            while i < members.len() {
                let cur = members[i];
                for (j, s) in o.slots.iter().enumerate() {
                    if s.parent_slot == cur && !members.contains(&(j as u16)) {
                        members.push(j as u16);
                    }
                }
                i += 1;
            }
            (members, o.generation, root.cell, rc.px as i32, rc.py as i32)
        };
        let gen = if next_gen {
            generation.saturating_add(1)
        } else {
            generation
        };
        let new_org = self.alloc_org(root_cell, gen, ox, oy, tick);
        // Copy slots into the new organism with remapped parents.
        let mut remap: Vec<(u16, u16)> = Vec::with_capacity(members.len());
        let mut new_slots: Vec<Slot> = Vec::with_capacity(members.len());
        let mut n_alive = 0u16;
        {
            let o = &self.orgs[org_id as usize];
            for (ni, &mi) in members.iter().enumerate() {
                let mut s = o.slots[mi as usize];
                s.rel_x = (o.origin_x + s.rel_x as i32 - ox) as i16;
                s.rel_y = (o.origin_y + s.rel_y as i32 - oy) as i16;
                remap.push((mi, ni as u16));
                if s.cell != NONE {
                    n_alive += 1;
                }
                new_slots.push(s);
            }
        }
        for (ni, s) in new_slots.iter_mut().enumerate() {
            if ni == 0 {
                s.parent_slot = NONE16;
                s.bond = 0;
            } else {
                s.parent_slot = remap
                    .iter()
                    .find(|(old, _)| *old == s.parent_slot)
                    .map(|(_, new)| *new)
                    .unwrap_or(NONE16);
            }
        }
        let mut body_cost = 0i32;
        // Update member cells and depths (BFS order ⇒ parents before children).
        for (ni, s) in new_slots.iter().enumerate() {
            if s.cell == NONE {
                continue;
            }
            let parent_depth = if s.parent_slot == NONE16 {
                None
            } else {
                new_slots
                    .get(s.parent_slot as usize)
                    .and_then(|ps| self.cells.get(ps.cell as usize))
                    .map(|pc| pc.depth)
            };
            let c = &mut self.cells[s.cell as usize];
            c.org = new_org;
            c.slot = ni as u16;
            c.depth = parent_depth.map(|d| d.saturating_add(1)).unwrap_or(0);
            if ni == 0 {
                c.parent_cell = NONE;
                if next_gen {
                    c.brood = false;
                    c.detach_gate = Gate::always();
                }
            }
            body_cost += genomes.coeffs(c.genotype, c.mode).build_fp;
        }
        {
            let n = &mut self.orgs[new_org as usize];
            n.slots = new_slots;
            n.n_alive = n_alive;
            n.body_cost_fp = body_cost;
        }
        Some((new_org, members))
    }

    /// Remove slots from an organism, remapping parent indices and cell slots.
    fn remove_slots(&mut self, org_id: u32, remove: &[u16]) {
        let Some(o) = self.orgs.get_mut(org_id as usize) else {
            return;
        };
        let old_len = o.slots.len();
        let mut new_index: Vec<u16> = vec![NONE16; old_len];
        let mut kept: Vec<Slot> = Vec::with_capacity(old_len);
        let mut n_alive = 0u16;
        for (i, s) in o.slots.iter().enumerate() {
            if remove.contains(&(i as u16)) {
                continue;
            }
            new_index[i] = kept.len() as u16;
            if s.cell != NONE {
                n_alive += 1;
            }
            kept.push(*s);
        }
        for s in kept.iter_mut() {
            if s.parent_slot != NONE16 {
                s.parent_slot = new_index
                    .get(s.parent_slot as usize)
                    .copied()
                    .unwrap_or(NONE16);
            }
        }
        o.slots = kept;
        o.n_alive = n_alive;
        let updates: Vec<(u32, u16)> = o
            .slots
            .iter()
            .enumerate()
            .filter(|(_, s)| s.cell != NONE)
            .map(|(i, s)| (s.cell, i as u16))
            .collect();
        for (cid, si) in updates {
            if let Some(c) = self.cells.get_mut(cid as usize) {
                c.slot = si;
            }
        }
    }

    /// Recompute `body_cost_fp` for an organism from its alive slots.
    pub fn refresh_body_cost(&mut self, genomes: &GenomeStore, org_id: u32) {
        let Some(o) = self.orgs.get(org_id as usize) else {
            return;
        };
        if !o.alive {
            return;
        }
        let mut cost = 0i32;
        for s in &o.slots {
            if s.cell == NONE {
                continue;
            }
            if let Some(c) = self.cells.get(s.cell as usize) {
                cost += genomes.coeffs(c.genotype, c.mode).build_fp;
            }
        }
        self.orgs[org_id as usize].body_cost_fp = cost;
    }

    pub fn move_cell(&mut self, pixels: &mut PixelLayer, id: u32, nx: u16, ny: u16) -> bool {
        let Some(cell) = self.get(id).copied() else {
            return false;
        };
        if !footprint_free(pixels, nx, ny, cell.w, cell.h, id + OWNER_BIAS) {
            return false;
        }
        release_rect(pixels, cell.px, cell.py, cell.w, cell.h, id + OWNER_BIAS);
        claim_rect(pixels, nx, ny, cell.w, cell.h, id + OWNER_BIAS);
        if let Some(c) = self.cells.get_mut(id as usize) {
            c.px = nx;
            c.py = ny;
        }
        if let Some(o) = self.orgs.get_mut(cell.org as usize) {
            if o.slots.len() == 1 {
                o.origin_x = nx as i32;
                o.origin_y = ny as i32;
            }
        }
        true
    }

    /// Translate a whole organism by `(dx, dy)` if every footprint is free
    /// (pixels owned by the organism itself do not block).
    pub fn move_org(&mut self, pixels: &mut PixelLayer, org_id: u32, dx: i32, dy: i32) -> bool {
        let Some(o) = self.org(org_id) else {
            return false;
        };
        let ids: Vec<u32> = o
            .slots
            .iter()
            .filter(|s| s.cell != NONE)
            .map(|s| s.cell)
            .collect();
        let pw = pixels.w as i32;
        let ph = pixels.h as i32;
        for &id in &ids {
            let Some(c) = self.get(id) else {
                return false;
            };
            let nx = c.px as i32 + dx;
            let ny = c.py as i32 + dy;
            if nx < 0 || ny < 0 || nx + c.w as i32 > pw || ny + c.h as i32 > ph {
                return false;
            }
            for yy in 0..c.h as i32 {
                for xx in 0..c.w as i32 {
                    let idx = ((ny + yy) * pw + nx + xx) as usize;
                    let owner = pixels.owner[idx];
                    if owner == EMPTY_OWNER {
                        continue;
                    }
                    if owner < OWNER_BIAS {
                        return false;
                    }
                    match self.cells.get((owner - OWNER_BIAS) as usize) {
                        Some(oc) if oc.alive && oc.org == org_id => {}
                        _ => return false,
                    }
                }
            }
        }
        for &id in &ids {
            let c = self.cells[id as usize];
            release_rect(pixels, c.px, c.py, c.w, c.h, id + OWNER_BIAS);
        }
        for &id in &ids {
            let c = &mut self.cells[id as usize];
            c.px = (c.px as i32 + dx) as u16;
            c.py = (c.py as i32 + dy) as u16;
            let (px, py, w, h) = (c.px, c.py, c.w, c.h);
            claim_rect(pixels, px, py, w, h, id + OWNER_BIAS);
        }
        if let Some(o) = self.orgs.get_mut(org_id as usize) {
            o.origin_x += dx;
            o.origin_y += dy;
        }
        true
    }

    /// Grow one axis step when neighbouring pixels are free.
    pub fn try_grow(&mut self, pixels: &mut PixelLayer, id: u32, tw: u8, th: u8) -> bool {
        let Some(cell) = self.get(id).copied() else {
            return false;
        };
        let (nw, nh) = if cell.w < tw {
            (cell.w + 1, cell.h)
        } else if cell.h < th {
            (cell.w, cell.h + 1)
        } else {
            return false;
        };
        if !footprint_free(pixels, cell.px, cell.py, nw, nh, id + OWNER_BIAS) {
            return false;
        }
        release_rect(pixels, cell.px, cell.py, cell.w, cell.h, id + OWNER_BIAS);
        claim_rect(pixels, cell.px, cell.py, nw, nh, id + OWNER_BIAS);
        if let Some(c) = self.cells.get_mut(id as usize) {
            c.w = nw;
            c.h = nh;
        }
        true
    }

    pub fn seed_algae(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        px: u16,
        py: u16,
        tick: u64,
        world_seed: u64,
    ) -> Option<u32> {
        let g = genomes.seed_algae();
        let seed_mode = genomes.genome(g).seed_mode;
        self.spawn(
            genomes,
            pixels,
            px,
            py,
            g,
            seed_mode,
            0,
            8 * crate::consts::FP,
            2,
            tick,
            world_seed,
        )
    }

    /// Raw access for save/load.
    pub fn raw_cells(&self) -> &[CellRec] {
        &self.cells
    }

    pub fn raw_orgs(&self) -> &[OrgRec] {
        &self.orgs
    }

    pub fn from_raw(cells: Vec<CellRec>, orgs: Vec<OrgRec>) -> Self {
        let live = cells.iter().filter(|c| c.alive).count();
        let free = cells
            .iter()
            .enumerate()
            .filter(|(_, c)| !c.alive)
            .map(|(i, _)| i as u32)
            .collect();
        let org_free = orgs
            .iter()
            .enumerate()
            .filter(|(_, o)| !o.alive)
            .map(|(i, _)| i as u32)
            .collect();
        Self {
            cells,
            free,
            orgs,
            org_free,
            live,
        }
    }
}

impl Default for CellStore {
    fn default() -> Self {
        Self::new()
    }
}

pub fn footprint_free(pixels: &PixelLayer, px: u16, py: u16, w: u8, h: u8, owner: u32) -> bool {
    let pw = pixels.w as i32;
    let ph = pixels.h as i32;
    for dy in 0..h as i32 {
        for dx in 0..w as i32 {
            let x = px as i32 + dx;
            let y = py as i32 + dy;
            if x < 0 || y < 0 || x >= pw || y >= ph {
                return false;
            }
            let idx = (y as usize) * (pixels.w as usize) + x as usize;
            let cur = pixels.owner[idx];
            if cur != EMPTY_OWNER && cur != owner {
                return false;
            }
        }
    }
    true
}

pub fn claim_rect(pixels: &mut PixelLayer, px: u16, py: u16, w: u8, h: u8, owner: u32) {
    for dy in 0..h as u16 {
        for dx in 0..w as u16 {
            let x = px as u32 + dx as u32;
            let y = py as u32 + dy as u32;
            if x >= pixels.w || y >= pixels.h {
                continue;
            }
            let idx = (y as usize) * (pixels.w as usize) + x as usize;
            pixels.owner[idx] = owner;
        }
    }
}

pub fn release_rect(pixels: &mut PixelLayer, px: u16, py: u16, w: u8, h: u8, owner: u32) {
    for dy in 0..h as u16 {
        for dx in 0..w as u16 {
            let x = px as u32 + dx as u32;
            let y = py as u32 + dy as u32;
            if x >= pixels.w || y >= pixels.h {
                continue;
            }
            let idx = (y as usize) * (pixels.w as usize) + x as usize;
            if pixels.owner[idx] == owner {
                pixels.owner[idx] = EMPTY_OWNER;
            }
        }
    }
}
