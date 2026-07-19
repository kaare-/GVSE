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

    /// Convenience: an Air cell already fully saturated with water.
    /// Rules treat this the same as any other cell with `sat == FULL`
    /// — "fully waterlogged air" is our stand-in for a free-water
    /// cell (a lake surface, a raindrop, the interior of a pool).
    pub fn water() -> Self {
        Self {
            material: MaterialId::Air,
            sat: Sat::FULL,
            flags: CellFlags::empty(),
            _pad: 0,
        }
    }
}

/// Per-cell water-holding capacity as a `u8` on the same 0..255 scale
/// as [`Sat`]. Air cells are treated as fully waterable (255); porous
/// solids clamp to their [`wk_material::MaterialProps::porosity`];
/// impermeable solids return 0.
///
/// `wk-material`'s `porosity` field is defined against the *solid*
/// column model (fraction of the layer mass that can hold pore water)
/// so Air itself is listed there as `porosity = 0`. In the voxel
/// world an Air cell is empty *space* — the whole cell can turn into
/// water — so we shim it here rather than editing shared props.
pub fn water_capacity(material: MaterialId) -> u8 {
    use wk_material::MaterialRegistry;
    match material {
        // Free-air cells hold a full unit of water. Any Air cell with
        // `sat = FULL` reads as "this cell is water" in every rule.
        MaterialId::Air => u8::MAX,
        // Snow / Ice / Water materials are treated as impermeable in
        // v1 rules: water doesn't pool inside a snow cell, and a
        // liquid Water cell is redundant with Air+sat=FULL.
        MaterialId::Water | MaterialId::Ice | MaterialId::Snow => 0,
        // Rock and dirt: capacity = material porosity (0..255).
        _ => MaterialRegistry::props(material).porosity,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn air_holds_full_water() {
        assert_eq!(water_capacity(MaterialId::Air), u8::MAX);
    }

    #[test]
    fn bedrock_holds_none() {
        assert_eq!(water_capacity(MaterialId::Bedrock), 0);
    }

    #[test]
    fn sand_uses_porosity() {
        // Sand porosity is 180 in wk-material's registry today; the
        // exact value is a data question, we just check the shim
        // routes through the registry rather than shortcutting.
        assert_eq!(
            water_capacity(MaterialId::Sand),
            wk_material::MaterialRegistry::props(MaterialId::Sand).porosity
        );
    }

    #[test]
    fn ice_and_snow_treated_impermeable_here() {
        assert_eq!(water_capacity(MaterialId::Ice), 0);
        assert_eq!(water_capacity(MaterialId::Snow), 0);
    }

    #[test]
    fn cell_water_helper_is_saturated_air() {
        let w = Cell::water();
        assert_eq!(w.material, MaterialId::Air);
        assert!(w.sat.is_full());
    }
}
