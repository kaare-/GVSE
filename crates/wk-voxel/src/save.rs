//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Full-sim snapshot save / load (postcard bytes on disk).
//!
//! Lives in wk-voxel (not wk-io) so the greenfield stack stays
//! isolated. Format is intentionally independent of column-GVSE
//! `.gvse` / scenario files.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::clouds::CloudStore;
use crate::grid::World;
use crate::humidity::Humidity;
use crate::organism::OrganismStore;
use crate::temperature::Temperature;
use crate::wind::Wind;
use crate::worldgen::WorldgenParams;

/// Directory under the process cwd for demo saves.
pub const SIM_SAVE_DIR: &str = "saves";
/// File extension for voxel sim snapshots.
pub const SIM_SAVE_EXT: &str = "gvsesim";
/// Bump when the postcard shape changes incompatibly.
///
/// `Cell` is a fixed 4-byte layout (`material`, `sat`, `flags`, `_pad`).
/// Widening `_pad` or adding fields is a schema bump and needs a
/// migration path for `.gvsesim` files on disk.
///
/// v2: `World.hydro` ([`wk_material::HydroOverrides`]) saved with the sim.
/// v3: live `Atom` no longer stores a vestigial `Genome` field (Wave S);
/// plant knobs live on `body_traits` / `body_plan`. Demo `.gvsesim` from
/// v2 are not loadable (no organism shim).
/// v4: `Atom` / `Corpse` carry `body_integrity` (Wave V stem topple).
/// v5: `PixelTraits` / `BodyPlan` gain `host_leave_fraction` (Wave Y).
/// v6: `Atom.stem_wetness` for epiphyte drink (Wave Z).
/// v7: `PixelTraits` / `BodyPlan` gain `attach_prefer` (Wave AB).
/// v8: `World.preferential_root` ghost-path overlay (Wave AC).
pub const SIM_SCHEMA_VERSION: u32 = 8;

/// Serializable capture of a running voxel demo scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SimSnapshot {
    pub schema_version: u32,
    pub params: WorldgenParams,
    pub world: World,
    pub humidity: Humidity,
    pub wind: Wind,
    pub temperature: Temperature,
    pub clouds: CloudStore,
    pub organisms: OrganismStore,
}

impl SimSnapshot {
    pub fn new(
        params: WorldgenParams,
        world: World,
        humidity: Humidity,
        wind: Wind,
        temperature: Temperature,
        clouds: CloudStore,
        organisms: OrganismStore,
    ) -> Self {
        Self {
            schema_version: SIM_SCHEMA_VERSION,
            params,
            world,
            humidity,
            wind,
            temperature,
            clouds,
            organisms,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| e.to_string())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        let snap: Self = postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        if snap.schema_version > SIM_SCHEMA_VERSION {
            return Err(format!(
                "sim schema {} newer than supported {}",
                snap.schema_version, SIM_SCHEMA_VERSION
            ));
        }
        Ok(snap)
    }

    pub fn save_path(name: &str) -> PathBuf {
        let safe: String = name
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let safe = if safe.is_empty() {
            "world".to_string()
        } else {
            safe
        };
        PathBuf::from(SIM_SAVE_DIR).join(format!("{safe}.{SIM_SAVE_EXT}"))
    }

    pub fn save_to_disk(&self, name: &str) -> Result<PathBuf, String> {
        std::fs::create_dir_all(SIM_SAVE_DIR).map_err(|e| e.to_string())?;
        let path = Self::save_path(name);
        std::fs::write(&path, self.to_bytes()?).map_err(|e| e.to_string())?;
        Ok(path)
    }

    pub fn load_from_disk(path: &Path) -> Result<Self, String> {
        let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
        Self::from_bytes(&bytes)
    }

    pub fn list_disk() -> Vec<PathBuf> {
        let Ok(rd) = std::fs::read_dir(SIM_SAVE_DIR) else {
            return Vec::new();
        };
        let mut out: Vec<_> = rd
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().and_then(|s| s.to_str()) == Some(SIM_SAVE_EXT))
            .collect();
        out.sort();
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Cell;
    use wk_material::MaterialId;

    #[test]
    fn snapshot_round_trips_postcard() {
        let params = WorldgenParams {
            width_cols: 64,
            sky_ceiling_y: 64,
            sea_level_y: 20,
            ..WorldgenParams::default()
        };
        let mut world = World::new(params.seed);
        world.set_cell(3, 4, Cell::solid(MaterialId::Sand));
        world.set_cell(3, 5, Cell::water());
        world.tick = 42;
        world.hydro.set_porosity(MaterialId::Sand, 90);
        let humidity = Humidity::with_world_bounds(
            4,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
        );
        let wind = Wind::climate(
            4,
            0.05,
            params.seed,
            params.width_cols,
            params.sea_level_y,
            params.bedrock_floor_y,
            params.sky_ceiling_y,
            params.wrap_x,
        );
        let temperature = Temperature::with_world_bounds(
            4,
            0,
            params.bedrock_floor_y,
            params.width_cols,
            params.sky_ceiling_y,
            params.seed,
            params.width_cols,
            params.sea_level_y,
            params.wrap_x,
        );
        let snap = SimSnapshot::new(
            params,
            world,
            humidity,
            wind,
            temperature,
            CloudStore::new(),
            OrganismStore::new(),
        );
        let loaded = SimSnapshot::from_bytes(&snap.to_bytes().unwrap()).unwrap();
        assert_eq!(loaded.schema_version, SIM_SCHEMA_VERSION);
        assert_eq!(loaded.params.width_cols, 64);
        assert_eq!(loaded.world.tick, 42);
        assert_eq!(
            loaded.world.get_cell(3, 4).map(|c| c.material),
            Some(MaterialId::Sand)
        );
        assert!(loaded.world.get_cell(3, 5).unwrap().sat.is_full());
        assert_eq!(
            loaded.world.hydro.slots[MaterialId::Sand as usize].porosity,
            Some(90)
        );
    }
}
