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
///
/// Mycelium colonization intensity lives in [`Cell::_pad`] on
/// [`MaterialId::Organic`] cells (0 = clean litter, 255 = fully
/// threaded) — not in these flag bits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct CellFlags(pub u8);

impl CellFlags {
    /// Reserved for the eventual "cell is currently ticking" bit used
    /// by rule ordering.
    pub const ACTIVE_HINT: CellFlags = CellFlags(0b0000_0001);
    /// Soft sediment already pulsed a compaction exudation this cycle.
    /// Cleared when pore sat rises again (re-wetting).
    pub const COMPACTED: CellFlags = CellFlags(0b0000_0010);
    /// Fully soaked Organic that has waterlogged — no longer floats;
    /// sinks through standing water like a dense grain.
    pub const WATERLOGGED: CellFlags = CellFlags(0b0000_0100);

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
    /// On [`MaterialId::Organic`]: mycelium thread intensity (0..=255).
    /// Cleared on material change. Elsewhere reserved / zero.
    /// Widening `Cell` past 4 bytes bumps [`crate::SIM_SCHEMA_VERSION`].
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

    /// Mycelium colonization on Organic (0 = none, 255 = fully threaded).
    #[inline]
    pub fn mycelium(self) -> u8 {
        if self.material == MaterialId::Organic {
            self._pad
        } else {
            0
        }
    }

    /// Set mycelium intensity; only retained on Organic cells.
    #[inline]
    pub fn set_mycelium(&mut self, intensity: u8) {
        self._pad = if self.material == MaterialId::Organic {
            intensity
        } else {
            0
        };
    }

    /// True when Organic has waterlogged and should sink through lakes.
    #[inline]
    pub fn is_waterlogged_organic(self) -> bool {
        self.material == MaterialId::Organic && self.flags.contains(CellFlags::WATERLOGGED)
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

/// True for dense granular materials that fall under gravity through
/// Air (including water-filled Air). Sand / Gravel / Clay / LooseRock /
/// LooseLimestone. Snow / Ice use [`falls_through_empty_air`] instead
/// (float on water).
pub fn is_grain(material: MaterialId) -> bool {
    matches!(
        material,
        MaterialId::Sand
            | MaterialId::Gravel
            | MaterialId::Clay
            | MaterialId::Soil
            | MaterialId::LooseRock
            | MaterialId::LooseLimestone
    )
}

/// Soft pack / litter: falls through *empty* Air, floats on standing water.
/// Organic is included so dead leaves and dissolved stems drop to the bed
/// without soaking into lakes.
pub fn falls_through_empty_air(material: MaterialId) -> bool {
    matches!(
        material,
        MaterialId::Snow | MaterialId::Ice | MaterialId::Organic
    )
}

/// Materials that participate in angle-of-repose diagonal slides.
/// Includes [`is_grain`] plus Snow and Organic litter (leaf piles
/// should sprawl, not stack into 1-cell towers).
pub fn is_repose_grain(material: MaterialId) -> bool {
    is_grain(material)
        || matches!(material, MaterialId::Snow | MaterialId::Organic)
}

/// Dense grains soft enough for flow bedload / bank undercut.
/// Matches the column sim's `erosion_resistance < 150` cut (excludes
/// Stone / Limestone / Ice). Snow uses repose + phase, not bedload.
pub fn is_flow_erodible(material: MaterialId) -> bool {
    use wk_material::MaterialRegistry;
    is_grain(material) && MaterialRegistry::erosion_rank(material) < 150
}

/// Max stable height step (cells) between adjacent columns before a
/// grain slides diagonally. From `repose_rise_m / SAMPLE_WIDTH_M`:
/// Sand≈0 (won't hold a 1-cell cliff), LooseRock≈1 (holds 45° stairs),
/// Ice/Stone → effectively infinite (not grains).
pub fn grain_max_stable_step(material: MaterialId) -> i32 {
    use wk_material::{MaterialRegistry, SAMPLE_WIDTH_M};
    let rise = MaterialRegistry::props(material).repose_rise_m;
    if !rise.is_finite() || rise >= 1.0e6 {
        return i32::MAX / 4;
    }
    (rise / SAMPLE_WIDTH_M).floor().max(0.0) as i32
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
///
/// Uses **table defaults** (no sim overrides). Prefer
/// [`water_capacity_with`] / [`crate::grid::World::water_capacity`] when
/// the world's [`wk_material::HydroOverrides`] must apply.
pub fn water_capacity(material: MaterialId) -> u8 {
    water_capacity_with(material, &wk_material::HydroOverrides::default())
}

/// [`water_capacity`] with an explicit hydrology override table
/// (typically [`crate::grid::World::hydro`]).
pub fn water_capacity_with(material: MaterialId, hydro: &wk_material::HydroOverrides) -> u8 {
    use wk_material::MaterialRegistry;
    match material {
        // Free-air cells hold a full unit of water. Any Air cell with
        // `sat = FULL` reads as "this cell is water" in every rule.
        MaterialId::Air => u8::MAX,
        // Snow / Ice / Water materials are treated as impermeable in
        // v1 rules: water doesn't pool inside a snow cell, and a
        // liquid Water cell is redundant with Air+sat=FULL.
        MaterialId::Water | MaterialId::Ice | MaterialId::Snow => 0,
        // Rock and dirt: capacity = material porosity (0..255), with
        // optional sim overrides from `hydro`.
        _ => MaterialRegistry::props_with(material, hydro).porosity,
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
    fn hydro_overrides_change_capacity_without_install() {
        let base = water_capacity(MaterialId::Sand);
        let mut hydro = wk_material::HydroOverrides::default();
        hydro.set_porosity(MaterialId::Sand, 40);
        assert_eq!(water_capacity_with(MaterialId::Sand, &hydro), 40);
        // Bare helper still returns table defaults (no process-global state).
        assert_eq!(water_capacity(MaterialId::Sand), base);
    }

    #[test]
    fn cell_water_helper_is_saturated_air() {
        let w = Cell::water();
        assert_eq!(w.material, MaterialId::Air);
        assert!(w.sat.is_full());
    }

    #[test]
    fn grain_set_covers_granular_materials() {
        for m in [
            MaterialId::Sand,
            MaterialId::Gravel,
            MaterialId::Clay,
            MaterialId::Soil,
            MaterialId::LooseRock,
            MaterialId::LooseLimestone,
        ] {
            assert!(is_grain(m), "{m:?} should be granular");
            assert!(is_repose_grain(m), "{m:?} should repose");
        }
        assert!(is_repose_grain(MaterialId::Snow));
        assert!(!is_grain(MaterialId::Snow), "snow floats — not a dense grain");
        assert!(falls_through_empty_air(MaterialId::Snow));
        assert!(falls_through_empty_air(MaterialId::Ice));
        assert!(
            is_repose_grain(MaterialId::Organic),
            "Organic litter should sprawl sideways"
        );
        assert!(
            !is_grain(MaterialId::Organic),
            "Organic floats on water — not a dense grain"
        );
        assert!(
            falls_through_empty_air(MaterialId::Organic),
            "Organic litter must fall through empty Air"
        );
        for m in [
            MaterialId::Bedrock,
            MaterialId::Stone,
            MaterialId::Limestone,
            MaterialId::Water,
            MaterialId::Air,
            MaterialId::Ice,
        ] {
            assert!(!is_grain(m), "{m:?} must not be classified as a grain");
            assert!(!is_repose_grain(m), "{m:?} must not repose");
        }
        assert!(!falls_through_empty_air(MaterialId::Sand));
    }

    #[test]
    fn sand_repose_step_stricter_than_loose_rock() {
        assert_eq!(grain_max_stable_step(MaterialId::Sand), 0);
        assert!(grain_max_stable_step(MaterialId::LooseRock) >= 1);
        assert!(
            grain_max_stable_step(MaterialId::Sand)
                < grain_max_stable_step(MaterialId::LooseRock)
        );
    }

    #[test]
    fn flow_erodible_covers_soft_grains_not_ice() {
        assert!(is_flow_erodible(MaterialId::Sand));
        assert!(is_flow_erodible(MaterialId::Gravel));
        assert!(is_flow_erodible(MaterialId::Clay));
        assert!(is_flow_erodible(MaterialId::LooseRock));
        assert!(is_flow_erodible(MaterialId::LooseLimestone));
        assert!(!is_flow_erodible(MaterialId::Ice));
        assert!(!is_flow_erodible(MaterialId::Snow));
        assert!(!is_flow_erodible(MaterialId::Stone));
        assert!(!is_flow_erodible(MaterialId::Bedrock));
    }
}
