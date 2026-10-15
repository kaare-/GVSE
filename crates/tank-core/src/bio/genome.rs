//! Interned genomes for B0. One record per genotype; the table is the
//! lineage tree (parent pointer + live/born counts). Leaf folding on
//! extinction compresses extinct lineages into the parent edge.
//! Genomes are content-addressed so identical mutants share a node.

use std::collections::HashMap;

use crate::hash::hash_u64;

/// Compact B0 genome. Later phases widen this into the mode program.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GenomeV0 {
    /// 64..=192; 128 = 1.0 metabolic efficiency.
    pub metab_eff: u8,
    /// PHOTO units, 0..=8. Seed algae uses 5.
    pub photo: u8,
    /// Buoyancy bias. Positive floats toward the light.
    pub buoy: i8,
}

impl GenomeV0 {
    pub fn algae() -> Self {
        Self {
            metab_eff: 128,
            photo: 5,
            buoy: 20,
        }
    }

    pub fn mutate(self, seed: u64, salt: u64) -> Self {
        let h = hash_u64(seed, salt, self.metab_eff as u64, 0xB0_01);
        let mut next = self;
        match h % 3 {
            0 => {
                let d = (hash_u64(seed, salt, 1, 0xB0_02) % 17) as i32 - 8;
                next.metab_eff = (self.metab_eff as i32 + d).clamp(64, 192) as u8;
            }
            1 => {
                let d = (hash_u64(seed, salt, 2, 0xB0_03) % 5) as i32 - 2;
                next.photo = (self.photo as i32 + d).clamp(1, 8) as u8;
            }
            _ => {
                let d = (hash_u64(seed, salt, 3, 0xB0_04) % 21) as i32 - 10;
                next.buoy = (self.buoy as i32 + d).clamp(-100, 100) as i8;
            }
        }
        next
    }
}

#[derive(Debug, Clone)]
pub struct GenotypeNode {
    pub genome: GenomeV0,
    pub parent: u32,
    pub first_child: u32,
    pub next_sibling: u32,
    pub live_cells: u32,
    pub born: u32,
    pub folded_nodes: u32,
    pub tombstone: bool,
}

pub const NONE: u32 = u32::MAX;

#[derive(Debug, Clone)]
pub struct GenomeStore {
    nodes: Vec<GenotypeNode>,
    by_genome: HashMap<GenomeV0, u32>,
}

impl GenomeStore {
    pub fn new() -> Self {
        Self {
            nodes: Vec::new(),
            by_genome: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.nodes.iter().filter(|n| !n.tombstone).count()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    pub fn get(&self, id: u32) -> Option<&GenotypeNode> {
        self.nodes.get(id as usize)
    }

    pub fn genome(&self, id: u32) -> GenomeV0 {
        self.nodes
            .get(id as usize)
            .map(|n| n.genome)
            .unwrap_or_else(GenomeV0::algae)
    }

    /// Intern the seed algae as root genotype 0.
    pub fn seed_algae(&mut self) -> u32 {
        if self.nodes.is_empty() {
            let g = GenomeV0::algae();
            self.nodes.push(GenotypeNode {
                genome: g,
                parent: NONE,
                first_child: NONE,
                next_sibling: NONE,
                live_cells: 0,
                born: 0,
                folded_nodes: 0,
                tombstone: false,
            });
            self.by_genome.insert(g, 0);
        }
        0
    }

    pub fn intern_child(&mut self, parent: u32, genome: GenomeV0) -> u32 {
        if let Some(&id) = self.by_genome.get(&genome) {
            return id;
        }
        let id = self.nodes.len() as u32;
        let next_sib = self
            .nodes
            .get(parent as usize)
            .map(|p| p.first_child)
            .unwrap_or(NONE);
        self.nodes.push(GenotypeNode {
            genome,
            parent,
            first_child: NONE,
            next_sibling: next_sib,
            live_cells: 0,
            born: 0,
            folded_nodes: 0,
            tombstone: false,
        });
        if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.first_child = id;
        }
        self.by_genome.insert(genome, id);
        id
    }

    pub fn on_birth(&mut self, id: u32) {
        if let Some(n) = self.nodes.get_mut(id as usize) {
            n.live_cells = n.live_cells.saturating_add(1);
            n.born = n.born.saturating_add(1);
        }
    }

    pub fn on_death(&mut self, id: u32) {
        let parent = {
            let Some(n) = self.nodes.get_mut(id as usize) else {
                return;
            };
            n.live_cells = n.live_cells.saturating_sub(1);
            if n.live_cells > 0 || n.tombstone {
                return;
            }
            n.parent
        };
        self.fold_leaf(id, parent);
    }

    fn fold_leaf(&mut self, id: u32, parent: u32) {
        if parent == NONE {
            return;
        }
        let (next_sibling, born, folded, genome) = {
            let Some(n) = self.nodes.get(id as usize) else {
                return;
            };
            if n.first_child != NONE {
                return;
            }
            (n.next_sibling, n.born, n.folded_nodes, n.genome)
        };
        if let Some(p) = self.nodes.get_mut(parent as usize) {
            p.folded_nodes = p.folded_nodes.saturating_add(1 + folded);
            p.born = p.born.saturating_add(born);
            if p.first_child == id {
                p.first_child = next_sibling;
            } else {
                let mut c = p.first_child;
                while c != NONE {
                    let next = self.nodes.get(c as usize).map(|n| n.next_sibling);
                    if next == Some(id) {
                        if let Some(node) = self.nodes.get_mut(c as usize) {
                            node.next_sibling = next_sibling;
                        }
                        break;
                    }
                    c = next.unwrap_or(NONE);
                }
            }
        }
        self.by_genome.remove(&genome);
        if let Some(n) = self.nodes.get_mut(id as usize) {
            n.tombstone = true;
            n.live_cells = 0;
        }
    }

    pub fn live_total(&self) -> u32 {
        self.nodes.iter().map(|n| n.live_cells).sum()
    }
}

impl Default for GenomeStore {
    fn default() -> Self {
        Self::new()
    }
}
