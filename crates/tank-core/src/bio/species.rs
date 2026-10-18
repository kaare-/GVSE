//! Phase 3 — the species layer over the genotype tree (`cell-language.md`
//! §c.4, K3#1 / K4#3).
//!
//! A **species** is a connected subtree of genotypes that share a
//! *structural signature* (what the body is made of and how it is built,
//! ignoring numeric tuning) and lie within a *parametric distance* `D` of
//! the species founder. A new species starts at the first genotype that
//! breaks either rule. Names are a pure function of the tree: a stem from
//! the seed mode's two dominant blocks, one marker syllable per structural
//! feature the species added over its parent species, and a roman numeral
//! for the species depth.

use serde::{Deserialize, Serialize};

use crate::bio::chem::{LITTER_BONE_CODE, LITTER_SOFT_CODE};
use crate::bio::genome::{Genome, WALL_KEY_MAX, WALL_KEY_MIN};
use crate::bio::profile::{Block, Mode, SENSOR_LIGHT};
use crate::bio::program::{ChildKind, Detach};
use crate::hash::hash_bytes;

/// Parametric distance (Q8: 256 = 1.0) from the species founder beyond
/// which a genotype founds a new species (§c.4 proposal `D = 1.5`).
pub const SPECIES_D_Q8: u32 = 384;
/// Past this many species, a diverging genotype stays in its parent.
/// A long run was founding tens of thousands and the tree stalled.
pub const SPECIES_CAP: usize = 384;
/// Blocks with fewer units than this are tuning, not structure.
pub const STRUCTURAL_UNITS: u8 = 2;

pub const NO_SPECIES: u16 = u16::MAX;

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SpeciesRec {
    /// Genotype that founded the species (may be extinct).
    pub founder: u32,
    /// Founder genome kept for the parametric distance after the node is
    /// pruned.
    pub founder_genome: Genome,
    pub parent: u16,
    /// Species on the path from the root to this one (the numeral).
    pub depth: u16,
    pub sig: u64,
    /// Generated name (`Virdur II`); the archive may show its own name.
    pub name: String,
    /// Born tick of the founder, for the archive's tree.
    pub first_tick: u64,
    /// Set by the archive; archived species are never pruned.
    #[serde(default)]
    pub archived: bool,
}

/// Modes reachable from `seed_mode`, BFS order (children first, then
/// transitions), each mode at most once.
pub fn reachable_modes(g: &Genome) -> Vec<u8> {
    let n = g.modes.len() as u8;
    if n == 0 {
        return Vec::new();
    }
    let mut order = vec![g.seed_mode.min(n - 1)];
    let mut i = 0;
    while i < order.len() {
        let m = &g.modes[order[i] as usize];
        let targets = m.children[..m.n_children as usize]
            .iter()
            .map(|c| c.mode)
            .chain(m.transitions[..m.n_transitions as usize].iter().map(|t| t.mode));
        for t in targets {
            let t = t.min(n - 1);
            if !order.contains(&t) {
                order.push(t);
            }
        }
        i += 1;
    }
    order
}

/// Bitmask of the blocks a mode expresses structurally (`units ≥ 2`).
pub fn structural_blocks(m: &Mode) -> u32 {
    let mut mask = 0u32;
    for e in &m.entries[..m.n_entries as usize] {
        if e.units >= STRUCTURAL_UNITS {
            mask |= 1 << (e.block as u8);
        }
    }
    mask
}

/// Structural signature (§c.4): per reachable mode, its structural block
/// set, shape, children `(dir, kind, target in BFS order, detach kind)`
/// and transition targets.
pub fn signature(g: &Genome) -> u64 {
    let order = reachable_modes(g);
    let index = |mode: u8| -> u8 {
        let n = g.modes.len().max(1) as u8;
        order.iter().position(|&m| m == mode.min(n - 1)).unwrap_or(255) as u8
    };
    let mut bytes = Vec::with_capacity(order.len() * 24);
    for &mi in &order {
        let m = &g.modes[mi as usize];
        bytes.extend_from_slice(&structural_blocks(m).to_le_bytes());
        bytes.push(m.shape_w);
        bytes.push(m.shape_h);
        bytes.push(m.n_children);
        for c in &m.children[..m.n_children as usize] {
            bytes.push(c.dir as u8);
            bytes.push(c.kind as u8);
            bytes.push(index(c.mode));
            bytes.push(match c.detach {
                Detach::Never => 0,
                Detach::Immediate => 1,
                Detach::When(_) => 2,
            });
        }
        bytes.push(m.n_transitions);
        for t in &m.transitions[..m.n_transitions as usize] {
            bytes.push(index(t.mode));
        }
    }
    hash_bytes(&bytes)
}

fn ring(a: u8, b: u8) -> u32 {
    let d = (a as i32 - b as i32).unsigned_abs();
    d.min(256 - d)
}

fn frac(a: u8, b: u8, range: u32) -> u32 {
    (a as i32 - b as i32).unsigned_abs() * 256 / range.max(1)
}

/// Parametric distance (Q8) between two genomes: Σ `|Δ| / range` over the
/// numeric fields, keys and colour-affecting units weighted ×3; modes are
/// aligned by BFS order and an unpaired mode counts 1.0.
pub fn param_distance_q8(a: &Genome, b: &Genome) -> u32 {
    let mut d = 0u32;
    d += frac(a.metab_eff, b.metab_eff, 128);
    d += (a.buoy as i32 - b.buoy as i32).unsigned_abs() * 256 / 255;
    d += frac(a.temp_opt, b.temp_opt, 255);
    d += frac(a.temp_width, b.temp_width, 255);
    d += frac(a.base_age, b.base_age, 255);
    d += frac(a.sacrifice_threshold, b.sacrifice_threshold, 255);
    d += frac(a.cheap, b.cheap, 255) + frac(a.fast, b.fast, 255) + frac(a.accurate, b.accurate, 255);
    let sa = &a.stress;
    let sb = &b.stress;
    for (x, y) in [
        (sa.hunger, sb.hunger),
        (sa.heat, sb.heat),
        (sa.cold, sb.cold),
        (sa.damage, sb.damage),
        (sa.crowding, sb.crowding),
        (sa.waste, sb.waste),
        (sa.toxin, sb.toxin),
        (sa.dark, sb.dark),
    ] {
        d += frac(x, y, 255);
    }
    d += 3 * ring(a.wall_key, b.wall_key) * 256 / 128;
    d += 3 * ring(a.glue_key, b.glue_key) * 256 / 128;
    d += 3 * ring(a.resist_key, b.resist_key) * 256 / 128;
    d += frac(a.glue_width, b.glue_width, 255);
    let oa = reachable_modes(a);
    let ob = reachable_modes(b);
    let n = oa.len().max(ob.len());
    for i in 0..n {
        match (oa.get(i), ob.get(i)) {
            (Some(&ma), Some(&mb)) => d += mode_distance_q8(&a.modes[ma as usize], &b.modes[mb as usize]),
            _ => d += 256,
        }
    }
    d
}

fn mode_distance_q8(a: &Mode, b: &Mode) -> u32 {
    let mut d = 0u32;
    d += frac(a.shape_w, b.shape_w, 6) + frac(a.shape_h, b.shape_h, 6);
    d += frac(a.divide_when.energy_frac, b.divide_when.energy_frac, 255);
    d += frac(a.divide_when.age, b.divide_when.age, 255);
    d += frac(a.divide_when.stress, b.divide_when.stress, 255);
    // Entries aligned by block: the whole unit budget is colour-affecting,
    // PIGMENT (units and hue) counts ×3.
    let mut seen = 0u32;
    for e in a.entries[..a.n_entries as usize].iter().chain(b.entries[..b.n_entries as usize].iter()) {
        let bit = 1 << (e.block as u8);
        if seen & bit != 0 {
            continue;
        }
        seen |= bit;
        let ua = a.units_of(e.block);
        let ub = b.units_of(e.block);
        let w = if e.block == Block::Pigment { 3 } else { 1 };
        d += w * frac(ua, ub, 16);
        let pa = a.entries[..a.n_entries as usize].iter().find(|x| x.block == e.block);
        let pb = b.entries[..b.n_entries as usize].iter().find(|x| x.block == e.block);
        if let (Some(pa), Some(pb)) = (pa, pb) {
            let key_like = matches!(e.block, Block::Enzyme | Block::Glue | Block::Toxin | Block::Receptor | Block::Signal);
            let kw = if key_like { 3 } else { w };
            d += kw * ring(pa.param_a, pb.param_a) * 256 / 128;
            d += frac(pa.param_b, pb.param_b, 255);
            d += frac(pa.gate.threshold, pb.gate.threshold, 255);
        }
    }
    for (ca, cb) in a.children[..a.n_children as usize].iter().zip(b.children[..b.n_children as usize].iter()) {
        d += frac(ca.bond, cb.bond, 255) + frac(ca.repeat, cb.repeat, 255);
    }
    for (ta, tb) in a.transitions[..a.n_transitions as usize].iter().zip(b.transitions[..b.n_transitions as usize].iter()) {
        d += frac(ta.gate.threshold, tb.gate.threshold, 255);
    }
    for (la, lb) in a.links[..a.n_links as usize].iter().zip(b.links[..b.n_links as usize].iter()) {
        d += (la.weight as i32 - lb.weight as i32).unsigned_abs() * 256 / 254;
        d += frac(la.threshold, lb.threshold, 255);
    }
    d
}

// ------------------------------------------------------------------ names

/// Name syllable of a block (ENZYME by substrate band, PIGMENT by hue).
pub fn syllable(block: Block, param_a: u8) -> &'static str {
    match block {
        Block::Photo => "vir",
        Block::Chemo => "sulf",
        Block::Uptake => "sorb",
        Block::Store => "ple",
        Block::Enzyme => {
            if ring(param_a, LITTER_BONE_CODE) <= 24 {
                "oss"
            } else if (WALL_KEY_MIN..=WALL_KEY_MAX).contains(&param_a) {
                "rub"
            } else if ring(param_a, LITTER_SOFT_CODE) <= 24 {
                "sap"
            } else {
                "lys"
            }
        }
        Block::Repair => "san",
        Block::Wall => "dur",
        Block::Transparent => "vit",
        Block::Glue => "col",
        Block::Toxin => "ven",
        Block::Needle => "acu",
        Block::Signal => "voc",
        Block::Receptor => "aud",
        Block::Sensor => {
            if param_a % 3 == SENSOR_LIGHT {
                "ocu"
            } else {
                "tact"
            }
        }
        Block::Nerve => "neur",
        Block::Muscle => "mus",
        Block::Cilia => "cil",
        Block::Buoy => "aer",
        Block::Excrete => "purg",
        Block::Pigment => match (param_a as u32 * 6) / 256 {
            0 | 5 => "ros",
            1 => "flav",
            2 => "alb",
            _ => "caer",
        },
        Block::Thermo => "cal",
    }
}

/// Two dominant blocks of the seed mode by units (ties by block order).
fn dominant_blocks(m: &Mode) -> Vec<(Block, u8)> {
    let mut v: Vec<(Block, u8, u8)> = m.entries[..m.n_entries as usize]
        .iter()
        .filter(|e| e.units > 0)
        .map(|e| (e.block, e.units, e.param_a))
        .collect();
    v.sort_by(|a, b| b.1.cmp(&a.1).then((a.0 as u8).cmp(&(b.0 as u8))));
    v.dedup_by_key(|e| e.0);
    v.into_iter().take(2).map(|(b, _, p)| (b, p)).collect()
}

/// Structural features a genome has: dominant blocks of every reachable
/// mode, secretion kinds, and "big" (a mode larger than 1×1).
fn features(g: &Genome) -> Vec<&'static str> {
    let mut f: Vec<&'static str> = Vec::new();
    for &mi in &reachable_modes(g) {
        let m = &g.modes[mi as usize];
        for e in &m.entries[..m.n_entries as usize] {
            if e.units >= STRUCTURAL_UNITS {
                let s = syllable(e.block, e.param_a);
                if !f.contains(&s) {
                    f.push(s);
                }
            }
        }
        for c in &m.children[..m.n_children as usize] {
            let s = match c.kind {
                ChildKind::Grow => continue,
                ChildKind::SecreteBone => "ost",
                ChildKind::SecreteShell => "test",
            };
            if !f.contains(&s) {
                f.push(s);
            }
        }
        if (m.shape_w > 1 || m.shape_h > 1) && !f.contains(&"magn") {
            f.push("magn");
        }
    }
    f
}

fn roman(mut n: u16) -> String {
    let table = [
        (1000, "M"),
        (900, "CM"),
        (500, "D"),
        (400, "CD"),
        (100, "C"),
        (90, "XC"),
        (50, "L"),
        (40, "XL"),
        (10, "X"),
        (9, "IX"),
        (5, "V"),
        (4, "IV"),
        (1, "I"),
    ];
    let mut s = String::new();
    for (v, r) in table {
        while n >= v {
            s.push_str(r);
            n -= v;
        }
    }
    if s.is_empty() {
        s.push('I');
    }
    s
}

fn capitalise(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// Generated species name (§c.4): `<stem><markers> <numeral>`; the stem
/// is the seed mode's two dominant blocks, the markers are the structural
/// features added over the parent species' genome.
pub fn species_name(g: &Genome, parent: Option<&Genome>, depth: u16) -> String {
    let seed = g.mode(g.seed_mode);
    let mut stem = String::new();
    for (b, p) in dominant_blocks(seed) {
        stem.push_str(syllable(b, p));
    }
    if stem.is_empty() {
        stem.push_str("nul");
    }
    let mut name = stem;
    if let Some(p) = parent {
        let pf = features(p);
        for f in features(g) {
            if !pf.contains(&f) && !name.contains(f) {
                name.push_str(f);
            }
        }
    }
    format!("{} {}", capitalise(&name), roman(depth))
}

/// `"Name II-b"` for the `n`-th (1-based) sibling species sharing a name.
pub fn disambiguate(name: &str, n: u32) -> String {
    if n <= 1 {
        return name.to_string();
    }
    let letter = (b'a' + ((n - 1).min(25) as u8)) as char;
    format!("{name}-{letter}")
}
