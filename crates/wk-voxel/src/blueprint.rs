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

/// Soft cap on body size after morphological mutation.
pub const BODY_MUTATION_MAX_MODULES: usize = 28;
/// Max add/swap/delete attempts per clone (scaled by messiness).
pub const BODY_MUTATION_MAX_EDITS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BodyHabit {
    Atom,
    Plant,
    Fungus,
}

fn classify_body_habit(body: &[(i16, i16, ModuleId)]) -> BodyHabit {
    let has_root = body.iter().any(|(_, _, m)| *m == ModuleId::Root);
    let has_digest = body.iter().any(|(_, _, m)| *m == ModuleId::Digest);
    if has_root {
        BodyHabit::Plant
    } else if has_digest {
        BodyHabit::Fungus
    } else {
        BodyHabit::Atom
    }
}

fn habit_palette(habit: BodyHabit) -> &'static [ModuleId] {
    match habit {
        BodyHabit::Atom => &[ModuleId::Photosystem],
        BodyHabit::Plant => &[
            ModuleId::Photosystem,
            ModuleId::Root,
            ModuleId::Stem,
            ModuleId::ReproSpore,
        ],
        BodyHabit::Fungus => &[ModuleId::Digest, ModuleId::Hypha, ModuleId::ReproSpore],
    }
}

fn count_mid(body: &[(i16, i16, ModuleId)], mid: ModuleId) -> usize {
    body.iter().filter(|(_, _, m)| *m == mid).count()
}

fn module_is_required_singleton(
    habit: BodyHabit,
    mid: ModuleId,
    body: &[(i16, i16, ModuleId)],
) -> bool {
    if mid == ModuleId::Nucleus {
        return count_mid(body, ModuleId::Nucleus) <= 1;
    }
    match habit {
        BodyHabit::Atom => mid == ModuleId::Photosystem && count_mid(body, mid) <= 1,
        BodyHabit::Plant => {
            (mid == ModuleId::Root || mid == ModuleId::Photosystem) && count_mid(body, mid) <= 1
        }
        BodyHabit::Fungus => mid == ModuleId::Digest && count_mid(body, mid) <= 1,
    }
}

fn body_still_valid_habit(habit: BodyHabit, body: &[(i16, i16, ModuleId)]) -> bool {
    if count_mid(body, ModuleId::Nucleus) < 1 {
        return false;
    }
    match habit {
        BodyHabit::Atom => {
            count_mid(body, ModuleId::Photosystem) >= 1
                && count_mid(body, ModuleId::Root) == 0
                && count_mid(body, ModuleId::Digest) == 0
        }
        BodyHabit::Plant => {
            count_mid(body, ModuleId::Root) >= 1 && count_mid(body, ModuleId::Photosystem) >= 1
        }
        BodyHabit::Fungus => {
            count_mid(body, ModuleId::Digest) >= 1
                && count_mid(body, ModuleId::Root) == 0
                && count_mid(body, ModuleId::Stem) == 0
        }
    }
}

/// Morphological mutation of a body blueprint (module add / swap / delete).
///
/// Driven by `clone_fidelity` the same way as [`Genome::mutate`]: high
/// fidelity → few or no edits; low fidelity → messier offspring.
/// Never removes the last Nucleus or breaks the parent's habit class
/// (Atom / plant / fungus).
pub fn mutate_body(
    parent_body: &[(i16, i16, ModuleId)],
    fidelity: f32,
    world_seed: u64,
    tick: u64,
    parent_id: u32,
) -> Vec<(i16, i16, ModuleId)> {
    let mut body: Vec<(i16, i16, ModuleId)> = parent_body.to_vec();
    if body.is_empty() {
        return body;
    }
    let habit = classify_body_habit(&body);
    // Stemless plants (seaweed) must not invent a trunk via mutation.
    let has_stem = body.iter().any(|(_, _, m)| *m == ModuleId::Stem);
    let palette: &[ModuleId] = if habit == BodyHabit::Plant && !has_stem {
        &[
            ModuleId::Photosystem,
            ModuleId::Root,
            ModuleId::ReproSpore,
        ]
    } else {
        habit_palette(habit)
    };
    let fidelity = fidelity.clamp(0.0, 1.0);
    let mess = 1.0 - fidelity;
    // Default fidelity (~0.9) → gene-only clones; lower fidelity unlocks
    // 1..=MAX morphological edits.
    let edits =
        ((mess * BODY_MUTATION_MAX_EDITS as f32).floor() as usize).min(BODY_MUTATION_MAX_EDITS);
    let salt_base = tick
        .wrapping_mul(0x9E37_79B9)
        .wrapping_add(parent_id as u64)
        .wrapping_add(0xB0D4_0001);
    for edit_i in 0..edits {
        let h = hash_u64(world_seed, salt_base, edit_i as u64, 0xED17);
        // Bias: swap most common, then add, then delete.
        let kind = h % 5;
        match kind {
            0 | 1 => try_swap_module(&mut body, habit, palette, world_seed, salt_base, edit_i),
            2 | 3 => try_add_module(&mut body, habit, palette, world_seed, salt_base, edit_i),
            _ => try_delete_module(&mut body, habit, world_seed, salt_base, edit_i),
        }
    }
    // Safety net — should already hold if helpers refuse bad edits.
    if !body_still_valid_habit(habit, &body) {
        return parent_body.to_vec();
    }
    body
}

fn try_swap_module(
    body: &mut Vec<(i16, i16, ModuleId)>,
    habit: BodyHabit,
    palette: &[ModuleId],
    world_seed: u64,
    salt_base: u64,
    edit_i: usize,
) {
    let candidates: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, (_, _, m))| *m != ModuleId::Nucleus)
        .map(|(i, _)| i)
        .collect();
    if candidates.is_empty() || palette.is_empty() {
        return;
    }
    let pick =
        hash_u64(world_seed, salt_base, edit_i as u64, 0x5A10) as usize % candidates.len();
    let idx = candidates[pick];
    let old = body[idx].2;
    let new_mid = palette
        [hash_u64(world_seed, salt_base, edit_i as u64, 0x5A11) as usize % palette.len()];
    if new_mid == old {
        return;
    }
    // Don't strip the last required module of its type.
    if module_is_required_singleton(habit, old, body) {
        return;
    }
    let prev = body[idx].2;
    body[idx].2 = new_mid;
    if !body_still_valid_habit(habit, body) {
        body[idx].2 = prev;
    }
}

fn try_add_module(
    body: &mut Vec<(i16, i16, ModuleId)>,
    habit: BodyHabit,
    palette: &[ModuleId],
    world_seed: u64,
    salt_base: u64,
    edit_i: usize,
) {
    if body.len() >= BODY_MUTATION_MAX_MODULES || palette.is_empty() {
        return;
    }
    let occupied: std::collections::HashSet<(i16, i16)> =
        body.iter().map(|&(x, y, _)| (x, y)).collect();
    let anchors: Vec<(i16, i16)> = body.iter().map(|&(x, y, _)| (x, y)).collect();
    if anchors.is_empty() {
        return;
    }
    let a_i = hash_u64(world_seed, salt_base, edit_i as u64, 0xAD01) as usize % anchors.len();
    let (ax, ay) = anchors[a_i];
    const NEIGH: [(i16, i16); 8] = [
        (1, 0),
        (-1, 0),
        (0, 1),
        (0, -1),
        (1, 1),
        (1, -1),
        (-1, 1),
        (-1, -1),
    ];
    let n_i = hash_u64(world_seed, salt_base, edit_i as u64, 0xAD02) as usize % NEIGH.len();
    let (dx, dy) = NEIGH[n_i];
    let nx = ax + dx;
    let ny = ay + dy;
    // Keep blueprints near the nucleus — avoid runaway diagonals.
    if nx.abs() > 8 || ny.abs() > 8 {
        return;
    }
    if occupied.contains(&(nx, ny)) {
        return;
    }
    let mid = palette
        [hash_u64(world_seed, salt_base, edit_i as u64, 0xAD03) as usize % palette.len()];
    body.push((nx, ny, mid));
    if !body_still_valid_habit(habit, body) {
        body.pop();
    }
}

fn try_delete_module(
    body: &mut Vec<(i16, i16, ModuleId)>,
    habit: BodyHabit,
    world_seed: u64,
    salt_base: u64,
    edit_i: usize,
) {
    let candidates: Vec<usize> = body
        .iter()
        .enumerate()
        .filter(|(_, (_, _, m))| *m != ModuleId::Nucleus)
        .filter(|(_, (_, _, m))| !module_is_required_singleton(habit, *m, body))
        .map(|(i, _)| i)
        .collect();
    if candidates.is_empty() {
        return;
    }
    let pick =
        hash_u64(world_seed, salt_base, edit_i as u64, 0xDE01) as usize % candidates.len();
    let idx = candidates[pick];
    let removed = body.remove(idx);
    if !body_still_valid_habit(habit, body) {
        body.insert(idx, removed);
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

    /// Stemless aquatic ribbon: one holdfast root, nucleus, leaf string.
    /// No Stem — underwater plants don't need a trunk; shoot growth elongates
    /// Photosystems upward as a seaweed frond.
    pub fn minimal_seaweed() -> Self {
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedModule {
                    x: 8,
                    y: 3,
                    lane: LaneId::Mid,
                    module: ModuleId::Root,
                },
                PlacedModule {
                    x: 8,
                    y: 4,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                },
                // Vertical leaf ribbon (no olive Stem).
                PlacedModule {
                    x: 8,
                    y: 5,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 8,
                    y: 6,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 8,
                    y: 7,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 8,
                    y: 8,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
            ],
            genome: Genome {
                alloc_stem: 0.0,
                alloc_leaf: 0.70,
                alloc_root: 0.30,
                // Slight sink so the holdfast stays on the bed.
                buoyancy_bias: 0.55,
                root_depth_bias: 0.35,
                shade_efficiency: 0.55,
                ..Genome::default()
            },
            name: "seaweed".into(),
            notes: "Stemless ribbon — one Root holdfast + Photosystem string; thrives submerged".into(),
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
    fn minimal_seaweed_is_stemless_plant_ribbon() {
        let bp = Blueprint::minimal_seaweed();
        assert!(bp.is_valid_plant());
        assert!(!bp.is_valid_fungus());
        let rel = bp.modules_relative_to_nucleus();
        assert!(rel.contains(&(0, 0, ModuleId::Nucleus)));
        assert!(rel
            .iter()
            .any(|&(dx, dy, m)| m == ModuleId::Root && dy < 0 && dx == 0));
        assert!(
            !rel.iter().any(|&(_, _, m)| m == ModuleId::Stem),
            "seaweed must not paint a trunk"
        );
        let photos: Vec<i16> = rel
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .collect();
        assert!(photos.len() >= 3, "ribbon of leaves");
        assert!(photos.iter().all(|&y| y > 0), "leaves above nucleus");
        assert!(bp.genome.alloc_stem <= 0.05);
        assert!(bp.genome.alloc_leaf >= 0.5);
    }

    #[test]
    fn stemless_plant_mutation_does_not_invent_stem() {
        let parent = Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        for tick in 0..40u64 {
            let child = mutate_body(&parent, 0.05, 42, tick, 9);
            assert!(
                !child.iter().any(|&(_, _, m)| m == ModuleId::Stem),
                "seaweed clone must stay stemless (tick={tick})"
            );
            assert!(child.iter().any(|&(_, _, m)| m == ModuleId::Root));
            assert!(child.iter().any(|&(_, _, m)| m == ModuleId::Photosystem));
        }
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
    fn high_fidelity_body_mutate_is_stable() {
        let parent = Blueprint::minimal_plant().modules_relative_to_nucleus();
        // Default-ish fidelity (0.9) and higher → no morph edits.
        let child = mutate_body(&parent, 0.9, 1, 10, 3);
        assert_eq!(
            child, parent,
            "clone_fidelity ≥ ~0.67 should skip morphology at default max-edits"
        );
    }

    #[test]
    fn low_fidelity_body_mutate_edits_but_keeps_habit() {
        let parent = Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut saw_change = false;
        for tick in 0..200u64 {
            let child = mutate_body(&parent, 0.05, 99, tick, 11);
            assert!(
                child.iter().any(|(_, _, m)| *m == ModuleId::Nucleus),
                "must keep a Nucleus"
            );
            assert!(
                child.iter().any(|(_, _, m)| *m == ModuleId::Root),
                "plant must keep a Root"
            );
            assert!(
                child.iter().any(|(_, _, m)| *m == ModuleId::Photosystem),
                "plant must keep a Photosystem"
            );
            assert!(
                !child.iter().any(|(_, _, m)| *m == ModuleId::Digest),
                "plant must not gain Digest"
            );
            if child != parent {
                saw_change = true;
            }
        }
        assert!(saw_change, "messy fidelity should sometimes change pixels");
    }

    #[test]
    fn fungus_body_mutate_never_gains_roots() {
        let parent = Blueprint::minimal_fungus().modules_relative_to_nucleus();
        for tick in 0..100u64 {
            let child = mutate_body(&parent, 0.05, 7, tick, 2);
            assert!(child.iter().any(|(_, _, m)| *m == ModuleId::Digest));
            assert!(!child.iter().any(|(_, _, m)| matches!(
                m,
                ModuleId::Root | ModuleId::Stem
            )));
        }
    }
}
