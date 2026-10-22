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
    /// Prey organism ids currently enclosed in an eater's cavity (Phase 5 M4).
    #[serde(default)]
    swallowed: Vec<u32>,
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
                ev::MOVED => {
                    self.fire_once(ev::MS_MOVED, e.arg0, out);
                    // Muscle-driven swim is locomotion (Phase 5).
                    if !self.has_fired(ev::MS_LOCOMOTION) {
                        if let Some(c) = cells.get(e.arg0) {
                            if org_has_muscle(cells, genomes, c.org) {
                                out.push_back(SimEvent {
                                    kind: ev::LOCOMOTION,
                                    arg0: c.org,
                                });
                                self.fire_once(ev::MS_LOCOMOTION, c.org, out);
                            }
                        }
                    }
                }
                ev::WARM_BLOODED => self.fire_once(ev::MS_WARM_BLOODED, e.arg0, out),
                ev::LIFE_ON_LAND => self.fire_once(ev::MS_LIFE_ON_LAND, e.arg0, out),
                ev::CUTTING_TOOK_ROOT => self.fire_once(ev::MS_CUTTING_TOOK_ROOT, e.arg0, out),
                ev::COLOUR_CROWN => self.fire_once(ev::MS_COLOUR_CROWN, e.arg0, out),
                ev::EYE => self.fire_once(ev::MS_EYE, e.arg0, out),
                ev::LOCOMOTION => self.fire_once(ev::MS_LOCOMOTION, e.arg0, out),
                ev::ESCAPE => self.fire_once(ev::MS_ESCAPE, e.arg0, out),
                ev::CHAMBER => self.fire_once(ev::MS_CHAMBER, e.arg0, out),
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
            if !self.has_fired(ev::MS_EYE) {
                if let Some(org) = geometric_eye(cells, genomes) {
                    out.push_back(SimEvent {
                        kind: ev::EYE,
                        arg0: org,
                    });
                    self.fire_once(ev::MS_EYE, org, out);
                }
            }
            if !self.has_fired(ev::MS_CHAMBER) {
                if let Some(org) = geometric_mouth(cells, genomes) {
                    out.push_back(SimEvent {
                        kind: ev::CHAMBER,
                        arg0: org,
                    });
                    self.fire_once(ev::MS_CHAMBER, org, out);
                }
            }
            self.scan_escape(cells, genomes, out);
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

fn org_has_muscle(cells: &CellStore, genomes: &GenomeStore, org_id: u32) -> bool {
    let Some(o) = cells.org(org_id) else {
        return false;
    };
    o.slots.iter().any(|s| {
        s.cell != crate::bio::store::NONE
            && cells
                .get(s.cell)
                .map(|c| genomes.coeffs(c.genotype, c.mode).muscle > 0)
                .unwrap_or(false)
    })
}

/// Geometric eye (§d.6 / Phase 5 M3): a TRANSPARENT window with ≥2
/// adjacent light-SENSOR cells on a body that also has NERVE and a motor.
fn geometric_eye(cells: &CellStore, genomes: &GenomeStore) -> Option<u32> {
    use crate::bio::profile::SENSOR_LIGHT;
    for (org_id, o) in cells.iter_orgs() {
        if o.n_alive < 3 {
            continue;
        }
        let mut has_nerve = false;
        let mut has_motor = false;
        let mut windows: Vec<(u16, u16, u8, u8)> = Vec::new();
        let mut sensors: Vec<(u16, u16, u8, u8)> = Vec::new();
        for s in o.slots.iter().filter(|s| s.cell != crate::bio::store::NONE) {
            let Some(c) = cells.get(s.cell) else {
                continue;
            };
            let mc = genomes.coeffs(c.genotype, c.mode);
            if mc.nerve > 0 {
                has_nerve = true;
            }
            if mc.muscle > 0 || mc.cilia > 0 {
                has_motor = true;
            }
            if mc.transparent > 0 {
                windows.push((c.px, c.py, c.w, c.h));
            }
            if mc.sensor > 0 && mc.sensor_kind == SENSOR_LIGHT {
                sensors.push((c.px, c.py, c.w, c.h));
            }
        }
        if !has_nerve || !has_motor || windows.is_empty() || sensors.len() < 2 {
            continue;
        }
        for &a in &windows {
            let n = sensors.iter().filter(|&&b| footprints_touch(a, b)).count();
            if n >= 2 {
                return Some(org_id);
            }
        }
    }
    None
}

fn footprints_touch(a: (u16, u16, u8, u8), b: (u16, u16, u8, u8)) -> bool {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    let a1x = ax as i32 + aw as i32;
    let a1y = ay as i32 + ah as i32;
    let b1x = bx as i32 + bw as i32;
    let b1y = by as i32 + bh as i32;
    ax as i32 <= b1x && bx as i32 <= a1x && ay as i32 <= b1y && by as i32 <= a1y
}

/// Geometric mouth (§d.7 / Phase 5 M4): hard-walled body with living-band
/// ENZYME lining and MUSCLE at an opening that encloses open water.
fn geometric_mouth(cells: &CellStore, genomes: &GenomeStore) -> Option<u32> {
    for (org_id, o) in cells.iter_orgs() {
        if o.n_alive < 5 {
            continue;
        }
        let mut hard: Vec<(u16, u16, u8, u8)> = Vec::new();
        let mut has_enzyme = false;
        let mut has_muscle = false;
        for s in o.slots.iter().filter(|s| s.cell != crate::bio::store::NONE) {
            let Some(c) = cells.get(s.cell) else {
                continue;
            };
            let mc = genomes.coeffs(c.genotype, c.mode);
            if mc.is_hard {
                hard.push((c.px, c.py, c.w, c.h));
            }
            if mc.enzyme > 0 && (64..192).contains(&mc.enzyme_key) {
                has_enzyme = true;
            }
            if mc.muscle > 0 {
                has_muscle = true;
            }
        }
        if !has_enzyme || !has_muscle || hard.len() < 4 {
            continue;
        }
        if cavity_open_pixel(&hard).is_some() {
            return Some(org_id);
        }
    }
    None
}

/// An open-water pixel of the body's bbox that touches ≥ 2 hard footprints
/// and is not itself covered by hard tissue — the cavity.
fn cavity_open_pixel(hard: &[(u16, u16, u8, u8)]) -> Option<(i32, i32)> {
    let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
    for &(px, py, w, h) in hard {
        x0 = x0.min(px as i32);
        y0 = y0.min(py as i32);
        x1 = x1.max(px as i32 + w as i32);
        y1 = y1.max(py as i32 + h as i32);
    }
    // Probe the interior of the bbox for a free cell neighboured by hard walls.
    for y in y0..y1 {
        for x in x0..x1 {
            if hard.iter().any(|&(px, py, w, h)| {
                x >= px as i32
                    && x < px as i32 + w as i32
                    && y >= py as i32
                    && y < py as i32 + h as i32
            }) {
                continue;
            }
            let n = hard
                .iter()
                .filter(|&&(px, py, w, h)| {
                    footprints_touch((x as u16, y as u16, 1, 1), (px, py, w, h))
                })
                .count();
            if n >= 2 {
                return Some((x, y));
            }
        }
    }
    None
}

impl Milestones {
    /// Escape detector (§12): prey that was enclosed in an eater's cavity
    /// and is now free (and alive) fires ESCAPE once.
    fn scan_escape(
        &mut self,
        cells: &CellStore,
        genomes: &GenomeStore,
        out: &mut VecDeque<SimEvent>,
    ) {
        let now = swallowed_orgs(cells, genomes);
        let prev = self.swallowed.clone();
        for prey in prev {
            if !now.contains(&prey) {
                // Still alive and no longer enclosed ⇒ escaped.
                if cells.org(prey).map(|o| o.alive && o.n_alive > 0).unwrap_or(false) {
                    out.push_back(SimEvent {
                        kind: ev::ESCAPE,
                        arg0: prey,
                    });
                    self.fire_once(ev::MS_ESCAPE, prey, out);
                }
            }
        }
        self.swallowed = now;
    }
}

type Footprint = (u16, u16, u8, u8);

/// Prey orgs whose seed pixel sits in another org's hard-walled cavity.
fn swallowed_orgs(cells: &CellStore, genomes: &GenomeStore) -> Vec<u32> {
    let mut eaters: Vec<(u32, Vec<Footprint>)> = Vec::new();
    for (org_id, o) in cells.iter_orgs() {
        if o.n_alive < 4 {
            continue;
        }
        let mut hard: Vec<Footprint> = Vec::new();
        let mut enzyme = false;
        for s in o.slots.iter().filter(|s| s.cell != crate::bio::store::NONE) {
            let Some(c) = cells.get(s.cell) else {
                continue;
            };
            let mc = genomes.coeffs(c.genotype, c.mode);
            if mc.is_hard {
                hard.push((c.px, c.py, c.w, c.h));
            }
            if mc.enzyme > 0 && (64..192).contains(&mc.enzyme_key) {
                enzyme = true;
            }
        }
        if enzyme && hard.len() >= 4 && cavity_open_pixel(&hard).is_some() {
            eaters.push((org_id, hard));
        }
    }
    if eaters.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for (org_id, o) in cells.iter_orgs() {
        if !o.alive || o.n_alive == 0 {
            continue;
        }
        let Some(seed) = cells.get(o.seed) else {
            continue;
        };
        let sx = seed.px as i32;
        let sy = seed.py as i32;
        for &(eater, ref hard) in &eaters {
            if eater == org_id {
                continue;
            }
            // Prey seed is inside the eater bbox and not on hard tissue,
            // and touches ≥ 2 hard cells ⇒ in the cavity.
            let (mut x0, mut y0, mut x1, mut y1) = (i32::MAX, i32::MAX, i32::MIN, i32::MIN);
            for &(px, py, w, h) in hard {
                x0 = x0.min(px as i32);
                y0 = y0.min(py as i32);
                x1 = x1.max(px as i32 + w as i32);
                y1 = y1.max(py as i32 + h as i32);
            }
            if sx < x0 || sy < y0 || sx >= x1 || sy >= y1 {
                continue;
            }
            if hard.iter().any(|&(px, py, w, h)| {
                sx >= px as i32
                    && sx < px as i32 + w as i32
                    && sy >= py as i32
                    && sy < py as i32 + h as i32
            }) {
                continue;
            }
            let n = hard
                .iter()
                .filter(|&&(px, py, w, h)| {
                    footprints_touch((sx as u16, sy as u16, 1, 1), (px, py, w, h))
                })
                .count();
            if n >= 2 {
                out.push(org_id);
                break;
            }
        }
    }
    out
}
