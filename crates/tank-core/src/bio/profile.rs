//! Blocks, profiles, modes and the compiled per-mode coefficients.
//! Spec: `cell-language.md` §a.1, §a.4, §a.5, §b.4, §c.1.

use serde::{Deserialize, Serialize};
use crate::bio::program::{
    ChildKind, ChildSpec, Detach, Dir, Gate, HeadingRule, Scalar, Transition,
};
use crate::consts::{
    FP, MAX_CHILDREN, MAX_ENTRIES, MAX_TRANSITIONS, P_UNITS, SHAPE_CAP, STORE_BASE_FP,
    STORE_PER_UNIT_FP, STRUCTURE_G_FP, STRUCTURE_UPKEEP_FP_TICK, TICK_HZ,
};
use crate::hash::hash_u64;

/// The 21 blocks (§a.4). UPTAKE kind lives in `param_a`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Block {
    Photo = 1,
    Chemo = 2,
    Uptake = 3,
    Store = 4,
    Enzyme = 5,
    Repair = 6,
    Wall = 7,
    Transparent = 8,
    Glue = 9,
    Toxin = 10,
    Needle = 11,
    Signal = 12,
    Receptor = 13,
    Sensor = 14,
    Nerve = 15,
    Muscle = 16,
    Cilia = 17,
    Buoy = 18,
    Excrete = 19,
    Pigment = 20,
    Thermo = 21,
}

pub const N_BLOCKS: u8 = 21;

/// UPTAKE kinds in `param_a`.
pub const UPTAKE_GLUCOSE: u8 = 0;
pub const UPTAKE_MINERAL: u8 = 1;
pub const UPTAKE_WATER: u8 = 2;

/// SENSOR kinds in `param_a`.
pub const SENSOR_LIGHT: u8 = 0;
pub const SENSOR_TOUCH: u8 = 1;
pub const SENSOR_CHEM: u8 = 2;

impl Block {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            1 => Self::Photo,
            2 => Self::Chemo,
            3 => Self::Uptake,
            4 => Self::Store,
            5 => Self::Enzyme,
            6 => Self::Repair,
            7 => Self::Wall,
            8 => Self::Transparent,
            9 => Self::Glue,
            10 => Self::Toxin,
            11 => Self::Needle,
            12 => Self::Signal,
            13 => Self::Receptor,
            14 => Self::Sensor,
            15 => Self::Nerve,
            16 => Self::Muscle,
            17 => Self::Cilia,
            18 => Self::Buoy,
            19 => Self::Excrete,
            20 => Self::Pigment,
            21 => Self::Thermo,
            _ => return None,
        })
    }

    /// Build cost per unit, milli-g (§a.4).
    pub fn build_mg(self) -> i32 {
        match self {
            Self::Photo => 1000,
            Self::Chemo => 1200,
            Self::Uptake => 800,
            Self::Store => 600,
            Self::Enzyme => 1000,
            Self::Repair => 800,
            Self::Wall => 500,
            Self::Transparent => 300,
            Self::Glue => 500,
            Self::Toxin => 1000,
            Self::Needle => 800,
            Self::Signal => 500,
            Self::Receptor => 500,
            Self::Sensor => 800,
            Self::Nerve => 800,
            Self::Muscle => 1200,
            Self::Cilia => 600,
            Self::Buoy => 400,
            Self::Excrete => 500,
            Self::Pigment => 100,
            Self::Thermo => 800,
        }
    }

    /// Upkeep per unit per second, milli-g (§a.4).
    pub fn upkeep_mg_s(self) -> i32 {
        match self {
            Self::Photo => 20,
            Self::Chemo => 20,
            Self::Uptake => 15,
            Self::Store => 5,
            Self::Enzyme => 30,
            Self::Repair => 10,
            Self::Wall => 10,
            Self::Transparent => 2,
            Self::Glue => 5,
            Self::Toxin => 20,
            Self::Needle => 10,
            Self::Signal => 5,
            Self::Receptor => 5,
            Self::Sensor => 10,
            Self::Nerve => 15,
            Self::Muscle => 10,
            Self::Cilia => 3,
            Self::Buoy => 10,
            Self::Excrete => 5,
            Self::Pigment => 2,
            Self::Thermo => 5,
        }
    }

    /// Palette (§a.4/§a.5). ENZYME colour depends on its key band.
    pub fn colour(self, param_a: u8) -> [u8; 3] {
        match self {
            Self::Photo => [0x3F, 0xA3, 0x4D],
            Self::Chemo => [0xE0, 0x7B, 0x2A],
            Self::Uptake => {
                if param_a == UPTAKE_WATER {
                    [0xC9, 0xA9, 0x6E]
                } else {
                    [0xD9, 0xA4, 0x41]
                }
            }
            Self::Store => [0xB8, 0x86, 0x2B],
            Self::Enzyme => match param_a {
                0..=63 => [0xF2, 0xE8, 0xD0],
                64..=191 => [0x8B, 0x1A, 0x1A],
                _ => [0xC9, 0xC2, 0xB6],
            },
            Self::Repair => [0xF5, 0xDD, 0xE0],
            Self::Wall => [0x7A, 0x52, 0x30],
            Self::Transparent => [0xC0, 0xD8, 0xE0],
            Self::Glue => [0x8C, 0x8A, 0x3C],
            Self::Toxin => [0x7B, 0x3F, 0xA0],
            Self::Needle => [0x4A, 0x2A, 0x60],
            Self::Signal => [0x3B, 0xBF, 0xD3],
            Self::Receptor => [0x2A, 0x8C, 0x8C],
            Self::Sensor => [0x3A, 0x6F, 0xD8],
            Self::Nerve => [0x9A, 0x8F, 0x8F],
            Self::Muscle => [0xE5, 0x7F, 0x96],
            Self::Cilia => [0xF0, 0xA6, 0xC0],
            Self::Buoy => [0xF0, 0xF0, 0xF0],
            Self::Excrete => [0xA8, 0x9A, 0x5B],
            Self::Pigment => hue_rgb(param_a),
            Self::Thermo => [0xE3, 0x6A, 0x50],
        }
    }
}

/// Pigment hue (0..=255 around the wheel) to a saturated RGB.
pub fn hue_rgb(h: u8) -> [u8; 3] {
    let seg = (h as u32 * 6) / 256;
    let f = ((h as u32 * 6) % 256) as u8;
    let q = 255 - f;
    match seg {
        0 => [255, f, 0],
        1 => [q, 255, 0],
        2 => [0, 255, f],
        3 => [0, q, 255],
        4 => [f, 0, 255],
        _ => [255, 0, q],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Entry {
    pub block: Block,
    pub units: u8,
    /// Block-specific: key / hardness / kind / hue.
    pub param_a: u8,
    /// Block-specific: width / strength / amount / secrete flag.
    pub param_b: u8,
    pub gate: Gate,
}

impl Entry {
    pub const fn new(block: Block, units: u8) -> Self {
        Self {
            block,
            units,
            param_a: 0,
            param_b: 0,
            gate: Gate::always(),
        }
    }

    pub const fn with_params(block: Block, units: u8, a: u8, b: u8) -> Self {
        Self {
            block,
            units,
            param_a: a,
            param_b: b,
            gate: Gate::always(),
        }
    }

    const fn blank() -> Self {
        Self::new(Block::Photo, 0)
    }
}

/// `divide_when` thresholds (§b.4): energy_frac ≥ e, age ≥ a·8 ticks, stress ≤ s.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DivideWhen {
    pub energy_frac: u8,
    pub age: u8,
    pub stress: u8,
}

/// Ticks per `divide_when.age` unit.
pub const DIVIDE_AGE_UNIT_TICKS: u16 = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Mode {
    pub shape_w: u8,
    pub shape_h: u8,
    pub entries: [Entry; MAX_ENTRIES],
    pub n_entries: u8,
    pub heading_rule: HeadingRule,
    pub divide_when: DivideWhen,
    pub children: [ChildSpec; MAX_CHILDREN],
    pub n_children: u8,
    pub transitions: [Transition; MAX_TRANSITIONS],
    pub n_transitions: u8,
    pub sacrifice_order: u8,
    pub regrow: bool,
}

impl Mode {
    pub const fn blank() -> Self {
        Self {
            shape_w: 1,
            shape_h: 1,
            entries: [Entry::blank(); MAX_ENTRIES],
            n_entries: 0,
            heading_rule: HeadingRule::Inherit,
            divide_when: DivideWhen {
                energy_frac: 204, // 0.8
                age: 15,          // 120 ticks = 4 s
                stress: 230,
            },
            children: [ChildSpec::empty(); MAX_CHILDREN],
            n_children: 0,
            transitions: [Transition::none(); MAX_TRANSITIONS],
            n_transitions: 0,
            sacrifice_order: 0,
            regrow: true,
        }
    }

    /// d.1 — free-living 1×1 algae, PHOTO 5, propagule ×15.
    pub fn algae() -> Self {
        let mut m = Self::blank();
        m.push_entry(Entry::new(Block::Photo, 5));
        m.push_child(ChildSpec::propagule(0, 15));
        m
    }

    pub fn push_entry(&mut self, e: Entry) -> bool {
        if self.n_entries as usize >= MAX_ENTRIES {
            return false;
        }
        self.entries[self.n_entries as usize] = e;
        self.n_entries += 1;
        true
    }

    pub fn push_child(&mut self, c: ChildSpec) -> bool {
        if self.n_children as usize >= MAX_CHILDREN {
            return false;
        }
        self.children[self.n_children as usize] = c;
        self.n_children += 1;
        true
    }

    pub fn push_transition(&mut self, t: Transition) -> bool {
        if self.n_transitions as usize >= MAX_TRANSITIONS {
            return false;
        }
        self.transitions[self.n_transitions as usize] = t;
        self.n_transitions += 1;
        true
    }

    pub fn remove_entry(&mut self, i: usize) {
        if i >= self.n_entries as usize {
            return;
        }
        for j in i..self.n_entries as usize - 1 {
            self.entries[j] = self.entries[j + 1];
        }
        self.n_entries -= 1;
        self.entries[self.n_entries as usize] = Entry::blank();
    }

    pub fn area(&self) -> u16 {
        self.shape_w.max(1) as u16 * self.shape_h.max(1) as u16
    }

    pub fn capacity(&self) -> u16 {
        P_UNITS as u16 * self.area()
    }

    pub fn used_units(&self) -> u16 {
        (0..self.n_entries as usize)
            .map(|i| self.entries[i].units as u16)
            .sum()
    }

    pub fn units_of(&self, block: Block) -> u8 {
        (0..self.n_entries as usize)
            .filter(|&i| self.entries[i].block == block)
            .map(|i| self.entries[i].units)
            .sum()
    }

    pub fn entry_of(&self, block: Block) -> Option<&Entry> {
        (0..self.n_entries as usize)
            .map(|i| &self.entries[i])
            .find(|e| e.block == block && e.units > 0)
    }

    /// Full build cost at mature shape: 1 g/px structure + units × build.
    pub fn build_fp(&self) -> i32 {
        let mut mg = 0i64;
        for i in 0..self.n_entries as usize {
            let e = self.entries[i];
            mg += e.block.build_mg() as i64 * e.units as i64;
        }
        (mg * FP as i64 / 1000) as i32 + STRUCTURE_G_FP * self.area() as i32
    }

    /// Cost of a 1×1 newborn: 1 g + units proportioned to one pixel.
    pub fn birth_fp(&self) -> i32 {
        let area = self.area() as i32;
        let units_fp = self.build_fp() - STRUCTURE_G_FP * area;
        STRUCTURE_G_FP + units_fp / area
    }

    /// Cost of one growth pixel.
    pub fn grow_pixel_fp(&self) -> i32 {
        self.birth_fp()
    }

    /// Upkeep per tick for a cell of `area` pixels (units are spread over
    /// the whole shape, so upkeep is per mode, plus structure per pixel).
    pub fn upkeep_fp_tick(&self, area: i32) -> i32 {
        let mut mg_s = 0i64;
        for i in 0..self.n_entries as usize {
            let e = self.entries[i];
            // 1-unit overhead per block entry (§a.1) is already in `units`;
            // the effect side subtracts it.
            mg_s += e.block.upkeep_mg_s() as i64 * e.units as i64;
        }
        let per_tick = mg_s * FP as i64 / (1000 * TICK_HZ as i64);
        per_tick as i32 + STRUCTURE_UPKEEP_FP_TICK * area
    }

    /// Store capacity for a cell of `area` pixels.
    pub fn store_cap_fp(&self, area: i32) -> i32 {
        STORE_BASE_FP * area + STORE_PER_UNIT_FP * self.units_of(Block::Store) as i32
    }

    /// Units-weighted colour (§a.5): unused capacity pale amber, PIGMENT
    /// weight `min(0.6, 0.1·units)`, THERMO half weight, brightness
    /// from `metab_eff`. CILIA fringe is drawn separately, not blended.
    pub fn colour(&self, metab_eff: u8) -> [u8; 3] {
        let cap = self.capacity().max(1) as u32;
        let used = (self.used_units() as u32).min(cap);
        let unused = cap - used;
        let mut r = 0u32;
        let mut g = 0u32;
        let mut b = 0u32;
        let mut wsum = 0u32;
        let mut pig: Option<([u8; 3], u32)> = None;
        for i in 0..self.n_entries as usize {
            let e = self.entries[i];
            if e.units == 0 {
                continue;
            }
            let c = e.block.colour(e.param_a);
            let w = match e.block {
                Block::Pigment => {
                    let p = (e.units as u32 * 10).min(60);
                    pig = Some((c, p));
                    continue;
                }
                Block::Cilia => continue,
                Block::Thermo => e.units as u32 * 10 / 2,
                _ => e.units as u32 * 10,
            };
            r += c[0] as u32 * w;
            g += c[1] as u32 * w;
            b += c[2] as u32 * w;
            wsum += w;
        }
        if unused > 0 {
            let w = unused * 10;
            r += 0xE6 * w;
            g += 0xC8 * w;
            b += 0x8A * w;
            wsum += w;
        }
        let mut rgb = if wsum == 0 {
            [0x3F, 0xA3, 0x4D]
        } else {
            [(r / wsum) as u8, (g / wsum) as u8, (b / wsum) as u8]
        };
        if let Some((c, p)) = pig {
            for k in 0..3 {
                rgb[k] = ((rgb[k] as u32 * (100 - p) + c[k] as u32 * p) / 100) as u8;
            }
        }
        let bright = metab_eff as u32;
        [
            ((rgb[0] as u32) * bright / 128).min(255) as u8,
            ((rgb[1] as u32) * bright / 128).min(255) as u8,
            ((rgb[2] as u32) * bright / 128).min(255) as u8,
        ]
    }

    /// Shrink entries until the profile fits the capacity.
    pub fn clamp_to_capacity(&mut self) {
        while self.used_units() > self.capacity() && self.n_entries > 0 {
            let last = self.n_entries as usize - 1;
            if self.entries[last].units > 1 {
                self.entries[last].units -= 1;
            } else {
                self.remove_entry(last);
            }
        }
    }

    /// Re-normalise units to a new capacity after a shape change (§c.2).
    pub fn rescale_units(&mut self, old_cap: u16, new_cap: u16) {
        if old_cap == 0 || old_cap == new_cap {
            self.clamp_to_capacity();
            return;
        }
        for i in 0..self.n_entries as usize {
            let u = self.entries[i].units as u32 * new_cap as u32 / old_cap as u32;
            self.entries[i].units = u.clamp(1, 255) as u8;
        }
        self.clamp_to_capacity();
    }

    /// Drop zero-unit entries.
    pub fn compact(&mut self) {
        let mut i = 0;
        while i < self.n_entries as usize {
            if self.entries[i].units == 0 {
                self.remove_entry(i);
            } else {
                i += 1;
            }
        }
    }

    pub fn is_valid(&self, n_modes: u8) -> bool {
        self.shape_w >= 1
            && self.shape_h >= 1
            && self.shape_w <= SHAPE_CAP
            && self.shape_h <= SHAPE_CAP
            && self.n_entries as usize <= MAX_ENTRIES
            && self.n_children as usize <= MAX_CHILDREN
            && self.n_transitions as usize <= MAX_TRANSITIONS
            && self.used_units() <= self.capacity()
            && (0..self.n_entries as usize).all(|i| self.entries[i].units > 0)
            && (0..self.n_children as usize).all(|i| self.children[i].mode < n_modes)
            && (0..self.n_transitions as usize).all(|i| self.transitions[i].mode < n_modes)
    }

    // ---- mutation helpers (operate on one mode) ----

    pub fn mutate_point(&mut self, seed: u64, salt: u64) {
        let which = hash_u64(seed, salt, 1, 0xB1_01) % 6;
        match which {
            0 | 1 if self.n_entries > 0 => {
                let i = (hash_u64(seed, salt, 2, 0xB1_02) as usize) % self.n_entries as usize;
                let e = &mut self.entries[i];
                match hash_u64(seed, salt, 3, 0xB1_03) % 3 {
                    0 => {
                        let d = (hash_u64(seed, salt, 4, 0xB1_04) % 7) as i32 - 3;
                        e.param_a = (e.param_a as i32 + d).clamp(0, 255) as u8;
                    }
                    1 => {
                        let d = (hash_u64(seed, salt, 5, 0xB1_05) % 17) as i32 - 8;
                        e.param_b = (e.param_b as i32 + d).clamp(0, 255) as u8;
                    }
                    _ => e.gate.mutate(seed, salt),
                }
            }
            2 => {
                let d = (hash_u64(seed, salt, 6, 0xB1_06) % 41) as i32 - 20;
                self.divide_when.energy_frac =
                    (self.divide_when.energy_frac as i32 + d).clamp(32, 255) as u8;
            }
            3 => {
                let d = (hash_u64(seed, salt, 7, 0xB1_07) % 9) as i32 - 4;
                self.divide_when.age = (self.divide_when.age as i32 + d).clamp(2, 255) as u8;
            }
            4 => {
                let d = (hash_u64(seed, salt, 8, 0xB1_08) % 41) as i32 - 20;
                self.divide_when.stress = (self.divide_when.stress as i32 + d).clamp(0, 255) as u8;
            }
            _ => {
                let d = (hash_u64(seed, salt, 9, 0xB1_09) % 5) as i32 - 2;
                self.sacrifice_order = (self.sacrifice_order as i32 + d).clamp(0, 255) as u8;
            }
        }
    }

    /// Move 1–3 units from entry i to entry j (or to unused).
    pub fn shift_units(&mut self, seed: u64, salt: u64) {
        if self.n_entries == 0 {
            return;
        }
        let n = self.n_entries as usize;
        let i = (hash_u64(seed, salt, 1, 0xB1_10) as usize) % n;
        let amount = 1 + (hash_u64(seed, salt, 2, 0xB1_11) % 3) as u8;
        let amount = amount.min(self.entries[i].units);
        if amount == 0 {
            return;
        }
        // j == n means "to unused".
        let j = (hash_u64(seed, salt, 3, 0xB1_12) as usize) % (n + 1);
        self.entries[i].units -= amount;
        if j < n && j != i {
            self.entries[j].units = self.entries[j].units.saturating_add(amount);
        }
        self.compact();
        self.clamp_to_capacity();
    }

    pub fn gain_block(&mut self, seed: u64, salt: u64) {
        if self.n_entries as usize >= MAX_ENTRIES {
            return;
        }
        let kind = 1 + (hash_u64(seed, salt, 3, 0xB1_22) % N_BLOCKS as u64) as u8;
        let Some(block) = Block::from_u8(kind) else {
            return;
        };
        let units = 1 + (hash_u64(seed, salt, 4, 0xB1_23) % 2) as u8;
        let cap = self.capacity();
        let used = self.used_units();
        let mut e = Entry::new(block, units);
        e.param_a = hash_u64(seed, salt, 5, 0xB1_24) as u8;
        e.param_b = (hash_u64(seed, salt, 6, 0xB1_25) % 64) as u8 + 8;
        match block {
            Block::Uptake => e.param_a %= 3,
            Block::Sensor => e.param_a %= 3,
            Block::Wall => e.param_b = 0,
            _ => {}
        }
        if used + units as u16 > cap {
            // Take from the largest entry.
            let mut best = 0usize;
            for i in 1..self.n_entries as usize {
                if self.entries[i].units > self.entries[best].units {
                    best = i;
                }
            }
            if self.n_entries == 0 || self.entries[best].units <= units {
                return;
            }
            self.entries[best].units -= units;
        }
        self.push_entry(e);
        self.clamp_to_capacity();
    }

    pub fn lose_block(&mut self, seed: u64, salt: u64) {
        if self.n_entries <= 1 {
            return;
        }
        let i = (hash_u64(seed, salt, 2, 0xB1_21) as usize) % self.n_entries as usize;
        self.remove_entry(i);
    }

    /// Shape / heading rule / child bond + repeat (§c.2 "re-parameterise").
    pub fn reparameterise(&mut self, seed: u64, salt: u64) {
        match hash_u64(seed, salt, 1, 0xB1_30) % 4 {
            0 => {
                let old_cap = self.capacity();
                let axis = hash_u64(seed, salt, 4, 0xB0_05) % 2;
                let d = if hash_u64(seed, salt, 5, 0xB0_06) % 2 == 0 {
                    1i32
                } else {
                    -1
                };
                if axis == 0 {
                    self.shape_w = (self.shape_w as i32 + d).clamp(1, SHAPE_CAP as i32) as u8;
                } else {
                    self.shape_h = (self.shape_h as i32 + d).clamp(1, SHAPE_CAP as i32) as u8;
                }
                let new_cap = self.capacity();
                self.rescale_units(old_cap, new_cap);
            }
            1 => {
                self.heading_rule = HeadingRule::from_u8(hash_u64(seed, salt, 2, 0xB1_31) as u8);
            }
            _ => {
                if self.n_children > 0 {
                    let i = (hash_u64(seed, salt, 3, 0xB1_33) as usize) % self.n_children as usize;
                    crate::bio::program::reparam_child(&mut self.children[i], seed, salt);
                } else {
                    self.push_child(ChildSpec::propagule(0, 1));
                }
            }
        }
    }

    pub fn is_terminal(&self) -> bool {
        self.n_children == 0
    }
}

/// Compiled per-(genotype, mode) coefficients read in the hot loop.
#[derive(Debug, Clone, Copy)]
pub struct ModeCoeffs {
    pub shape_w: u8,
    pub shape_h: u8,
    pub photo: u8,
    pub chemo: u8,
    pub uptake_glucose: u8,
    pub uptake_water: u8,
    pub store: u8,
    pub enzyme: u8,
    pub enzyme_key: u8,
    pub enzyme_width: u8,
    pub enzyme_secrete: bool,
    pub enzyme_gate: Gate,
    pub repair: u8,
    /// Hardness 0..=255 (h · 255).
    pub wall_h: u8,
    pub wall_units: u8,
    pub transparent: u8,
    pub glue: u8,
    pub toxin: u8,
    pub toxin_key: u8,
    pub toxin_gate: Gate,
    pub needle: u8,
    /// NEEDLE sharpness 0..=255 (punctures walls with `wall_h < needle_s`).
    pub needle_s: u8,
    pub signal: u8,
    pub signal_key: u8,
    pub signal_source: Scalar,
    pub signal_gate: Gate,
    pub receptor: u8,
    pub receptor_key: u8,
    pub sensor: u8,
    pub sensor_kind: u8,
    pub nerve: u8,
    pub muscle: u8,
    pub cilia: u8,
    pub cilia_gate: Gate,
    pub buoy: u8,
    pub excrete: u8,
    pub thermo: u8,
    pub thermo_gate: Gate,
    /// Per-tick upkeep at full shape (structure excluded; added per area).
    pub upkeep_units_fp_tick: i32,
    pub build_fp: i32,
    pub birth_fp: i32,
    pub store_units_fp: i32,
    pub colour: [u8; 3],
    /// Light passthrough per pixel row, Q8.
    pub light_pass_q8: u8,
    pub divide_when: DivideWhen,
    pub n_children: u8,
    pub n_transitions: u8,
    pub sacrifice_order: u8,
    pub regrow: bool,
    pub is_hard: bool,
    /// WALL mineral cost per pixel in field units (§a.4 #7: `2·h` m per unit).
    pub mineral_px: u8,
}

impl ModeCoeffs {
    pub fn compile(mode: &Mode, metab_eff: u8) -> Self {
        let mut c = Self {
            shape_w: mode.shape_w.max(1),
            shape_h: mode.shape_h.max(1),
            photo: 0,
            chemo: 0,
            uptake_glucose: 0,
            uptake_water: 0,
            store: 0,
            enzyme: 0,
            enzyme_key: 0,
            enzyme_width: 8,
            enzyme_secrete: false,
            enzyme_gate: Gate::always(),
            repair: 0,
            wall_h: 0,
            wall_units: 0,
            transparent: 0,
            glue: 0,
            toxin: 0,
            toxin_key: 0,
            toxin_gate: Gate::always(),
            needle: 0,
            needle_s: 0,
            signal: 0,
            signal_key: 0,
            signal_source: Scalar::Always,
            signal_gate: Gate::always(),
            receptor: 0,
            receptor_key: 0,
            sensor: 0,
            sensor_kind: 0,
            nerve: 0,
            muscle: 0,
            cilia: 0,
            cilia_gate: Gate::always(),
            buoy: 0,
            excrete: 0,
            thermo: 0,
            thermo_gate: Gate::always(),
            upkeep_units_fp_tick: 0,
            build_fp: mode.build_fp(),
            birth_fp: mode.birth_fp(),
            store_units_fp: 0,
            colour: mode.colour(metab_eff),
            light_pass_q8: 255,
            divide_when: mode.divide_when,
            n_children: mode.n_children,
            n_transitions: mode.n_transitions,
            sacrifice_order: mode.sacrifice_order,
            regrow: mode.regrow,
            is_hard: false,
            mineral_px: 0,
        };
        let mut mg_s = 0i64;
        for i in 0..mode.n_entries as usize {
            let e = mode.entries[i];
            if e.units == 0 {
                continue;
            }
            mg_s += e.block.upkeep_mg_s() as i64 * e.units as i64;
            // Effect = units − 1 overhead (§a.1), floor 1 for single-unit entries.
            let eff = e.units.max(1);
            match e.block {
                Block::Photo => c.photo = c.photo.saturating_add(eff),
                Block::Chemo => c.chemo = c.chemo.saturating_add(eff),
                Block::Uptake => match e.param_a {
                    UPTAKE_WATER => c.uptake_water = c.uptake_water.saturating_add(eff),
                    _ => c.uptake_glucose = c.uptake_glucose.saturating_add(eff),
                },
                Block::Store => c.store = c.store.saturating_add(e.units),
                Block::Enzyme => {
                    c.enzyme = c.enzyme.saturating_add(eff);
                    c.enzyme_key = e.param_a;
                    c.enzyme_width = (e.param_b & 0x7F).clamp(1, 64);
                    c.enzyme_secrete = e.param_b & 0x80 != 0;
                    c.enzyme_gate = e.gate;
                }
                Block::Repair => c.repair = c.repair.saturating_add(eff),
                Block::Wall => {
                    c.wall_units = c.wall_units.saturating_add(e.units);
                    c.wall_h = c.wall_h.max(e.param_a);
                }
                Block::Transparent => c.transparent = c.transparent.saturating_add(eff),
                Block::Glue => c.glue = c.glue.saturating_add(eff),
                Block::Toxin => {
                    c.toxin = c.toxin.saturating_add(eff);
                    c.toxin_key = e.param_a;
                    c.toxin_gate = e.gate;
                }
                Block::Needle => {
                    c.needle = c.needle.saturating_add(eff);
                    c.needle_s = c.needle_s.max(e.param_a);
                }
                Block::Signal => {
                    c.signal = c.signal.saturating_add(eff);
                    c.signal_key = e.param_a;
                    c.signal_source = Scalar::from_u8(e.param_b);
                    c.signal_gate = e.gate;
                }
                Block::Receptor => {
                    c.receptor = c.receptor.saturating_add(eff);
                    c.receptor_key = e.param_a;
                }
                Block::Sensor => {
                    c.sensor = c.sensor.saturating_add(eff);
                    c.sensor_kind = e.param_a % 3;
                }
                Block::Nerve => c.nerve = c.nerve.saturating_add(eff),
                Block::Muscle => c.muscle = c.muscle.saturating_add(eff),
                Block::Cilia => {
                    c.cilia = c.cilia.saturating_add(eff);
                    c.cilia_gate = e.gate;
                }
                Block::Buoy => c.buoy = c.buoy.saturating_add(eff),
                Block::Excrete => c.excrete = c.excrete.saturating_add(eff),
                Block::Pigment => {}
                Block::Thermo => {
                    c.thermo = c.thermo.saturating_add(eff);
                    c.thermo_gate = e.gate;
                }
            }
        }
        c.upkeep_units_fp_tick = (mg_s * FP as i64 / (1000 * TICK_HZ as i64)) as i32;
        c.store_units_fp = STORE_PER_UNIT_FP * c.store as i32;
        c.is_hard = c.wall_h > 128;
        c.mineral_px = crate::bio::minerals::wall_units_px(c.wall_units, c.wall_h);
        // Light passthrough: PHOTO absorbs 50 %, other tissue 25 %,
        // TRANSPARENT passes 95 %, WALL h cuts passthrough.
        let mut pass: u32 = if c.photo > 0 {
            crate::consts::TISSUE_PASS_PHOTO_Q8
        } else if c.transparent > 0 {
            242
        } else {
            crate::consts::TISSUE_PASS_OTHER_Q8
        };
        pass = pass * (255 - c.wall_h as u32 / 2) / 255;
        c.light_pass_q8 = pass.min(255) as u8;
        c
    }

    pub fn area(&self) -> i32 {
        self.shape_w as i32 * self.shape_h as i32
    }

    pub fn store_cap_fp(&self, area: i32) -> i32 {
        STORE_BASE_FP * area + self.store_units_fp
    }

    pub fn upkeep_fp_tick(&self, area: i32) -> i32 {
        self.upkeep_units_fp_tick + STRUCTURE_UPKEEP_FP_TICK * area
    }
}

/// Convenience for constructing test genomes.
pub fn child_part(dir: Dir, mode: u8, bond: u8, repeat: u8) -> ChildSpec {
    ChildSpec::part(dir, mode, bond, repeat)
}

pub fn child_propagule(mode: u8, repeat: u8) -> ChildSpec {
    ChildSpec::propagule(mode, repeat)
}

pub fn child_brood(dir: Dir, mode: u8, bond: u8, gate: Gate) -> ChildSpec {
    ChildSpec {
        dir,
        mode,
        kind: ChildKind::Grow,
        bond,
        repeat: 1,
        detach: Detach::When(gate),
    }
}
