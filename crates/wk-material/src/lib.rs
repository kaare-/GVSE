//! Material vocabulary and property tables for World Kernel 0.1.

use serde::{Deserialize, Serialize};

/// Number of [`MaterialId`] variants (shared by both stacks).
pub const MATERIAL_COUNT: usize = 16;
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
    /// **Flowstone** — carbonate precipitated back out of groundwater
    /// (travertine / tufa / cave flowstone).
    ///
    /// Chemically the same family as [`Limestone`] but a *deposit*, not a bed:
    /// dense and tight where it forms, and worth telling apart on sight so
    /// spring mounds and sealed conduits are readable rather than looking like
    /// native rock. Still soluble, so it can redissolve.
    Flowstone = 14,
    /// **Bentonite** — swelling clay, effectively an aquitard.
    ///
    /// Mechanically a clay (plastic, reposes, slumps when wet), but roughly an
    /// order of magnitude tighter than [`Clay`] and holding almost everything
    /// it takes on. Its job is to *stop* water: a real confining layer is what
    /// makes a confined aquifer confined, and so what makes artesian head and
    /// perched tables happen by design instead of by accident.
    Bentonite = 15,
}

impl MaterialId {
    /// Ground-forming solids (never fluid, never phase-changes at the
    /// world's normal temperature range).
    pub const ALL_SOLIDS: [MaterialId; 11] = [
        MaterialId::Bedrock,
        MaterialId::Stone,
        MaterialId::Limestone,
        MaterialId::LooseRock,
        MaterialId::LooseLimestone,
        MaterialId::Flowstone,
        MaterialId::Gravel,
        MaterialId::Sand,
        MaterialId::Clay,
        MaterialId::Bentonite,
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
            14 => Some(MaterialId::Flowstone),
            15 => Some(MaterialId::Bentonite),
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
                | MaterialId::Flowstone
                | MaterialId::Gravel
                | MaterialId::Sand
                | MaterialId::Clay
                | MaterialId::Bentonite
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
    /// stone is a small hint for a future dissolved field. Voxel karst
    /// (`apply_karst_dissolution`) uses its own surface / pore / stone
    /// scales rather than reading this knob today.
    pub solubility: u8,
    /// Maximum horizontal void span (metres) this material can roof
    /// before collapse. 0 = collapses immediately (sand/clay);
    /// `f32::INFINITY` = never collapses as a roof (bedrock).
    pub roof_span_max_m: f32,
    /// **Field capacity** — the share of pore space (0–255, as a fraction of
    /// [`Self::porosity`]) held against gravity by capillary action.
    ///
    /// Only saturation *above* this drains downward; the rest stays put until
    /// roots or evaporation take it. This is the counterforce to gravity: with
    /// no retention the only stable state is a saturated wedge growing up from
    /// bedrock, because every cell eventually drains into the one below.
    ///
    /// It is also what makes a lens behave like its material — clay perches
    /// water, gravel lets it straight through — which is the visible difference
    /// per-cell pore variation was supposed to produce.
    #[serde(default = "default_field_capacity")]
    pub field_capacity: u8,
}

/// Mid-range retention for materials predating the field (~20%).
fn default_field_capacity() -> u8 {
    51
}

pub struct MaterialRegistry;

/// Inclusive `0..=255` material-property range. A cell's stored pore
/// coordinate selects one value inside the range.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydroRange {
    pub min: u8,
    pub max: u8,
}

impl HydroRange {
    pub const fn new(min: u8, max: u8) -> Self {
        if min <= max {
            Self { min, max }
        } else {
            Self { min: max, max: min }
        }
    }

    pub const fn fixed(value: u8) -> Self {
        Self {
            min: value,
            max: value,
        }
    }

    /// Select a value with `pore=0` at `min`, `255` at `max`.
    #[inline]
    pub fn sample(self, pore: u8) -> u8 {
        let span = self.max as u16 - self.min as u16;
        (self.min as u16 + (span * pore as u16 + 127) / 255) as u8
    }

    /// Fracture sampling: `min` is the **matrix** value, reached by the whole
    /// lower half of the pore domain; the upper half ramps quadratically to
    /// `max`.
    ///
    /// Used for permeability. Two properties matter:
    ///
    /// - `pore = 128` (the constructor default) returns exactly `min`, so
    ///   painted and constructed cells keep the authored matrix value. A linear
    ///   sample over an upward-widened range would silently make every default
    ///   cell far more permeable.
    /// - Most of the domain is matrix and only a thin tail is conductive, which
    ///   is how rock actually behaves — tight almost everywhere, with flow
    ///   concentrated in sparse fractures.
    #[inline]
    pub fn sample_fracture(self, pore: u8) -> u8 {
        if pore <= 128 || self.max <= self.min {
            return self.min;
        }
        // Remap 129..=255 onto 0..=255, then square to weight the tail.
        let t = ((pore as u32 - 128) * 255) / 127;
        let sq = (t * t) / 255;
        let span = (self.max - self.min) as u32;
        (self.min as u32 + (span * sq) / 255).min(self.max as u32) as u8
    }

    #[inline]
    pub fn midpoint(self) -> u8 {
        ((self.min as u16 + self.max as u16 + 1) / 2) as u8
    }
}

/// Per-material hydrology ranges selected by each voxel's pore coordinate.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct MaterialHydrology {
    pub permeability: HydroRange,
    pub porosity: HydroRange,
}

/// Permeability range: matrix at the floor, fractures in the tail.
///
/// Keeps the table value as the *minimum* so typical rock is no leakier than
/// before, and lifts the ceiling to roughly 8× (clamped) so the rare
/// high-`pore` cell conducts like a fracture. For stone that is rate 1 in the
/// matrix and ~5 in a fracture — the contrast a symmetric band could not reach.
const fn fracture_range(matrix: u8) -> HydroRange {
    if matrix == 0 {
        return HydroRange::fixed(0);
    }
    // Saturating ×8 without overflow, with a floor so very tight rock still
    // gets a usable spread rather than one rate bucket.
    let ceiling = if matrix > 31 { 255 } else { matrix * 8 };
    let ceiling = if ceiling < 40 { 40 } else { ceiling };
    HydroRange::new(matrix, ceiling)
}

const fn centered_range(mid: u8) -> HydroRange {
    if mid == 0 {
        return HydroRange::fixed(0);
    }
    // ±25%, with at least four points of texture for tight rock.
    let half = if mid / 4 > 4 { mid / 4 } else { 4 };
    HydroRange::new(mid.saturating_sub(half), mid.saturating_add(half))
}

/// Per-material hydrology tuning (permeability / porosity ranges).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct HydroSlot {
    pub permeability: Option<HydroRange>,
    pub porosity: Option<HydroRange>,
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
        self.set_permeability_range(material, value, value);
    }

    pub fn set_permeability_range(&mut self, material: MaterialId, min: u8, max: u8) {
        if let Some(slot) = self.slots.get_mut(material as usize) {
            slot.permeability = Some(HydroRange::new(min, max));
        }
    }

    pub fn set_porosity(&mut self, material: MaterialId, value: u8) {
        self.set_porosity_range(material, value, value);
    }

    pub fn set_porosity_range(&mut self, material: MaterialId, min: u8, max: u8) {
        if let Some(slot) = self.slots.get_mut(material as usize) {
            slot.porosity = Some(HydroRange::new(min, max));
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn apply(self, material: MaterialId, mut p: MaterialProps) -> MaterialProps {
        if let Some(o) = self.slots.get(material as usize) {
            if let Some(v) = o.permeability {
                p.permeability = v.midpoint();
            }
            if let Some(v) = o.porosity {
                p.porosity = v.midpoint();
            }
        }
        p
    }

    pub fn hydrology(self, material: MaterialId, base: MaterialHydrology) -> MaterialHydrology {
        let Some(slot) = self.slots.get(material as usize) else {
            return base;
        };
        MaterialHydrology {
            permeability: slot.permeability.unwrap_or(base.permeability),
            porosity: slot.porosity.unwrap_or(base.porosity),
        }
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

    /// Default per-cell hydrology ranges.
    ///
    /// **Porosity** stays a symmetric band around the table value: it controls
    /// storage, not which path water takes, and several tests assert bed
    /// saturation against it.
    ///
    /// **Permeability widens upward only** — floor at the table value, ceiling
    /// well above it. Real rock is not a mean with a ±25% spread; the matrix is
    /// tight and flow concentrates in a small fraction of much more conductive
    /// fractures. Combined with a heavy-tailed pore field (worldgen puts most
    /// cells near the low end) the median cell stays about as tight as before
    /// while the thin tail becomes a genuine conduit.
    ///
    /// A symmetric band could not do this: `seepage_rate` quantizes to
    /// `(permeability × 32) / 255` with a floor of 1, so stone's whole ±25%
    /// band (1–9) collapsed into a single rate bucket and deep rock had *no*
    /// usable variation at all.
    pub fn hydrology(material: MaterialId) -> MaterialHydrology {
        let p = Self::base_props(material);
        MaterialHydrology {
            permeability: fracture_range(p.permeability),
            porosity: centered_range(p.porosity),
        }
    }

    pub fn hydrology_with(material: MaterialId, hydro: &HydroOverrides) -> MaterialHydrology {
        (*hydro).hydrology(material, Self::hydrology(material))
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
                // no pore space to retain
                field_capacity: 0,
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
                // tight matrix holds a fifth of its little pore space
                field_capacity: 51,
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
                // drains freely — classic sandy soil ~20%
                field_capacity: 51,
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
                // coarse talus barely retains
                field_capacity: 38,
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
                // coarse carbonate scree, as LooseRock
                field_capacity: 38,
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
                // nearly free-draining; this is why a gravel lens conducts
                field_capacity: 20,
            },
            MaterialId::Bentonite => MaterialProps {
                density: 1800,
                // The whole point: ~10x tighter than clay. Clay at 10 against
                // limestone's 140 is only ~14x, which still equalises over a
                // geological cadence — not a seal.
                permeability: 1,
                erosion_resistance: 70,
                cohesion: 200,
                porosity: 65,
                phase_change: None,
                render_alpha: 255,
                // Plastic like clay: holds a face until wet, then slumps.
                repose_rise_m: 0.15,
                thermal_diffusivity: 0.0012,
                heat_capacity: 3.6,
                albedo: 0.20,
                // An aquitard that dissolves is not an aquitard.
                solubility: 0,
                roof_span_max_m: 0.0,
                // Swelling clay gives up almost nothing to gravity, which is
                // what perches a table on top of it.
                field_capacity: 232,
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
                // holds most of its water — perches a water table above it
                field_capacity: 188,
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
                // peat / sapropel is a sponge
                field_capacity: 166,
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
                // loam sits between sand and clay
                field_capacity: 128,
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
                // not a porous solid
                field_capacity: 0,
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
                // not a porous solid
                field_capacity: 0,
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
                // impermeable in v1 rules
                field_capacity: 0,
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
                // impermeable in v1 rules
                field_capacity: 0,
            },
            MaterialId::Flowstone => MaterialProps {
                density: 2600,
                // A deposit fills the space it forms in, so it is tighter than
                // bedded limestone — this is what seals a conduit.
                permeability: 12,
                erosion_resistance: 170,
                cohesion: 190,
                porosity: 12,
                phase_change: None,
                render_alpha: 255,
                repose_rise_m: f32::INFINITY,
                thermal_diffusivity: 0.0011,
                heat_capacity: 5.0,
                albedo: 0.34,
                // Still carbonate: it can dissolve again.
                solubility: 40,
                roof_span_max_m: 10.0,
                // Dense precipitate holds little, and lets little go.
                field_capacity: 30,
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
                // fractured carbonate drains through its conduits
                field_capacity: 38,
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
            // Ivory with a faint blue cast — reads as wet mineral crust and
            // separates cleanly from Limestone's warm grey at a glance.
            MaterialId::Flowstone => [0xEC, 0xEC, 0xDE],
            // Cool blue-green grey. Wants to be legible as a *band*, since
            // reading where the seal sits is the point of drawing it at all.
            MaterialId::Bentonite => [0x6E, 0x82, 0x7A],
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hydro_range_samples_endpoints_and_midpoint() {
        let r = HydroRange::new(10, 30);
        assert_eq!(r.sample(0), 10);
        assert_eq!(r.sample(128), 20);
        assert_eq!(r.sample(255), 30);
        assert_eq!(r.midpoint(), 20);
    }

    #[test]
    fn porosity_is_centred_but_permeability_only_widens_upward() {
        for material in MaterialId::ALL_SOLIDS {
            let props = MaterialRegistry::base_props(material);
            let hydro = MaterialRegistry::hydrology(material);
            // Storage stays centred on the table value.
            assert_eq!(
                hydro.porosity.sample(128),
                props.porosity,
                "{material:?} porosity should keep the table value at its midpoint"
            );
            // Conductivity keeps the table value as its *floor*: the matrix is
            // never leakier than before, and the tail reaches fracture rates.
            assert_eq!(
                hydro.permeability.min, props.permeability,
                "{material:?} matrix permeability must stay at the table value"
            );
            if props.permeability > 0 {
                assert!(
                    hydro.permeability.max > props.permeability,
                    "{material:?} needs headroom above the matrix for fractures"
                );
            } else {
                assert_eq!(
                    hydro.permeability,
                    HydroRange::fixed(0),
                    "impermeable stays exactly 0..0"
                );
            }
        }
    }

    #[test]
    fn fracture_tail_gives_tight_rock_a_usable_rate_spread() {
        // The bug this exists to prevent: seepage rate is
        // `((permeability * 32) / 255).max(1)`, so a narrow band around a low
        // permeability collapses to one bucket and rock cannot vary at all.
        let rate = |p: u8| ((p as i32 * 32) / 255).max(1);
        let stone = MaterialRegistry::hydrology(MaterialId::Stone).permeability;
        assert!(
            rate(stone.max) >= rate(stone.min) * 4,
            "stone needs a 4x+ matrix-to-fracture rate contrast (min {} -> {}, max {} -> {})",
            stone.min,
            rate(stone.min),
            stone.max,
            rate(stone.max)
        );
    }

    #[test]
    fn fracture_sampling_keeps_the_default_cell_at_matrix() {
        // `Cell::solid()` uses pore = 128. If that did not land exactly on the
        // matrix value, every painted and constructed cell would silently
        // change permeability when the range widened upward.
        for material in MaterialId::ALL_SOLIDS {
            let props = MaterialRegistry::base_props(material);
            let perm = MaterialRegistry::hydrology(material).permeability;
            assert_eq!(
                perm.sample_fracture(128),
                props.permeability,
                "{material:?} default pore must sample the matrix value"
            );
            assert_eq!(perm.sample_fracture(0), props.permeability);
            assert_eq!(perm.sample_fracture(255), perm.max);
        }
    }

    #[test]
    fn fracture_sampling_is_mostly_matrix() {
        // A thin conductive tail, not uniformly leaky rock.
        let perm = MaterialRegistry::hydrology(MaterialId::Stone).permeability;
        let matrix = perm.min;
        let at_matrix = (0..=255u16)
            .filter(|&p| perm.sample_fracture(p as u8) <= matrix + 1)
            .count();
        assert!(
            at_matrix > 150,
            "most of the pore domain should stay matrix-tight (got {at_matrix}/256)"
        );
    }

    #[test]
    fn zero_override_is_zero_to_zero() {
        let mut overrides = HydroOverrides::default();
        overrides.set_permeability(MaterialId::Sand, 0);
        let h = MaterialRegistry::hydrology_with(MaterialId::Sand, &overrides);
        assert_eq!(h.permeability, HydroRange::fixed(0));
    }

    #[test]
    fn erosion_order_sand_clay_stone() {
        let sand = MaterialRegistry::erosion_rank(MaterialId::Sand);
        let clay = MaterialRegistry::erosion_rank(MaterialId::Clay);
        let stone = MaterialRegistry::erosion_rank(MaterialId::Stone);
        assert!(sand < clay);
        assert!(clay < stone);
    }

    /// Every ground-forming solid must report [`MaterialId::is_solid`].
    ///
    /// It is not a label: `support_map` uses it to decide what holds weight,
    /// and solidity changes are what wake the competent body pass. A solid
    /// missing from the list silently supports nothing. Flowstone shipped that
    /// way — the `is_solid` arm was the one edit that did not land with it.
    #[test]
    fn every_all_solids_entry_reports_solid() {
        for m in MaterialId::ALL_SOLIDS {
            assert!(m.is_solid(), "{m:?} is in ALL_SOLIDS but not is_solid()");
        }
    }

    #[test]
    fn bentonite_is_a_tighter_seal_than_clay() {
        let clay = MaterialRegistry::base_props(MaterialId::Clay);
        let bent = MaterialRegistry::base_props(MaterialId::Bentonite);
        assert!(
            bent.permeability * 4 < clay.permeability,
            "bentonite must be far tighter than clay to confine an aquifer \
             (clay {}, bentonite {})",
            clay.permeability,
            bent.permeability
        );
        assert!(
            bent.field_capacity > clay.field_capacity,
            "swelling clay should hold more against gravity than clay"
        );
        // An aquitard that dissolves is not an aquitard.
        assert_eq!(bent.solubility, 0);
    }
}
