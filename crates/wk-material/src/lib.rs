//! Material vocabulary and property tables for World Kernel 0.1.

use serde::{Deserialize, Serialize};

pub const MATERIAL_COUNT: usize = 8;
pub const SAMPLE_WIDTH_M: f32 = 0.25;
pub const MAX_LAYERS: usize = 8;
pub const CHUNK_W: usize = 64;
pub const MAX_LOADED_CHUNKS: usize = 56;
pub const MAX_MARKERS: usize = 64;
pub const FIXED_SCALE: i64 = 1000;
pub const MERGE_GAP: u64 = 100;
pub const MERGE_MAX_THICKNESS: i64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum MaterialId {
    Bedrock = 0,
    Stone = 1,
    #[default]
    Sand = 2,
    Clay = 3,
    Organic = 4,
    Water = 5,
    Air = 6,
    /// Cold-weather precipitation deposit sitting on top of the ground like
    /// any other layer; melts back into surface water when warm (see
    /// wk-sim's run_snow_melt).
    Snow = 7,
}

impl MaterialId {
    pub const ALL_SOLIDS: [MaterialId; 6] = [
        MaterialId::Bedrock,
        MaterialId::Stone,
        MaterialId::Sand,
        MaterialId::Clay,
        MaterialId::Organic,
        MaterialId::Snow,
    ];

    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            0 => Some(MaterialId::Bedrock),
            1 => Some(MaterialId::Stone),
            2 => Some(MaterialId::Sand),
            3 => Some(MaterialId::Clay),
            4 => Some(MaterialId::Organic),
            5 => Some(MaterialId::Water),
            6 => Some(MaterialId::Air),
            7 => Some(MaterialId::Snow),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    pub fn is_solid(self) -> bool {
        matches!(
            self,
            MaterialId::Bedrock
                | MaterialId::Stone
                | MaterialId::Sand
                | MaterialId::Clay
                | MaterialId::Organic
                | MaterialId::Snow
        )
    }

    pub fn is_erodible(self) -> bool {
        !matches!(self, MaterialId::Bedrock | MaterialId::Air | MaterialId::Water)
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MaterialProps {
    /// kg per m³ (used for height ↔ mass conversion)
    pub density: u32,
    /// 0–255
    pub permeability: u8,
    /// higher = harder to erode; bedrock uses u32::MAX
    pub erosion_resistance: u32,
    /// 0–255, higher = resists suspension
    pub cohesion: u8,
    /// 0–255, max moisture as fraction of layer mass (scaled)
    pub porosity: u8,
}

pub struct MaterialRegistry;

impl MaterialRegistry {
    pub fn props(material: MaterialId) -> MaterialProps {
        match material {
            MaterialId::Bedrock => MaterialProps {
                density: 2700,
                permeability: 0,
                erosion_resistance: u32::MAX,
                cohesion: 255,
                porosity: 0,
            },
            MaterialId::Stone => MaterialProps {
                density: 2600,
                permeability: 5,
                erosion_resistance: 200,
                cohesion: 200,
                porosity: 20,
            },
            MaterialId::Sand => MaterialProps {
                density: 1600,
                permeability: 220,
                erosion_resistance: 30,
                cohesion: 20,
                porosity: 180,
            },
            MaterialId::Clay => MaterialProps {
                density: 1900,
                permeability: 10,
                erosion_resistance: 80,
                cohesion: 180,
                porosity: 60,
            },
            MaterialId::Organic => MaterialProps {
                density: 600,
                permeability: 120,
                erosion_resistance: 60,
                cohesion: 100,
                porosity: 200,
            },
            MaterialId::Water => MaterialProps {
                density: 1000,
                permeability: 0,
                erosion_resistance: 0,
                cohesion: 0,
                porosity: 0,
            },
            MaterialId::Air => MaterialProps {
                density: 0,
                permeability: 0,
                erosion_resistance: 0,
                cohesion: 0,
                porosity: 0,
            },
            MaterialId::Snow => MaterialProps {
                density: 250,
                permeability: 40,
                erosion_resistance: 10,
                cohesion: 30,
                porosity: 100,
            },
        }
    }

    /// Erosion susceptibility: lower resistance value = erodes faster.
    pub fn erosion_rank(material: MaterialId) -> u32 {
        Self::props(material).erosion_resistance
    }

    pub fn colour_rgb(material: MaterialId) -> [u8; 3] {
        match material {
            MaterialId::Bedrock => [0x40, 0x40, 0x40],
            MaterialId::Stone => [0x80, 0x80, 0x80],
            MaterialId::Sand => [0xFF, 0xFF, 0x00],
            MaterialId::Clay => [0x80, 0x40, 0x00],
            MaterialId::Organic => [0x00, 0xAA, 0x00],
            MaterialId::Water => [0x00, 0x00, 0xFF],
            MaterialId::Air => [0x87, 0xCE, 0xEB],
            MaterialId::Snow => [0xFF, 0xFF, 0xFF],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn erosion_order_sand_clay_stone() {
        let sand = MaterialRegistry::erosion_rank(MaterialId::Sand);
        let clay = MaterialRegistry::erosion_rank(MaterialId::Clay);
        let stone = MaterialRegistry::erosion_rank(MaterialId::Stone);
        assert!(sand < clay);
        assert!(clay < stone);
    }
}
