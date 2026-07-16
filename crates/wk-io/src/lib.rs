//! Save/load format.
//!
//! Schema v1: columns + climate + weather.
//! Schema v2: optional per-chunk field patches (thermal, humidity, …).
//! Older v1 bytes still load — new field slots use `#[serde(default)]`.

use serde::{Deserialize, Serialize};
use wk_material::CHUNK_W;
use wk_sim::Simulation;
use wk_world::climate::ClimateSettings;
use wk_world::column::{Activity, Ecology, ResidualBucket, SedimentLoad, Void, VoidOrigin};
use wk_world::fields::{
    DissolvedField, GroundwaterHeadField, HumidityField, PressureField, ThermalField, WindField,
};
use wk_world::marker::Marker;
use wk_world::weather::{Cloud, WeatherSettings};
use wk_world::world::{MassAudit, World};
use wk_world::Layer;

pub const SCHEMA_VERSION: u32 = 2;

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
    /// Stage 6 field patches. Absent in schema v1 saves → `None`.
    #[serde(default)]
    pub thermal: Option<ThermalField>,
    #[serde(default)]
    pub humidity: Option<HumidityField>,
    #[serde(default)]
    pub pressure: Option<PressureField>,
    #[serde(default)]
    pub wind: Option<WindField>,
    #[serde(default)]
    pub gw_head: Option<GroundwaterHeadField>,
    #[serde(default)]
    pub dissolved: Option<DissolvedField>,
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
    /// Stage 7 karst voids. Absent in older saves → empty.
    #[serde(default)]
    pub voids: Vec<VoidSnapshot>,
    /// Stage 8 ecology bucket. Absent in older saves → barren.
    #[serde(default)]
    pub ecology: EcologySnapshot,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EcologySnapshot {
    pub root_density: f32,
    pub leaf_area: f32,
    pub dead_biomass: i64,
    pub alive_biomass: i64,
    pub nutrient: f32,
    #[serde(default = "default_snap_air_co2")]
    pub air_co2: f32,
    #[serde(default = "default_snap_air_o2")]
    pub air_o2: f32,
    #[serde(default = "default_snap_water_co2")]
    pub water_co2: f32,
    #[serde(default = "default_snap_water_o2")]
    pub water_o2: f32,
}

fn default_snap_air_co2() -> f32 {
    1.0
}
fn default_snap_air_o2() -> f32 {
    1.0
}
fn default_snap_water_co2() -> f32 {
    0.85
}
fn default_snap_water_o2() -> f32 {
    0.90
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoidSnapshot {
    pub top_y: f32,
    pub height_m: f32,
    pub water_mass: i64,
    pub roof_material: u8,
    pub origin: u8,
    pub light: u8,
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
                    voids: col
                        .voids
                        .iter()
                        .map(|v| VoidSnapshot {
                            top_y: v.top_y,
                            height_m: v.height_m,
                            water_mass: v.water_mass,
                            roof_material: v.roof_material as u8,
                            origin: match v.origin {
                                VoidOrigin::Karst => 0,
                                VoidOrigin::Burrow => 1,
                                VoidOrigin::Collapse => 2,
                            },
                            light: v.light,
                        })
                        .collect(),
                    ecology: EcologySnapshot {
                        root_density: col.ecology.root_density,
                        leaf_area: col.ecology.leaf_area,
                        dead_biomass: col.ecology.dead_biomass,
                        alive_biomass: col.ecology.alive_biomass,
                        nutrient: col.ecology.nutrient,
                        air_co2: col.ecology.air_co2,
                        air_o2: col.ecology.air_o2,
                        water_co2: col.ecology.water_co2,
                        water_o2: col.ecology.water_o2,
                    },
                })
                .collect();
            (
                coord,
                ChunkSnapshot {
                    coord,
                    bedrock_y: chunk.bedrock_y,
                    columns,
                    thermal: chunk.thermal.clone(),
                    humidity: chunk.humidity.clone(),
                    pressure: chunk.pressure.clone(),
                    wind: chunk.wind.clone(),
                    gw_head: chunk.gw_head.clone(),
                    dissolved: chunk.dissolved.clone(),
                },
            )
        })
        .collect();

    SaveFileV1 {
        schema_version: SCHEMA_VERSION,
        build_id: *b"wk0.1.1\0",
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
            col.voids = cs
                .voids
                .iter()
                .map(|v| Void {
                    top_y: v.top_y,
                    height_m: v.height_m,
                    water_mass: v.water_mass,
                    roof_material: MaterialId::from_u8(v.roof_material)
                        .unwrap_or(MaterialId::Stone),
                    origin: match v.origin {
                        1 => VoidOrigin::Burrow,
                        2 => VoidOrigin::Collapse,
                        _ => VoidOrigin::Karst,
                    },
                    light: v.light,
                })
                .collect();
            col.ecology = Ecology {
                root_density: cs.ecology.root_density,
                leaf_area: cs.ecology.leaf_area,
                dead_biomass: cs.ecology.dead_biomass,
                alive_biomass: cs.ecology.alive_biomass,
                nutrient: cs.ecology.nutrient,
                air_co2: cs.ecology.air_co2,
                air_o2: cs.ecology.air_o2,
                water_co2: cs.ecology.water_co2,
                water_o2: cs.ecology.water_o2,
            };
            col.recompute_surface_y(snap.bedrock_y);
        }
        chunk.thermal = snap.thermal.clone();
        chunk.humidity = snap.humidity.clone();
        // Source buffer is scratch — rebuild empty if the field is present.
        chunk.humidity_source = snap
            .humidity
            .as_ref()
            .map(|h| h.0.zeros_like());
        chunk.pressure = snap.pressure.clone();
        chunk.wind = snap.wind.clone();
        chunk.gw_head = snap.gw_head.clone();
        chunk.dissolved = snap.dissolved.clone();
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

#[cfg(test)]
mod tests {
    use super::*;
    use wk_world::fields::ThermalField;
    use wk_world::terrain::generate_flat_sand;

    #[test]
    fn field_slots_default_none_on_fresh_chunk() {
        let chunk = wk_world::chunk::Chunk::new(0, 0.0);
        assert!(chunk.thermal.is_none());
        assert!(chunk.humidity.is_none());
        assert!(chunk.pressure.is_none());
        assert!(chunk.wind.is_none());
        assert!(chunk.gw_head.is_none());
        assert!(chunk.dissolved.is_none());
    }

    #[test]
    fn thermal_field_round_trips_through_save() {
        let mut world = World::new(42);
        world.sea_level = 10.0;
        let mut chunk = generate_flat_sand(0, 0.0, 20.0);
        let mut thermal = ThermalField::new_for_chunk(0, chunk.bedrock_y, world.sea_level, 12.0);
        thermal.0.set_cell(1, 2, 33.5);
        chunk.thermal = Some(thermal);
        world.insert_chunk(chunk);

        let bytes = save_to_bytes(&world, 99);
        let (world2, tick) = load_from_bytes(&bytes).expect("load");
        assert_eq!(tick, 99);
        let restored = world2.chunks.get(&0).unwrap();
        let field = restored.thermal.as_ref().expect("thermal present");
        assert!((field.0.cell_at(1, 2) - 33.5).abs() < 1e-4);
        assert!(restored.humidity.is_none());
    }
}
