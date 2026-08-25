//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Full-sim snapshot save / load (postcard bytes on disk).
//!
//! Lives in wk-voxel (not wk-io) so the greenfield stack stays
//! isolated. Format is intentionally independent of column-GVSE
//! `.gvse` / scenario files.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wk_material::HydroOverrides;

use crate::carbon::CarbonBudget;
use crate::chunk::{Chunk, ChunkCoord};
use crate::clouds::CloudStore;
use crate::grid::{World, WorldSeed};
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
/// v3: `MaterialId::Soil` + mycelium intensity in `Cell::_pad` on Organic
/// (HydroOverrides slot table grows with [`wk_material::MATERIAL_COUNT`]).
/// v4: `MaterialId::LooseLimestone` (MATERIAL_COUNT 13→14).
/// v5: [`CarbonBudget`] (atm + dissolved CO₂ buckets) saved with the sim.
/// v6: [`World::mycelium_lineage`] (editor/spore fruiting-body stamps) +
/// mycelium cream on porous mineral hosts (Soil/Sand/…).
/// v7: [`World::mycelium_strains`] per-cell strain ids for overlay colors.
/// v8: mycelium_strains become multi-share lists `(strain, intensity)`.
/// v9: [`World::mycelium_energy`] sparse network sugar / glucose analog.
/// v10: [`World::sym_net_flow`] strain-keyed symbiont exchange counters.
/// v11: sym_net_flow gains harvest (water_in / sugar_out) ledger fields.
/// v12: [`World::mycelium_strain_lineage`] strain→treaty map for strain trade.
pub const SIM_SCHEMA_VERSION: u32 = 12;

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
    /// Crude atmosphere + dissolved CO₂ buckets.
    #[serde(default)]
    pub carbon: CarbonBudget,
}

/// Pre-v6 world (no mycelium_lineage map).
#[derive(Debug, Clone, Deserialize)]
struct WorldV5 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
}

impl WorldV5 {
    fn into_world(self) -> World {
        World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: crate::fungi::MyceliumLineageMap::default(),
            mycelium_strains: HashMap::new(),
            next_mycelium_strain_id: 1,
            mycelium_energy: HashMap::new(),
            sym_net_flow: HashMap::new(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        }
    }
}

/// Pre-v7 world (lineage, no strain ownership map).
#[derive(Debug, Clone, Deserialize)]
struct WorldV6 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
    #[serde(default)]
    mycelium_lineage: crate::fungi::MyceliumLineageMap,
}

impl WorldV6 {
    fn into_world(self) -> World {
        World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: self.mycelium_lineage,
            mycelium_strains: HashMap::new(),
            next_mycelium_strain_id: 1,
            mycelium_energy: HashMap::new(),
            sym_net_flow: HashMap::new(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        }
    }
}

/// Pre-v8 world: single strain id per cell (not multi-share).
#[derive(Debug, Clone, Deserialize)]
struct WorldV7 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
    #[serde(default)]
    mycelium_lineage: crate::fungi::MyceliumLineageMap,
    #[serde(default)]
    mycelium_strains: HashMap<(i32, i32), u32>,
    #[serde(default)]
    next_mycelium_strain_id: u32,
}

impl WorldV7 {
    fn into_world(self) -> World {
        let sole = self.mycelium_strains;
        let mut world = World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: self.mycelium_lineage,
            mycelium_strains: HashMap::new(),
            next_mycelium_strain_id: self.next_mycelium_strain_id.max(1),
            mycelium_energy: HashMap::new(),
            sym_net_flow: HashMap::new(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        };
        // Promote sole ownership → one share matching current `_pad`.
        for ((gx, gy), strain) in sole {
            let amt = world
                .get_cell(gx, gy)
                .map(|c| c.mycelium())
                .unwrap_or(1)
                .max(1);
            world
                .mycelium_strains
                .insert((gx, gy), vec![(strain, amt)]);
        }
        world
    }
}

/// Pre-v9 world (strains, no network energy map).
#[derive(Debug, Clone, Deserialize)]
struct WorldV8 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
    #[serde(default)]
    mycelium_lineage: crate::fungi::MyceliumLineageMap,
    #[serde(default)]
    mycelium_strains: HashMap<(i32, i32), Vec<(u32, u8)>>,
    #[serde(default)]
    next_mycelium_strain_id: u32,
}

impl WorldV8 {
    fn into_world(self) -> World {
        World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: self.mycelium_lineage,
            mycelium_strains: self.mycelium_strains,
            next_mycelium_strain_id: self.next_mycelium_strain_id.max(1),
            mycelium_energy: HashMap::new(),
            sym_net_flow: HashMap::new(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        }
    }
}

/// Pre-v12 world (has sym_net_flow harvest fields, no strain→lineage map).
#[derive(Debug, Clone, Deserialize)]
struct WorldV11 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
    #[serde(default)]
    mycelium_lineage: crate::fungi::MyceliumLineageMap,
    #[serde(default)]
    mycelium_strains: HashMap<(i32, i32), Vec<(u32, u8)>>,
    #[serde(default)]
    next_mycelium_strain_id: u32,
    #[serde(default)]
    mycelium_energy: HashMap<(i32, i32), u8>,
    #[serde(default)]
    sym_net_flow: HashMap<u32, crate::symbiosis::SymNetFlow>,
}

impl WorldV11 {
    fn into_world(self) -> World {
        World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: self.mycelium_lineage,
            mycelium_strains: self.mycelium_strains,
            next_mycelium_strain_id: self.next_mycelium_strain_id.max(1),
            mycelium_energy: self.mycelium_energy,
            sym_net_flow: self.sym_net_flow,
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        }
    }
}

/// Pre-v12 postcard (no mycelium_strain_lineage).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV11 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV11,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v11 network ledger (supply-only counters).
#[derive(Debug, Clone, Copy, Deserialize)]
struct SymNetFlowV10 {
    water_out_total: u32,
    sugar_in_total: u32,
    water_out_last: u8,
    sugar_in_last: u8,
    last_tick: u64,
}

impl SymNetFlowV10 {
    fn into_current(self) -> crate::symbiosis::SymNetFlow {
        crate::symbiosis::SymNetFlow {
            water_out_total: self.water_out_total,
            sugar_in_total: self.sugar_in_total,
            water_in_total: 0,
            sugar_out_total: 0,
            water_out_last: self.water_out_last,
            sugar_in_last: self.sugar_in_last,
            water_in_last: 0,
            sugar_out_last: 0,
            last_tick: self.last_tick,
        }
    }
}

/// Pre-v11 world (sym_net_flow without harvest ledger fields).
#[derive(Debug, Clone, Deserialize)]
struct WorldV10 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
    #[serde(default)]
    mycelium_lineage: crate::fungi::MyceliumLineageMap,
    #[serde(default)]
    mycelium_strains: HashMap<(i32, i32), Vec<(u32, u8)>>,
    #[serde(default)]
    next_mycelium_strain_id: u32,
    #[serde(default)]
    mycelium_energy: HashMap<(i32, i32), u8>,
    #[serde(default)]
    sym_net_flow: HashMap<u32, SymNetFlowV10>,
}

impl WorldV10 {
    fn into_world(self) -> World {
        World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: self.mycelium_lineage,
            mycelium_strains: self.mycelium_strains,
            next_mycelium_strain_id: self.next_mycelium_strain_id.max(1),
            mycelium_energy: self.mycelium_energy,
            sym_net_flow: self
                .sym_net_flow
                .into_iter()
                .map(|(k, v)| (k, v.into_current()))
                .collect(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        }
    }
}

/// Pre-v11 postcard (supply-only sym_net_flow).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV10 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV10,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v10 world (network sugar, no symbiont flow counters).
#[derive(Debug, Clone, Deserialize)]
struct WorldV9 {
    seed: WorldSeed,
    chunks: HashMap<ChunkCoord, Chunk>,
    tick: u64,
    wrap_width: Option<i32>,
    #[serde(default)]
    soft_litter: HashMap<i32, u16>,
    #[serde(default)]
    spore_bank: crate::spore_bank::SporeBank,
    #[serde(default)]
    hydro: HydroOverrides,
    #[serde(default)]
    mycelium_lineage: crate::fungi::MyceliumLineageMap,
    #[serde(default)]
    mycelium_strains: HashMap<(i32, i32), Vec<(u32, u8)>>,
    #[serde(default)]
    next_mycelium_strain_id: u32,
    #[serde(default)]
    mycelium_energy: HashMap<(i32, i32), u8>,
}

impl WorldV9 {
    fn into_world(self) -> World {
        World {
            seed: self.seed,
            chunks: self.chunks.into_iter().collect(),
            tick: self.tick,
            wrap_width: self.wrap_width,
            soft_litter: self.soft_litter,
            spore_bank: self.spore_bank,
            hydro: self.hydro,
            mycelium_lineage: self.mycelium_lineage,
            mycelium_strains: self.mycelium_strains,
            next_mycelium_strain_id: self.next_mycelium_strain_id.max(1),
            mycelium_energy: self.mycelium_energy,
            sym_net_flow: HashMap::new(),
            mycelium_strain_lineage: HashMap::new(),
            competent_cell_moves: Vec::new(),
            competent_moved_cells: Vec::new(),
            competent_wake: Vec::new(),
            competent_settled: Default::default(),
            chunk_cache_id: Default::default(),
        }
    }
}

/// Pre-v10 postcard (no sym_net_flow).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV9 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV9,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v9 postcard (no mycelium_energy).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV8 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV8,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v8 postcard (single strain id per cell).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV7 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV7,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v7 postcard shape (lineage, no strains).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV6 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV6,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v6 postcard shape (carbon, no mycelium_lineage).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV5 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV5,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
    #[serde(default)]
    carbon: CarbonBudget,
}

/// Pre-v5 postcard shape (no carbon field).
#[derive(Debug, Clone, Deserialize)]
struct SimSnapshotV4 {
    schema_version: u32,
    params: WorldgenParams,
    world: WorldV5,
    humidity: Humidity,
    wind: Wind,
    temperature: Temperature,
    clouds: CloudStore,
    organisms: OrganismStore,
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
        carbon: CarbonBudget,
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
            carbon,
        }
    }

    pub fn to_bytes(&self) -> Result<Vec<u8>, String> {
        postcard::to_allocvec(self).map_err(|e| e.to_string())
    }

    pub fn from_bytes(bytes: &[u8]) -> Result<Self, String> {
        // Prefer current schema; fall back v11 → v10 → v9 → …
        if let Ok(snap) = postcard::from_bytes::<Self>(bytes) {
            if snap.schema_version > SIM_SCHEMA_VERSION {
                return Err(format!(
                    "sim schema {} newer than supported {}",
                    snap.schema_version, SIM_SCHEMA_VERSION
                ));
            }
            return Ok(snap);
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV11>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV10>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV9>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV8>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV7>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV6>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        if let Ok(old) = postcard::from_bytes::<SimSnapshotV5>(bytes) {
            return Ok(Self {
                schema_version: SIM_SCHEMA_VERSION,
                params: old.params,
                world: old.world.into_world(),
                humidity: old.humidity,
                wind: old.wind,
                temperature: old.temperature,
                clouds: old.clouds,
                organisms: old.organisms,
                carbon: old.carbon,
            });
        }
        let old: SimSnapshotV4 =
            postcard::from_bytes(bytes).map_err(|e| e.to_string())?;
        Ok(Self {
            schema_version: SIM_SCHEMA_VERSION,
            params: old.params,
            world: old.world.into_world(),
            humidity: old.humidity,
            wind: old.wind,
            temperature: old.temperature,
            clouds: old.clouds,
            organisms: old.organisms,
            carbon: CarbonBudget::default(),
        })
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

    fn demo_climate(params: &WorldgenParams) -> (Humidity, Wind, Temperature) {
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
        (humidity, wind, temperature)
    }

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
        let (humidity, wind, temperature) = demo_climate(&params);
        let carbon = CarbonBudget {
            atmosphere: 777.0,
            dissolved: 88.0,
        };
        let snap = SimSnapshot::new(
            params,
            world,
            humidity,
            wind,
            temperature,
            CloudStore::new(),
            OrganismStore::new(),
            carbon,
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
        assert_eq!(loaded.carbon.atmosphere, 777.0);
        assert_eq!(loaded.carbon.dissolved, 88.0);
    }

    #[test]
    fn snapshot_loads_v4_bytes_with_ambient_carbon() {
        // Pre-lineage world shape (schema ≤5) — must not include mycelium_lineage.
        #[derive(Serialize)]
        struct WorldOut {
            seed: WorldSeed,
            chunks: HashMap<ChunkCoord, Chunk>,
            tick: u64,
            wrap_width: Option<i32>,
            soft_litter: HashMap<i32, u16>,
            spore_bank: crate::spore_bank::SporeBank,
            hydro: HydroOverrides,
        }
        #[derive(Serialize)]
        struct V4Out {
            schema_version: u32,
            params: WorldgenParams,
            world: WorldOut,
            humidity: Humidity,
            wind: Wind,
            temperature: Temperature,
            clouds: CloudStore,
            organisms: OrganismStore,
        }
        let params = WorldgenParams {
            width_cols: 32,
            sky_ceiling_y: 32,
            sea_level_y: 10,
            ..WorldgenParams::default()
        };
        let mut world = World::new(params.seed);
        world.tick = 7;
        let (humidity, wind, temperature) = demo_climate(&params);
        let bytes = postcard::to_allocvec(&V4Out {
            schema_version: 4,
            params,
            world: WorldOut {
                seed: world.seed,
                chunks: world.chunks.into_iter().collect(),
                tick: world.tick,
                wrap_width: world.wrap_width,
                soft_litter: world.soft_litter,
                spore_bank: world.spore_bank,
                hydro: world.hydro,
            },
            humidity,
            wind,
            temperature,
            clouds: CloudStore::new(),
            organisms: OrganismStore::new(),
        })
        .unwrap();
        let loaded = SimSnapshot::from_bytes(&bytes).expect("migrate v4");
        assert_eq!(loaded.world.tick, 7);
        assert_eq!(loaded.carbon, CarbonBudget::default());
        assert_eq!(loaded.schema_version, SIM_SCHEMA_VERSION);
        assert!(loaded.world.mycelium_lineage.cells.is_empty());
    }

    #[test]
    fn snapshot_loads_v5_bytes_with_empty_lineage() {
        #[derive(Serialize)]
        struct WorldOut {
            seed: WorldSeed,
            chunks: HashMap<ChunkCoord, Chunk>,
            tick: u64,
            wrap_width: Option<i32>,
            soft_litter: HashMap<i32, u16>,
            spore_bank: crate::spore_bank::SporeBank,
            hydro: HydroOverrides,
        }
        #[derive(Serialize)]
        struct V5Out {
            schema_version: u32,
            params: WorldgenParams,
            world: WorldOut,
            humidity: Humidity,
            wind: Wind,
            temperature: Temperature,
            clouds: CloudStore,
            organisms: OrganismStore,
            carbon: CarbonBudget,
        }
        let params = WorldgenParams {
            width_cols: 32,
            sky_ceiling_y: 32,
            sea_level_y: 10,
            ..WorldgenParams::default()
        };
        let mut world = World::new(params.seed);
        world.tick = 9;
        let (humidity, wind, temperature) = demo_climate(&params);
        let bytes = postcard::to_allocvec(&V5Out {
            schema_version: 5,
            params,
            world: WorldOut {
                seed: world.seed,
                chunks: world.chunks.into_iter().collect(),
                tick: world.tick,
                wrap_width: world.wrap_width,
                soft_litter: world.soft_litter,
                spore_bank: world.spore_bank,
                hydro: world.hydro,
            },
            humidity,
            wind,
            temperature,
            clouds: CloudStore::new(),
            organisms: OrganismStore::new(),
            carbon: CarbonBudget {
                atmosphere: 12.0,
                dissolved: 3.0,
            },
        })
        .unwrap();
        let loaded = SimSnapshot::from_bytes(&bytes).expect("migrate v5");
        assert_eq!(loaded.world.tick, 9);
        assert_eq!(loaded.carbon.atmosphere, 12.0);
        assert!(loaded.world.mycelium_lineage.cells.is_empty());
        assert_eq!(loaded.schema_version, SIM_SCHEMA_VERSION);
    }
}
