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

    #[inline]
    pub fn id(self) -> MaterialId {
        MaterialId::from_u8(self.material).unwrap_or(MaterialId::Air)
    }

    #[inline]
    pub fn is_air(self) -> bool {
        self.material == MaterialId::Air as u8
    }

    #[inline]
    pub fn is_standing_water(self) -> bool {
        self.is_air() && self.sat >= STANDING_AIR_SAT
    }

    /// Low 2 bits: render-only palette index for sand and rock.
    #[inline]
    pub fn colour_variant(self) -> u8 {
        self.flags & 0b0000_0011
    }
}

/// Grains that fall and slump. Competent rock is static (Phase 1.7:
/// rocks never move; no competent-fall solver).
pub fn is_mobile_grain(id: MaterialId) -> bool {
    matches!(
        id,
        MaterialId::Sand
            | MaterialId::Gravel
            | MaterialId::Clay
            | MaterialId::Bentonite
            | MaterialId::Soil
            | MaterialId::LooseRock
            | MaterialId::LooseLimestone
            | MaterialId::Organic
            | MaterialId::Snow
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
/// (a readable ~45° pile). Infinite repose never slumps.
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
