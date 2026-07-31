//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Creature blueprints (MS-Paint drawings) for `wk-voxel-app`.
//! Set A Atom + minimal Set D plant. Same `.gvsecrt` postcard shape as
//! column-GVSE so files can be shared later.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::aggregate::{body_plan_from, BodyPlan};
use crate::organism::ModuleId;

/// Postcard schema for `.gvsecrt`.
///
/// - **1** — modules (+ Wave K `traits`) + vestigial `Genome` field.
/// - **2** — modules with traits only; plant knobs live on pixels.
/// - **3** — `PixelTraits.host_leave_fraction` (Wave Y).
/// - **4** — `PixelTraits.attach_prefer` (Wave AB). Schema-1/2/3 still
///   load via [`Blueprint::from_bytes`] (defaults fill the new field).
pub const BLUEPRINT_SCHEMA_VERSION: u16 = 4;
pub const BLUEPRINT_DIR: &str = "blueprints";
pub const BLUEPRINT_EXT: &str = "gvsecrt";

/// Depth lane — Mid only for Set A voxel editor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[repr(u8)]
pub enum LaneId {
    Fore = 0,
    #[default]
    Mid = 1,
    Back = 2,
}

fn one() -> f32 {
    1.0
}
fn default_clone_fidelity_bias() -> f32 {
    0.9
}
fn default_reproduce_at_bias() -> f32 {
    0.85
}
fn default_buoyancy_bias() -> f32 {
    0.0
}

/// Per-pixel gene payload (Wave K). Every painted module carries these
/// scalars; aggregates form the [`BodyPlan`]. Wave M/O bind live physics
/// to those aggregates (upkeep, photo, drink, buoyancy, repro, plant knobs).
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct PixelTraits {
    #[serde(default = "one")]
    pub mass: f32,
    #[serde(default = "one")]
    pub density: f32,
    #[serde(default = "one")]
    pub stiffness: f32,
    #[serde(default = "one")]
    pub strength: f32,
    #[serde(default = "one")]
    pub upkeep_bias: f32,
    #[serde(default = "one")]
    pub absorb_bias: f32,
    #[serde(default = "one")]
    pub drink_bias: f32,
    /// Matches vestigial [`Genome::clone_fidelity`] default.
    #[serde(default = "default_clone_fidelity_bias")]
    pub clone_fidelity_bias: f32,
    /// Matches vestigial [`Genome::reproduce_at`] default.
    #[serde(default = "default_reproduce_at_bias")]
    pub reproduce_at_bias: f32,
    /// Matches vestigial [`Genome::buoyancy_bias`] default (floater).
    #[serde(default = "default_buoyancy_bias")]
    pub buoyancy_bias: f32,
    /// Nucleus habit — surplus toward stem (Wave O).
    #[serde(default = "default_alloc_stem")]
    pub alloc_stem: f32,
    #[serde(default = "default_alloc_leaf")]
    pub alloc_leaf: f32,
    #[serde(default = "default_alloc_root")]
    pub alloc_root: f32,
    /// Root dive lean (Wave O).
    #[serde(default = "default_root_depth_bias")]
    pub root_depth_bias: f32,
    /// Photosystem dim-light harvest lean (Wave O).
    #[serde(default = "default_shade_efficiency")]
    pub shade_efficiency: f32,
    /// Digest / Hypha litter rate (Wave O).
    #[serde(default = "default_digest_rate")]
    pub digest_rate: f32,
    /// Fraction of light left for modules below (Wave Y epiphyte smother).
    /// 0 = smotherer; 1 = fully gentle rider.
    #[serde(default = "default_host_leave_fraction")]
    pub host_leave_fraction: f32,
    /// Holdfast seating bias (Wave AB). 0 = no re-seek; 1 = seek ~5 cells.
    #[serde(default = "default_attach_prefer")]
    pub attach_prefer: f32,
}

impl Default for PixelTraits {
    fn default() -> Self {
        Self {
            mass: 1.0,
            density: 1.0,
            stiffness: 1.0,
            strength: 1.0,
            upkeep_bias: 1.0,
            absorb_bias: 1.0,
            drink_bias: 1.0,
            clone_fidelity_bias: default_clone_fidelity_bias(),
            reproduce_at_bias: default_reproduce_at_bias(),
            buoyancy_bias: default_buoyancy_bias(),
            alloc_stem: default_alloc_stem(),
            alloc_leaf: default_alloc_leaf(),
            alloc_root: default_alloc_root(),
            root_depth_bias: default_root_depth_bias(),
            shade_efficiency: default_shade_efficiency(),
            digest_rate: default_digest_rate(),
            host_leave_fraction: default_host_leave_fraction(),
            attach_prefer: default_attach_prefer(),
        }
    }
}

/// Scale relative to a trait's paint-default. `1.0` when `value == default`.
fn trait_scale(value: f32, default: f32) -> f32 {
    if default <= 1e-6 {
        return 1.0;
    }
    (value / default).clamp(0.65, 1.45)
}

fn mul_channel(c: u8, factor: f32) -> u8 {
    ((c as f32) * factor).round().clamp(0.0, 255.0) as u8
}

/// Wave P: tint frozen [`ModuleId`] RGB from local traits.
///
/// Palette hex stays the identity ([`ModuleId::rgb`]); this only shifts
/// brightness / coolness for draw. **Default traits → identical RGB.**
pub fn modulate_module_rgb(module: ModuleId, traits: &PixelTraits) -> (u8, u8, u8) {
    let (r, g, b) = module.rgb();
    match module {
        ModuleId::Nucleus => (r, g, b), // keep #000000 identity
        ModuleId::Photosystem => {
            let bright = trait_scale(traits.absorb_bias, 1.0);
            let cool = trait_scale(traits.shade_efficiency, default_shade_efficiency());
            (
                mul_channel(r, 0.92 * bright + 0.08),
                mul_channel(g, bright),
                mul_channel(b, bright * (0.85 + 0.15 * cool)),
            )
        }
        ModuleId::Root => {
            let moist = trait_scale(traits.drink_bias, 1.0);
            let deep = trait_scale(traits.root_depth_bias, default_root_depth_bias());
            // Deeper roots read darker; drinky roots a touch richer.
            let darken = 1.15 - 0.15 * deep;
            (
                mul_channel(r, moist * darken),
                mul_channel(g, moist * darken * 0.98 + 0.02),
                mul_channel(b, moist * darken * 0.95 + 0.05),
            )
        }
        ModuleId::Stem | ModuleId::Holdfast => {
            let mass = trait_scale(traits.mass * traits.density, 1.0);
            let darken = 1.12 - 0.12 * mass;
            (
                mul_channel(r, darken),
                mul_channel(g, darken),
                mul_channel(b, darken),
            )
        }
        ModuleId::Digest => {
            let rate = trait_scale(traits.digest_rate, default_digest_rate());
            (
                mul_channel(r, rate),
                mul_channel(g, 0.9 * rate + 0.1),
                mul_channel(b, 0.9 * rate + 0.1),
            )
        }
        ModuleId::Hypha => {
            let rate = trait_scale(traits.digest_rate, default_digest_rate());
            let dens = trait_scale(traits.density, 1.0);
            let darken = 1.1 - 0.1 * dens;
            (
                mul_channel(r, rate * darken),
                mul_channel(g, rate * darken),
                mul_channel(b, rate * darken),
            )
        }
        ModuleId::Bone => {
            let dens = trait_scale(traits.density, 1.0);
            let stiff = trait_scale(traits.stiffness, 1.0);
            let darken = 1.12 - 0.12 * dens;
            (
                mul_channel(r, darken),
                mul_channel(g, darken * (0.96 + 0.04 * stiff)),
                mul_channel(b, darken * (0.92 + 0.08 * stiff)),
            )
        }
        ModuleId::Muscle => {
            let str = trait_scale(traits.strength, 1.0);
            let dens = trait_scale(traits.density, 1.0);
            let darken = 1.1 - 0.1 * dens;
            (
                mul_channel(r, str * darken),
                mul_channel(g, (0.85 * str + 0.15) * darken),
                mul_channel(b, (0.85 * str + 0.15) * darken),
            )
        }
        ModuleId::Skin => {
            let dens = trait_scale(traits.density, 1.0);
            let darken = 1.1 - 0.1 * dens;
            (
                mul_channel(r, darken),
                mul_channel(g, darken),
                mul_channel(b, darken),
            )
        }
    }
}

/// One painted module cell + its gene traits.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedModule {
    pub x: i16,
    pub y: i16,
    pub lane: LaneId,
    pub module: ModuleId,
    /// Per-pixel traits. Old postcard blobs omit this; serde defaults apply.
    #[serde(default)]
    pub traits: PixelTraits,
}

impl PlacedModule {
    pub fn new(x: i16, y: i16, module: ModuleId) -> Self {
        Self {
            x,
            y,
            lane: LaneId::Mid,
            module,
            traits: PixelTraits::default(),
        }
    }
}

/// Tab paint DTO for organism-wide plant knobs (not stored on `.gvsecrt`).
///
/// Wave T: [`Blueprint`] no longer carries a genome field. Studio Tab and
/// spawn / Apply Genes still build this bag and route through
/// [`crate::plant::apply_genome`] / [`paint_genome_onto_modules`] onto
/// [`PixelTraits`] / [`BodyPlan`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Genome {
    pub metabolic_rate: f32,
    pub reproduce_at: f32,
    pub clone_fidelity: f32,
    /// 0 = floater, 1 = sinker — mutated on fission.
    #[serde(default)]
    pub buoyancy_bias: f32,
    /// Deep-dive bias for root elongation (0 = shallow, 1 = dive).
    #[serde(default = "default_root_depth_bias")]
    pub root_depth_bias: f32,
    /// Surplus allocation toward stem / leaf / root (normalized at use).
    #[serde(default = "default_alloc_stem")]
    pub alloc_stem: f32,
    #[serde(default = "default_alloc_leaf")]
    pub alloc_leaf: f32,
    #[serde(default = "default_alloc_root")]
    pub alloc_root: f32,
    /// How hard Photosystems shade neighbours / self-stack (D2).
    #[serde(default = "default_leaf_absorb")]
    pub leaf_absorb: f32,
    /// Dim-light harvest lean (D2).
    #[serde(default = "default_shade_efficiency")]
    pub shade_efficiency: f32,
    /// Litter digest rate (Set E fungi).
    #[serde(default = "default_digest_rate")]
    pub digest_rate: f32,
    /// Light left for host / modules below (Wave Y). Epiphyte smother gene.
    #[serde(default = "default_host_leave_fraction")]
    pub host_leave_fraction: f32,
    /// Holdfast seating bias (Wave AB). Epiphyte re-seek gene.
    #[serde(default = "default_attach_prefer")]
    pub attach_prefer: f32,
}

fn default_root_depth_bias() -> f32 {
    0.55
}
fn default_alloc_stem() -> f32 {
    0.25
}
fn default_alloc_leaf() -> f32 {
    0.45
}
fn default_alloc_root() -> f32 {
    0.30
}
fn default_leaf_absorb() -> f32 {
    0.45
}
fn default_shade_efficiency() -> f32 {
    0.40
}
fn default_digest_rate() -> f32 {
    0.8
}
fn default_host_leave_fraction() -> f32 {
    0.0
}
fn default_attach_prefer() -> f32 {
    0.0
}

impl Default for Genome {
    fn default() -> Self {
        Self {
            metabolic_rate: 1.0,
            reproduce_at: 0.85,
            clone_fidelity: 0.9,
            buoyancy_bias: 0.0,
            root_depth_bias: default_root_depth_bias(),
            alloc_stem: default_alloc_stem(),
            alloc_leaf: default_alloc_leaf(),
            alloc_root: default_alloc_root(),
            leaf_absorb: default_leaf_absorb(),
            shade_efficiency: default_shade_efficiency(),
            digest_rate: default_digest_rate(),
            host_leave_fraction: default_host_leave_fraction(),
            attach_prefer: default_attach_prefer(),
        }
    }
}

/// Mutation strength scale (column `MUTATION_SIGMA`).
const MUTATION_SIGMA: f32 = 0.12;

/// Related kinds a pixel may become under Wave Q kind-swap.
///
/// Empty = never swapped (Nucleus). Pairs stay within a tissue family so
/// plants don't casually sprout Digest, etc.
pub fn kind_swap_partners(module: ModuleId) -> &'static [ModuleId] {
    match module {
        ModuleId::Nucleus | ModuleId::Holdfast => &[],
        ModuleId::Photosystem => &[ModuleId::Stem, ModuleId::Skin],
        ModuleId::Stem => &[ModuleId::Photosystem, ModuleId::Root],
        ModuleId::Root => &[ModuleId::Stem],
        ModuleId::Digest => &[ModuleId::Hypha],
        ModuleId::Hypha => &[ModuleId::Digest],
        ModuleId::Bone => &[ModuleId::Muscle],
        ModuleId::Muscle => &[ModuleId::Bone, ModuleId::Skin],
        ModuleId::Skin => &[ModuleId::Muscle, ModuleId::Photosystem],
    }
}

impl Genome {
    /// Normalized `(stem, leaf, root)` surplus weights (sum to 1).
    pub fn alloc_weights(self) -> (f32, f32, f32) {
        let s = self.alloc_stem.max(0.0);
        let l = self.alloc_leaf.max(0.0);
        let r = self.alloc_root.max(0.0);
        let sum = (s + l + r).max(1e-6);
        (s / sum, l / sum, r / sum)
    }
}

/// Paint one module's traits from a Tab / schema-1 [`Genome`] bag.
pub fn paint_genome_onto_traits(module: ModuleId, traits: &mut PixelTraits, genome: &Genome) {
    match module {
        ModuleId::Nucleus => {
            traits.alloc_stem = genome.alloc_stem.clamp(0.0, 1.0);
            traits.alloc_leaf = genome.alloc_leaf.clamp(0.0, 1.0);
            traits.alloc_root = genome.alloc_root.clamp(0.0, 1.0);
            traits.clone_fidelity_bias = genome.clone_fidelity.clamp(0.05, 1.0);
            traits.reproduce_at_bias = genome.reproduce_at.clamp(0.05, 0.99);
            traits.buoyancy_bias = genome.buoyancy_bias.clamp(0.0, 1.0);
        }
        ModuleId::Root => {
            traits.root_depth_bias = genome.root_depth_bias.clamp(0.0, 1.0);
        }
        ModuleId::Photosystem => {
            traits.shade_efficiency = genome.shade_efficiency.clamp(0.0, 1.0);
            traits.absorb_bias = genome.leaf_absorb.clamp(0.05, 1.0);
            traits.host_leave_fraction = genome.host_leave_fraction.clamp(0.0, 1.0);
        }
        ModuleId::Holdfast => {
            traits.attach_prefer = genome.attach_prefer.clamp(0.0, 1.0);
        }
        ModuleId::Digest | ModuleId::Hypha => {
            traits.digest_rate = genome.digest_rate.clamp(0.05, 2.0);
        }
        _ => {}
    }
}

/// Paint Tab / schema-1 [`Genome`] knobs onto kinded module traits.
pub fn paint_genome_onto_modules(modules: &mut [PlacedModule], genome: Genome) {
    for m in modules.iter_mut() {
        paint_genome_onto_traits(m.module, &mut m.traits, &genome);
    }
}

fn hash_u64(seed: u64, a: u64, b: u64, salt: u64) -> u64 {
    let mut x = seed
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(a)
        .wrapping_add(b)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x = x.wrapping_mul(0x94D0_49BB_1331_11EB);
    x ^= x >> 31;
    x
}

/// Voxel-local creature blueprint.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blueprint {
    pub schema_version: u16,
    pub canvas_w: u16,
    pub canvas_h: u16,
    pub modules: Vec<PlacedModule>,
    pub name: String,
    pub notes: String,
}

impl Blueprint {
    pub fn atom() -> Self {
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedModule {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 1,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: PixelTraits::default(),
                    },
            ],
            name: "atom".into(),
            notes: "Set A Atom".into(),
        }
    }

    /// Minimal land plant C (`docs/organism/PLANTS.md`): root crown,
    /// two stem, two leaves.
    pub fn minimal_plant() -> Self {
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedModule {
                    x: 8,
                    y: 4,
                    lane: LaneId::Mid,
                    module: ModuleId::Root,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 8,
                    y: 5,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 8,
                    y: 6,
                    lane: LaneId::Mid,
                    module: ModuleId::Stem,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 8,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Stem,
                    traits: PixelTraits::default(),
                    },
                // Leaves sit beside the upper stem so the tip column stays
                // free for olive elongation (no leaf→stem→leaf tower).
                PlacedModule {
                    x: 7,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 9,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: PixelTraits::default(),
                    },
            ],
            name: "plant".into(),
            notes: "Set D minimal land plant".into(),
        }
    }

    /// Minimal litter fungus (E): nucleus + digest + a short hypha thread.
    pub fn minimal_fungus() -> Self {
        let digest_traits = PixelTraits {
            digest_rate: 1.0,
            ..PixelTraits::default()
        };
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedModule {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 1,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Digest,
                    traits: digest_traits,
                    },
                PlacedModule {
                    x: 2,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Hypha,
                    traits: digest_traits,
                    },
                PlacedModule {
                    x: 3,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Hypha,
                    traits: digest_traits,
                    },
            ],
            name: "fungus".into(),
            notes: "Set E litter fungus".into(),
        }
    }

    /// Minimal epiphyte (E2): pink Holdfast + nucleus + leaf (no Root).
    ///
    /// Seat the Holdfast on a host Stem cell at spawn; relative offsets
    /// place Holdfast at nucleus, leaf above.
    pub fn minimal_epiphyte() -> Self {
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedModule {
                    x: 8,
                    y: 5,
                    lane: LaneId::Mid,
                    module: ModuleId::Holdfast,
                    traits: PixelTraits::default(),
                },
                PlacedModule {
                    x: 8,
                    y: 5,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                    traits: PixelTraits::default(),
                },
                PlacedModule {
                    x: 8,
                    y: 6,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: PixelTraits::default(),
                },
            ],
            name: "epiphyte".into(),
            notes: "Set E2 epiphyte — Holdfast on host Stem".into(),
        }
    }

    pub fn nucleus_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Nucleus)
            .count()
    }

    pub fn photosystem_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Photosystem)
            .count()
    }

    pub fn root_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Root)
            .count()
    }

    pub fn digest_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Digest)
            .count()
    }

    pub fn holdfast_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Holdfast)
            .count()
    }

    pub fn is_valid_atom(&self) -> bool {
        self.nucleus_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() == 0
            && self.digest_count() == 0
            && self.holdfast_count() == 0
    }

    pub fn is_valid_plant(&self) -> bool {
        self.nucleus_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() >= 1
    }

    /// Nucleus + Digest, no root/stem/holdfast (detritus habit).
    pub fn is_valid_fungus(&self) -> bool {
        self.nucleus_count() >= 1
            && self.digest_count() >= 1
            && !self.modules.iter().any(|m| {
                matches!(
                    m.module,
                    ModuleId::Root | ModuleId::Stem | ModuleId::Holdfast
                )
            })
    }

    /// Nucleus + Holdfast + Photosystem, no Root (canopy freeloader).
    pub fn is_valid_epiphyte(&self) -> bool {
        self.nucleus_count() >= 1
            && self.holdfast_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() == 0
    }

    pub fn is_valid_creature(&self) -> bool {
        self.is_valid_atom()
            || self.is_valid_plant()
            || self.is_valid_fungus()
            || self.is_valid_epiphyte()
    }

    /// Editor spawn gate: any painted body with a Nucleus (habit rules are
    /// for thriving, not for placement).
    pub fn can_editor_spawn(&self) -> bool {
        self.nucleus_count() >= 1 && !self.modules.is_empty()
    }

    /// Anchor = first Nucleus (canvas coords).
    pub fn nucleus_origin(&self) -> Option<(i16, i16)> {
        self.modules
            .iter()
            .find(|m| m.module == ModuleId::Nucleus)
            .map(|m| (m.x, m.y))
    }

    /// Modules as offsets from the nucleus (for world placement).
    pub fn modules_relative_to_nucleus(&self) -> Vec<(i16, i16, ModuleId)> {
        self.modules_relative_with_traits().0
    }

    /// Body offsets + per-pixel traits for live spawn (Wave M).
    pub fn modules_relative_with_traits(
        &self,
    ) -> (Vec<(i16, i16, ModuleId)>, Vec<PixelTraits>) {
        let Some((ox, oy)) = self.nucleus_origin() else {
            return (Vec::new(), Vec::new());
        };
        let mut body = Vec::with_capacity(self.modules.len());
        let mut traits = Vec::with_capacity(self.modules.len());
        for m in &self.modules {
            body.push((m.x - ox, m.y - oy, m.module));
            traits.push(m.traits);
        }
        (body, traits)
    }

    /// Aggregate body plan from painted pixel genes.
    pub fn body_plan(&self) -> BodyPlan {
        body_plan_from(&self.modules)
    }

    /// Anchor for placement: first Nucleus, else bounding-box centre.
    pub fn anchor_origin(&self) -> Option<(i16, i16)> {
        if let Some(o) = self.nucleus_origin() {
            return Some(o);
        }
        if self.modules.is_empty() {
            return None;
        }
        let min_x = self.modules.iter().map(|m| m.x).min().unwrap();
        let max_x = self.modules.iter().map(|m| m.x).max().unwrap();
        let min_y = self.modules.iter().map(|m| m.y).min().unwrap();
        let max_y = self.modules.iter().map(|m| m.y).max().unwrap();
        Some(((min_x + max_x) / 2, (min_y + max_y) / 2))
    }

    /// Deterministic child blueprint: jitter traits, rare chain-grow /
    /// kind-swap / delete. Budget and sigma scale with aggregate
    /// `clone_fidelity`.
    ///
    pub fn mutate_child(&self, world_seed: u64, tick: u64, parent_id: u32) -> Blueprint {
        let plan = self.body_plan();
        let fidelity = plan.clone_fidelity.clamp(0.05, 1.0);
        let strength = (1.0 - fidelity) * MUTATION_SIGMA;
        let salt_base = tick
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(parent_id as u64);

        let mut child = self.clone();
        let mut trait_i = 0u64;
        {
            let mut jitter = |value: f32, lo: f32, hi: f32| -> f32 {
                trait_i += 1;
                let h = hash_u64(world_seed, salt_base, trait_i, 0xC0DE);
                let u = (h as f32 / u64::MAX as f32) * 2.0 - 1.0;
                (value + u * strength * (hi - lo).max(0.1)).clamp(lo, hi)
            };
            for m in &mut child.modules {
                let t = &mut m.traits;
                t.mass = jitter(t.mass, 0.05, 4.0);
                t.density = jitter(t.density, 0.05, 4.0);
                t.stiffness = jitter(t.stiffness, 0.05, 4.0);
                t.strength = jitter(t.strength, 0.05, 4.0);
                t.upkeep_bias = jitter(t.upkeep_bias, 0.05, 4.0);
                t.absorb_bias = jitter(t.absorb_bias, 0.05, 4.0);
                t.drink_bias = jitter(t.drink_bias, 0.05, 4.0);
                t.clone_fidelity_bias = jitter(t.clone_fidelity_bias, 0.05, 1.0);
                t.reproduce_at_bias = jitter(t.reproduce_at_bias, 0.05, 1.0);
                t.buoyancy_bias = jitter(t.buoyancy_bias, 0.0, 1.0);
                t.alloc_stem = jitter(t.alloc_stem, 0.0, 1.0);
                t.alloc_leaf = jitter(t.alloc_leaf, 0.0, 1.0);
                t.alloc_root = jitter(t.alloc_root, 0.0, 1.0);
                t.root_depth_bias = jitter(t.root_depth_bias, 0.0, 1.0);
                t.shade_efficiency = jitter(t.shade_efficiency, 0.0, 1.0);
                t.digest_rate = jitter(t.digest_rate, 0.05, 2.0);
                t.host_leave_fraction = jitter(t.host_leave_fraction, 0.0, 1.0);
                t.attach_prefer = jitter(t.attach_prefer, 0.0, 1.0);
            }
        }

        // Structural ops share the same deterministic stream as trait jitter.
        let mut u01 = || {
            trait_i += 1;
            let h = hash_u64(world_seed, salt_base, trait_i, 0xC0DE);
            h as f32 / u64::MAX as f32
        };

        // Chain-grow: chance rises as fidelity falls.
        let grow_p = (1.0 - fidelity) * 0.55;
        if !child.modules.is_empty() && u01() < grow_p {
            let idx = (u01() * child.modules.len() as f32) as usize % child.modules.len();
            let src = child.modules[idx].clone();
            let dirs = [(1i16, 0i16), (-1, 0), (0, 1), (0, -1)];
            let di = (u01() * 4.0) as usize % 4;
            let (dx, dy) = dirs[di];
            let nx = src.x + dx;
            let ny = src.y + dy;
            let occupied = child
                .modules
                .iter()
                .any(|m| m.x == nx && m.y == ny && m.lane == src.lane);
            let in_bounds = nx >= 0
                && ny >= 0
                && (nx as u16) < child.canvas_w
                && (ny as u16) < child.canvas_h;
            if !occupied && in_bounds {
                let mut grown = src;
                grown.x = nx;
                grown.y = ny;
                let um = u01() * 2.0 - 1.0;
                let ud = u01() * 2.0 - 1.0;
                grown.traits.mass =
                    (grown.traits.mass + um * strength * 3.95).clamp(0.05, 4.0);
                grown.traits.density =
                    (grown.traits.density + ud * strength * 3.95).clamp(0.05, 4.0);
                child.modules.push(grown);
            }
        }

        // Kind-swap (Wave Q): rarer than grow, never the last Nucleus.
        let swap_p = (1.0 - fidelity) * 0.22;
        if !child.modules.is_empty() && u01() < swap_p {
            let nucleus_count = child
                .modules
                .iter()
                .filter(|m| m.module == ModuleId::Nucleus)
                .count();
            let candidates: Vec<usize> = (0..child.modules.len())
                .filter(|&i| {
                    let m = child.modules[i].module;
                    if m == ModuleId::Nucleus && nucleus_count <= 1 {
                        return false;
                    }
                    !kind_swap_partners(m).is_empty()
                })
                .collect();
            if !candidates.is_empty() {
                let pick = (u01() * candidates.len() as f32) as usize % candidates.len();
                let idx = candidates[pick];
                let partners = kind_swap_partners(child.modules[idx].module);
                if !partners.is_empty() {
                    let pi = (u01() * partners.len() as f32) as usize % partners.len();
                    child.modules[idx].module = partners[pi];
                }
            }
        }

        // Delete: rare, never the last Nucleus.
        let delete_p = (1.0 - fidelity) * 0.12;
        if child.modules.len() > 1 && u01() < delete_p {
            let nucleus_count = child
                .modules
                .iter()
                .filter(|m| m.module == ModuleId::Nucleus)
                .count();
            let candidates: Vec<usize> = (0..child.modules.len())
                .filter(|i| {
                    child.modules[*i].module != ModuleId::Nucleus || nucleus_count > 1
                })
                .collect();
            if !candidates.is_empty() {
                let pick = (u01() * candidates.len() as f32) as usize % candidates.len();
                child.modules.remove(candidates[pick]);
            }
        }

        child.name = format!("{}-child", self.name);
        child.notes = format!("mutated from {} @ tick {tick}", self.name);
        child.schema_version = BLUEPRINT_SCHEMA_VERSION;
        child
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| e.to_string())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        // Schema 4+: modules with traits including attach_prefer.
        if let Ok(bp) = postcard::from_bytes::<Blueprint>(bytes) {
            if bp.schema_version >= 4 {
                if bp.schema_version > BLUEPRINT_SCHEMA_VERSION {
                    return Err(format!(
                        "blueprint schema {} newer than supported {}",
                        bp.schema_version, BLUEPRINT_SCHEMA_VERSION
                    ));
                }
                return Ok(bp);
            }
            // schema_version < 4 under the v4 layout is a mis-parse — fall through.
        }

        // Schema 3: PixelTraits with host_leave_fraction but no attach_prefer.
        #[derive(Deserialize)]
        struct PixelTraitsV3 {
            mass: f32,
            density: f32,
            stiffness: f32,
            strength: f32,
            upkeep_bias: f32,
            absorb_bias: f32,
            drink_bias: f32,
            clone_fidelity_bias: f32,
            reproduce_at_bias: f32,
            buoyancy_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            root_depth_bias: f32,
            shade_efficiency: f32,
            digest_rate: f32,
            host_leave_fraction: f32,
        }
        #[derive(Deserialize)]
        struct PlacedModuleV3 {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
            traits: PixelTraitsV3,
        }
        #[derive(Deserialize)]
        struct BlueprintV3 {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<PlacedModuleV3>,
            name: String,
            notes: String,
        }
        if let Ok(old) = postcard::from_bytes::<BlueprintV3>(bytes) {
            if old.schema_version == 3 {
                let modules = old
                    .modules
                    .into_iter()
                    .map(|m| {
                        let t = m.traits;
                        PlacedModule {
                            x: m.x,
                            y: m.y,
                            lane: m.lane,
                            module: m.module,
                            traits: PixelTraits {
                                mass: t.mass,
                                density: t.density,
                                stiffness: t.stiffness,
                                strength: t.strength,
                                upkeep_bias: t.upkeep_bias,
                                absorb_bias: t.absorb_bias,
                                drink_bias: t.drink_bias,
                                clone_fidelity_bias: t.clone_fidelity_bias,
                                reproduce_at_bias: t.reproduce_at_bias,
                                buoyancy_bias: t.buoyancy_bias,
                                alloc_stem: t.alloc_stem,
                                alloc_leaf: t.alloc_leaf,
                                alloc_root: t.alloc_root,
                                root_depth_bias: t.root_depth_bias,
                                shade_efficiency: t.shade_efficiency,
                                digest_rate: t.digest_rate,
                                host_leave_fraction: t.host_leave_fraction,
                                attach_prefer: default_attach_prefer(),
                            },
                        }
                    })
                    .collect();
                return Ok(Blueprint {
                    schema_version: BLUEPRINT_SCHEMA_VERSION,
                    canvas_w: old.canvas_w,
                    canvas_h: old.canvas_h,
                    modules,
                    name: old.name,
                    notes: old.notes,
                });
            }
        }

        // Schema 2: PixelTraits without host_leave_fraction (Wave T–X).
        #[derive(Deserialize)]
        struct PixelTraitsV2 {
            mass: f32,
            density: f32,
            stiffness: f32,
            strength: f32,
            upkeep_bias: f32,
            absorb_bias: f32,
            drink_bias: f32,
            clone_fidelity_bias: f32,
            reproduce_at_bias: f32,
            buoyancy_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            root_depth_bias: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        #[derive(Deserialize)]
        struct PlacedModuleV2 {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
            traits: PixelTraitsV2,
        }
        #[derive(Deserialize)]
        struct BlueprintV2 {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<PlacedModuleV2>,
            name: String,
            notes: String,
        }
        if let Ok(old) = postcard::from_bytes::<BlueprintV2>(bytes) {
            if old.schema_version == 2 {
                let modules = old
                    .modules
                    .into_iter()
                    .map(|m| {
                        let t = m.traits;
                        PlacedModule {
                            x: m.x,
                            y: m.y,
                            lane: m.lane,
                            module: m.module,
                            traits: PixelTraits {
                                mass: t.mass,
                                density: t.density,
                                stiffness: t.stiffness,
                                strength: t.strength,
                                upkeep_bias: t.upkeep_bias,
                                absorb_bias: t.absorb_bias,
                                drink_bias: t.drink_bias,
                                clone_fidelity_bias: t.clone_fidelity_bias,
                                reproduce_at_bias: t.reproduce_at_bias,
                                buoyancy_bias: t.buoyancy_bias,
                                alloc_stem: t.alloc_stem,
                                alloc_leaf: t.alloc_leaf,
                                alloc_root: t.alloc_root,
                                root_depth_bias: t.root_depth_bias,
                                shade_efficiency: t.shade_efficiency,
                                digest_rate: t.digest_rate,
                                host_leave_fraction: default_host_leave_fraction(),
                                attach_prefer: default_attach_prefer(),
                            },
                        }
                    })
                    .collect();
                return Ok(Blueprint {
                    schema_version: BLUEPRINT_SCHEMA_VERSION,
                    canvas_w: old.canvas_w,
                    canvas_h: old.canvas_h,
                    modules,
                    name: old.name,
                    notes: old.notes,
                });
            }
        }

        // Schema 1: modules with traits + vestigial Genome (Wave K–S).
        // Genome shape predates host_leave_fraction / attach_prefer.
        #[derive(Deserialize)]
        struct GenomeV1 {
            metabolic_rate: f32,
            reproduce_at: f32,
            clone_fidelity: f32,
            buoyancy_bias: f32,
            root_depth_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            leaf_absorb: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        impl From<GenomeV1> for Genome {
            fn from(g: GenomeV1) -> Self {
                Genome {
                    metabolic_rate: g.metabolic_rate,
                    reproduce_at: g.reproduce_at,
                    clone_fidelity: g.clone_fidelity,
                    buoyancy_bias: g.buoyancy_bias,
                    root_depth_bias: g.root_depth_bias,
                    alloc_stem: g.alloc_stem,
                    alloc_leaf: g.alloc_leaf,
                    alloc_root: g.alloc_root,
                    leaf_absorb: g.leaf_absorb,
                    shade_efficiency: g.shade_efficiency,
                    digest_rate: g.digest_rate,
                    host_leave_fraction: default_host_leave_fraction(),
                    attach_prefer: default_attach_prefer(),
                }
            }
        }
        #[derive(Deserialize)]
        struct PixelTraitsV1 {
            mass: f32,
            density: f32,
            stiffness: f32,
            strength: f32,
            upkeep_bias: f32,
            absorb_bias: f32,
            drink_bias: f32,
            clone_fidelity_bias: f32,
            reproduce_at_bias: f32,
            buoyancy_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            root_depth_bias: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        impl From<PixelTraitsV1> for PixelTraits {
            fn from(t: PixelTraitsV1) -> Self {
                PixelTraits {
                    mass: t.mass,
                    density: t.density,
                    stiffness: t.stiffness,
                    strength: t.strength,
                    upkeep_bias: t.upkeep_bias,
                    absorb_bias: t.absorb_bias,
                    drink_bias: t.drink_bias,
                    clone_fidelity_bias: t.clone_fidelity_bias,
                    reproduce_at_bias: t.reproduce_at_bias,
                    buoyancy_bias: t.buoyancy_bias,
                    alloc_stem: t.alloc_stem,
                    alloc_leaf: t.alloc_leaf,
                    alloc_root: t.alloc_root,
                    root_depth_bias: t.root_depth_bias,
                    shade_efficiency: t.shade_efficiency,
                    digest_rate: t.digest_rate,
                    host_leave_fraction: default_host_leave_fraction(),
                    attach_prefer: default_attach_prefer(),
                }
            }
        }
        #[derive(Deserialize)]
        struct PlacedModuleV1 {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
            traits: PixelTraitsV1,
        }
        #[derive(Deserialize)]
        struct BlueprintV1 {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<PlacedModuleV1>,
            genome: GenomeV1,
            name: String,
            notes: String,
        }
        if let Ok(old) = postcard::from_bytes::<BlueprintV1>(bytes) {
            if old.schema_version <= 1 {
                let mut modules: Vec<PlacedModule> = old
                    .modules
                    .into_iter()
                    .map(|m| PlacedModule {
                        x: m.x,
                        y: m.y,
                        lane: m.lane,
                        module: m.module,
                        traits: m.traits.into(),
                    })
                    .collect();
                paint_genome_onto_modules(&mut modules, old.genome.into());
                return Ok(Blueprint {
                    schema_version: BLUEPRINT_SCHEMA_VERSION,
                    canvas_w: old.canvas_w,
                    canvas_h: old.canvas_h,
                    modules,
                    name: old.name,
                    notes: old.notes,
                });
            }
        }

        // Pre-Wave-K postcard: PlacedModule had no `traits` field.
        // Postcard is positional, so serde(default) cannot fill the gap.
        #[derive(Deserialize)]
        struct LegacyPlaced {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
        }
        #[derive(Deserialize)]
        struct LegacyBlueprint {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<LegacyPlaced>,
            genome: GenomeV1,
            name: String,
            notes: String,
        }
        let old: LegacyBlueprint = postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        if old.schema_version > 1 {
            return Err(format!(
                "blueprint schema {} newer than supported {}",
                old.schema_version, BLUEPRINT_SCHEMA_VERSION
            ));
        }
        let mut modules: Vec<PlacedModule> = old
            .modules
            .into_iter()
            .map(|m| PlacedModule {
                x: m.x,
                y: m.y,
                lane: m.lane,
                module: m.module,
                traits: PixelTraits::default(),
            })
            .collect();
        paint_genome_onto_modules(&mut modules, old.genome.into());
        Ok(Blueprint {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: old.canvas_w,
            canvas_h: old.canvas_h,
            modules,
            name: old.name,
            notes: old.notes,
        })
    }

    pub fn save_path(name: &str) -> PathBuf {
        PathBuf::from(BLUEPRINT_DIR).join(format!("{name}.{BLUEPRINT_EXT}"))
    }

    pub fn save_to_disk(&self) -> Result<PathBuf, String> {
        std::fs::create_dir_all(BLUEPRINT_DIR).map_err(|e| e.to_string())?;
        let path = Self::save_path(&self.name);
        std::fs::write(&path, self.to_bytes()?).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn load_from_disk(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    pub fn list_disk() -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(BLUEPRINT_DIR) else {
            return Vec::new();
        };
        let mut out: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(BLUEPRINT_EXT))
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_round_trips_postcard() {
        let bp = Blueprint::atom();
        let loaded = Blueprint::from_bytes(&bp.to_bytes().unwrap()).unwrap();
        assert!(loaded.is_valid_atom());
        assert_eq!(loaded.modules.len(), 2);
    }

    #[test]
    fn relative_modules_center_on_nucleus() {
        let bp = Blueprint::atom();
        let rel = bp.modules_relative_to_nucleus();
        assert!(rel.contains(&(0, 0, ModuleId::Nucleus)));
        assert!(rel.contains(&(1, 0, ModuleId::Photosystem)));
    }

    #[test]
    fn minimal_plant_is_valid_and_anchors_root_below_crown() {
        let bp = Blueprint::minimal_plant();
        assert!(bp.is_valid_plant());
        assert!(!bp.is_valid_atom());
        let rel = bp.modules_relative_to_nucleus();
        assert!(rel.contains(&(0, 0, ModuleId::Nucleus)));
        assert!(rel
            .iter()
            .any(|&(dx, dy, m)| m == ModuleId::Root && dy < 0 && dx == 0));
        assert!(rel.iter().any(|&(_, dy, m)| m == ModuleId::Stem && dy > 0));
    }

    #[test]
    fn minimal_fungus_is_valid_detritus_habit() {
        let bp = Blueprint::minimal_fungus();
        assert!(bp.is_valid_fungus());
        assert!(!bp.is_valid_atom());
        assert!(!bp.is_valid_plant());
        assert!(bp.digest_count() >= 1);
    }

    #[test]
    fn minimal_epiphyte_is_valid_holdfast_habit() {
        let bp = Blueprint::minimal_epiphyte();
        assert!(bp.is_valid_epiphyte());
        assert!(!bp.is_valid_atom());
        assert!(!bp.is_valid_plant());
        assert!(!bp.is_valid_fungus());
        assert_eq!(bp.holdfast_count(), 1);
        assert_eq!(ModuleId::Holdfast.rgb(), (0xFF, 0x3D, 0x9A));
    }

    #[test]
    fn holdfast_appended_keeps_bone_postcard_index() {
        // Variant index order (not hex): … Skin=7, Muscle=8, Bone=9, Holdfast=10.
        #[derive(Serialize, Deserialize, PartialEq, Debug)]
        #[repr(u8)]
        enum Probe {
            Nucleus = 0x00,
            Photosystem = 0x01,
            Digest = 0x0A,
            Hypha = 0x0B,
            Root = 0x0D,
            Stem = 0x0E,
            Skin = 0x13,
            Muscle = 0x14,
            Bone = 0x15,
            Holdfast = 0x0F,
        }
        let bone = postcard::to_allocvec(&Probe::Bone).unwrap();
        let hold = postcard::to_allocvec(&Probe::Holdfast).unwrap();
        let live_bone = postcard::to_allocvec(&ModuleId::Bone).unwrap();
        let live_hold = postcard::to_allocvec(&ModuleId::Holdfast).unwrap();
        assert_eq!(bone, live_bone, "Bone index must stay stable");
        assert_eq!(hold, live_hold);
        assert_ne!(bone, hold);
    }

    #[test]
    fn editor_spawn_allows_any_nucleus_body() {
        let mut bp = Blueprint::atom();
        assert!(bp.can_editor_spawn());
        // Nucleus-only (no photo) is not a classified habit, still spawnable.
        bp.modules.retain(|m| m.module == ModuleId::Nucleus);
        assert!(!bp.is_valid_creature());
        assert!(bp.can_editor_spawn());
        bp.modules.clear();
        assert!(!bp.can_editor_spawn());
    }

    #[test]
    fn alloc_weights_normalize() {
        let g = Genome {
            alloc_stem: 1.0,
            alloc_leaf: 1.0,
            alloc_root: 2.0,
            ..Genome::default()
        };
        let (s, l, r) = g.alloc_weights();
        assert!((s + l + r - 1.0).abs() < 1e-5);
        assert!((r - 0.5).abs() < 1e-5);
    }

    #[test]
    fn pixel_traits_round_trips_postcard() {
        let mut bp = Blueprint::atom();
        bp.modules[1].traits.density = 2.25;
        bp.modules[1].traits.absorb_bias = 0.6;
        let loaded = Blueprint::from_bytes(&bp.to_bytes().unwrap()).unwrap();
        assert!((loaded.modules[1].traits.density - 2.25).abs() < 1e-5);
        assert!((loaded.modules[1].traits.absorb_bias - 0.6).abs() < 1e-5);
    }

    #[test]
    fn old_blueprint_loads_and_bakes_genome_into_traits() {
        // Pre-Wave-K postcard shape: PlacedModule without traits field.
        #[derive(Serialize)]
        struct OldPlaced {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
        }
        #[derive(Serialize)]
        struct OldGenome {
            metabolic_rate: f32,
            reproduce_at: f32,
            clone_fidelity: f32,
            buoyancy_bias: f32,
            root_depth_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            leaf_absorb: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        #[derive(Serialize)]
        struct OldBlueprint {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<OldPlaced>,
            genome: OldGenome,
            name: String,
            notes: String,
        }
        let g = Genome::default();
        let old = OldBlueprint {
            schema_version: 1,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                OldPlaced {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                },
                OldPlaced {
                    x: 1,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
            ],
            genome: OldGenome {
                metabolic_rate: g.metabolic_rate,
                reproduce_at: g.reproduce_at,
                clone_fidelity: g.clone_fidelity,
                buoyancy_bias: g.buoyancy_bias,
                root_depth_bias: g.root_depth_bias,
                alloc_stem: g.alloc_stem,
                alloc_leaf: g.alloc_leaf,
                alloc_root: g.alloc_root,
                leaf_absorb: g.leaf_absorb,
                shade_efficiency: g.shade_efficiency,
                digest_rate: g.digest_rate,
            },
            name: "legacy".into(),
            notes: String::new(),
        };
        let bytes = postcard::to_allocvec(&old).unwrap();
        let loaded = Blueprint::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.schema_version, BLUEPRINT_SCHEMA_VERSION);
        assert_eq!(loaded.modules.len(), 2);
        // Default Genome leaf_absorb paints Photosystem absorb.
        assert!((loaded.modules[1].traits.absorb_bias - 0.45).abs() < 1e-5);
        assert!(loaded.is_valid_atom());
    }

    #[test]
    fn schema1_with_traits_bakes_genome_and_upgrades() {
        #[derive(Serialize)]
        struct TraitsV1 {
            mass: f32,
            density: f32,
            stiffness: f32,
            strength: f32,
            upkeep_bias: f32,
            absorb_bias: f32,
            drink_bias: f32,
            clone_fidelity_bias: f32,
            reproduce_at_bias: f32,
            buoyancy_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            root_depth_bias: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        #[derive(Serialize)]
        struct PlacedV1 {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
            traits: TraitsV1,
        }
        #[derive(Serialize)]
        struct GenomeV1 {
            metabolic_rate: f32,
            reproduce_at: f32,
            clone_fidelity: f32,
            buoyancy_bias: f32,
            root_depth_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            leaf_absorb: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        #[derive(Serialize)]
        struct V1 {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<PlacedV1>,
            genome: GenomeV1,
            name: String,
            notes: String,
        }
        let d = PixelTraits::default();
        let mut g = Genome::default();
        g.leaf_absorb = 0.8;
        g.alloc_root = 0.9;
        let from_traits = |t: PixelTraits| TraitsV1 {
            mass: t.mass,
            density: t.density,
            stiffness: t.stiffness,
            strength: t.strength,
            upkeep_bias: t.upkeep_bias,
            absorb_bias: t.absorb_bias,
            drink_bias: t.drink_bias,
            clone_fidelity_bias: t.clone_fidelity_bias,
            reproduce_at_bias: t.reproduce_at_bias,
            buoyancy_bias: t.buoyancy_bias,
            alloc_stem: t.alloc_stem,
            alloc_leaf: t.alloc_leaf,
            alloc_root: t.alloc_root,
            root_depth_bias: t.root_depth_bias,
            shade_efficiency: t.shade_efficiency,
            digest_rate: t.digest_rate,
        };
        let mut photo_t = d;
        photo_t.density = 2.0;
        let old = V1 {
            schema_version: 1,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedV1 {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                    traits: from_traits(d),
                },
                PlacedV1 {
                    x: 1,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: from_traits(photo_t),
                },
            ],
            genome: GenomeV1 {
                metabolic_rate: g.metabolic_rate,
                reproduce_at: g.reproduce_at,
                clone_fidelity: g.clone_fidelity,
                buoyancy_bias: g.buoyancy_bias,
                root_depth_bias: g.root_depth_bias,
                alloc_stem: g.alloc_stem,
                alloc_leaf: g.alloc_leaf,
                alloc_root: g.alloc_root,
                leaf_absorb: g.leaf_absorb,
                shade_efficiency: g.shade_efficiency,
                digest_rate: g.digest_rate,
            },
            name: "v1".into(),
            notes: String::new(),
        };
        let loaded = Blueprint::from_bytes(&postcard::to_allocvec(&old).unwrap()).unwrap();
        assert_eq!(loaded.schema_version, BLUEPRINT_SCHEMA_VERSION);
        assert!((loaded.modules[1].traits.density - 2.0).abs() < 1e-5);
        assert!((loaded.modules[1].traits.absorb_bias - 0.8).abs() < 1e-5);
        assert!((loaded.modules[0].traits.alloc_root - 0.9).abs() < 1e-5);
    }

    #[test]
    fn schema2_upgrades_with_default_host_leave() {
        #[derive(Serialize)]
        struct TraitsV2 {
            mass: f32,
            density: f32,
            stiffness: f32,
            strength: f32,
            upkeep_bias: f32,
            absorb_bias: f32,
            drink_bias: f32,
            clone_fidelity_bias: f32,
            reproduce_at_bias: f32,
            buoyancy_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            root_depth_bias: f32,
            shade_efficiency: f32,
            digest_rate: f32,
        }
        #[derive(Serialize)]
        struct PlacedV2 {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
            traits: TraitsV2,
        }
        #[derive(Serialize)]
        struct V2 {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<PlacedV2>,
            name: String,
            notes: String,
        }
        let d = PixelTraits::default();
        let old = V2 {
            schema_version: 2,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedV2 {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                    traits: TraitsV2 {
                        mass: d.mass,
                        density: d.density,
                        stiffness: d.stiffness,
                        strength: d.strength,
                        upkeep_bias: d.upkeep_bias,
                        absorb_bias: d.absorb_bias,
                        drink_bias: d.drink_bias,
                        clone_fidelity_bias: d.clone_fidelity_bias,
                        reproduce_at_bias: d.reproduce_at_bias,
                        buoyancy_bias: d.buoyancy_bias,
                        alloc_stem: d.alloc_stem,
                        alloc_leaf: d.alloc_leaf,
                        alloc_root: d.alloc_root,
                        root_depth_bias: d.root_depth_bias,
                        shade_efficiency: d.shade_efficiency,
                        digest_rate: d.digest_rate,
                    },
                },
                PlacedV2 {
                    x: 0,
                    y: 1,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: TraitsV2 {
                        mass: d.mass,
                        density: d.density,
                        stiffness: d.stiffness,
                        strength: d.strength,
                        upkeep_bias: d.upkeep_bias,
                        absorb_bias: 0.7,
                        drink_bias: d.drink_bias,
                        clone_fidelity_bias: d.clone_fidelity_bias,
                        reproduce_at_bias: d.reproduce_at_bias,
                        buoyancy_bias: d.buoyancy_bias,
                        alloc_stem: d.alloc_stem,
                        alloc_leaf: d.alloc_leaf,
                        alloc_root: d.alloc_root,
                        root_depth_bias: d.root_depth_bias,
                        shade_efficiency: d.shade_efficiency,
                        digest_rate: d.digest_rate,
                    },
                },
            ],
            name: "v2".into(),
            notes: String::new(),
        };
        let loaded = Blueprint::from_bytes(&postcard::to_allocvec(&old).unwrap()).unwrap();
        assert_eq!(loaded.schema_version, BLUEPRINT_SCHEMA_VERSION);
        assert!((loaded.modules[1].traits.absorb_bias - 0.7).abs() < 1e-5);
        assert!((loaded.modules[1].traits.host_leave_fraction - 0.0).abs() < 1e-5);
        assert!((loaded.modules[1].traits.attach_prefer - 0.0).abs() < 1e-5);
    }

    #[test]
    fn schema3_upgrades_with_default_attach_prefer() {
        #[derive(Serialize)]
        struct TraitsV3 {
            mass: f32,
            density: f32,
            stiffness: f32,
            strength: f32,
            upkeep_bias: f32,
            absorb_bias: f32,
            drink_bias: f32,
            clone_fidelity_bias: f32,
            reproduce_at_bias: f32,
            buoyancy_bias: f32,
            alloc_stem: f32,
            alloc_leaf: f32,
            alloc_root: f32,
            root_depth_bias: f32,
            shade_efficiency: f32,
            digest_rate: f32,
            host_leave_fraction: f32,
        }
        #[derive(Serialize)]
        struct PlacedV3 {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
            traits: TraitsV3,
        }
        #[derive(Serialize)]
        struct V3 {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<PlacedV3>,
            name: String,
            notes: String,
        }
        let d = PixelTraits::default();
        let old = V3 {
            schema_version: 3,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedV3 {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Holdfast,
                    traits: TraitsV3 {
                        mass: d.mass,
                        density: d.density,
                        stiffness: d.stiffness,
                        strength: d.strength,
                        upkeep_bias: d.upkeep_bias,
                        absorb_bias: d.absorb_bias,
                        drink_bias: d.drink_bias,
                        clone_fidelity_bias: d.clone_fidelity_bias,
                        reproduce_at_bias: d.reproduce_at_bias,
                        buoyancy_bias: d.buoyancy_bias,
                        alloc_stem: d.alloc_stem,
                        alloc_leaf: d.alloc_leaf,
                        alloc_root: d.alloc_root,
                        root_depth_bias: d.root_depth_bias,
                        shade_efficiency: d.shade_efficiency,
                        digest_rate: d.digest_rate,
                        host_leave_fraction: 0.0,
                    },
                },
                PlacedV3 {
                    x: 0,
                    y: 1,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                    traits: TraitsV3 {
                        mass: d.mass,
                        density: d.density,
                        stiffness: d.stiffness,
                        strength: d.strength,
                        upkeep_bias: d.upkeep_bias,
                        absorb_bias: 0.6,
                        drink_bias: d.drink_bias,
                        clone_fidelity_bias: d.clone_fidelity_bias,
                        reproduce_at_bias: d.reproduce_at_bias,
                        buoyancy_bias: d.buoyancy_bias,
                        alloc_stem: d.alloc_stem,
                        alloc_leaf: d.alloc_leaf,
                        alloc_root: d.alloc_root,
                        root_depth_bias: d.root_depth_bias,
                        shade_efficiency: d.shade_efficiency,
                        digest_rate: d.digest_rate,
                        host_leave_fraction: 0.75,
                    },
                },
            ],
            name: "v3".into(),
            notes: String::new(),
        };
        let loaded = Blueprint::from_bytes(&postcard::to_allocvec(&old).unwrap()).unwrap();
        assert_eq!(loaded.schema_version, BLUEPRINT_SCHEMA_VERSION);
        assert!((loaded.modules[1].traits.host_leave_fraction - 0.75).abs() < 1e-5);
        assert!((loaded.modules[0].traits.attach_prefer - 0.0).abs() < 1e-5);
    }

    #[test]
    fn mutate_child_is_deterministic_for_seed_tick_parent() {
        let mut parent = Blueprint::atom();
        // Low fidelity → strong mutation so structural ops can fire.
        for m in &mut parent.modules {
            m.traits.clone_fidelity_bias = 0.1;
        }
        let a = parent.mutate_child(99, 7, 3);
        let b = parent.mutate_child(99, 7, 3);
        assert_eq!(a.modules.len(), b.modules.len());
        for (ma, mb) in a.modules.iter().zip(b.modules.iter()) {
            assert_eq!(ma.x, mb.x);
            assert_eq!(ma.y, mb.y);
            assert_eq!(ma.module, mb.module);
            assert_eq!(ma.traits, mb.traits);
        }
    }

    #[test]
    fn mutate_child_chain_grow_extends_same_kind_neighbour() {
        let mut parent = Blueprint::atom();
        for m in &mut parent.modules {
            m.traits.clone_fidelity_bias = 0.0;
        }
        // Search a small seed/tick space for a grow event.
        let mut grew = false;
        for tick in 0..64u64 {
            for pid in 0..16u32 {
                let child = parent.mutate_child(12345, tick, pid);
                if child.modules.len() > parent.modules.len() {
                    // New pixel shares a kind with some neighbour of that kind.
                    let parent_cells: std::collections::HashSet<_> = parent
                        .modules
                        .iter()
                        .map(|m| (m.x, m.y, m.module))
                        .collect();
                    let added: Vec<_> = child
                        .modules
                        .iter()
                        .filter(|m| !parent_cells.contains(&(m.x, m.y, m.module)))
                        .collect();
                    assert!(!added.is_empty());
                    for a in &added {
                        let adj = [(1i16, 0), (-1, 0), (0, 1), (0, -1)];
                        let has_same_kind_neighbour = child.modules.iter().any(|m| {
                            m.module == a.module
                                && adj.iter().any(|(dx, dy)| {
                                    m.x == a.x + dx && m.y == a.y + dy
                                })
                        });
                        assert!(has_same_kind_neighbour);
                    }
                    grew = true;
                    break;
                }
            }
            if grew {
                break;
            }
        }
        assert!(grew, "expected at least one chain-grow across seed search");
    }

    #[test]
    fn kind_swap_can_change_module_kind() {
        let mut parent = Blueprint::atom();
        // Low fidelity → frequent structural ops.
        for m in &mut parent.modules {
            m.traits.clone_fidelity_bias = 0.05;
        }
        // Extra swappable tissue so Nucleus isn't the only candidate.
        parent.modules.push(PlacedModule {
            x: 2,
            y: 0,
            lane: LaneId::Mid,
            module: ModuleId::Muscle,
            traits: PixelTraits {
                clone_fidelity_bias: 0.05,
                ..PixelTraits::default()
            },
        });
        let parent_kinds: Vec<_> = parent.modules.iter().map(|m| m.module).collect();
        let mut swapped = false;
        for seed in 0..400u64 {
            for tick in 0..40u64 {
                let child = parent.mutate_child(seed, tick, 7);
                let child_kinds: Vec<_> = child.modules.iter().map(|m| m.module).collect();
                if child_kinds != parent_kinds
                    && child.modules.iter().any(|m| m.module == ModuleId::Nucleus)
                {
                    // At least one pixel became a partner kind.
                    let changed = child.modules.iter().any(|cm| {
                        parent.modules.iter().any(|pm| {
                            pm.x == cm.x
                                && pm.y == cm.y
                                && pm.module != cm.module
                                && kind_swap_partners(pm.module).contains(&cm.module)
                        })
                    });
                    if changed {
                        swapped = true;
                        break;
                    }
                }
            }
            if swapped {
                break;
            }
        }
        assert!(swapped, "expected at least one kind-swap across seed search");
    }

    #[test]
    fn kind_swap_never_removes_last_nucleus() {
        let mut parent = Blueprint::atom();
        for m in &mut parent.modules {
            m.traits.clone_fidelity_bias = 0.05;
        }
        for seed in 0..80u64 {
            for tick in 0..40u64 {
                let child = parent.mutate_child(seed, tick, 3);
                let n = child
                    .modules
                    .iter()
                    .filter(|m| m.module == ModuleId::Nucleus)
                    .count();
                assert!(n >= 1, "child must keep a Nucleus (seed={seed} tick={tick})");
            }
        }
    }

    #[test]
    fn default_traits_preserve_frozen_palette_rgb() {
        let t = PixelTraits::default();
        for m in [
            ModuleId::Nucleus,
            ModuleId::Photosystem,
            ModuleId::Root,
            ModuleId::Stem,
            ModuleId::Digest,
            ModuleId::Hypha,
            ModuleId::Bone,
            ModuleId::Muscle,
            ModuleId::Skin,
            ModuleId::Holdfast,
        ] {
            assert_eq!(
                modulate_module_rgb(m, &t),
                m.rgb(),
                "{:?} default traits must match frozen palette",
                m
            );
        }
    }

    #[test]
    fn high_absorb_brightens_photosystem() {
        let mut t = PixelTraits::default();
        t.absorb_bias = 2.0;
        let base = ModuleId::Photosystem.rgb();
        let tinted = modulate_module_rgb(ModuleId::Photosystem, &t);
        assert!(
            tinted.1 > base.1,
            "high absorb should raise green (base={base:?} tinted={tinted:?})"
        );
        // Still recognizably green-dominant.
        assert!(tinted.1 > tinted.0 && tinted.1 > tinted.2);
    }

    #[test]
    fn dense_bone_darkens() {
        let mut t = PixelTraits::default();
        t.density = 2.5;
        let base = ModuleId::Bone.rgb();
        let tinted = modulate_module_rgb(ModuleId::Bone, &t);
        assert!(
            tinted.0 < base.0 && tinted.1 < base.1 && tinted.2 < base.2,
            "dense bone should darken (base={base:?} tinted={tinted:?})"
        );
    }
}
