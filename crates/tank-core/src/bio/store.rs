//! Living cells and organisms. B0: every cell is its own organism.
#![allow(clippy::too_many_arguments)]

use crate::bio::genome::{GenomeStore, GenomeV0};
use crate::hash::hash_u64;
use crate::pixels::PixelLayer;

pub const EMPTY_OWNER: u32 = 0;
/// Owner ids for living cells start at 1 (index into CellStore + 1).
pub const OWNER_BIAS: u32 = 1;

#[derive(Debug, Clone, Copy)]
pub struct CellRec {
    pub px: u16,
    pub py: u16,
    /// Current realised size in pixels (grows toward the mode shape).
    pub w: u8,
    pub h: u8,
    pub genotype: u32,
    pub org: u32,
    pub age: u16,
    pub energy: u16,
    /// 0..=255 composite stress (hunger/dark/heat/cold).
    pub stress: u8,
    pub heading: u8,
    /// Index into the active child list (which child entry is next).
    pub child_idx: u8,
    /// Remaining repeats on the current child entry.
    pub child_reps: u8,
    /// Hop count from the organism seed along division bonds.
    pub depth: u8,
    /// Division-bond parent cell id (`u32::MAX` = seed).
    pub parent_cell: u32,
    /// GLUE attachment to another organism's cell (`u32::MAX` = none).
    pub glue_to: u32,
    pub glue_strength: u8,
    /// Stuck as front-pane film.
    pub film: bool,
    pub alive: bool,
}

#[derive(Debug, Clone, Copy)]
pub struct OrgRec {
    pub seed: u32,
    pub generation: u32,
    pub alive: bool,
    /// Realised-map slot count (cell ids in `slots`).
    pub n_slots: u8,
    pub slots: [u32; 16],
}

#[derive(Debug, Clone)]
pub struct CellStore {
    cells: Vec<CellRec>,
    free: Vec<u32>,
    orgs: Vec<OrgRec>,
    org_free: Vec<u32>,
}

impl CellStore {
    pub fn new() -> Self {
        Self {
            cells: Vec::new(),
            free: Vec::new(),
            orgs: Vec::new(),
            org_free: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.cells.iter().filter(|c| c.alive).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: u32) -> Option<&CellRec> {
        self.cells.get(id as usize)
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut CellRec> {
        self.cells.get_mut(id as usize)
    }

    pub fn iter_alive(&self) -> impl Iterator<Item = (u32, &CellRec)> {
        self.cells
            .iter()
            .enumerate()
            .filter(|(_, c)| c.alive)
            .map(|(i, c)| (i as u32, c))
    }

    pub fn spawn(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        px: u16,
        py: u16,
        genotype: u32,
        generation: u32,
        energy: u16,
        heading: u8,
        seed: u64,
    ) -> Option<u32> {
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if idx >= pixels.owner.len() || pixels.owner[idx] != EMPTY_OWNER {
            return None;
        }
        let cell_id = if let Some(id) = self.free.pop() {
            id
        } else {
            let id = self.cells.len() as u32;
            self.cells.push(CellRec {
                px,
                py,
                w: 1,
                h: 1,
                genotype,
                org: 0,
                age: 0,
                energy,
                stress: 0,
                heading,
                child_idx: 0,
                child_reps: 0,
                depth: 0,
                parent_cell: u32::MAX,
                glue_to: u32::MAX,
                glue_strength: 0,
                film: false,
                alive: false,
            });
            id
        };
        let org_id = if let Some(id) = self.org_free.pop() {
            id
        } else {
            let id = self.orgs.len() as u32;
            self.orgs.push(OrgRec {
                seed: cell_id,
                generation,
                alive: false,
                n_slots: 0,
                slots: [u32::MAX; 16],
            });
            id
        };
        self.orgs[org_id as usize] = OrgRec {
            seed: cell_id,
            generation,
            alive: true,
            n_slots: 1,
            slots: {
                let mut s = [u32::MAX; 16];
                s[0] = cell_id;
                s
            },
        };
        let jitter = (hash_u64(seed, cell_id as u64, genotype as u64, 0xC1) % 8) as u8;
        let g = genomes.genome(genotype);
        let first_reps = if g.mode.n_children > 0 {
            g.mode.children[0].repeat
        } else {
            0
        };
        self.cells[cell_id as usize] = CellRec {
            px,
            py,
            w: 1,
            h: 1,
            genotype,
            org: org_id,
            age: 0,
            energy,
            stress: 0,
            heading: heading.wrapping_add(jitter) % 8,
            child_idx: 0,
            child_reps: first_reps,
            depth: 0,
            parent_cell: u32::MAX,
            glue_to: u32::MAX,
            glue_strength: 0,
            film: false,
            alive: true,
        };
        claim_rect(pixels, px, py, 1, 1, cell_id + OWNER_BIAS);
        genomes.on_birth(genotype);
        Some(cell_id)
    }

    /// Spawn a bonded body part into an existing organism.
    pub fn spawn_bonded(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        px: u16,
        py: u16,
        genotype: u32,
        org: u32,
        energy: u16,
        heading: u8,
        parent_cell: u32,
        depth: u8,
        seed: u64,
    ) -> Option<u32> {
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if idx >= pixels.owner.len() || pixels.owner[idx] != EMPTY_OWNER {
            return None;
        }
        let org_rec = self.orgs.get_mut(org as usize)?;
        if !org_rec.alive || org_rec.n_slots as usize >= org_rec.slots.len() {
            return None;
        }
        let cell_id = if let Some(id) = self.free.pop() {
            id
        } else {
            let id = self.cells.len() as u32;
            self.cells.push(CellRec {
                px,
                py,
                w: 1,
                h: 1,
                genotype,
                org: 0,
                age: 0,
                energy,
                stress: 0,
                heading,
                child_idx: 0,
                child_reps: 0,
                depth: 0,
                parent_cell: u32::MAX,
                glue_to: u32::MAX,
                glue_strength: 0,
                film: false,
                alive: false,
            });
            id
        };
        let slot = org_rec.n_slots as usize;
        org_rec.slots[slot] = cell_id;
        org_rec.n_slots += 1;
        let g = genomes.genome(genotype);
        let first_reps = if g.mode.n_children > 0 {
            g.mode.children[0].repeat
        } else {
            0
        };
        self.cells[cell_id as usize] = CellRec {
            px,
            py,
            w: 1,
            h: 1,
            genotype,
            org,
            age: 0,
            energy,
            stress: 0,
            heading,
            child_idx: 0,
            child_reps: first_reps,
            depth,
            parent_cell,
            glue_to: u32::MAX,
            glue_strength: 0,
            film: false,
            alive: true,
        };
        let _ = seed;
        claim_rect(pixels, px, py, 1, 1, cell_id + OWNER_BIAS);
        genomes.on_birth(genotype);
        Some(cell_id)
    }

    pub fn org_slot_count(&self, org: u32) -> u8 {
        self.orgs.get(org as usize).map(|o| o.n_slots).unwrap_or(0)
    }

    pub fn glue_attachments(&self) -> u32 {
        self.cells
            .iter()
            .filter(|c| c.alive && c.glue_to != u32::MAX)
            .count() as u32
    }

    pub fn kill(
        &mut self,
        genomes: &mut GenomeStore,
        pixels: &mut PixelLayer,
        id: u32,
        to_film: bool,
    ) {
        let Some(cell) = self.cells.get_mut(id as usize) else {
            return;
        };
        if !cell.alive {
            return;
        }
        let (px, py, w, h, genotype, org) =
            (cell.px, cell.py, cell.w, cell.h, cell.genotype, cell.org);
        cell.alive = false;
        release_rect(pixels, px, py, w, h, id + OWNER_BIAS);
        let idx = (py as usize) * (pixels.w as usize) + px as usize;
        if to_film && idx < pixels.film.len() {
            pixels.film[idx] = pixels.film[idx].saturating_add(180).min(220);
        }
        genomes.on_death(genotype);
        if let Some(o) = self.orgs.get_mut(org as usize) {
            // Drop this slot from the realised map.
            let mut write = 0u8;
            for i in 0..o.n_slots {
                if o.slots[i as usize] != id {
                    o.slots[write as usize] = o.slots[i as usize];
                    write += 1;
                }
            }
            o.n_slots = write;
            if o.n_slots == 0 {
                o.alive = false;
                self.org_free.push(org);
            } else if o.seed == id {
                o.seed = o.slots[0];
            }
        }
        self.free.push(id);
    }

    pub fn move_cell(&mut self, pixels: &mut PixelLayer, id: u32, nx: u16, ny: u16) -> bool {
        let Some(cell) = self.cells.get(id as usize).copied() else {
            return false;
        };
        if !cell.alive {
            return false;
        }
        if !footprint_free(pixels, nx, ny, cell.w, cell.h, id + OWNER_BIAS) {
            return false;
        }
        release_rect(pixels, cell.px, cell.py, cell.w, cell.h, id + OWNER_BIAS);
        claim_rect(pixels, nx, ny, cell.w, cell.h, id + OWNER_BIAS);
        if let Some(c) = self.cells.get_mut(id as usize) {
            c.px = nx;
            c.py = ny;
        }
        true
    }

    /// Grow one axis step when neighbouring pixels are free.
    pub fn try_grow(&mut self, pixels: &mut PixelLayer, id: u32, tw: u8, th: u8) -> bool {
        let Some(cell) = self.cells.get(id as usize).copied() else {
            return false;
        };
        if !cell.alive {
            return false;
        }
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
        world_seed: u64,
    ) -> Option<u32> {
        let g = genomes.seed_algae();
        self.spawn(genomes, pixels, px, py, g, 0, 8 * 256, 2, world_seed)
    }

    pub fn genome_of(&self, genomes: &GenomeStore, id: u32) -> GenomeV0 {
        self.get(id)
            .map(|c| genomes.genome(c.genotype))
            .unwrap_or_else(GenomeV0::algae)
    }

    pub fn org_generation(&self, org: u32) -> u32 {
        self.orgs
            .get(org as usize)
            .map(|o| o.generation)
            .unwrap_or(0)
    }
}

impl Default for CellStore {
    fn default() -> Self {
        Self::new()
    }
}

fn footprint_free(pixels: &PixelLayer, px: u16, py: u16, w: u8, h: u8, owner: u32) -> bool {
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

fn claim_rect(pixels: &mut PixelLayer, px: u16, py: u16, w: u8, h: u8, owner: u32) {
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

fn release_rect(pixels: &mut PixelLayer, px: u16, py: u16, w: u8, h: u8, owner: u32) {
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
