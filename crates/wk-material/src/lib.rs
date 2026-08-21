//! Material vocabulary and property tables for World Kernel 0.1.

use serde::{Deserialize, Serialize};

/// Number of [`MaterialId`] variants (shared by both stacks).
pub const MATERIAL_COUNT: usize = 14;
/// Horizontal cell / column width in metres (shared scale).
pub const SAMPLE_WIDTH_M: f32 = 0.25;

// Column-stack layout constants live in `wk_world` / `wk_sim`
// (`crates/legacy/`). Voxel uses its own `CHUNK_CELLS_*` in `wk_voxel`.

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
    /// Waterlogged organic detritus / sapropel. Denser than water so
    /// dissolved creature corpses sink and build bed sediment. Terrain
    /// generation does not emit it as a free dry layer (that would wash
    /// off slopes); organism death is the live source.
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
    /// Soluble carbonate rock. High permeability + non-zero solubility
    /// drive karst cave formation (stage 7). Do not represent caves as
    /// Air layers — voids are sparse column annotations (see `Void`).
    Limestone = 11,
    /// Humified soil — long-term product of mycelium composting
    /// [`Organic`] litter. Slightly denser / less porous than fresh
    /// Organic (mild compaction) so pore water mostly survives the
    /// conversion; excess sat is pushed to neighbours.
    Soil = 12,
    /// Broken limestone rubble / carbonate talus. Fallable debris from
    /// [`Limestone`] roof collapse and face shear — distinct from
    /// [`LooseRock`] (silicate cobbles) so karst cliffs shed pale scree.
    LooseLimestone = 13,
}

impl MaterialId {
    /// Ground-forming solids (never fluid, never phase-changes at the
    /// world's normal temperature range).
    pub const ALL_SOLIDS: [MaterialId; 9] = [
        MaterialId::Bedrock,
        MaterialId::Stone,
        MaterialId::Limestone,
        MaterialId::LooseRock,
        MaterialId::LooseLimestone,
        MaterialId::Gravel,
        MaterialId::Sand,
        MaterialId::Clay,
        MaterialId::Soil,
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
            11 => Some(MaterialId::Limestone),
            12 => Some(MaterialId::Soil),
            13 => Some(MaterialId::LooseLimestone),
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
                | MaterialId::Limestone
                | MaterialId::LooseRock
                | MaterialId::LooseLimestone
                | MaterialId::Gravel
                | MaterialId::Sand
                | MaterialId::Clay
                | MaterialId::Soil
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
fn default_heat_capacity() -> f32 {
    1.0
}

fn default_albedo() -> f32 {
    0.25
}

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
    /// Thermal diffusivity in m²/s (game-tuned). Used by the thermal
    /// field solver; values are larger than real-rock diffusivities so
    /// heat redistributes on a playable timescale while remaining
    /// stable at 0.5 m cells with a 10-tick field step.
    pub thermal_diffusivity: f32,
    /// Relative volumetric heat capacity (rock ≈ 1). Water and organics
    /// are high so landscape / ponds lag climate snaps; air is low.
    /// Voxel `Temperature` uses this for thermal inertia (not every tick).
    #[serde(default = "default_heat_capacity")]
    pub heat_capacity: f32,
    /// Broadband surface albedo 0..1. Snow/ice reflect solar; water is dark.
    #[serde(default = "default_albedo")]
    pub albedo: f32,
    /// 0–255 mineral solubility in flowing water. Limestone is non-zero;
    /// everything else is 0. Karst dissolution (`run_karst`) is driven by
    /// lateral water flux through soluble layers, not moisture-in-place.
    pub solubility: u8,
    /// Maximum horizontal void span (metres) this material can roof
    /// before collapse. 0 = collapses immediately (sand/clay);
    /// `f32::INFINITY` = never collapses as a roof (bedrock).
    pub roof_span_max_m: f32,
}

pub struct MaterialRegistry;

/// Per-material hydrology tuning (permeability / porosity).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydroSlot {
    pub permeability: Option<u8>,
    pub porosity: Option<u8>,
}

/// Full material hydrology override table.
///
/// Canonical storage for the voxel stack is [`wk_voxel::World::hydro`]
/// (serialized with the sim). Hot paths pass this table into
/// [`MaterialRegistry::props_with`] / voxel `water_capacity_with` —
/// there is no process-global install step.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydroOverrides {
    pub slots: [HydroSlot; MATERIAL_COUNT],
}

impl Default for HydroOverrides {
    fn default() -> Self {
        Self {
            slots: [HydroSlot::default(); MATERIAL_COUNT],
        }
    }
}

impl HydroOverrides {
    pub fn set_permeability(&mut self, material: MaterialId, value: u8) {
        if let Some(slot) = self.slots.get_mut(material as usize) {
            slot.permeability = Some(value);
        }
    }

    pub fn set_porosity(&mut self, material: MaterialId, value: u8) {
        if let Some(slot) = self.slots.get_mut(material as usize) {
            slot.porosity = Some(value);
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn apply(self, material: MaterialId, mut p: MaterialProps) -> MaterialProps {
        if let Some(o) = self.slots.get(material as usize) {
            if let Some(v) = o.permeability {
                p.permeability = v;
            }
            if let Some(v) = o.porosity {
                p.porosity = v;
            }
        }
        p
    }
}

impl MaterialRegistry {
    /// Compile-time material table (no runtime hydrology overrides).
    ///
    /// For porosity / permeability with sim overrides, use
    /// [`Self::props_with`] and the world's [`HydroOverrides`].
    pub fn props(material: MaterialId) -> MaterialProps {
        Self::base_props(material)
    }

    /// Props with an explicit override table (voxel `World::hydro`).
    pub fn props_with(material: MaterialId, hydro: &HydroOverrides) -> MaterialProps {
        hydro.apply(material, Self::base_props(material))
    }

    /// Compile-time material table (ignores runtime overrides).
    pub fn base_props(material: MaterialId) -> MaterialProps {
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
                thermal_diffusivity: 0.001,
                heat_capacity: 7.0,
                albedo: 0.2,
                solubility: 0,
                roof_span_max_m: f32::INFINITY,
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
                thermal_diffusivity: 0.0012,
                heat_capacity: 5.5,
                albedo: 0.25,
                solubility: 0,
                roof_span_max_m: 15.0,
            },
            MaterialId::Sand => MaterialProps {
                density: 1600,
                // Keep sand open enough to wet, but not so open that a
                // painted lake sinks a freefall plume through the hill.
                permeability: 96,
                erosion_resistance: 30,
                cohesion: 20,
                porosity: 110,
                phase_change: None,
                render_alpha: 255,
                // ~31° angle of repose over a 0.25 m column width.
                repose_rise_m: 0.15,
                thermal_diffusivity: 0.0018,
                heat_capacity: 2.2,
                albedo: 0.35,
                solubility: 0,
                roof_span_max_m: 0.0,
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
                thermal_diffusivity: 0.0015,
                heat_capacity: 3.0,
                albedo: 0.3,
                solubility: 0,
                roof_span_max_m: 2.0,
            },
            MaterialId::LooseLimestone => MaterialProps {
                density: 2400,
                permeability: 50,
                erosion_resistance: 100,
                cohesion: 90,
                porosity: 30,
                phase_change: None,
                render_alpha: 255,
                // Same talus angle as LooseRock — pale carbonate scree
                // still stacks steeper than sand.
                repose_rise_m: 0.25,
                thermal_diffusivity: 0.0014,
                heat_capacity: 3.0,
                albedo: 0.32,
                solubility: 0,
                roof_span_max_m: 1.5,
            },
            MaterialId::Gravel => MaterialProps {
                density: 2000,
                permeability: 160,
                erosion_resistance: 60,
                cohesion: 40,
                porosity: 90,
                phase_change: None,
                render_alpha: 255,
                // ~40° — larger, more interlocking grains than sand.
                repose_rise_m: 0.20,
                thermal_diffusivity: 0.0018,
                heat_capacity: 2.5,
                albedo: 0.3,
                solubility: 0,
                roof_span_max_m: 0.5,
            },
            MaterialId::Clay => MaterialProps {
                density: 1900,
                permeability: 10,
                erosion_resistance: 80,
                cohesion: 180,
                porosity: 60,
                phase_change: None,
                render_alpha: 255,
                // Table value = dry powder (sand-like, max_step 0).
                // Plastic / mud behaviour is pore-wetness gated in
                // voxel `grain_repose_max_step` (semi-wet holds shape;
                // near-saturated flows as mud).
                repose_rise_m: 0.15,
                thermal_diffusivity: 0.0012,
                heat_capacity: 3.5,
                albedo: 0.22,
                solubility: 0,
                roof_span_max_m: 0.0,
            },
            MaterialId::Organic => MaterialProps {
                // > water so corpse ooze settles on the bed instead of
                // floating as a dry litter cap (the old density-600 behaviour).
                density: 1150,
                permeability: 120,
                erosion_resistance: 40,
                cohesion: 80,
                porosity: 200,
                phase_change: None,
                render_alpha: 255,
                repose_rise_m: 0.10,
                thermal_diffusivity: 0.002,
                heat_capacity: 3.5,
                albedo: 0.18,
                solubility: 0,
                roof_span_max_m: 0.0,
            },
            MaterialId::Soil => MaterialProps {
                // Humus-rich loam — holds water, conducts slowly so recharge
                // spreads instead of punching a vertical plume.
                density: 1350,
                permeability: 48,
                erosion_resistance: 55,
                cohesion: 110,
                porosity: 100,
                phase_change: None,
                render_alpha: 255,
                repose_rise_m: 0.14,
                thermal_diffusivity: 0.0015,
                heat_capacity: 3.5,
                albedo: 0.16,
                solubility: 0,
                roof_span_max_m: 0.0,
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
                thermal_diffusivity: 0.0015,
                heat_capacity: 10.0,
                albedo: 0.08,
                solubility: 0,
                roof_span_max_m: 0.0,
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
                thermal_diffusivity: 0.004,
                heat_capacity: 0.25,
                albedo: 0.0,
                solubility: 0,
                roof_span_max_m: 0.0,
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
                thermal_diffusivity: 0.001,
                heat_capacity: 2.0,
                albedo: 0.75,
                solubility: 0,
                roof_span_max_m: 0.0,
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
                thermal_diffusivity: 0.001,
                heat_capacity: 5.0,
                albedo: 0.55,
                solubility: 0,
                roof_span_max_m: 0.0,
            },
            MaterialId::Limestone => MaterialProps {
                density: 2500,
                // Much higher than stone — water infiltrates and flows
                // laterally through limestone, which is why karst forms.
                permeability: 140,
                erosion_resistance: 150,
                cohesion: 180,
                porosity: 40,
                phase_change: None,
                render_alpha: 255,
                repose_rise_m: f32::INFINITY,
                thermal_diffusivity: 0.0011,
                heat_capacity: 5.0,
                albedo: 0.28,
                solubility: 40,
                roof_span_max_m: 10.0,
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
            // Pale chalky rubble — between Limestone and LooseRock grey.
            MaterialId::LooseLimestone => [0xB0, 0xA8, 0x96],
            // Mix of tan and grey (mixed-grain aggregate).
            MaterialId::Gravel => [0xB4, 0xA4, 0x80],
            MaterialId::Sand => [0xE8, 0xD6, 0x6B],
            // Cool dusty tan — far from living Root sienna `#7A4B2A`
            // (was `#804000`, which read as the same brown underground).
            MaterialId::Clay => [0xB8, 0xA4, 0x90],
            // Dark olive mud — reads as bed ooze, not living green.
            MaterialId::Organic => [0x3A, 0x4A, 0x28],
            // Warm dark loam — distinct from Clay's dusty tan and Sand.
            MaterialId::Soil => [0x5A, 0x42, 0x2E],
            MaterialId::Water => [0x23, 0x64, 0xD2],
            MaterialId::Air => [0x87, 0xCE, 0xEB],
            MaterialId::Snow => [0xF6, 0xF8, 0xFF],
            MaterialId::Ice => [0xC7, 0xE0, 0xF2],
            // Warm pale grey — distinct from cooler Stone.
            MaterialId::Limestone => [0xC8, 0xC2, 0xB0],
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
