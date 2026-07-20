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

/// Slim live genes for Set A (postcard may grow; unknown fields ignored
/// on older files via defaults).
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct Genome {
    pub metabolic_rate: f32,
    pub reproduce_at: f32,
    pub clone_fidelity: f32,
    /// 0 = floater, 1 = sinker — mutated on fission.
    #[serde(default)]
    pub buoyancy_bias: f32,
}

impl Default for Genome {
    fn default() -> Self {
        Self {
            metabolic_rate: 1.0,
            reproduce_at: 0.85,
            clone_fidelity: 0.9,
            buoyancy_bias: 0.0,
        }
    }
}

/// Voxel-local Set A blueprint. Same *idea* as column `.gvsecrt`, but
/// a slim schema (no wires / full Genome) so we stay isolated.
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
                PlacedModule {
                    x: 8,
                    y: 8,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 9,
                    y: 8,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
            ],
            genome: Genome::default(),
            name: "plant".into(),
            notes: "Set D minimal land plant".into(),
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

    pub fn is_valid_atom(&self) -> bool {
        self.nucleus_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() == 0
    }

    pub fn is_valid_plant(&self) -> bool {
        self.nucleus_count() >= 1
            && self.photosystem_count() >= 1
            && self.root_count() >= 1
    }

    pub fn is_valid_creature(&self) -> bool {
        self.is_valid_atom() || self.is_valid_plant()
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
        assert!(rel.iter().any(|&(dx, dy, m)| m == ModuleId::Root && dy < 0 && dx == 0));
        assert!(rel.iter().any(|&(_, dy, m)| m == ModuleId::Stem && dy > 0));
    }
}
