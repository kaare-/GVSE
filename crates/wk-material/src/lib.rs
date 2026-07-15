//! Material vocabulary and property tables for World Kernel 0.1.

use serde::{Deserialize, Serialize};

pub const MATERIAL_COUNT: usize = 11;
pub const SAMPLE_WIDTH_M: f32 = 0.25;
pub const MAX_LAYERS: usize = 8;
pub const CHUNK_W: usize = 64;
pub const MAX_LOADED_CHUNKS: usize = 96;
pub const MAX_MARKERS: usize = 64;
pub const FIXED_SCALE: i64 = 1000;
pub const MERGE_GAP: u64 = 100;
pub const MERGE_MAX_THICKNESS: i64 = 1_000_000;

/// Every substance in the simulation is one of these — including water,
/// ice, and snow. Materials differ only in their property table (density,
/// erosion, phase-change threshold, whether they flow, etc.); the layer
/// stack treats them uniformly. Bedrock is the sole exception: it never
/// appears in the layer stack — it's the immutable substrate line under
/// every column (see `Chunk::bedrock_y`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[repr(u8)]
pub enum MaterialId {
    Bedrock = 0,
    Stone = 1,
    #[default]
    Sand = 2,
    Clay = 3,
    /// Reserved for a future ecology / biomass pass. Real topsoil is
    /// sand + clay + decayed organic matter *held together by living
    /// roots*; without an ecology sim to grow and maintain those roots,
    /// generating a free-floating "organic" layer would be dishonest
    /// (and would, correctly, wash off any slope in the first storm).
    /// Kept in the enum for save-file compatibility with older worlds
    /// that did generate it; terrain generation no longer emits it.
    Organic = 4,
    Water = 5,
    Air = 6,
    Snow = 7,
    /// Frozen water. Solid, low density, transforms back into `Water` above
    /// the freeze point (see `MaterialProps::phase_change`).
    Ice = 8,
    /// Loose rock — cobbles and boulders, coarser than gravel. Harder to
    /// erode than sand/gravel but still moves under strong flow;
    /// characteristic mountain-slope talus / scree cover.
    LooseRock = 9,
    /// Coarse aggregate, between sand and loose rock in grain size.
    /// Very permeable (water flows through easily); harder to erode than
    /// sand but easier than clay. Beach cobbles, riverbed lag.
    Gravel = 10,
}

impl MaterialId {
    /// Ground-forming solids (never fluid, never phase-changes at the
    /// world's normal temperature range).
    pub const ALL_SOLIDS: [MaterialId; 6] = [
        MaterialId::Bedrock,
        MaterialId::Stone,
        MaterialId::LooseRock,
        MaterialId::Gravel,
        MaterialId::Sand,
        MaterialId::Clay,
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
            8 => Some(MaterialId::Ice),
            9 => Some(MaterialId::LooseRock),
            10 => Some(MaterialId::Gravel),
            _ => None,
        }
    }

    pub fn index(self) -> usize {
        self as usize
    }

    /// True for materials that have real load-bearing shape (sand piles,
    /// stone columns, ice slabs, snow packs). Water is the only non-solid
    /// among the ones we actually place in the layer stack.
    pub fn is_solid(self) -> bool {
        matches!(
            self,
            MaterialId::Bedrock
                | MaterialId::Stone
                | MaterialId::LooseRock
                | MaterialId::Gravel
                | MaterialId::Sand
                | MaterialId::Clay
                | MaterialId::Organic
                | MaterialId::Snow
                | MaterialId::Ice
        )
    }

    pub fn is_erodible(self) -> bool {
        !matches!(
            self,
            MaterialId::Bedrock | MaterialId::Air | MaterialId::Water | MaterialId::Ice
        )
    }

    /// True for materials that flow laterally under head gradients
    /// (currently just liquid water). The surface-water flow subsystem
    /// only ever acts on materials returning true from this.
    pub fn is_fluid(self) -> bool {
        matches!(self, MaterialId::Water)
    }
}

/// Temperature-driven material transition. The unified `run_phase_change`
/// subsystem walks each column's top layer and, if `temp > threshold_c`,
/// converts a fraction of that layer's mass into `above` (if Some); if
/// `temp <= threshold_c`, converts into `below` (if Some). This is what
/// unifies "snow melts into water", "water freezes into ice", "ice thaws
/// into water" — one mechanism, three different property rows.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct PhaseChange {
    pub threshold_c: f32,
    pub below: Option<MaterialId>,
    pub above: Option<MaterialId>,
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
    /// If Some, this material transitions to another when temperature
    /// crosses `threshold_c`. Everything else has None.
    pub phase_change: Option<PhaseChange>,
    /// Rendering: extra alpha applied to the material's colour when
    /// drawing this layer. 255 = fully opaque, lower = translucent.
    /// Used to give Water/Ice/Snow a see-through look uniformly with
    /// the rest of the layer stack (no more separate rendering pass).
    pub render_alpha: u8,
    /// Angle of repose expressed as the maximum stable rise (metres)
    /// between adjacent columns before the material slumps downhill.
    /// A value of 0.15 with a 0.25 m column width means the top solid
    /// surface can differ by at most 0.15 m before loose material
    /// starts sliding — that's ≈31° from horizontal, right around
    /// dry sand's real angle of repose.
    ///
    /// `f32::INFINITY` = never slumps (bedrock, effectively stone).
    pub repose_rise_m: f32,
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
                phase_change: None,
                render_alpha: 255,
                repose_rise_m: f32::INFINITY,
            },
            MaterialId::Stone => MaterialProps {
                density: 2600,
                permeability: 5,
                erosion_resistance: 200,
                cohesion: 200,
                porosity: 20,
                phase_change: None,
                render_alpha: 255,
                // Effectively cliff-stable — bedded stone doesn't slump.
                repose_rise_m: f32::INFINITY,
            },
            MaterialId::Sand => MaterialProps {
                density: 1600,
                // Sand is permeable in reality but the previous 220
                // (out of 255) made a puddle of ~100 kg drain into the
                // ground in ~100 ticks — visibly instant. Modest cut so
                // pools linger for a few seconds and shallow dams have
                // a chance to fill up before their body seeps away.
                permeability: 160,
                erosion_resistance: 30,
                cohesion: 20,
                porosity: 180,
                phase_change: None,
                render_alpha: 255,
                // ~31° angle of repose over a 0.25 m column width.
                repose_rise_m: 0.15,
            },
            MaterialId::LooseRock => MaterialProps {
                density: 2500,
                permeability: 40,
                erosion_resistance: 120,
                cohesion: 100,
                porosity: 25,
                phase_change: None,
                render_alpha: 255,
                // ~45° — cobbles interlock but a very steep talus
                // slope still gives way under load.
                repose_rise_m: 0.25,
            },
            MaterialId::Gravel => MaterialProps {
                density: 2000,
                permeability: 240,
                erosion_resistance: 60,
                cohesion: 40,
                porosity: 120,
                phase_change: None,
                render_alpha: 255,
                // ~40° — larger, more interlocking grains than sand.
                repose_rise_m: 0.20,
            },
            MaterialId::Clay => MaterialProps {
                density: 1900,
                permeability: 10,
                erosion_resistance: 80,
                cohesion: 180,
                porosity: 60,
                phase_change: None,
                render_alpha: 255,
                // Cohesive but slumps once saturated (which run_sediment
                // already reflects via the wet-erosion multiplier).
                repose_rise_m: 0.22,
            },
            MaterialId::Organic => MaterialProps {
                density: 600,
                permeability: 120,
                erosion_resistance: 60,
                cohesion: 100,
                porosity: 200,
                phase_change: None,
                render_alpha: 255,
                repose_rise_m: 0.10,
            },
            MaterialId::Water => MaterialProps {
                density: 1000,
                permeability: 0,
                erosion_resistance: 0,
                cohesion: 0,
                porosity: 0,
                // Water freezes into Ice below 0C. No above transition
                // (evaporation isn't a phase change here — it's handled
                // as mass leaving the world in run_evaporation).
                phase_change: Some(PhaseChange {
                    threshold_c: 0.0,
                    below: Some(MaterialId::Ice),
                    above: None,
                }),
                render_alpha: 180,
                // Fluids never "slump" in the granular sense —
                // surface-water flow already handles their lateral
                // spreading. Marked infinite so run_slumping ignores.
                repose_rise_m: f32::INFINITY,
            },
            MaterialId::Air => MaterialProps {
                density: 0,
                permeability: 0,
                erosion_resistance: 0,
                cohesion: 0,
                porosity: 0,
                phase_change: None,
                render_alpha: 0,
                repose_rise_m: f32::INFINITY,
            },
            MaterialId::Snow => MaterialProps {
                density: 900,
                permeability: 40,
                erosion_resistance: 10,
                cohesion: 30,
                porosity: 100,
                phase_change: Some(PhaseChange {
                    threshold_c: 0.0,
                    below: None,
                    above: Some(MaterialId::Water),
                }),
                render_alpha: 240,
                // Snow slides easily on steeper slopes (avalanches).
                repose_rise_m: 0.12,
            },
            MaterialId::Ice => MaterialProps {
                density: 917,
                permeability: 0,
                erosion_resistance: 120,
                cohesion: 200,
                porosity: 0,
                phase_change: Some(PhaseChange {
                    threshold_c: 0.0,
                    below: None,
                    above: Some(MaterialId::Water),
                }),
                render_alpha: 210,
                // Ice creeps like a glacier over long time scales; on
                // the sim's tick scale it's effectively rigid.
                repose_rise_m: f32::INFINITY,
            },
        }
    }

    /// Erosion susceptibility: lower resistance value = erodes faster.
    pub fn erosion_rank(material: MaterialId) -> u32 {
        Self::props(material).erosion_resistance
    }

    pub fn colour_rgb(material: MaterialId) -> [u8; 3] {
        match material {
            MaterialId::Bedrock => [0x2E, 0x2E, 0x34],
            MaterialId::Stone => [0x80, 0x80, 0x80],
            // Cobble grey with a warm tint — reads as darker than Stone.
            MaterialId::LooseRock => [0x66, 0x62, 0x60],
            // Mix of tan and grey (mixed-grain aggregate).
            MaterialId::Gravel => [0xB4, 0xA4, 0x80],
            MaterialId::Sand => [0xE8, 0xD6, 0x6B],
            MaterialId::Clay => [0x80, 0x40, 0x00],
            MaterialId::Organic => [0x00, 0xAA, 0x00],
            MaterialId::Water => [0x23, 0x64, 0xD2],
            MaterialId::Air => [0x87, 0xCE, 0xEB],
            MaterialId::Snow => [0xF6, 0xF8, 0xFF],
            MaterialId::Ice => [0xC7, 0xE0, 0xF2],
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
