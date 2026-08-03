//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Creature blueprints (MS-Paint drawings) for `wk-voxel-app`.
//! Set A Atom + minimal Set D plant. Same `.gvsecrt` postcard shape as
//! column-GVSE so files can be shared later.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedModule {
    pub x: i16,
    pub y: i16,
    pub lane: LaneId,
    pub module: ModuleId,
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
                },
                PlacedModule {
                    x: 1,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
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
                },
                PlacedModule {
                    x: 8,
                    y: 5,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                },
                PlacedModule {
                    x: 8,
                    y: 6,
                    lane: LaneId::Mid,
                    module: ModuleId::Stem,
                },
                PlacedModule {
                    x: 8,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Stem,
                },
                // Leaves sit beside the upper stem so the tip column stays
                // free for olive elongation (no leaf→stem→leaf tower).
                PlacedModule {
                    x: 7,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 9,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
            ],
            genome: Genome::default(),
            name: "plant".into(),
            notes: "Set D minimal land plant".into(),
        }
    }

    /// Minimal fruiting body (E): nucleus + digest + hyphae + spore packet.
    /// Underground mycelium is a ground field on Organic, not painted here.
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
                },
                PlacedModule {
                    x: 1,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Digest,
                },
                PlacedModule {
                    x: 2,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Hypha,
                },
                PlacedModule {
                    x: 3,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Hypha,
                },
                PlacedModule {
                    x: 1,
                    y: 1,
                    lane: LaneId::Mid,
                    module: ModuleId::ReproSpore,
                },
            ],
            genome: Genome {
                digest_rate: 1.0,
                ..Genome::default()
            },
            name: "fruiting body".into(),
            notes: "Fruiting body — mycelium lives in moist Organic as a ground field; ReproSpore sheds wind spores".into(),
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

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| e.to_string())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let bp: Blueprint = postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        if bp.schema_version > BLUEPRINT_SCHEMA_VERSION {
            return Err(format!(
                "blueprint schema {} newer than supported {}",
                bp.schema_version, BLUEPRINT_SCHEMA_VERSION
            ));
        }
        Ok(bp)
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
        assert!(
            bp.modules.iter().any(|m| m.module == ModuleId::ReproSpore),
            "fruiting body template includes a spore packet"
        );
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
}
