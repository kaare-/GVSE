//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Per-cell data: material tag + water saturation + a few flag bits.
//! Deliberately packed to 4 bytes so a 64×64 chunk fits in 16 KiB and
//! stays cache-friendly for the future 4-pass checkerboard update
//! (Noita).

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

/// Water saturation on the closed interval [0, 1], encoded as u8.
///
/// This one scalar covers everything the column model used to split
/// across `Layer[Water]` mass, `Column::moisture`, and
/// `Void::water_mass`. Cell material determines the *capacity* — an
/// `Air` cell holds a full unit of free water, a porous solid holds
/// `porosity` × cell volume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct Sat(pub u8);

impl Sat {
    pub const EMPTY: Sat = Sat(0);
    pub const FULL: Sat = Sat(u8::MAX);

    pub fn from_f32(v: f32) -> Self {
        Self((v.clamp(0.0, 1.0) * 255.0).round() as u8)
    }

    pub fn as_f32(self) -> f32 {
        self.0 as f32 / 255.0
    }

    pub fn is_empty(self) -> bool {
        self.0 == 0
    }

    pub fn is_full(self) -> bool {
        self.0 == u8::MAX
    }
}

/// Reserved per-cell flags. Kept as an opaque `u8` bitfield today so
/// future rules (frozen, sediment-carrying, momentum-carrying) can
/// slot in without changing `Cell`'s memory shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CellFlags(pub u8);

impl CellFlags {
    /// Reserved for the eventual "cell is currently ticking" bit used
    /// by rule ordering.
    pub const ACTIVE_HINT: CellFlags = CellFlags(0b0000_0001);

    pub const fn empty() -> Self {
        Self(0)
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub fn set(&mut self, other: Self) {
        self.0 |= other.0;
    }

    pub fn clear(&mut self, other: Self) {
        self.0 &= !other.0;
    }
}

/// One cell in the 2D grid. 4 bytes exactly on a normal Rust target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub material: MaterialId,
    pub sat: Sat,
    pub flags: CellFlags,
    /// Reserved for future use (temperature quantile, sediment
    /// carrier, etc.). Kept in the layout so growing the cell state
    /// later doesn't ripple through every save.
    pub _pad: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            material: MaterialId::Air,
            sat: Sat::EMPTY,
            flags: CellFlags::empty(),
            _pad: 0,
        }
    }
}

impl Cell {
    pub fn air() -> Self {
        Self::default()
    }

    pub fn solid(material: MaterialId) -> Self {
        Self {
            material,
            sat: Sat::EMPTY,
            flags: CellFlags::empty(),
            _pad: 0,
        }
    }
}
