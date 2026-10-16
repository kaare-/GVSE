//! 2.3.8 — milestone detectors (`evolution-system.md` §12).
//!
//! The biology step emits a raw stream (`ev::DIVISION` on every division,
//! `ev::MOVED` on every swim window …). This layer watches that stream
//! and the model itself and turns it into the player-facing milestones:
//! each fires **once per tank** unless the table says otherwise, and the
//! state survives save/load so a reloaded tank does not announce "first
//! life" again. Detectors read the model (cells, organisms, genotypes),
//! never the rendered pixels.

use std::collections::VecDeque;

use serde::{Deserialize, Serialize};

use crate::bio::{ev, CellStore, GenomeStore};
use crate::tank::SimEvent;

/// Colour distance (Σ |Δchannel|, of 765) a lineage must drift from its
/// founder before "new colour" fires the first time …
pub const NEW_COLOUR_DIST: u32 = 60;
/// … and the further step it must drift before the same lineage fires
/// again.
pub const NEW_COLOUR_STEP: u32 = 30;
/// Minimum ticks between two "new colour" events of one lineage.
pub const NEW_COLOUR_COOLDOWN: u64 = 1800;
/// Slow scans (colour drift, giant cells) run this often.
pub const SCAN_EVERY: u64 = 64;
/// A cell this large (pixel area) is a "giant cell".
pub const GIANT_AREA: i32 = 16;
/// Pigment units that make a tip "pigment-dominant" (weight ≥ 0.4).
pub const CROWN_PIGMENT: u8 = 4;
/// Minimum pigment-dominant tips for a colour crown.
pub const CROWN_TIPS: usize = 3;
/// A glucose-fed lineage this many generations deep is a heterotroph.
pub const HETEROTROPH_GENERATIONS: u32 = 3;
/// "Tank is lifeless" after this many ticks without a living cell.
pub const LIFELESS_TICKS: u64 = 300;
/// Front-pane film coverage check period and threshold (1/`GLASS_DIV`
/// of the pane with film > 80), and the re-arm cooldown while it stays
/// dirty.
pub const GLASS_EVERY: u64 = 300;
pub const GLASS_DIV: usize = 40;
pub const GLASS_COOLDOWN: u64 = 5400;
/// Minimum ticks between two "new species" announcements.
pub const NEW_SPECIES_COOLDOWN: u64 = 1800;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
struct ColourRec {
    root: u32,
    last_tick: u64,
    fired_dist: u32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Milestones {
    /// Once-per-tank milestones already announced (bit = `kind − 32`).
    fired: u32,
    life_seen: bool,
    lifeless_since: Option<u64>,
    lifeless_fired: bool,
    glass_dirty: bool,
    glass_last: u64,
    #[serde(default)]
    species_last: Option<u64>,
    colour: Vec<ColourRec>,
    /// `(genotype, organisms of it that reached ≥ 2 cells)`.
    multicell: Vec<(u32, u16)>,
}

/// What the detectors need from the tank besides the model.
pub struct Probe {
    /// Front-pane pixels with film > 80, and the pane size.
    pub film_n: usize,
    pub film_len: usize,
}

impl Milestones {
    fn bit(kind: u32) -> u32 {
        1 << (kind - ev::MS_FIRST_LIFE)
    }

    pub fn has_fired(&self, kind: u32) -> bool {
        kind >= ev::MS_FIRST_LIFE && self.fired & Self::bit(kind) != 0
    }

    fn fire_once(&mut self, kind: u32, arg0: u32, out: &mut VecDeque<SimEvent>) {
        if self.has_fired(kind) {
            return;
        }
        self.fired |= Self::bit(kind);
        out.push_back(SimEvent { kind, arg0 });
    }

    /// Run after the biology step of `tick`; `raw` is that step's event
    /// stream (already queued for the shell).
    pub fn run(
        &mut self,
        tick: u64,
        raw: &[SimEvent],
        cells: &CellStore,
        genomes: &GenomeStore,
        probe: &Probe,
        out: &mut VecDeque<SimEvent>,
    ) {
        if !cells.is_empty() {
            if !self.life_seen {
                self.life_seen = true;
                let first = cells.iter_alive().map(|(id, _)| id).next().unwrap_or(0);
                self.fire_once(ev::MS_FIRST_LIFE, first, out);
            }
            self.lifeless_since = None;
            self.lifeless_fired = false;
        } else if self.life_seen {
            let since = *self.lifeless_since.get_or_insert(tick);
            if !self.lifeless_fired && tick.saturating_sub(since) >= LIFELESS_TICKS {
                self.lifeless_fired = true;
                out.push_back(SimEvent {
                    kind: ev::LIFELESS,
                    arg0: 0,
                });
            }
        }

        for e in raw {
            match e.kind {
                ev::DIVISION => self.fire_once(ev::MS_FIRST_DIVISION, e.arg0, out),
                ev::DEATH => self.fire_once(ev::MS_FIRST_DEATH, e.arg0, out),
                ev::FUNGUS => self.fire_once(ev::MS_FUNGUS, e.arg0, out),
                ev::PREDATION => self.fire_once(ev::MS_PREDATION, e.arg0, out),
                ev::POISON => self.fire_once(ev::MS_POISON, e.arg0, out),
                ev::SIGNALLING => self.fire_once(ev::MS_SIGNALLING, e.arg0, out),
                ev::MOVED => self.fire_once(ev::MS_MOVED, e.arg0, out),
                ev::WARM_BLOODED => self.fire_once(ev::MS_WARM_BLOODED, e.arg0, out),
                ev::LIFE_ON_LAND => self.fire_once(ev::MS_LIFE_ON_LAND, e.arg0, out),
                ev::CUTTING_TOOK_ROOT => self.fire_once(ev::MS_CUTTING_TOOK_ROOT, e.arg0, out),
                ev::COLOUR_CROWN => self.fire_once(ev::MS_COLOUR_CROWN, e.arg0, out),
                ev::HETEROTROPH => {
                    // A lineage living on glucose this many generations deep.
                    let deep = cells
                        .get(e.arg0)
                        .map(|c| cells.org_generation(c.org) >= HETEROTROPH_GENERATIONS)
                        .unwrap_or(false);
                    if deep {
                        self.fire_once(ev::MS_HETEROTROPH, e.arg0, out);
                    }
                }
                ev::MULTICELLULAR => self.multicellular(e.arg0, cells, out),
                ev::NEW_SPECIES => {
                    // Roots (placed genomes) are not news; later species
                    // are, one announcement per cooldown.
                    let derived = genomes
                        .species(e.arg0 as u16)
                        .map(|s| s.parent != crate::bio::NO_SPECIES)
                        .unwrap_or(false);
                    let due = self.species_last.map(|t| tick.saturating_sub(t) >= NEW_SPECIES_COOLDOWN).unwrap_or(true);
                    if derived && due {
                        self.species_last = Some(tick);
                        out.push_back(SimEvent {
                            kind: ev::MS_NEW_SPECIES,
                            arg0: e.arg0,
                        });
                    }
                }
                _ => {}
            }
        }

        if tick % SCAN_EVERY == 0 && self.life_seen {
            self.scan_colour(tick, genomes, out);
            if !self.has_fired(ev::MS_GIANT) {
                if let Some((id, _)) = cells.iter_alive().find(|(_, c)| c.area() >= GIANT_AREA) {
                    self.fire_once(ev::MS_GIANT, id, out);
                }
            }
            if !self.has_fired(ev::MS_COLOUR_CROWN) {
                if let Some(org) = colour_crown(cells, genomes) {
                    out.push_back(SimEvent {
                        kind: ev::COLOUR_CROWN,
                        arg0: org,
                    });
                    self.fire_once(ev::MS_COLOUR_CROWN, org, out);
                }
            }
        }

        if tick % GLASS_EVERY == 0 && probe.film_len > 0 {
            let dirty = probe.film_n > probe.film_len / GLASS_DIV;
            if dirty && (!self.glass_dirty || tick.saturating_sub(self.glass_last) >= GLASS_COOLDOWN) {
                self.glass_dirty = true;
                self.glass_last = tick;
                out.push_back(SimEvent {
                    kind: ev::GLASS,
                    arg0: probe.film_n as u32,
                });
            } else if !dirty && probe.film_n <= probe.film_len / (2 * GLASS_DIV) {
                self.glass_dirty = false;
            }
        }
    }

    /// "Multicellular" means it breeds true: a second organism of the
    /// same genotype, itself born of another organism, reaches two cells.
    fn multicellular(&mut self, org: u32, cells: &CellStore, out: &mut VecDeque<SimEvent>) {
        if self.has_fired(ev::MS_MULTICELLULAR) {
            return;
        }
        let Some(o) = cells.org(org) else {
            return;
        };
        let Some(seed) = cells.get(o.seed) else {
            return;
        };
        let geno = seed.genotype;
        let count = match self.multicell.iter_mut().find(|(g, _)| *g == geno) {
            Some(rec) => {
                rec.1 = rec.1.saturating_add(1);
                rec.1
            }
            None => {
                self.multicell.push((geno, 1));
                1
            }
        };
        if count >= 2 && o.generation >= 1 {
            self.fire_once(ev::MS_MULTICELLULAR, org, out);
        }
    }

    /// "New colour": a living genotype's seed-mode colour has drifted from
    /// its lineage founder's; per lineage, stepped and rate-limited.
    fn scan_colour(&mut self, tick: u64, genomes: &GenomeStore, out: &mut VecDeque<SimEvent>) {
        let n = genomes.node_count() as u32;
        for id in 0..n {
            let Some(node) = genomes.get(id) else {
                continue;
            };
            if node.live_cells == 0 || node.tombstone {
                continue;
            }
            let root = genomes.root_of(id);
            if root == id {
                continue;
            }
            let dist = colour_distance(genomes, id, root);
            let rec = match self.colour.iter().position(|r| r.root == root) {
                Some(i) => i,
                None => {
                    self.colour.push(ColourRec {
                        root,
                        last_tick: 0,
                        fired_dist: 0,
                    });
                    self.colour.len() - 1
                }
            };
            let r = self.colour[rec];
            let threshold = if r.fired_dist == 0 {
                NEW_COLOUR_DIST
            } else {
                r.fired_dist + NEW_COLOUR_STEP
            };
            if dist >= threshold && (r.fired_dist == 0 || tick.saturating_sub(r.last_tick) >= NEW_COLOUR_COOLDOWN) {
                self.colour[rec] = ColourRec {
                    root,
                    last_tick: tick,
                    fired_dist: dist,
                };
                out.push_back(SimEvent {
                    kind: ev::MS_NEW_COLOUR,
                    arg0: id,
                });
            }
        }
    }
}

fn genotype_colour(genomes: &GenomeStore, id: u32) -> [u8; 3] {
    let g = genomes.genome(id);
    genomes.coeffs(id, g.seed_mode).colour
}

/// Σ |Δchannel| between two genotypes' seed-mode colours.
pub fn colour_distance(genomes: &GenomeStore, a: u32, b: u32) -> u32 {
    let ca = genotype_colour(genomes, a);
    let cb = genotype_colour(genomes, b);
    ca.iter().zip(cb.iter()).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs()).sum()
}

/// A colour crown (§h.4): ≥ `CROWN_TIPS` pigment-dominant tip cells
/// (`depth ≥ 3`, no live child slots) on a body whose other cells are
/// PHOTO-dominant.
fn colour_crown(cells: &CellStore, genomes: &GenomeStore) -> Option<u32> {
    for (org_id, o) in cells.iter_orgs() {
        if o.n_alive < (CROWN_TIPS as u16) + 1 {
            continue;
        }
        let mut tips = 0usize;
        let mut photo = 0usize;
        for (si, s) in o.slots.iter().enumerate() {
            if s.cell == crate::bio::store::NONE {
                continue;
            }
            let Some(c) = cells.get(s.cell) else {
                continue;
            };
            let mc = genomes.coeffs(c.genotype, c.mode);
            let has_child = o.slots.iter().any(|ch| {
                ch.parent_slot == si as u16 && ch.cell != crate::bio::store::NONE
            });
            if c.depth >= 3 && !has_child && mc.pigment >= CROWN_PIGMENT {
                tips += 1;
            } else if mc.photo > mc.pigment {
                photo += 1;
            }
        }
        if tips >= CROWN_TIPS && photo > 0 {
            return Some(org_id);
        }
    }
    None
}
