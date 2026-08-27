//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Per-cell data: material tag + water saturation + a few flag bits.
//! Deliberately byte-packed; the stored pore coordinate makes this 5
//! bytes so a 64×64 chunk's cell slab is 20 KiB.
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
/// Mycelium colonization intensity lives in [`Cell::_pad`] on porous
/// hosts ([`MaterialId::Organic`], Soil, Sand, Clay, loose rock) —
/// 0 = clean, 255 = fully threaded. Not in these flag bits.
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
    /// Detached competent rock (fallen / tipped / slid). Flood-fill keeps
    /// mobile and unmarked strata in separate components — contact cannot
    /// grow a boulder by welding into painted terrain.
    pub const MOBILE_ROCK: CellFlags = CellFlags(0b0000_1000);
    /// High nibble: **rock body tag** (see [`Cell::rock_body_tag`]).
    pub const ROCK_BODY_TAG: CellFlags = CellFlags(0b1111_0000);

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

/// One cell in the 2D grid. `pore` deliberately widens the old 4-byte
/// layout; voxel saves are disposable across this schema change.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub material: MaterialId,
    pub sat: Sat,
    pub flags: CellFlags,
    /// Mycelium thread intensity (0..=255) on porous hosts
    /// ([`hosts_mycelium`]). Cleared on material change / non-hosts.
    /// Mycelium storage (separate from the pore coordinate).
    pub _pad: u8,
    /// Position inside this material's porosity / permeability ranges.
    /// `0` selects both minima, `255` both maxima; constructors use the
    /// midpoint and worldgen writes coherent spatial variation.
    pub pore: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self {
            material: MaterialId::Air,
            sat: Sat::EMPTY,
            flags: CellFlags::empty(),
            _pad: 0,
            pore: 128,
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
            pore: 128,
        }
    }

    /// Mycelium colonization on a porous host (0 = none, 255 = fully threaded).
    #[inline]
    pub fn mycelium(self) -> u8 {
        if hosts_mycelium(self.material) {
            self._pad
        } else {
            0
        }
    }

    /// Set mycelium intensity; retained only on porous hosts.
    #[inline]
    pub fn set_mycelium(&mut self, intensity: u8) {
        self._pad = if hosts_mycelium(self.material) {
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

    /// Which loose rock body this competent cell belongs to, `0` = none.
    ///
    /// `0` means the cell is **deliberately joined** rock: world generation
    /// strata, editor paint, or (future) geological compaction. Those merge with
    /// each other, which is what makes continuous terrain behave as one mass.
    ///
    /// `1..=15` identifies a distinct detached body. Flood-fill only merges
    /// equal tags, so a rolling boulder cannot glue itself to another rock it
    /// happens to touch on the way past. Four bits is plenty because the tag
    /// only has to separate bodies that are *adjacent*; see `pick_body_tag`.
    #[inline]
    pub fn rock_body_tag(self) -> u8 {
        (self.flags.0 & CellFlags::ROCK_BODY_TAG.0) >> 4
    }

    /// Set the loose-body tag (low 4 bits of `tag` are used).
    #[inline]
    pub fn set_rock_body_tag(&mut self, tag: u8) {
        self.flags.0 = (self.flags.0 & !CellFlags::ROCK_BODY_TAG.0) | ((tag & 0x0F) << 4);
    }

    /// Drop the loose-body tag (rock rejoining strata, or becoming debris).
    #[inline]
    pub fn clear_rock_body_tag(&mut self) {
        self.flags.0 &= !CellFlags::ROCK_BODY_TAG.0;
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
            pore: 128,
        }
    }
}

/// Materials that can carry a mycelium cream field in `_pad`.
///
/// Organic is the food substrate; Soil / Sand / Clay / loose rock are
/// mineral corridors (harder to thread). Competent Stone is allowed only
/// as a rare crack path (see fungi spread costs). Bedrock refuses.
#[inline]
pub fn hosts_mycelium(material: MaterialId) -> bool {
    matches!(
        material,
        MaterialId::Organic
            | MaterialId::Soil
            | MaterialId::Sand
            | MaterialId::Clay
            | MaterialId::Bentonite
            | MaterialId::LooseRock
            | MaterialId::LooseLimestone
            | MaterialId::Stone
            | MaterialId::Limestone
            | MaterialId::Flowstone
            | MaterialId::Sandstone
            | MaterialId::Conglomerate
    )
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
            | MaterialId::Bentonite
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
    is_grain(material) || matches!(material, MaterialId::Snow | MaterialId::Organic)
}

/// Competent bedrock-class solids that can move as rigid bodies (not Bedrock).
pub fn is_competent_rock(material: MaterialId) -> bool {
    // Flowstone is a cemented deposit — structurally rock, like limestone.
    matches!(
        material,
        MaterialId::Stone
            | MaterialId::Limestone
            | MaterialId::Flowstone
            | MaterialId::Sandstone
            | MaterialId::Conglomerate
    )
}

/// Dense grains soft enough for flow bedload / bank undercut.
/// Matches the column sim's `erosion_resistance < 150` cut (excludes
/// Stone / Limestone / Ice). Snow uses repose + phase, not bedload.
///
/// **Organic** is not included here — it floats, so bedload uses the
/// world-aware gate in [`crate::rules::grain`] (grounded / waterlogged
/// only; floating rafts stay). **Soil** is a dense grain and is included.
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

/// Cell-aware capacity. This is the authoritative lookup for every pass
/// that reads, writes, clamps, or audits pore saturation.
#[inline]
pub fn water_capacity_cell(cell: Cell, hydro: &wk_material::HydroOverrides) -> u8 {
    use wk_material::MaterialRegistry;
    match cell.material {
        MaterialId::Air => u8::MAX,
        MaterialId::Water | MaterialId::Ice | MaterialId::Snow => 0,
        _ => MaterialRegistry::hydrology_with(cell.material, hydro)
            .porosity
            .sample(cell.pore),
    }
}

/// Saturation this cell holds against gravity (capillary retention).
///
/// Only sat **above** this drains downward — see
/// [`wk_material::MaterialProps::field_capacity`]. Scales with the cell's own
/// capacity, so a high-`pore` cell both stores and retains more.
#[inline]
pub fn retained_sat_cell(cell: Cell, hydro: &wk_material::HydroOverrides) -> u8 {
    use wk_material::MaterialRegistry;
    if cell.material == MaterialId::Air {
        return 0;
    }
    let cap = water_capacity_cell(cell, hydro) as u32;
    if cap == 0 {
        return 0;
    }
    let fc = MaterialRegistry::base_props(cell.material).field_capacity as u32;
    // Retention *fraction* falls as pore space opens.
    //
    // Without this, retention scaled with capacity, so a fractured cell held more
    // absolute water than a tight one. Every cell then filled to its own retention
    // and stopped, and a long soak ended with the whole rock wet and the veins
    // slightly wetter — the underground read as uniform, and conduits stored water
    // instead of transmitting it. Playtest called it "decoration".
    //
    // Open media genuinely drain better: it is why gravel retains 20/255 and clay
    // 188/255. The same has to hold *within* a material, or opening a cell buys
    // capacity and retention in equal measure and nothing changes character.
    //
    // With it, a vein drains nearly dry between flows while the matrix beside it
    // perches — low storage and high flux, which is what a conduit is, and it
    // makes the structure persistent rather than transient.
    // Competent rock only. Conduits are a rock phenomenon: fine sediments are
    // matrix, not channels, and shedding from clay turned the aquitard into a
    // drain (open clay fell from 74% retention to 29%, which is not a seal).
    if !is_competent_rock(cell.material) {
        return ((cap * fc) / 255) as u8;
    }
    let open = (cell.pore as u32).saturating_sub(PORE_MATRIX_MID as u32);
    let span = (u8::MAX - PORE_MATRIX_MID) as u32;
    let shed = (DRAINAGE_SHED_NUM * open) / span.max(1);
    let fc = fc.saturating_sub((fc * shed) / DRAINAGE_SHED_DEN);
    ((cap * fc) / 255) as u8
}

/// Pore value treated as a material's matrix: below it, retention is unchanged.
const PORE_MATRIX_MID: u8 = 128;
/// How much of the retention fraction a fully open cell sheds, as
/// `NUM / DEN` — 60%, so an open vein holds well under half what its tight
/// neighbour does once the larger capacity is accounted for.
const DRAINAGE_SHED_NUM: u32 = 60;
const DRAINAGE_SHED_DEN: u32 = 100;

/// Saturation in this cell that is free to drain downward.
#[inline]
pub fn drainable_sat_cell(cell: Cell, hydro: &wk_material::HydroOverrides) -> u8 {
    cell.sat.0.saturating_sub(retained_sat_cell(cell, hydro))
}

/// Cell-aware permeability selected from the same stored pore coordinate.
#[inline]
pub fn permeability_cell(cell: Cell, hydro: &wk_material::HydroOverrides) -> u8 {
    use wk_material::MaterialRegistry;
    MaterialRegistry::hydrology_with(cell.material, hydro)
        .permeability
        .sample_fracture(cell.pore)
}

#[cfg(test)]
mod retention_tests {
    use super::*;

    /// A vein must end up *drier* than the matrix beside it, not wetter.
    ///
    /// Retention used to scale with capacity, so opening a cell bought capacity
    /// and retention in equal measure: a fractured cell held more absolute water
    /// than a tight one, every cell filled to its own retention and stopped, and a
    /// long soak left the whole rock uniformly wet with conduits *storing* water.
    /// Low storage and high flux is what makes a conduit a conduit.
    #[test]
    fn an_open_cell_retains_less_than_a_tight_one() {
        let h = wk_material::HydroOverrides::default();
        let mut tight = Cell::solid(MaterialId::Stone);
        tight.pore = PORE_MATRIX_MID;
        let mut open = Cell::solid(MaterialId::Stone);
        open.pore = u8::MAX;
        let (r_tight, r_open) = (retained_sat_cell(tight, &h), retained_sat_cell(open, &h));
        assert!(
            r_open < r_tight,
            "an open vein should shed water a tight matrix holds ({r_open} vs {r_tight})"
        );
        // And it has more room, so the contrast is real rather than an artifact of
        // a smaller container.
        assert!(
            water_capacity_cell(open, &h) > water_capacity_cell(tight, &h),
            "the open cell should still hold more when full"
        );
    }

    #[test]
    fn matrix_pore_retention_is_unchanged() {
        // Only *opening* sheds retention. A default cell must behave exactly as
        // before, or every tuned material shifts underneath us.
        let h = wk_material::HydroOverrides::default();
        for m in [
            MaterialId::Stone,
            MaterialId::Clay,
            MaterialId::Sand,
            MaterialId::Limestone,
        ] {
            let mut c = Cell::solid(m);
            c.pore = PORE_MATRIX_MID;
            let cap = water_capacity_cell(c, &h) as u32;
            let fc = wk_material::MaterialRegistry::base_props(m).field_capacity as u32;
            assert_eq!(
                retained_sat_cell(c, &h) as u32,
                (cap * fc) / 255,
                "{m:?} at matrix pore must retain exactly the material value"
            );
        }
    }

    #[test]
    fn fine_sediment_sheds_nothing() {
        // Conduits are a rock phenomenon. Shedding from clay turned the aquitard
        // into a drain — open clay fell from 74% retention to 29%, which is not a
        // seal — and sand and gravel are matrix, not channels.
        let h = wk_material::HydroOverrides::default();
        for m in [
            MaterialId::Clay,
            MaterialId::Bentonite,
            MaterialId::Sand,
            MaterialId::Gravel,
        ] {
            let mut tight = Cell::solid(m);
            tight.pore = PORE_MATRIX_MID;
            let mut open = Cell::solid(m);
            open.pore = u8::MAX;
            let fc = wk_material::MaterialRegistry::base_props(m).field_capacity as u32;
            for c in [tight, open] {
                let cap = water_capacity_cell(c, &h) as u32;
                assert_eq!(
                    retained_sat_cell(c, &h) as u32,
                    (cap * fc) / 255,
                    "{m:?} retention must not depend on pore"
                );
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cell_layout_includes_stored_pore_byte() {
        assert_eq!(std::mem::size_of::<Cell>(), 5);
    }

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
    fn pore_coordinate_selects_capacity_and_permeability_ranges() {
        let hydro = wk_material::HydroOverrides::default();
        let mut low = Cell::solid(MaterialId::Sand);
        low.pore = 0;
        let mut high = low;
        high.pore = 255;
        assert!(water_capacity_cell(low, &hydro) < water_capacity_cell(high, &hydro));
        assert!(permeability_cell(low, &hydro) < permeability_cell(high, &hydro));
    }

    #[test]
    fn zero_to_zero_override_seals_every_pore_coordinate() {
        let mut hydro = wk_material::HydroOverrides::default();
        hydro.set_permeability_range(MaterialId::Sand, 0, 0);
        for pore in [0, 128, 255] {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.pore = pore;
            assert_eq!(permeability_cell(sand, &hydro), 0);
        }
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
        assert!(
            !is_grain(MaterialId::Snow),
            "snow floats — not a dense grain"
        );
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
            grain_max_stable_step(MaterialId::Sand) < grain_max_stable_step(MaterialId::LooseRock)
        );
    }

    #[test]
    fn flow_erodible_covers_soft_grains_not_ice() {
        assert!(is_flow_erodible(MaterialId::Sand));
        assert!(is_flow_erodible(MaterialId::Gravel));
        assert!(is_flow_erodible(MaterialId::Clay));
        assert!(is_flow_erodible(MaterialId::Soil));
        assert!(is_flow_erodible(MaterialId::LooseRock));
        assert!(is_flow_erodible(MaterialId::LooseLimestone));
        assert!(!is_flow_erodible(MaterialId::Organic)); // world-aware gate in grain.rs
        assert!(!is_flow_erodible(MaterialId::Ice));
        assert!(!is_flow_erodible(MaterialId::Snow));
        assert!(!is_flow_erodible(MaterialId::Stone));
        assert!(!is_flow_erodible(MaterialId::Bedrock));
    }
}
