//! Four-byte physics cell. GvsE's `Cell` with the pore coordinate and
//! the mycelium pad removed: material, saturation, colour-variant flags.

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};

use crate::consts::STANDING_AIR_SAT;

/// One physics cell. Colour variants live in the low bits of `flags`
/// (render only). `pad` is reserved for Phase 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Cell {
    pub material: u8,
    pub sat: u8,
    pub flags: u8,
    pub pad: u8,
}

impl Default for Cell {
    fn default() -> Self {
        Self::air()
    }
}

impl Cell {
    pub const fn air() -> Self {
        Self {
            material: MaterialId::Air as u8,
            sat: 0,
            flags: 0,
            pad: 0,
        }
    }

    pub const fn water() -> Self {
        Self {
            material: MaterialId::Air as u8,
            sat: 255,
            flags: 0,
            pad: 0,
        }
    }

    pub const fn solid(material: MaterialId) -> Self {
        Self {
            material: material as u8,
            sat: 0,
            flags: 0,
            pad: 0,
        }
    }

    pub const fn sand() -> Self {
        Self::solid(MaterialId::Sand)
    }

    pub const fn stone() -> Self {
        Self::solid(MaterialId::Stone)
    }

    pub const fn bedrock() -> Self {
        Self::solid(MaterialId::Bedrock)
    }

    /// Player-drawn glass wall (Phase 3). Tank-local material byte
    /// outside `wk_material`'s table: solid and impervious to water and
    /// grains (it maps to Bedrock for the physics rules), but light
    /// passes through and it renders as a pale transparent pane.
    pub const GLASS: u8 = 18;

    pub const fn glass() -> Self {
        Self {
            material: Self::GLASS,
            sat: 0,
            flags: 0,
            pad: 0,
        }
    }

    #[inline]
    pub fn is_glass(self) -> bool {
        self.material == Self::GLASS
    }

    #[inline]
    pub fn id(self) -> MaterialId {
        if self.material == Self::GLASS {
            return MaterialId::Bedrock;
        }
        MaterialId::from_u8(self.material).unwrap_or(MaterialId::Air)
    }

    /// `flags` bit 7: the cell is at least 5/9 hard living tissue and
    /// behaves as a solid toward water and grains (§b.3 dam rule).
    pub const TISSUE: u8 = 0b1000_0000;
    /// `flags` bit 6: ≥5 root pixels co-occupy this sand/soil cell —
    /// bound, so it neither falls nor slumps (§h.1). A drawn rock is
    /// bound too, so its cells do not fall one by one.
    pub const BOUND: u8 = 0b0100_0000;
    /// `flags` bit 4: this stone cell belongs to one drawn rock. `pad`
    /// is that rock's id (1..=255), not the wetness film.
    pub const RIGID: u8 = 0b0001_0000;
    /// `flags` bit 5: stone drawn and not yet released. It holds still
    /// until play, then becomes one rigid rock.
    pub const SILHOUETTE: u8 = 0b0010_0000;

    /// Fluid seat for the physics rules: air/water that is not dammed by
    /// hard tissue.
    #[inline]
    pub fn is_air(self) -> bool {
        self.material == MaterialId::Air as u8 && self.flags & Self::TISSUE == 0
    }

    /// Air or water regardless of tissue damming — what living cells see.
    #[inline]
    pub fn is_open(self) -> bool {
        self.material == MaterialId::Air as u8
    }

    #[inline]
    pub fn is_tissue(self) -> bool {
        self.flags & Self::TISSUE != 0
    }

    #[inline]
    pub fn is_bound(self) -> bool {
        self.flags & Self::BOUND != 0
    }

    /// One cell of a drawn rock that falls, turns, and settles as a whole.
    #[inline]
    pub fn is_rigid(self) -> bool {
        self.flags & Self::RIGID != 0
    }

    /// Stone still being drawn. It does not fall until play.
    #[inline]
    pub fn is_silhouette(self) -> bool {
        self.flags & Self::SILHOUETTE != 0
    }

    #[inline]
    pub fn is_standing_water(self) -> bool {
        self.is_air() && self.sat >= STANDING_AIR_SAT
    }

    /// Low 2 bits: render-only palette index for sand and rock (0..=3).
    #[inline]
    pub fn colour_variant(self) -> u8 {
        self.flags & 0b0000_0011
    }

    #[inline]
    pub fn set_colour_variant(&mut self, variant: u8) {
        self.flags = (self.flags & !0b0000_0011) | (variant & 0b0000_0011);
    }

    /// Bits 2–3: which way a cobble is facing. Render only; advanced
    /// one turn each time the cell falls or slumps.
    pub const TUMBLE: u8 = 0b0000_1100;

    #[inline]
    pub fn tumble(self) -> u8 {
        (self.flags & Self::TUMBLE) >> 2
    }

    #[inline]
    pub fn spin(&mut self) {
        let n = self.tumble().wrapping_add(1) & 0b11;
        self.flags = (self.flags & !Self::TUMBLE) | (n << 2);
    }

    /// Cobbles that should show a turn when they move.
    #[inline]
    pub fn tumbles(self) -> bool {
        matches!(
            self.id(),
            MaterialId::Stone | MaterialId::LooseRock | MaterialId::LooseLimestone
        )
    }

    /// Rock surface film (0..=255) stored in `pad`. Sand uses `sat`.
    #[inline]
    pub fn rock_film(self) -> u8 {
        if self.is_rock() && !self.is_rigid() {
            self.pad
        } else {
            0
        }
    }

    #[inline]
    pub fn set_rock_film(&mut self, film: u8) {
        // A drawn rock keeps its body id in `pad`. Wetness must not
        // overwrite it, or the stone splits when it meets water.
        if self.is_rock() && !self.is_rigid() && !self.is_silhouette() {
            self.pad = film;
        }
    }

    #[inline]
    pub fn is_rock(self) -> bool {
        matches!(
            self.id(),
            MaterialId::Stone
                | MaterialId::Limestone
                | MaterialId::Sandstone
                | MaterialId::Conglomerate
                | MaterialId::Flowstone
                | MaterialId::LooseRock
                | MaterialId::LooseLimestone
        )
    }

    /// Visible wetness for damp/dry rendering: pore sat for sand, film for rock.
    #[inline]
    pub fn moisture(self) -> u8 {
        if self.is_rock() && !self.is_rigid() {
            self.pad.max(self.sat)
        } else {
            self.sat
        }
    }
}

/// Grains that fall and slump. Bedrock and the bedded limestones stay
/// put. Stone and loose rock are cobbles here: they drop, pile, and tumble.
pub fn is_mobile_grain(id: MaterialId) -> bool {
    matches!(
        id,
        MaterialId::Sand
            | MaterialId::Gravel
            | MaterialId::Clay
            | MaterialId::Bentonite
            | MaterialId::Soil
            | MaterialId::Organic
            | MaterialId::Snow
            | MaterialId::Stone
            | MaterialId::LooseRock
            | MaterialId::LooseLimestone
    )
}

/// Cached material row. `MaterialRegistry::props` returns a large struct
/// by value; the hot passes only need a few bytes, looked up once.
#[derive(Clone, Copy)]
pub struct MatInfo {
    pub capacity: u8,
    pub rate: u8,
    pub retain: u8,
    pub mobile: bool,
    pub step: i32,
}

pub fn mat_info(id: MaterialId) -> MatInfo {
    table()[id as usize]
}

fn table() -> &'static [MatInfo; 18] {
    use std::sync::OnceLock;
    static T: OnceLock<[MatInfo; 18]> = OnceLock::new();
    T.get_or_init(|| {
        let mut rows = [MatInfo {
            capacity: 0,
            rate: 0,
            retain: 0,
            mobile: false,
            step: i32::MAX / 4,
        }; 18];
        for (i, row) in rows.iter_mut().enumerate() {
            let id = MaterialId::from_u8(i as u8).expect("material id");
            *row = MatInfo::compute(id);
        }
        rows
    })
}

impl MatInfo {
    fn compute(id: MaterialId) -> Self {
        let mobile = is_mobile_grain(id);
        if id == MaterialId::Air {
            return Self {
                capacity: 255,
                rate: 0,
                retain: 0,
                mobile,
                step: i32::MAX / 4,
            };
        }
        let props = MaterialRegistry::props(id);
        let capacity = props.porosity;
        let rate = if props.permeability == 0 {
            0
        } else {
            (props.permeability / 16).max(1)
        };
        let retain = ((capacity as u16) * (props.field_capacity as u16) / 255).min(255) as u8;
        // Stone keeps its natural repose. An infinite rise means a cobble
        // falls and tumbles, then stacks, instead of flowing like powder.
        let step = stable_step_of(props.repose_rise_m);
        Self {
            capacity,
            rate,
            retain,
            mobile,
            step,
        }
    }
}

/// Water capacity on the 0–255 sat scale. Air is empty space (255);
/// `wk-material` lists air porosity as 0 because that table describes
/// solid layers.
pub fn water_capacity(id: MaterialId) -> u8 {
    mat_info(id).capacity
}

/// Saturation held against gravity, in absolute sat units.
pub fn retained_sat(id: MaterialId) -> u8 {
    mat_info(id).retain
}

/// Tallest stable diagonal drop, in cells.
///
/// GvsE computes `(repose_rise_m / 0.25 m).floor()`, which is 0 for
/// sand because a column there is a quarter-metre wide. Tank cells are
/// about a millimetre, so a finite repose becomes one cell of drop
/// (a readable ~45° pile). Stone and the other infinite-repose rocks
/// never slump: a painted cobble drops, tumbles, and stacks.
pub fn stable_step(id: MaterialId) -> i32 {
    mat_info(id).step
}

fn stable_step_of(rise: f32) -> i32 {
    if !rise.is_finite() || rise >= 1.0e6 {
        return i32::MAX / 4;
    }
    let scaled = (rise / SAMPLE_WIDTH_M).floor() as i32;
    scaled.max(1)
}

/// Seepage step in sat units. Zero permeability does not move water.
pub fn seepage_rate(id: MaterialId) -> u8 {
    mat_info(id).rate
}
