//! Creature blueprint (MS-Paint drawing) + postcard save format.
//! See `docs/organism/EDITOR.md`.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

use crate::module::{LaneId, ModuleId};
use crate::Genome;

pub const BLUEPRINT_SCHEMA_VERSION: u16 = 1;
pub const BLUEPRINT_DIR: &str = "blueprints";
pub const BLUEPRINT_EXT: &str = "gvsecrt";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum WireKind {
    Axon = 0,
    Hypha = 1,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PlacedModule {
    pub x: i16,
    pub y: i16,
    pub lane: LaneId,
    pub module: ModuleId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wire {
    pub from_pixel_idx: u16,
    pub to_pixel_idx: u16,
    pub kind: WireKind,
    pub sign: i8,
    pub weight: f32,
    pub delay: u8,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Blueprint {
    pub schema_version: u16,
    pub canvas_w: u16,
    pub canvas_h: u16,
    pub modules: Vec<PlacedModule>,
    pub wires: Vec<Wire>,
    pub genome: Genome,
    pub name: String,
    pub notes: String,
}

impl Blueprint {
    pub fn empty(name: impl Into<String>) -> Self {
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: Vec::new(),
            wires: Vec::new(),
            genome: Genome::default(),
            name: name.into(),
            notes: String::new(),
        }
    }

    /// Canonical Atom: nucleus + photosystem at Mid lane origin.
    pub fn atom(genome: Genome) -> Self {
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
            wires: Vec::new(),
            genome,
            name: "atom".into(),
            notes: "Set A Atom".into(),
        }
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

    pub fn stem_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Stem)
            .count()
    }

    pub fn digest_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Digest)
            .count()
    }

    pub fn hypha_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Hypha)
            .count()
    }

    pub fn nucleus_count(&self) -> usize {
        self.modules
            .iter()
            .filter(|m| m.module == ModuleId::Nucleus)
            .count()
    }

    /// Minimal land plant (C): leaf + stem + nucleus crown + root anchor.
    pub fn minimal_plant(genome: Genome) -> Self {
        Self {
            schema_version: BLUEPRINT_SCHEMA_VERSION,
            canvas_w: 16,
            canvas_h: 16,
            modules: vec![
                PlacedModule {
                    x: 0,
                    y: 3,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 1,
                    y: 3,
                    lane: LaneId::Mid,
                    module: ModuleId::Photosystem,
                },
                PlacedModule {
                    x: 0,
                    y: 2,
                    lane: LaneId::Mid,
                    module: ModuleId::Stem,
                },
                PlacedModule {
                    x: 0,
                    y: 1,
                    lane: LaneId::Mid,
                    module: ModuleId::Stem,
                },
                PlacedModule {
                    x: 0,
                    y: 0,
                    lane: LaneId::Mid,
                    module: ModuleId::Nucleus,
                },
                PlacedModule {
                    x: 0,
                    y: -1,
                    lane: LaneId::Mid,
                    module: ModuleId::Root,
                },
            ],
            wires: Vec::new(),
            genome,
            name: "plant".into(),
            notes: "Set D minimal land plant".into(),
        }
    }

    /// Minimal litter fungus (E): nucleus + digest + a short hypha thread.
    pub fn minimal_fungus(genome: Genome) -> Self {
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
            ],
            wires: Vec::new(),
            genome,
            name: "fungus".into(),
            notes: "Set E litter fungus".into(),
        }
    }

    pub fn is_valid_atom(&self) -> bool {
        self.nucleus_count() >= 1 && self.photosystem_count() >= 1
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

    /// Set A "algae" — photo Atom with no root/stem/digest. Spawns in water.
    pub fn is_plankton(&self) -> bool {
        self.is_valid_atom()
            && !self.is_fungus()
            && !self
                .modules
                .iter()
                .any(|m| matches!(m.module, ModuleId::Root | ModuleId::Stem))
    }

    /// Land habit once roots or stems are present (Set D+).
    pub fn is_rooted(&self) -> bool {
        self.modules
            .iter()
            .any(|m| matches!(m.module, ModuleId::Root | ModuleId::Stem))
    }

    /// Detritus habit (Set E): Digest chassis on litter / Organic.
    pub fn is_fungus(&self) -> bool {
        self.is_valid_fungus()
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
        let bytes = self.to_bytes()?;
        std::fs::write(&path, bytes).map_err(|e| e.to_string())?;
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
        let mut out = Vec::new();
        for e in rd.flatten() {
            let p = e.path();
            if p.extension().and_then(|s| s.to_str()) == Some(BLUEPRINT_EXT) {
                out.push(p);
            }
        }
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_round_trips_postcard() {
        let bp = Blueprint::atom(Genome::default());
        let bytes = bp.to_bytes().unwrap();
        let loaded = Blueprint::from_bytes(&bytes).unwrap();
        assert_eq!(loaded.modules.len(), 2);
        assert_eq!(loaded.name, "atom");
        assert!(loaded.is_valid_atom());
    }
}
