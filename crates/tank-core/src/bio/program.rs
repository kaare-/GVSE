//! B2 mode-program primitives: children, heading rules, detach.
//! Spec: `cell-language.md` §b.4.

use crate::hash::hash_u64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

    /// Relative offset in pixel space given a heading (0=east … 7).
    pub fn delta(self, heading: u8) -> (i32, i32) {
        let base = heading % 8;
        let turn = match self {
            Self::Ahead => 0,
            Self::Left => 6,  // −90° in 45° steps
            Self::Right => 2, // +90°
            Self::Behind => 4,
        };
        crate_heading_delta(base.wrapping_add(turn) % 8)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum Detach {
    Never = 0,
    Immediate = 1,
}

impl Detach {
    pub fn from_u8(v: u8) -> Self {
        if v == 0 {
            Self::Never
        } else {
            Self::Immediate
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ChildSpec {
    pub dir: Dir,
    pub mode: u8,
    pub bond: u8,
    pub repeat: u8,
    pub detach: Detach,
}

impl ChildSpec {
    pub fn empty() -> Self {
        Self {
            dir: Dir::Ahead,
            mode: 0,
            bond: 0,
            repeat: 0,
            detach: Detach::Immediate,
        }
    }
}

fn crate_heading_delta(h: u8) -> (i32, i32) {
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

/// Ring-distance match in 0..=128 (0 = perfect).
pub fn ring_distance(a: u8, b: u8) -> u8 {
    let d = (a as i16 - b as i16).unsigned_abs() as u8;
    d.min(255 - d)
}

/// Glue bond strength from key match and declared strength.
pub fn glue_bond(my_key: u8, their_key: u8, width: u8, strength: u8) -> u8 {
    let d = ring_distance(my_key, their_key);
    if d > width {
        return 0;
    }
    let match_q = 255u16.saturating_sub(d as u16 * 255 / width.max(1) as u16);
    ((strength as u16 * match_q) / 255).min(255) as u8
}

pub fn mutate_child(child: &mut ChildSpec, seed: u64, salt: u64) {
    let which = hash_u64(seed, salt, 1, 0xB2_01) % 4;
    match which {
        0 => child.dir = Dir::from_u8(hash_u64(seed, salt, 2, 0xB2_02) as u8),
        1 => child.mode = (hash_u64(seed, salt, 3, 0xB2_03) % 4) as u8,
        2 => {
            let r = child.repeat as i32 + (hash_u64(seed, salt, 4, 0xB2_04) % 3) as i32 - 1;
            child.repeat = r.clamp(0, 15) as u8;
        }
        _ => {
            child.detach = if hash_u64(seed, salt, 5, 0xB2_05) % 2 == 0 {
                Detach::Never
            } else {
                Detach::Immediate
            };
        }
    }
}
