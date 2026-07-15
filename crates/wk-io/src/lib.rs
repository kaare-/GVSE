//! Save/load format v1.

use serde::{Deserialize, Serialize};
use wk_material::CHUNK_W;
use wk_sim::Simulation;
use wk_world::climate::ClimateSettings;
use wk_world::column::{Activity, ResidualBucket, SedimentLoad};
use wk_world::marker::Marker;
use wk_world::weather::{Cloud, WeatherSettings};
use wk_world::world::{MassAudit, World};
use wk_world::Layer;

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SaveFileV1 {
    pub schema_version: u32,
    pub build_id: [u8; 8],
    pub world_seed: u64,
    pub sim_tick: u64,
    pub sea_level: f32,
    pub rain_rate: f32,
    pub rain_enabled: bool,
    pub mass_audit: MassAudit,
    pub chunks: Vec<(i32, ChunkSnapshot)>,
    pub markers: Vec<Marker>,
    pub next_marker_id: u32,
    pub climate: ClimateSettings,
    #[serde(default)]
    pub weather: WeatherSettings,
    #[serde(default)]
    pub clouds: Vec<Cloud>,
    #[serde(default)]
    pub next_cloud_spawn_tick: u64,
    pub extensions: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChunkSnapshot {
    pub coord: i32,
    pub bedrock_y: f32,
    pub columns: Vec<ColumnSnapshot>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ColumnSnapshot {
    pub surface_y: f32,
    pub layers: Vec<LayerSnapshot>,
    pub moisture: i64,
    pub sediment: SedimentLoad,
    pub residual: ResidualBucket,
    pub activity: u8,
    pub marker: Option<u32>,
    /// Legacy fields from before Water/Ice/Snow became first-class
    /// stratigraphic materials. Preserved as `#[serde(default)]` so
    /// older save files still deserialize; on `restore_world` they
    /// get migrated into their equivalent top-of-stack layers so the
    /// live column state stays consistent.
    #[serde(default)]
    pub legacy_surface_water: i64,
    #[serde(default)]
    pub legacy_ice: i64,
    #[serde(default)]
    pub legacy_snow: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerSnapshot {
    pub material: u8,
    pub thickness: i64,
    pub age_start: u64,
    pub age_end: u64,
}

pub fn snapshot_world(world: &World, sim_tick: u64) -> SaveFileV1 {
    let chunks = world
        .chunks
        .iter()
        .map(|(&coord, chunk)| {
            let columns: Vec<_> = chunk
                .columns
                .iter()
                .map(|col| ColumnSnapshot {
                    surface_y: col.surface_y,
                    layers: (0..col.layer_count as usize)
                        .map(|i| LayerSnapshot {
                            material: col.layers[i].material as u8,
                            thickness: col.layers[i].thickness,
                            age_start: col.layers[i].age_start,
                            age_end: col.layers[i].age_end,
                        })
                        .collect(),
                    moisture: col.moisture,
                    sediment: col.sediment,
                    residual: col.residual,
                    activity: match col.activity {
                        Activity::Dormant => 0,
                        Activity::HydrologyActive => 1,
                    },
                    marker: col.marker.map(|m| m.0),
                    legacy_surface_water: 0,
                    legacy_ice: 0,
                    legacy_snow: 0,
                })
                .collect();
            (
                coord,
                ChunkSnapshot {
                    coord,
                    bedrock_y: chunk.bedrock_y,
                    columns,
                },
            )
        })
        .collect();

    SaveFileV1 {
        schema_version: SCHEMA_VERSION,
        build_id: *b"wk0.1.0\0",
        world_seed: world.seed,
        sim_tick,
        sea_level: world.sea_level,
        rain_rate: world.rain_rate,
        rain_enabled: world.rain_enabled,
        mass_audit: world.mass_audit.clone(),
        chunks,
        markers: world.markers.clone(),
        next_marker_id: world.next_marker_id,
        climate: world.climate.clone(),
        weather: world.weather.clone(),
        clouds: world.clouds.clone(),
        next_cloud_spawn_tick: world.next_cloud_spawn_tick,
        extensions: Vec::new(),
    }
}

pub fn restore_world(save: &SaveFileV1) -> (World, u64) {
    use wk_material::MaterialId;
    use wk_world::chunk::Chunk;
    use wk_world::column::MarkerId;

    let mut world = World::new(save.world_seed);
    world.sea_level = save.sea_level;
    world.rain_rate = save.rain_rate;
    world.rain_enabled = save.rain_enabled;
    world.mass_audit = save.mass_audit.clone();
    world.markers = save.markers.clone();
    world.next_marker_id = save.next_marker_id;
    world.climate = save.climate.clone();
    world.weather = save.weather.clone();
    world.clouds = save.clouds.clone();
    world.next_cloud_spawn_tick = save.next_cloud_spawn_tick;

    for (coord, snap) in &save.chunks {
        let mut chunk = Chunk::new(*coord, snap.bedrock_y);
        for (i, cs) in snap.columns.iter().enumerate().take(CHUNK_W) {
            let col = &mut chunk.columns[i];
            col.surface_y = cs.surface_y;
            col.moisture = cs.moisture;
            col.sediment = cs.sediment;
            col.residual = cs.residual;
            col.activity = if cs.activity == 0 {
                Activity::Dormant
            } else {
                Activity::HydrologyActive
            };
            col.marker = cs.marker.map(MarkerId);
            col.layer_count = cs.layers.len().min(wk_material::MAX_LAYERS) as u8;
            for (j, ls) in cs.layers.iter().enumerate().take(wk_material::MAX_LAYERS) {
                col.layers[j] = Layer {
                    material: MaterialId::from_u8(ls.material).unwrap_or(MaterialId::Sand),
                    thickness: ls.thickness,
                    age_start: ls.age_start,
                    age_end: ls.age_end,
                };
            }
            // Migrate any pre-unification scalars into equivalent top-of-
            // stack layers so restored old saves land in the new state
            // shape. Order matters: surface_water goes on last (ends up
            // topmost) — same as a fresh-world puddle.
            let now = save.sim_tick;
            if cs.legacy_snow > 0 {
                col.deposit_to_top(MaterialId::Snow, cs.legacy_snow, now);
            }
            if cs.legacy_ice > 0 {
                col.deposit_to_top(MaterialId::Ice, cs.legacy_ice, now);
            }
            if cs.legacy_surface_water > 0 {
                col.deposit_to_top(MaterialId::Water, cs.legacy_surface_water, now);
            }
            col.recompute_surface_y(snap.bedrock_y);
        }
        world.insert_chunk(chunk);
    }

    (world, save.sim_tick)
}

pub fn save_to_bytes(world: &World, sim_tick: u64) -> Vec<u8> {
    let save = snapshot_world(world, sim_tick);
    postcard::to_allocvec(&save).expect("serialize save")
}

pub fn load_from_bytes(bytes: &[u8]) -> Result<(World, u64), postcard::Error> {
    let save: SaveFileV1 = postcard::from_bytes(bytes)?;
    Ok(restore_world(&save))
}

pub fn save_simulation(world: &World, sim: &Simulation) -> Vec<u8> {
    save_to_bytes(world, sim.clock.tick)
}

pub fn load_simulation(bytes: &[u8]) -> Result<(World, Simulation), postcard::Error> {
    let (world, tick) = load_from_bytes(bytes)?;
    let mut sim = Simulation::new(&world);
    sim.clock.tick = tick;
    Ok((world, sim))
}
