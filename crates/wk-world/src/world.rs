use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wk_material::{CHUNK_W, MaterialId, MAX_LOADED_CHUNKS, MAX_MARKERS, MATERIAL_COUNT};

use crate::chunk::Chunk;
use crate::climate::{biome_for, temperature_at, Biome, ClimateSettings};
use crate::column::{Activity, Column, MarkerId, ResidualBucket, SedimentLoad};
use crate::fields::ThermalField;
use crate::marker::Marker;
use crate::weather::{Cloud, WeatherSettings};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MassAudit {
    pub by_material: [i64; MATERIAL_COUNT],
    pub evap_out_total: i64,
    pub sea_inject_total: i64,
    pub rain_inject_total: i64,
    pub boundary_out_total: i64,
    pub tick: u64,
}

impl MassAudit {
    pub fn total_tracked(&self) -> i64 {
        self.by_material.iter().sum()
    }

    pub fn bookkeeping_balance(&self) -> i64 {
        self.rain_inject_total + self.sea_inject_total
            - self.evap_out_total
            - self.boundary_out_total
    }
}

#[derive(Debug, Clone)]
pub struct World {
    pub seed: u64,
    pub sea_level: f32,
    pub rain_enabled: bool,
    pub rain_rate: f32,
    pub chunks: BTreeMap<i32, Chunk>,
    pub markers: Vec<Marker>,
    pub mass_audit: MassAudit,
    pub next_marker_id: u32,
    pub climate: ClimateSettings,
    pub weather: WeatherSettings,
    pub clouds: Vec<Cloud>,
    pub next_cloud_spawn_tick: u64,
    /// When true, chunks carry a `ThermalField` and the thermal
    /// subsystem / temperature accessors use it. Default false so
    /// existing scenario tests stay on the climate-only path.
    pub thermal_fields_enabled: bool,
    /// Dirichlet temperature (°C) imposed on the bottom row of every
    /// thermal field — a stand-in for Earth's geothermal heat.
    pub geothermal_bottom_c: f32,
}

impl World {
    /// Min/max surface elevation across all loaded columns.
    pub fn surface_bounds(&self) -> Option<(f32, f32)> {
        let mut min = f32::MAX;
        let mut max = f32::MIN;
        let mut any = false;
        for chunk in self.chunks.values() {
            for col in &chunk.columns {
                any = true;
                min = min.min(col.surface_y);
                max = max.max(col.surface_y);
            }
        }
        if any {
            Some((min, max))
        } else {
            None
        }
    }

    /// Inclusive world-x range covered by loaded chunks.
    pub fn world_x_bounds(&self) -> Option<(i32, i32)> {
        if self.chunks.is_empty() {
            return None;
        }
        let min_coord = *self.chunks.keys().next().unwrap();
        let max_coord = *self.chunks.keys().next_back().unwrap();
        let x_min = min_coord * CHUNK_W as i32;
        let x_max = (max_coord + 1) * CHUNK_W as i32 - 1;
        Some((x_min, x_max))
    }

    /// First world-x where terrain breaks the surface (for camera start).
    pub fn first_emergent_x(&self, sea_level: f32) -> Option<i32> {
        for chunk in self.chunks.values() {
            let base = chunk.world_x_base();
            for i in 0..CHUNK_W {
                if chunk.columns[i].surface_y > sea_level + 1.0 {
                    return Some(base + i as i32);
                }
            }
        }
        None
    }

    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            sea_level: 10.0,
            rain_enabled: false,
            rain_rate: 50.0,
            chunks: BTreeMap::new(),
            markers: Vec::new(),
            mass_audit: MassAudit::default(),
            next_marker_id: 1,
            climate: ClimateSettings::default(),
            weather: WeatherSettings::default(),
            clouds: Vec::new(),
            next_cloud_spawn_tick: 0,
            thermal_fields_enabled: false,
            geothermal_bottom_c: 55.0,
        }
    }

    /// Climate-only temperature at an elevation (sky / free-air model).
    /// Prefer [`Self::temperature_at_point`] when a column position is known
    /// so an active thermal field can be sampled.
    pub fn temperature_at(&self, surface_y: f32, tick: u64) -> f32 {
        temperature_at(surface_y, self.sea_level, tick, &self.climate)
    }

    /// Temperature at a world point. Samples the chunk thermal field when
    /// present; otherwise falls back to the climate function at `y_m`.
    pub fn temperature_at_point(&self, world_x: i32, y_m: f32, tick: u64) -> f32 {
        let coord = Self::chunk_coord_for_world_x(world_x);
        if let Some(chunk) = self.chunks.get(&coord) {
            if let Some(thermal) = &chunk.thermal {
                let x_m = world_x as f32 * wk_material::SAMPLE_WIDTH_M;
                return thermal.0.sample_bilinear(x_m, y_m);
            }
        }
        temperature_at(y_m, self.sea_level, tick, &self.climate)
    }

    /// Allocate and initialise a thermal field on every loaded chunk
    /// (no-op for chunks that already have one). Seeds each cell with a
    /// linear geothermal→sky gradient so the first ticks aren't a cold
    /// shock.
    pub fn enable_thermal_fields(&mut self) {
        self.thermal_fields_enabled = true;
        let sea = self.sea_level;
        let geo = self.geothermal_bottom_c;
        let climate = self.climate.clone();
        let coords: Vec<i32> = self.chunks.keys().copied().collect();
        for coord in coords {
            let Some(chunk) = self.chunks.get_mut(&coord) else {
                continue;
            };
            if chunk.thermal.is_some() {
                continue;
            }
            let mut field =
                ThermalField::new_for_chunk(coord, chunk.bedrock_y, sea, climate.base_temp_c);
            let w = field.0.width_cells as usize;
            let h = field.0.height_cells as usize;
            let origin_y = field.0.origin_y_m;
            let extent = (h as f32) * field.0.cell_size_m;
            let sky = temperature_at(sea, sea, 0, &climate);
            for cy in 0..h {
                for cx in 0..w {
                    let (_, y) = field.0.cell_center(cx, cy);
                    let t = if extent > 0.0 {
                        geo + (sky - geo) * ((y - origin_y) / extent).clamp(0.0, 1.0)
                    } else {
                        sky
                    };
                    field.0.set_cell(cx, cy, t);
                }
            }
            // Match halos to the seeded interior so the first stencil
            // step doesn't see a zero-halo cold wall.
            field.0.halo = wk_field::FieldHalo::zeros(field.0.width_cells, field.0.height_cells);
            for cy in 0..h {
                field.0.halo.left[cy] = field.0.cell_at(0, cy);
                field.0.halo.right[cy] = field.0.cell_at(w - 1, cy);
            }
            for cx in 0..w {
                field.0.halo.bottom[cx] = geo;
                field.0.halo.top[cx] = sky;
            }
            chunk.thermal = Some(field);
        }
    }

    pub fn biome_at(&self, surface_y: f32) -> Biome {
        biome_for(surface_y, self.sea_level)
    }

    pub fn insert_chunk(&mut self, chunk: Chunk) {
        if self.chunks.len() >= MAX_LOADED_CHUNKS && !self.chunks.contains_key(&chunk.coord) {
            if let Some(&farthest) = self.chunks.keys().next() {
                self.chunks.remove(&farthest);
            }
        }
        self.chunks.insert(chunk.coord, chunk);
    }

    pub fn get_chunk_mut(&mut self, coord: i32) -> Option<&mut Chunk> {
        self.chunks.get_mut(&coord)
    }

    pub fn get_chunk(&self, coord: i32) -> Option<&Chunk> {
        self.chunks.get(&coord)
    }

    pub fn chunk_coord_for_world_x(world_x: i32) -> i32 {
        let w = CHUNK_W as i32;
        if world_x >= 0 {
            world_x / w
        } else {
            (world_x - w + 1) / w
        }
    }

    pub fn local_x(world_x: i32) -> usize {
        let w = CHUNK_W as i32;
        let lx = world_x.rem_euclid(w);
        lx as usize
    }

    pub fn column_at_mut(&mut self, world_x: i32) -> Option<&mut Column> {
        let coord = Self::chunk_coord_for_world_x(world_x);
        let lx = Self::local_x(world_x);
        self.chunks.get_mut(&coord).map(|c| &mut c.columns[lx])
    }

    pub fn column_at(&self, world_x: i32) -> Option<&Column> {
        let coord = Self::chunk_coord_for_world_x(world_x);
        let lx = Self::local_x(world_x);
        self.chunks.get(&coord).map(|c| &c.columns[lx])
    }

    pub fn world_x_to_chunk_local(&self, world_x: i32) -> Option<(i32, usize)> {
        let coord = Self::chunk_coord_for_world_x(world_x);
        let lx = Self::local_x(world_x);
        self.chunks.get(&coord).map(|_| (coord, lx))
    }

    pub fn recompute_mass_audit(&mut self) {
        let mut audit = MassAudit {
            evap_out_total: self.mass_audit.evap_out_total,
            sea_inject_total: self.mass_audit.sea_inject_total,
            rain_inject_total: self.mass_audit.rain_inject_total,
            boundary_out_total: self.mass_audit.boundary_out_total,
            tick: self.mass_audit.tick,
            ..Default::default()
        };

        for chunk in self.chunks.values() {
            for col in &chunk.columns {
                // Every physical substance lives in `layers` now
                // (water/ice/snow included), so the audit is just a
                // straight sum over layer masses plus the moisture
                // side-channel and any sediment in suspension.
                for i in 0..col.layer_count as usize {
                    let m = col.layers[i].material.index();
                    audit.by_material[m] += col.layers[i].thickness;
                }
                audit.by_material[MaterialId::Water.index()] += col.moisture;
                if col.sediment.total > 0 {
                    audit.by_material[col.sediment.dominant.index()] += col.sediment.total;
                }
            }
        }
        self.mass_audit.by_material = audit.by_material;
    }

    pub fn add_marker(&mut self, world_x: i32, label: String, tick: u64) -> Option<MarkerId> {
        if self.markers.len() >= MAX_MARKERS {
            return None;
        }
        let id = MarkerId(self.next_marker_id);
        self.next_marker_id += 1;
        let pinned = self
            .column_at(world_x)
            .map(|_| 0u8)
            .unwrap_or(0);
        if let Some(col) = self.column_at_mut(world_x) {
            col.marker = Some(id);
        }
        self.markers.push(Marker {
            id,
            world_x,
            label: label.chars().take(32).collect(),
            created_tick: tick,
            pinned_layer_index: pinned,
        });
        Some(id)
    }

    pub fn active_chunk_coords(&self) -> Vec<i32> {
        self.chunks
            .values()
            .filter(|c| c.any_hydrology_active())
            .map(|c| c.coord)
            .collect()
    }

    pub fn wake_all(&mut self) {
        for chunk in self.chunks.values_mut() {
            chunk.set_all_active();
        }
    }
}

/// Read-only view for rendering. Water/ice/snow are exposed here as
/// convenience projections computed from the top of the layer stack;
/// they don't live as separate fields on `Column` any more.
#[derive(Debug, Clone)]
pub struct ColumnView {
    pub world_x: i32,
    pub surface_y: f32,
    pub bedrock_y: f32,
    pub layers: Vec<(MaterialId, i64, u64, u64)>,
    /// Mass (kg) of the top Water layer, or 0 if the top isn't water.
    pub surface_water: i64,
    pub moisture: i64,
    /// `moisture / moisture_cap` in [0, 1]. Used by the renderer to
    /// tint waterlogged solid layers so a saturated hillside reads as
    /// visibly wet.
    pub saturation: f32,
    /// Mass (kg) of the top Ice layer, or 0 if the top isn't ice.
    pub ice: i64,
    /// Mass (kg) of the top Snow layer, or 0 if the top isn't snow.
    pub snow: i64,
    pub sediment: SedimentLoad,
    pub activity: Activity,
    pub water_flux: i64,
    pub erosion_flux: i64,
    pub residual: ResidualBucket,
    pub temperature_c: f32,
    pub biome: Biome,
}

#[derive(Debug, Clone)]
pub struct MarkerView {
    pub world_x: i32,
    pub label: String,
    pub pinned_layer_index: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum OverlayMode {
    #[default]
    None,
    WaterFlux,
    Erosion,
    Activity,
    Conservation,
    /// Colour-ramp of the thermal field sampled at each column's
    /// climate elevation (cold blue → hot red).
    TemperatureField,
}

#[derive(Debug, Clone, Default)]
pub struct OverlayData {
    pub mode: OverlayMode,
    pub per_column_flux: Vec<i64>,
    pub per_column_erosion: Vec<i64>,
    pub global_delta: i64,
}

#[derive(Debug, Clone)]
pub struct RenderSnapshot {
    pub tick: u64,
    pub viewport_x: i32,
    pub sea_level: f32,
    pub world_x_min: i32,
    pub world_x_max: i32,
    pub elev_min: f32,
    pub elev_max: f32,
    pub rain_enabled: bool,
    pub columns: Vec<ColumnView>,
    pub markers: Vec<MarkerView>,
    pub overlay: OverlayData,
    pub mass_audit: MassAudit,
    pub xray: bool,
    pub climate: ClimateSettings,
    pub clouds: Vec<Cloud>,
}

impl World {
    pub fn snapshot(
        &self,
        tick: u64,
        viewport_x: i32,
        width: usize,
        overlay: OverlayData,
        xray: bool,
    ) -> RenderSnapshot {
        let (world_x_min, world_x_max) = self
            .world_x_bounds()
            .unwrap_or((viewport_x, viewport_x + width as i32 - 1));
        let bedrock_floor = self
            .chunks
            .values()
            .map(|c| c.bedrock_y)
            .fold(f32::MAX, f32::min);
        let bedrock_floor = if bedrock_floor == f32::MAX {
            crate::terrain::BEDROCK_FLOOR_M
        } else {
            bedrock_floor
        };
        let (elev_min, elev_max) = self
            .surface_bounds()
            .map(|(min, max)| {
                (
                    min.min(bedrock_floor).min(self.sea_level - 40.0),
                    max.max(self.sea_level + 8.0),
                )
            })
            .unwrap_or((bedrock_floor, self.sea_level + 30.0));
        let mut columns = Vec::with_capacity(width);
        for i in 0..width {
            let wx = viewport_x + i as i32;
            if let Some((coord, local)) = self.world_x_to_chunk_local(wx) {
                let chunk = &self.chunks[&coord];
                let col = &chunk.columns[local];
                let layers: Vec<_> = (0..col.layer_count as usize)
                    .filter(|&j| col.layers[j].thickness > 0)
                    .map(|j| {
                        (
                            col.layers[j].material,
                            col.layers[j].thickness,
                            col.layers[j].age_start,
                            col.layers[j].age_end,
                        )
                    })
                    .collect();
                let cap = col.moisture_cap().max(1) as f32;
                let saturation = (col.moisture as f32 / cap).clamp(0.0, 1.0);
                columns.push(ColumnView {
                    world_x: wx,
                    surface_y: col.surface_y,
                    bedrock_y: chunk.bedrock_y,
                    layers,
                    surface_water: col.top_water_mass(),
                    moisture: col.moisture,
                    saturation,
                    ice: col.top_ice_mass(),
                    snow: col.top_snow_mass(),
                    sediment: col.sediment,
                    activity: col.activity,
                    water_flux: overlay
                        .per_column_flux
                        .get(i)
                        .copied()
                        .unwrap_or(0),
                    erosion_flux: overlay
                        .per_column_erosion
                        .get(i)
                        .copied()
                        .unwrap_or(0),
                    residual: col.residual,
                    temperature_c: self.temperature_at_point(
                        wx,
                        col.climate_elevation(),
                        tick,
                    ),
                    biome: self.biome_at(col.climate_elevation()),
                });
            } else {
                columns.push(ColumnView {
                    world_x: wx,
                    surface_y: self.sea_level,
                    bedrock_y: 0.0,
                    layers: vec![],
                    surface_water: 0,
                    moisture: 0,
                    saturation: 0.0,
                    ice: 0,
                    snow: 0,
                    sediment: SedimentLoad::default(),
                    activity: Activity::Dormant,
                    water_flux: 0,
                    erosion_flux: 0,
                    residual: ResidualBucket::default(),
                    temperature_c: self.temperature_at_point(wx, self.sea_level, tick),
                    biome: Biome::Ocean,
                });
            }
        }

        let markers: Vec<_> = self
            .markers
            .iter()
            .filter(|m| m.world_x >= viewport_x && m.world_x < viewport_x + width as i32)
            .map(|m| MarkerView {
                world_x: m.world_x,
                label: m.label.clone(),
                pinned_layer_index: m.pinned_layer_index,
            })
            .collect();

        RenderSnapshot {
            tick,
            viewport_x,
            sea_level: self.sea_level,
            world_x_min,
            world_x_max,
            elev_min,
            elev_max,
            rain_enabled: self.rain_enabled,
            climate: self.climate.clone(),
            clouds: self.clouds.clone(),
            columns,
            markers,
            overlay,
            mass_audit: self.mass_audit.clone(),
            xray,
        }
    }
}
