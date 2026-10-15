//! Interned genomes for B0. One record per genotype; the table is the
//! lineage tree (parent pointer + live/born counts). Leaf folding on
//! extinction compresses extinct lineages into the parent edge.
//! Genomes are content-addressed so identical mutants share a node.

use std::collections::HashMap;

use crate::bio::profile::Mode;
use crate::hash::hash_u64;

/// Compact B0/B1 genome: baseline + one mode (the mode program grows later).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GenomeV0 {
    /// 64..=192; 128 = 1.0 metabolic efficiency.
    pub metab_eff: u8,
    /// Buoyancy bias. Positive floats toward the light.
    pub buoy: i8,
    /// Preferred temperature (°C).
    pub temp_opt: u8,
    /// Half-width of the efficiency curve (°C).
    pub temp_width: u8,
    /// Pick-two strategy triangle (cheap / fast / accurate), 0..=255 each.
    pub cheap: u8,
    pub fast: u8,
    pub accurate: u8,
    /// Stress weights: hunger, dark, heat, cold (0..=255).
    pub stress_hunger: u8,
    pub stress_dark: u8,
    pub stress_heat: u8,
    pub stress_cold: u8,
    pub mode: Mode,
}

impl GenomeV0 {
    pub fn algae() -> Self {
        Self {
            metab_eff: 128,
            buoy: 20,
            temp_opt: 22,
            temp_width: 8,
            cheap: 100,
            fast: 100,
            accurate: 55,
            stress_hunger: 180,
            stress_dark: 120,
            stress_heat: 160,
            stress_cold: 160,
            mode: Mode::algae(),
        }
    }

    pub fn photo(&self) -> u8 {
        self.mode.units_of(crate::bio::profile::Block::Photo)
    }

    /// Temperature efficiency curve (§a.2): `exp(−((T−opt)/width)²)` in Q8.
    pub fn temp_eff_q8(&self, t_c: f32) -> u8 {
        let w = self.temp_width.max(1) as f32;
        let d = (t_c - self.temp_opt as f32) / w;
        let e = (-(d * d)).exp();
        (e * 255.0).clamp(0.0, 255.0) as u8
    }

    /// Mutation rate multiplier from strategy + stress + Q10 (handoff λ).
    pub fn lambda_milli(&self, stress: u8, t_c: f32) -> u32 {
        let base = 100u32; // λ0 ≈ 0.1 → milli-units
        let strat = 1000
            + 2 * self.cheap as u32
            + self.fast as u32
            - (3 * self.accurate as u32) / 2;
        let stress_m = 1000 + (stress as u32) * 2;
        // Q10 = 2 around 22 °C.
        let q10 = 2f32.powf((t_c - 22.0) / 10.0).clamp(0.25, 4.0);
        let q_m = (q10 * 1000.0) as u32;
        (base * strat / 1000 * stress_m / 1000 * q_m / 1000).clamp(10, 800)
    }

    pub fn mutate(self, seed: u64, salt: u64) -> Self {
        let h = hash_u64(seed, salt, self.metab_eff as u64, 0xB0_01);
        let mut next = self;
        match h % 8 {
            0 => {
                let d = (hash_u64(seed, salt, 1, 0xB0_02) % 17) as i32 - 8;
                next.metab_eff = (self.metab_eff as i32 + d).clamp(64, 192) as u8;
            }
            1 => {
                next.mode.mutate_units(seed, salt);
            }
            2 => {
                let d = (hash_u64(seed, salt, 3, 0xB0_04) % 21) as i32 - 10;
                next.buoy = (self.buoy as i32 + d).clamp(-100, 100) as i8;
            }
            3 => {
                // Shape ±1 inside [1, CAP].
                let axis = hash_u64(seed, salt, 4, 0xB0_05) % 2;
                let d = if hash_u64(seed, salt, 5, 0xB0_06) % 2 == 0 {
                    1i32
                } else {
                    -1
                };
                if axis == 0 {
                    next.mode.shape_w =
                        (self.mode.shape_w as i32 + d).clamp(1, crate::consts::SHAPE_CAP as i32)
                            as u8;
                } else {
                    next.mode.shape_h =
                        (self.mode.shape_h as i32 + d).clamp(1, crate::consts::SHAPE_CAP as i32)
                            as u8;
                }
            }
            4 => {
                next.mode.gain_or_lose_block(seed, salt);
            }
            5 => {
                let d = (hash_u64(seed, salt, 6, 0xB1_10) % 7) as i32 - 3;
                next.temp_opt = (self.temp_opt as i32 + d).clamp(0, 40) as u8;
            }
            6 => {
                // Pick-two: nudge one strategy vertex.
                let which = hash_u64(seed, salt, 7, 0xB1_11) % 3;
                let d = (hash_u64(seed, salt, 8, 0xB1_12) % 33) as i32 - 16;
                match which {
                    0 => next.cheap = (self.cheap as i32 + d).clamp(0, 255) as u8,
                    1 => next.fast = (self.fast as i32 + d).clamp(0, 255) as u8,
                    _ => next.accurate = (self.accurate as i32 + d).clamp(0, 255) as u8,
                }
            }
            _ => {
                let which = hash_u64(seed, salt, 9, 0xB1_13) % 4;
                let d = (hash_u64(seed, salt, 10, 0xB1_14) % 41) as i32 - 20;
                match which {
                    0 => {
                        next.stress_hunger = (self.stress_hunger as i32 + d).clamp(0, 255) as u8
                    }
                    1 => next.stress_dark = (self.stress_dark as i32 + d).clamp(0, 255) as u8,
                    2 => next.stress_heat = (self.stress_heat as i32 + d).clamp(0, 255) as u8,
                    _ => next.stress_cold = (self.stress_cold as i32 + d).clamp(0, 255) as u8,
                }
            }
        }
        next.mode.clamp_to_capacity();
        next
    }

    /// Fuzz-friendly: apply many mutations and assert invariants.
    pub fn is_valid(&self) -> bool {
        self.metab_eff >= 64
            && self.metab_eff <= 192
            && self.temp_opt <= 40
            && self.temp_width >= 1
            && self.mode.shape_w >= 1
            && self.mode.shape_h >= 1
            && self.mode.shape_w <= crate::consts::SHAPE_CAP
            && self.mode.shape_h <= crate::consts::SHAPE_CAP
            && self.mode.n_entries <= 6
            && self.mode.used_units() <= self.mode.capacity()
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
