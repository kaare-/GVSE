//! Mode-program primitives: relative directions, heading rules, gates,
//! children (grow / secrete, detach kinds) and transitions.
//! Spec: `cell-language.md` §b.2, §b.4.

use serde::{Deserialize, Serialize};
use crate::hash::hash_u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Dir {
    Ahead = 0,
    Left = 1,
    Right = 2,
    Behind = 3,
}

impl Dir {
    pub fn from_u8(v: u8) -> Self {
        match v % 4 {
            0 => Self::Ahead,
            1 => Self::Left,
            2 => Self::Right,
            _ => Self::Behind,
        }
    }

    /// Absolute heading (0 = east, counter-clockwise in 45° steps) after
    /// turning from `heading` by this relative direction.
    pub fn heading(self, heading: u8) -> u8 {
        let turn = match self {
            Self::Ahead => 0,
            Self::Left => 6,
            Self::Right => 2,
            Self::Behind => 4,
        };
        (heading % 8).wrapping_add(turn) % 8
    }

    /// Relative pixel offset given a heading.
    pub fn delta(self, heading: u8) -> (i32, i32) {
        heading_delta(self.heading(heading))
    }
}

/// Unit step for an absolute heading; pixel y grows downward.
pub fn heading_delta(h: u8) -> (i32, i32) {
    match h % 8 {
        0 => (1, 0),
        1 => (1, -1),
        2 => (0, -1),
        3 => (-1, -1),
        4 => (-1, 0),
        5 => (-1, 1),
        6 => (0, 1),
        _ => (1, 1),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum HeadingRule {
    Inherit = 0,
    TurnLeft = 1,
    TurnRight = 2,
    Flip = 3,
}

impl HeadingRule {
    pub fn from_u8(v: u8) -> Self {
        match v % 4 {
            0 => Self::Inherit,
            1 => Self::TurnLeft,
            2 => Self::TurnRight,
            _ => Self::Flip,
        }
    }

    pub fn apply(self, parent_heading: u8) -> u8 {
        match self {
            Self::Inherit => parent_heading % 8,
            Self::TurnLeft => parent_heading.wrapping_add(6) % 8,
            Self::TurnRight => parent_heading.wrapping_add(2) % 8,
            Self::Flip => parent_heading.wrapping_add(4) % 8,
        }
    }
}

/// Cell scalars a gate can read (§b.2). `Always` passes unconditionally.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum Scalar {
    Always = 0,
    EnergyFrac = 1,
    AgeFrac = 2,
    Stress = 3,
    Damage = 4,
    Light = 5,
    Touch = 6,
    Depth = 7,
    Sig = 8,
    Kin = 9,
    Waste = 10,
    TempDev = 11,
    Sense = 12,
}

pub const N_SCALARS: u8 = 13;

impl Scalar {
    pub fn from_u8(v: u8) -> Self {
        match v % N_SCALARS {
            0 => Self::Always,
            1 => Self::EnergyFrac,
            2 => Self::AgeFrac,
            3 => Self::Stress,
            4 => Self::Damage,
            5 => Self::Light,
            6 => Self::Touch,
            7 => Self::Depth,
            8 => Self::Sig,
            9 => Self::Kin,
            10 => Self::Waste,
            11 => Self::TempDev,
            _ => Self::Sense,
        }
    }
}

/// Per-tick scalar set of one cell, all 0..=255 except `touch` (0..=4)
/// and `depth` (hops).
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize)]
pub struct Scalars {
    pub energy_frac: u8,
    pub age_frac: u8,
    pub stress: u8,
    pub damage: u8,
    pub light: u8,
    pub touch: u8,
    pub depth: u8,
    pub sig: u8,
    pub kin: u8,
    pub waste: u8,
    pub temp_dev: u8,
    pub sense: u8,
}

impl Scalars {
    pub fn read(&self, s: Scalar) -> u8 {
        match s {
            Scalar::Always => 255,
            Scalar::EnergyFrac => self.energy_frac,
            Scalar::AgeFrac => self.age_frac,
            Scalar::Stress => self.stress,
            Scalar::Damage => self.damage,
            Scalar::Light => self.light,
            Scalar::Touch => self.touch,
            Scalar::Depth => self.depth,
            Scalar::Sig => self.sig,
            Scalar::Kin => self.kin,
            Scalar::Waste => self.waste,
            Scalar::TempDev => self.temp_dev,
            Scalar::Sense => self.sense,
        }
    }
}

/// `(scalar, threshold, above|below)`; `Always` passes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Gate {
    pub scalar: Scalar,
    pub above: bool,
    pub threshold: u8,
}

impl Gate {
    pub const fn always() -> Self {
        Self {
            scalar: Scalar::Always,
            above: true,
            threshold: 0,
        }
    }

    pub const fn above(scalar: Scalar, threshold: u8) -> Self {
        Self {
            scalar,
            above: true,
            threshold,
        }
    }

    pub const fn below(scalar: Scalar, threshold: u8) -> Self {
        Self {
            scalar,
            above: false,
            threshold,
        }
    }

    pub fn passes(&self, s: &Scalars) -> bool {
        if self.scalar == Scalar::Always {
            return true;
        }
        let v = s.read(self.scalar);
        if self.above {
            v >= self.threshold
        } else {
            v < self.threshold
        }
    }

    pub fn mutate(&mut self, seed: u64, salt: u64) {
        match hash_u64(seed, salt, 1, 0xB2_40) % 3 {
            0 => self.scalar = Scalar::from_u8(hash_u64(seed, salt, 2, 0xB2_41) as u8),
            1 => self.above = !self.above,
            _ => {
                let d = (hash_u64(seed, salt, 3, 0xB2_42) % 65) as i32 - 32;
                self.threshold = (self.threshold as i32 + d).clamp(0, 255) as u8;
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum Detach {
    /// Body part: stays bonded for life.
    Never,
    /// New organism at birth (propagule).
    Immediate,
    /// Brood / egg: bonded and fed until the daughter's gate fires.
    When(Gate),
}

impl Detach {
    pub fn from_u8(v: u8) -> Self {
        match v % 3 {
            0 => Self::Never,
            1 => Self::Immediate,
            _ => Self::When(Gate::above(Scalar::AgeFrac, 77)),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[repr(u8)]
pub enum ChildKind {
    Grow = 0,
    SecreteBone = 1,
    SecreteShell = 2,
}

impl ChildKind {
    pub fn from_u8(v: u8) -> Self {
        match v % 3 {
            0 => Self::Grow,
            1 => Self::SecreteBone,
            _ => Self::SecreteShell,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ChildSpec {
    pub dir: Dir,
    pub mode: u8,
    pub kind: ChildKind,
    pub bond: u8,
    pub repeat: u8,
    pub detach: Detach,
}

impl ChildSpec {
    pub const fn empty() -> Self {
        Self {
            dir: Dir::Ahead,
            mode: 0,
            kind: ChildKind::Grow,
            bond: 0,
            repeat: 0,
            detach: Detach::Immediate,
        }
    }

    pub const fn propagule(mode: u8, repeat: u8) -> Self {
        Self {
            dir: Dir::Ahead,
            mode,
            kind: ChildKind::Grow,
            bond: 0,
            repeat,
            detach: Detach::Immediate,
        }
    }

    pub const fn part(dir: Dir, mode: u8, bond: u8, repeat: u8) -> Self {
        Self {
            dir,
            mode,
            kind: ChildKind::Grow,
            bond,
            repeat,
            detach: Detach::Never,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Transition {
    pub gate: Gate,
    pub mode: u8,
}

impl Transition {
    pub const fn none() -> Self {
        Self {
            gate: Gate::always(),
            mode: 0,
        }
    }
}

/// Ring distance in 0..=128.
pub fn ring_distance(a: u8, b: u8) -> u8 {
    let d = (a as i16 - b as i16).unsigned_abs();
    d.min(256 - d) as u8
}

/// `match(a, b) = max(0, 1 − d/w)` in Q8 (255 = perfect).
pub fn ring_match_q8(a: u8, b: u8, width: u8) -> u8 {
    let d = ring_distance(a, b) as u32;
    let w = width.clamp(1, 64) as u32;
    if d >= w {
        0
    } else {
        (255 - d * 255 / w) as u8
    }
}

/// Keyed potency per unit: `match / sqrt(w/8)` in Q8 (w = 8 ⇒ ×1).
pub fn keyed_potency_q8(a: u8, b: u8, width: u8) -> u8 {
    let m = ring_match_q8(a, b, width) as u32;
    let w = width.clamp(1, 64) as u32;
    // sqrt(w/8) via integer sqrt of (w·32)/8 scaled: sqrt(w/8)·16 = sqrt(w·32).
    let s16 = isqrt(w * 32).max(1);
    (m * 16 / s16).min(255) as u8
}

fn isqrt(v: u32) -> u32 {
    let mut r = (v as f32).sqrt() as u32;
    while r * r > v {
        r -= 1;
    }
    while (r + 1) * (r + 1) <= v {
        r += 1;
    }
    r
}

/// Glue bond strength from key match and declared strength.
pub fn glue_bond(my_key: u8, their_key: u8, width: u8, strength: u8) -> u8 {
    let m = ring_match_q8(my_key, their_key, width) as u32;
    (strength as u32 * m / 255).min(255) as u8
}

/// Re-wire child: dir / mode / detach (§c.2 "re-wire child").
pub fn rewire_child(child: &mut ChildSpec, n_modes: u8, seed: u64, salt: u64) {
    match hash_u64(seed, salt, 1, 0xB2_01) % 3 {
        0 => child.dir = Dir::from_u8(hash_u64(seed, salt, 2, 0xB2_02) as u8),
        1 => child.mode = (hash_u64(seed, salt, 3, 0xB2_03) % n_modes.max(1) as u64) as u8,
        _ => {
            child.detach = match hash_u64(seed, salt, 5, 0xB2_05) % 4 {
                0 | 1 => Detach::Never,
                2 => Detach::Immediate,
                _ => {
                    let mut g = Gate::above(Scalar::AgeFrac, 77);
                    g.mutate(seed, salt ^ 0x77);
                    if g.scalar == Scalar::Always {
                        g.scalar = Scalar::AgeFrac;
                    }
                    Detach::When(g)
                }
            };
        }
    }
}

/// Re-parameterise child: bond / repeat.
pub fn reparam_child(child: &mut ChildSpec, seed: u64, salt: u64) {
    if hash_u64(seed, salt, 1, 0xB2_11) % 2 == 0 {
        let d = (hash_u64(seed, salt, 2, 0xB2_12) % 81) as i32 - 40;
        child.bond = (child.bond as i32 + d).clamp(0, 255) as u8;
    } else {
        let d = (hash_u64(seed, salt, 4, 0xB2_04) % 5) as i32 - 2;
        child.repeat = (child.repeat as i32 + d).clamp(0, 15) as u8;
    }
}

/// Re-wire transition: gate or target.
pub fn rewire_transition(t: &mut Transition, n_modes: u8, seed: u64, salt: u64) {
    if hash_u64(seed, salt, 1, 0xB2_21) % 2 == 0 {
        t.gate.mutate(seed, salt);
    } else {
        t.mode = (hash_u64(seed, salt, 2, 0xB2_22) % n_modes.max(1) as u64) as u8;
    }
}
