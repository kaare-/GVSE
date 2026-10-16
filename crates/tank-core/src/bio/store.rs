//! Living cells and organisms with a realised map (§b.5).
//!
//! An organism is the set of cells connected by division bonds plus the
//! slots they grew into. Slots outlive their cells so a parent can regrow
//! a lost part; a subtree cut off from the seed re-roots as a new
//! organism; a brood daughter detaches when her gate fires.
#![allow(clippy::too_many_arguments)]

use serde::{Deserialize, Serialize};
use crate::bio::genome::GenomeStore;
use crate::bio::program::{ChildKind, Gate};
use crate::consts::MAX_SLOTS;
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;

pub const EMPTY_OWNER: u32 = 0;
/// Owner ids for living cells start at 1 (index into CellStore + 1).
pub const OWNER_BIAS: u32 = 1;
pub const NONE: u32 = u32::MAX;
pub const NONE16: u16 = u16::MAX;
/// Dead secretion pixels (§b.3): owned by an organism through
/// `OrgRec::secretions`, never by a cell. They block placement, drift,
/// litter and the fields like living tissue does.
pub const BONE_OWNER: u32 = u32::MAX - 1;
pub const SHELL_OWNER: u32 = u32::MAX - 2;

/// Which dead secretion an owner value stands for, if any.
#[inline]
pub fn secretion_kind(owner: u32) -> Option<ChildKind> {
    match owner {
        BONE_OWNER => Some(ChildKind::SecreteBone),
        SHELL_OWNER => Some(ChildKind::SecreteShell),
        _ => None,
    }
}

/// Owner value for a secretion kind (`Grow` is not a secretion).
#[inline]
pub fn secretion_owner(kind: ChildKind) -> u32 {
    match kind {
        ChildKind::SecreteShell => SHELL_OWNER,
        _ => BONE_OWNER,
    }
}

/// Mineral units a secretion pixel holds (its bone-litter mass).
#[inline]
pub fn secretion_units(kind: ChildKind) -> u8 {
    match kind {
        ChildKind::SecreteShell => crate::bio::minerals::SHELL_M_UNITS,
        _ => crate::bio::minerals::BONE_M_UNITS,
    }
}

/// One dead bone / shell pixel attached to an organism.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Secretion {
    pub px: u16,
    pub py: u16,
    pub kind: ChildKind,
}

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
    /// Chemistry hits taken since the cell last ran (`chem::HIT_*`).
    pub hit: u8,
    /// Ticks of white division flash left.
    pub flash: u8,
    /// Cilia drive state (running vs tumbling).
    pub running: bool,
    /// Stuck as front-pane film.
    pub film: bool,
    pub alive: bool,
    /// Ticks left as a visible corpse (§a.4 detritus). A corpse is not
    /// alive, keeps its pixels and id, sinks, and is eaten by contact
    /// enzyme; `energy` then holds its soft mass and `corpse_bone` its
    /// wall minerals, both released as litter when it expires.
    #[serde(default)]
    pub corpse: u8,
    #[serde(default)]
    pub corpse_bone: u16,
    /// Running averages (Q8) of each intra-cell link's source scalar, for
    /// `Rise` links (§a.6 run-and-tumble).
    #[serde(default)]
    pub link_avg: [u16; crate::consts::MAX_LINKS],
    /// Net cilia-driven displacement since the last "something moved" check.
    #[serde(default)]
    pub moved_dx: i8,
    #[serde(default)]
    pub moved_dy: i8,
    /// Ticks a THERMO cell has held ≥ +3 °C over ambient (Phase 4).
    #[serde(default)]
    pub warm: u8,
    /// The cell's water column (`cell-language.md` §h.1): 255 submerged,
    /// sourced from moist substrate, conducted along division bonds.
    #[serde(default = "hydration_default")]
    pub hydration: u8,
    /// Last SENSOR reading (Phase 5 M2 net input); 0 without a SENSOR.
    #[serde(default)]
    pub sense: u8,
}

fn hydration_default() -> u8 {
    255
}

/// How long a dead cell stays a visible corpse before it crumbles.
pub const CORPSE_TICKS: u8 = 90;

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
            hit: 0,
            flash: 0,
            running: true,
            film: false,
            alive: false,
            corpse: 0,
            corpse_bone: 0,
            link_avg: [0; crate::consts::MAX_LINKS],
            moved_dx: 0,
            moved_dy: 0,
            warm: 0,
            hydration: 255,
            sense: 0,
        }
    }

    /// A dead cell still occupying its pixels.
    #[inline]
    pub fn is_corpse(&self) -> bool {
        !self.alive && self.corpse > 0
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
    /// Dead bone / shell pixels this body has laid (rigid, no upkeep).
    #[serde(default)]
    pub secretions: Vec<Secretion>,
    /// Daughter organisms released (propagules and broods), for inspect.
    #[serde(default)]
    pub children: u32,
    /// Set when this organism was born as a cut-off fragment (Phase 4);
    /// a depth-0 root into moist substrate fires "cutting took root".
    #[serde(default)]
    pub cut: bool,
}

/// One organism lifted out of the tank by the scoop (Phase 3):
/// its record and `(old id, cell)` pairs with absolute coordinates.
#[derive(Debug, Clone)]
pub struct OrgBundle {
    pub org: OrgRec,
    pub cells: Vec<(u32, CellRec)>,
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

    /// Count a released daughter organism on `org`.
    pub fn note_child(&mut self, org: u32) {
        if let Some(o) = self.orgs.get_mut(org as usize) {
            o.children = o.children.saturating_add(1);
        }
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

    /// A corpse record (dead, still on its pixels).
    pub fn corpse(&self, id: u32) -> Option<&CellRec> {
        self.cells.get(id as usize).filter(|c| c.is_corpse())
    }

    pub fn corpse_ids(&self) -> Vec<u32> {
        self.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.is_corpse())
            .map(|(i, _)| i as u32)
            .collect()
    }

    pub fn corpse_count(&self) -> usize {
        self.cells.iter().filter(|c| c.is_corpse()).count()
    }

    /// Age a corpse by `by` ticks. Returns true when it is due to crumble
    /// (the record stays a corpse until `expire_corpse` is called).
    pub fn corpse_decay(&mut self, id: u32, by: u8) -> bool {
        match self.cells.get_mut(id as usize) {
            Some(c) if c.is_corpse() => {
                if c.corpse <= by {
                    true
                } else {
                    c.corpse -= by;
                    false
                }
            }
            _ => false,
        }
    }

    /// Eat `units` of a corpse's soft mass (glucose units). Returns what
    /// was actually taken.
    pub fn corpse_eat(&mut self, id: u32, units: u32) -> u32 {
        let Some(c) = self.cells.get_mut(id as usize).filter(|c| c.is_corpse()) else {
            return 0;
        };
        let have = (c.energy.max(0) / crate::consts::GLUCOSE_UNIT_FP) as u32;
        let take = units.min(have);
        c.energy -= take as i32 * crate::consts::GLUCOSE_UNIT_FP;
        take
    }

    /// Move a corpse to `(nx, ny)` if the footprint is free.
    pub fn move_corpse(&mut self, pixels: &mut PixelLayer, id: u32, nx: u16, ny: u16) -> bool {
        let Some(c) = self.corpse(id).copied() else {
            return false;
        };
        if !footprint_free(pixels, nx, ny, c.w, c.h, id + OWNER_BIAS) {
            return false;
        }
        release_rect(pixels, c.px, c.py, c.w, c.h, id + OWNER_BIAS);
        claim_rect(pixels, nx, ny, c.w, c.h, id + OWNER_BIAS);
        let c = &mut self.cells[id as usize];
        c.px = nx;
        c.py = ny;
        true
    }

    /// Remove a corpse: free its pixels and recycle the id. Returns the
    /// record so the caller can drop its mass as litter.
    pub fn expire_corpse(&mut self, pixels: &mut PixelLayer, id: u32) -> Option<CellRec> {
        let c = *self.corpse(id)?;
        release_rect(pixels, c.px, c.py, c.w, c.h, id + OWNER_BIAS);
        let rec = &mut self.cells[id as usize];
        rec.corpse = 0;
        rec.corpse_bone = 0;
        self.free.push(id);
        Some(c)
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
            secretions: Vec::new(),
            children: 0,
            cut: false,
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
        // edge-adjacent); a propagule turns ±90° at random so clones
        // spread. A placed seed (generation 0) heads exactly as asked
        // (§b.4 item 1: "heading up").
        let heading = heading & 6;
        let heading = if generation == 0 {
            heading
        } else {
            match (hash_u64(seed, cell_id as u64, genotype as u64, 0xC1) % 3) as u8 {
                0 => heading.wrapping_add(6) % 8,
                1 => heading,
                _ => heading.wrapping_add(2) % 8,
            }
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
    /// re-roots as a new organism (§b.5 fragments). With `corpse =
    /// Some((soft_fp, bone_units))` the body stays on its pixels as a
    /// corpse holding that mass; otherwise it vanishes at once. Returns
    /// the ids of the new fragment organisms.
    pub fn kill(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        id: u32,
        to_film: bool,
        tick: u64,
        corpse: Option<(i32, u16)>,
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
        match corpse {
            Some((soft_fp, bone)) => {
                let c = &mut self.cells[id as usize];
                c.corpse = CORPSE_TICKS;
                c.corpse_bone = bone;
                c.energy = soft_fp.max(0);
                c.glue_to = NONE;
                c.glue_strength = 0;
            }
            None => release_rect(pixels, cell.px, cell.py, cell.w, cell.h, id + OWNER_BIAS),
        }
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
                if corpse.is_none() {
                    self.free.push(id);
                }
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
            self.retire_org(pixels, org_id);
        } else if self.orgs.get(org_id as usize).is_some_and(|o| o.n_alive == 0) {
            self.retire_org(pixels, org_id);
        }
        if corpse.is_none() {
            self.free.push(id);
        }
        fragments
    }

    /// An organism with no living cell is gone; its bone and shell stay
    /// behind as bone litter (§a.4 S1).
    fn retire_org(&mut self, pixels: &mut PixelLayer, org_id: u32) {
        let Some(o) = self.orgs.get_mut(org_id as usize) else {
            return;
        };
        if !o.alive {
            return;
        }
        o.alive = false;
        o.slots = Vec::new();
        o.n_alive = 0;
        let secretions = std::mem::take(&mut o.secretions);
        self.org_free.push(org_id);
        for sec in secretions {
            let idx = (sec.py as usize) * (pixels.w as usize) + sec.px as usize;
            if pixels.owner[idx] == secretion_owner(sec.kind) {
                pixels.owner[idx] = EMPTY_OWNER;
            }
            crate::bio::fields::add_litter(
                pixels,
                idx,
                secretion_units(sec.kind),
                crate::bio::chem::LITTER_BONE_CODE,
            );
        }
    }

    /// Lay a dead bone / shell pixel for `org_id` at a free pixel.
    pub fn add_secretion(
        &mut self,
        pixels: &mut PixelLayer,
        org_id: u32,
        px: u16,
        py: u16,
        kind: ChildKind,
    ) -> bool {
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if idx >= pixels.owner.len() || pixels.owner[idx] != EMPTY_OWNER {
            return false;
        }
        let Some(o) = self.orgs.get_mut(org_id as usize) else {
            return false;
        };
        if !o.alive || o.secretions.len() >= MAX_SLOTS * 2 {
            return false;
        }
        o.secretions.push(Secretion { px, py, kind });
        pixels.owner[idx] = secretion_owner(kind);
        true
    }

    /// Organism owning the secretion pixel at `(px, py)`.
    pub fn secretion_org_at(&self, px: u16, py: u16) -> Option<u32> {
        self.orgs
            .iter()
            .enumerate()
            .filter(|(_, o)| o.alive && !o.secretions.is_empty())
            .find(|(_, o)| o.secretions.iter().any(|s| s.px == px && s.py == py))
            .map(|(i, _)| i as u32)
    }

    /// Detach the secretion pixel at `(px, py)` from its organism and free
    /// the pixel. Returns its kind.
    pub fn remove_secretion_at(&mut self, pixels: &mut PixelLayer, px: u16, py: u16) -> Option<ChildKind> {
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        let kind = secretion_kind(*pixels.owner.get(idx)?)?;
        pixels.owner[idx] = EMPTY_OWNER;
        if let Some(org) = self.secretion_org_at(px, py) {
            let o = &mut self.orgs[org as usize];
            o.secretions.retain(|s| !(s.px == px && s.py == py));
        }
        Some(kind)
    }

    /// Shell cracking (§d.8): shell pixels of *another* organism inside the
    /// rectangle a cell of `org_id` is growing into are removed and fall
    /// as bone litter. Returns true when the rectangle is now free of
    /// foreign shell.
    pub fn crack_shells(
        &mut self,
        pixels: &mut PixelLayer,
        org_id: u32,
        px: u16,
        py: u16,
        w: u8,
        h: u8,
    ) -> bool {
        let pw = pixels.w as usize;
        for dy in 0..h as u16 {
            for dx in 0..w as u16 {
                let (x, y) = (px + dx, py + dy);
                if x as u32 >= pixels.w || y as u32 >= pixels.h {
                    return false;
                }
                let idx = (y as usize) * pw + x as usize;
                if pixels.owner[idx] != SHELL_OWNER {
                    continue;
                }
                if self.secretion_org_at(x, y) == Some(org_id) {
                    return false;
                }
                self.remove_secretion_at(pixels, x, y);
                crate::bio::fields::add_litter(
                    pixels,
                    idx,
                    secretion_units(ChildKind::SecreteShell),
                    crate::bio::chem::LITTER_BONE_CODE,
                );
            }
        }
        true
    }

    /// True when the rectangle is blocked only by another organism's shell.
    pub fn blocked_by_foreign_shell(&self, pixels: &PixelLayer, org_id: u32, px: u16, py: u16, w: u8, h: u8, owner: u32) -> bool {
        let pw = pixels.w as usize;
        let mut any = false;
        for dy in 0..h as u16 {
            for dx in 0..w as u16 {
                let (x, y) = (px + dx, py + dy);
                if x as u32 >= pixels.w || y as u32 >= pixels.h {
                    return false;
                }
                let cur = pixels.owner[(y as usize) * pw + x as usize];
                if cur == EMPTY_OWNER || cur == owner {
                    continue;
                }
                if cur != SHELL_OWNER || self.secretion_org_at(x, y) == Some(org_id) {
                    return false;
                }
                any = true;
            }
        }
        any
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
        // A fragment (not a brood/propagule generation bump) is a cutting.
        if !next_gen {
            if let Some(o) = self.orgs.get_mut(new_org as usize) {
                o.cut = true;
            }
        }
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
        let secretions = o.secretions.clone();
        let pw = pixels.w as i32;
        let ph = pixels.h as i32;
        // A destination pixel is free when empty, or held by this body
        // (a living cell of it or one of its own secretions).
        let own_pixel = |pixels: &PixelLayer, idx: usize, x: i32, y: i32| -> bool {
            let owner = pixels.owner[idx];
            if owner == EMPTY_OWNER {
                return true;
            }
            if secretion_kind(owner).is_some() {
                return secretions.iter().any(|s| s.px as i32 == x && s.py as i32 == y);
            }
            if owner < OWNER_BIAS {
                return false;
            }
            matches!(self.cells.get((owner - OWNER_BIAS) as usize), Some(oc) if oc.alive && oc.org == org_id)
        };
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
                    let (x, y) = (nx + xx, ny + yy);
                    if !own_pixel(pixels, (y * pw + x) as usize, x, y) {
                        return false;
                    }
                }
            }
        }
        for s in &secretions {
            let (x, y) = (s.px as i32 + dx, s.py as i32 + dy);
            if x < 0 || y < 0 || x >= pw || y >= ph || !own_pixel(pixels, (y * pw + x) as usize, x, y) {
                return false;
            }
        }
        for &id in &ids {
            let c = self.cells[id as usize];
            release_rect(pixels, c.px, c.py, c.w, c.h, id + OWNER_BIAS);
        }
        for s in &secretions {
            let idx = (s.py as i32 * pw + s.px as i32) as usize;
            if pixels.owner[idx] == secretion_owner(s.kind) {
                pixels.owner[idx] = EMPTY_OWNER;
            }
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
            for s in o.secretions.iter_mut() {
                s.px = (s.px as i32 + dx) as u16;
                s.py = (s.py as i32 + dy) as u16;
                let idx = (s.py as i32 * pw + s.px as i32) as usize;
                pixels.owner[idx] = secretion_owner(s.kind);
            }
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

    /// Lift a whole organism out of the tank (Phase 3 scoop): its cells
    /// and secretions leave their pixels and the records travel in a
    /// bundle with absolute coordinates. Glue to other organisms breaks
    /// (division bonds travel). Genotype live counts are untouched —
    /// the cells are alive in the bucket; `discard_bundle` settles them.
    pub fn lift_org(&mut self, pixels: &mut PixelLayer, org_id: u32) -> Option<OrgBundle> {
        let org = self.org(org_id)?.clone();
        let ids: Vec<u32> = org.slots.iter().filter(|s| s.cell != NONE).map(|s| s.cell).collect();
        let mut cells = Vec::with_capacity(ids.len());
        for &id in &ids {
            let Some(mut c) = self.cells.get(id as usize).copied().filter(|c| c.alive) else {
                continue;
            };
            release_rect(pixels, c.px, c.py, c.w, c.h, id + OWNER_BIAS);
            c.glue_to = NONE;
            c.glue_strength = 0;
            cells.push((id, c));
            self.cells[id as usize].alive = false;
            self.free.push(id);
            self.live -= 1;
        }
        for c in self.cells.iter_mut() {
            if c.alive && ids.contains(&c.glue_to) {
                c.glue_to = NONE;
                c.glue_strength = 0;
            }
        }
        let pw = pixels.w as usize;
        for s in &org.secretions {
            let idx = s.py as usize * pw + s.px as usize;
            if pixels.owner[idx] == secretion_owner(s.kind) {
                pixels.owner[idx] = EMPTY_OWNER;
            }
        }
        if let Some(o) = self.orgs.get_mut(org_id as usize) {
            o.alive = false;
            o.slots = Vec::new();
            o.secretions = Vec::new();
            o.n_alive = 0;
        }
        self.org_free.push(org_id);
        Some(OrgBundle { org, cells })
    }

    /// Whether `bundle` fits at offset `(dx, dy)`: every cell footprint
    /// and secretion lands in bounds, on an unowned pixel that `open`
    /// accepts.
    pub fn bundle_fits(
        &self,
        pixels: &PixelLayer,
        bundle: &OrgBundle,
        dx: i32,
        dy: i32,
        open: &dyn Fn(i32, i32) -> bool,
    ) -> bool {
        let pw = pixels.w as i32;
        let ph = pixels.h as i32;
        let free = |x: i32, y: i32| -> bool {
            x >= 0
                && y >= 0
                && x < pw
                && y < ph
                && pixels.owner[(y * pw + x) as usize] == EMPTY_OWNER
                && open(x, y)
        };
        for (_, c) in &bundle.cells {
            for yy in 0..c.h as i32 {
                for xx in 0..c.w as i32 {
                    if !free(c.px as i32 + dx + xx, c.py as i32 + dy + yy) {
                        return false;
                    }
                }
            }
        }
        bundle
            .org
            .secretions
            .iter()
            .all(|s| free(s.px as i32 + dx, s.py as i32 + dy))
    }

    /// Put a lifted organism back at offset `(dx, dy)` under fresh ids
    /// (check `bundle_fits` first). Returns the new organism id.
    pub fn place_bundle(&mut self, pixels: &mut PixelLayer, bundle: &OrgBundle, dx: i32, dy: i32) -> u32 {
        let map: Vec<(u32, u32)> = bundle.cells.iter().map(|(old, _)| (*old, self.alloc_cell())).collect();
        let remap = |old: u32| map.iter().find(|(o, _)| *o == old).map(|(_, n)| *n).unwrap_or(NONE);
        let mut org = bundle.org.clone();
        org.alive = true;
        org.seed = remap(org.seed);
        org.origin_x += dx;
        org.origin_y += dy;
        org.n_alive = 0;
        for s in org.slots.iter_mut() {
            if s.cell != NONE {
                s.cell = remap(s.cell);
            }
        }
        let org_id = if let Some(id) = self.org_free.pop() {
            self.orgs[id as usize] = org;
            id
        } else {
            let id = self.orgs.len() as u32;
            self.orgs.push(org);
            id
        };
        let pw = pixels.w as i32;
        for (old, c) in &bundle.cells {
            let new = remap(*old);
            let mut c = *c;
            c.px = (c.px as i32 + dx) as u16;
            c.py = (c.py as i32 + dy) as u16;
            c.org = org_id;
            c.parent_cell = if c.parent_cell == NONE { NONE } else { remap(c.parent_cell) };
            c.alive = true;
            c.corpse = 0;
            claim_rect(pixels, c.px, c.py, c.w, c.h, new + OWNER_BIAS);
            self.cells[new as usize] = c;
            self.live += 1;
            if let Some(o) = self.orgs.get_mut(org_id as usize) {
                o.n_alive += 1;
            }
        }
        if let Some(o) = self.orgs.get_mut(org_id as usize) {
            for s in o.secretions.iter_mut() {
                s.px = (s.px as i32 + dx) as u16;
                s.py = (s.py as i32 + dy) as u16;
                pixels.owner[(s.py as i32 * pw + s.px as i32) as usize] = secretion_owner(s.kind);
            }
        }
        org_id
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
            .filter(|(_, c)| !c.alive && !c.is_corpse())
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
