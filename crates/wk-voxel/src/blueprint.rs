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

pub const BLUEPRINT_SCHEMA_VERSION: u16 = 1;
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

/// Per-pixel gene payload (Wave K). Every painted module carries these
/// scalars; aggregates form the [`BodyPlan`]. Fields unused by a kind
/// stay inert until a later physics wave reads them.
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
    #[serde(default = "one")]
    pub clone_fidelity_bias: f32,
    #[serde(default = "one")]
    pub reproduce_at_bias: f32,
    #[serde(default = "one")]
    pub buoyancy_bias: f32,
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
            clone_fidelity_bias: 1.0,
            reproduce_at_bias: 1.0,
            buoyancy_bias: 1.0,
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

/// Live genes for Atom + land plant (Set D). Older postcard files get
/// plant fields via `#[serde(default)]`.
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
        }
    }
}

/// Mutation strength scale (column `MUTATION_SIGMA`).
const MUTATION_SIGMA: f32 = 0.12;

impl Genome {
    /// Normalized `(stem, leaf, root)` surplus weights (sum to 1).
    pub fn alloc_weights(self) -> (f32, f32, f32) {
        let s = self.alloc_stem.max(0.0);
        let l = self.alloc_leaf.max(0.0);
        let r = self.alloc_root.max(0.0);
        let sum = (s + l + r).max(1e-6);
        (s / sum, l / sum, r / sum)
    }

    /// Deterministic per-trait mutation. High `clone_fidelity` → small jitter.
    pub fn mutate(parent: Genome, world_seed: u64, tick: u64, parent_id: u32) -> Genome {
        let mut g = parent;
        let fidelity = parent.clone_fidelity.clamp(0.0, 1.0);
        let strength = (1.0 - fidelity) * MUTATION_SIGMA;
        let salt_base = tick
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(parent_id as u64);
        let mut trait_i = 0u64;
        let mut jitter = |value: f32, lo: f32, hi: f32| -> f32 {
            trait_i += 1;
            let h = hash_u64(world_seed, salt_base, trait_i, 0xE11);
            let u = (h as f32 / u64::MAX as f32) * 2.0 - 1.0;
            (value + u * strength * (hi - lo).max(0.1)).clamp(lo, hi)
        };
        g.metabolic_rate = jitter(g.metabolic_rate, 0.2, 2.0);
        g.reproduce_at = jitter(g.reproduce_at, 0.3, 0.95);
        g.clone_fidelity = jitter(g.clone_fidelity, 0.05, 1.0);
        g.buoyancy_bias = jitter(g.buoyancy_bias, 0.0, 1.0);
        g.root_depth_bias = jitter(g.root_depth_bias, 0.0, 1.0);
        g.alloc_stem = jitter(g.alloc_stem, 0.0, 1.0);
        g.alloc_leaf = jitter(g.alloc_leaf, 0.0, 1.0);
        g.alloc_root = jitter(g.alloc_root, 0.0, 1.0);
        g.leaf_absorb = jitter(g.leaf_absorb, 0.05, 1.0);
        g.shade_efficiency = jitter(g.shade_efficiency, 0.0, 1.0);
        g.digest_rate = jitter(g.digest_rate, 0.05, 2.0);
        g
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
    pub genome: Genome,
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
            genome: Genome::default(),
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
            genome: Genome::default(),
            name: "plant".into(),
            notes: "Set D minimal land plant".into(),
        }
    }

    /// Minimal litter fungus (E): nucleus + digest + a short hypha thread.
    pub fn minimal_fungus() -> Self {
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
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 2,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Hypha,
                    traits: PixelTraits::default(),
                    },
                PlacedModule {
                    x: 3,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Hypha,
                    traits: PixelTraits::default(),
                    },
            ],
            genome: Genome {
                digest_rate: 1.0,
                ..Genome::default()
            },
            name: "fungus".into(),
            notes: "Set E litter fungus".into(),
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

    pub fn is_valid_atom(&self) -> bool {
        self.nucleus_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() == 0
            && self.digest_count() == 0
    }

    pub fn is_valid_plant(&self) -> bool {
        self.nucleus_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() >= 1
    }

    /// Nucleus + Digest, no root/stem (detritus habit).
    pub fn is_valid_fungus(&self) -> bool {
        self.nucleus_count() >= 1
            && self.digest_count() >= 1
            && !self
                .modules
                .iter()
                .any(|m| matches!(m.module, ModuleId::Root | ModuleId::Stem))
    }

    pub fn is_valid_creature(&self) -> bool {
        self.is_valid_atom() || self.is_valid_plant() || self.is_valid_fungus()
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
        let Some((ox, oy)) = self.nucleus_origin() else {
            return Vec::new();
        };
        self.modules
            .iter()
            .map(|m| (m.x - ox, m.y - oy, m.module))
            .collect()
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
    /// delete. Budget and sigma scale with aggregate `clone_fidelity`.
    ///
    /// The vestigial [`Genome`] field is copied unchanged (Wave K).
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
        child
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| e.to_string())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        match postcard::from_bytes::<Blueprint>(bytes) {
            Ok(bp) => {
                if bp.schema_version > BLUEPRINT_SCHEMA_VERSION {
                    return Err(format!(
                        "blueprint schema {} newer than supported {}",
                        bp.schema_version, BLUEPRINT_SCHEMA_VERSION
                    ));
                }
                Ok(bp)
            }
            Err(_) => {
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
                    genome: Genome,
                    name: String,
                    notes: String,
                }
                let old: LegacyBlueprint =
                    postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
                if old.schema_version > BLUEPRINT_SCHEMA_VERSION {
                    return Err(format!(
                        "blueprint schema {} newer than supported {}",
                        old.schema_version, BLUEPRINT_SCHEMA_VERSION
                    ));
                }
                Ok(Blueprint {
                    schema_version: old.schema_version,
                    canvas_w: old.canvas_w,
                    canvas_h: old.canvas_h,
                    modules: old
                        .modules
                        .into_iter()
                        .map(|m| PlacedModule {
                            x: m.x,
                            y: m.y,
                            lane: m.lane,
                            module: m.module,
                            traits: PixelTraits::default(),
                        })
                        .collect(),
                    genome: old.genome,
                    name: old.name,
                    notes: old.notes,
                })
            }
        }
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
    fn mutate_jitters_with_low_fidelity() {
        let mut parent = Genome::default();
        parent.clone_fidelity = 0.1;
        parent.alloc_root = 0.5;
        parent.leaf_absorb = 0.5;
        let child = Genome::mutate(parent, 42, 100, 7);
        // With low fidelity, at least one plant gene should usually move.
        assert!(
            (child.alloc_root - parent.alloc_root).abs() > 1e-6
                || (child.leaf_absorb - parent.leaf_absorb).abs() > 1e-6
                || (child.root_depth_bias - parent.root_depth_bias).abs() > 1e-6
                || (child.alloc_stem - parent.alloc_stem).abs() > 1e-6,
            "low-fidelity mutate should jitter plant genes"
        );
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
    fn old_blueprint_loads_with_default_traits() {
        // Pre-Wave-K postcard shape: PlacedModule without traits field.
        #[derive(Serialize)]
        struct OldPlaced {
            x: i16,
            y: i16,
            lane: LaneId,
            module: ModuleId,
        }
        #[derive(Serialize)]
        struct OldBlueprint {
            schema_version: u16,
            canvas_w: u16,
            canvas_h: u16,
            modules: Vec<OldPlaced>,
            genome: Genome,
            name: String,
            notes: String,
        }
        let old = OldBlueprint {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
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
            genome: Genome::default(),
            name: "legacy".into(),
            notes: String::new(),
        };
        let bytes = postcard::to_allocvec(&old).unwrap();
        let loaded = Blueprint::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.modules.len(), 2);
        assert_eq!(loaded.modules[0].traits, PixelTraits::default());
        assert_eq!(loaded.modules[1].traits, PixelTraits::default());
        assert!(loaded.is_valid_atom());
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
}
