use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wk_material::{CHUNK_W, MaterialId, MAX_LOADED_CHUNKS, MAX_MARKERS, MATERIAL_COUNT};

use crate::chunk::Chunk;
use crate::climate::{biome_for, temperature_at, Biome, ClimateSettings};
use crate::column::{Activity, Column, MarkerId, ResidualBucket, SedimentLoad};
use crate::fields::{
    DissolvedField, GroundwaterHeadField, HumidityField, PressureField, ThermalField, WindField,
};
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
    /// Dissolved mineral mass (kg) currently in `DissolvedField` patches.
    /// Solid→dissolved moves mass out of `by_material` into this bucket;
    /// the combined total stays in the audit invariant. Trailing +
    /// `serde(default)` so schema-v2 saves without this field still load.
    #[serde(default)]
    pub dissolved_total: i64,
    /// Cumulative kg dissolved out of solid rock (karst / solubility).
    /// Bookkeeping counter — not part of `total_tracked`.
    #[serde(default)]
    pub dissolved_out_total: i64,
    /// Cumulative kg reprecipitated from dissolved minerals (speleothems).
    #[serde(default)]
    pub dissolved_return_total: i64,
    /// Living + dead plant mass (kg) currently in per-column ecology buckets.
    #[serde(default)]
    pub biomass_total: i64,
    /// Cumulative kg of biomass grown from atmosphere / soil (source).
    #[serde(default)]
    pub biomass_grow_total: i64,
    /// Cumulative kg of biomass decayed back out of the tracked pool (sink).
    #[serde(default)]
    pub biomass_decay_total: i64,
}

impl MassAudit {
    pub fn total_tracked(&self) -> i64 {
        self.by_material.iter().sum::<i64>() + self.dissolved_total + self.biomass_total
    }

    pub fn bookkeeping_balance(&self) -> i64 {
        self.rain_inject_total + self.sea_inject_total + self.biomass_grow_total
            - self.evap_out_total
            - self.boundary_out_total
            - self.biomass_decay_total
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
    /// When true, chunks carry a `HumidityField` and evaporation
    /// samples / sources it. Default false → hardcoded 0.4 RH.
    pub humidity_fields_enabled: bool,
    /// Regional / sky relative humidity target (0..1) used as the top
    /// Dirichlet boundary of the humidity field.
    pub ambient_humidity: f32,
    /// When true, chunks carry pressure + wind fields. Default false →
    /// weather falls back to `climate.wind_speed`.
    pub pressure_wind_fields_enabled: bool,
    /// Sky / free-air pressure (arbitrary game units; 1.0 = ambient).
    pub ambient_pressure: f32,
    /// When true, chunks carry a groundwater head field and
    /// `run_groundwater_flow` samples it for Darcy gradients.
    pub gw_head_fields_enabled: bool,
    /// When true, chunks carry a dissolved-mineral concentration field.
    pub dissolved_fields_enabled: bool,
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
            humidity_fields_enabled: false,
            ambient_humidity: 0.4,
            pressure_wind_fields_enabled: false,
            ambient_pressure: 1.0,
            gw_head_fields_enabled: false,
            dissolved_fields_enabled: false,
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

    /// Relative humidity at a world point. Samples the chunk humidity
    /// field when present; otherwise returns [`Self::ambient_humidity`].
    ///
    /// Samples are clamped below the top Dirichlet (sky) row so a
    /// water column that has grown past the field's vertical extent
    /// still reads the free-air cell rather than the forced sky BC.
    pub fn humidity_at_point(&self, world_x: i32, y_m: f32) -> f32 {
        let coord = Self::chunk_coord_for_world_x(world_x);
        if let Some(chunk) = self.chunks.get(&coord) {
            if let Some(humidity) = &chunk.humidity {
                let x_m = world_x as f32 * wk_material::SAMPLE_WIDTH_M;
                let h = humidity.0.height_cells as usize;
                let max_y = humidity.0.origin_y_m
                    + (h.saturating_sub(1) as f32 - 0.5) * humidity.0.cell_size_m;
                let y = y_m.min(max_y);
                return humidity.0.sample_bilinear(x_m, y).clamp(0.0, 1.0);
            }
        }
        self.ambient_humidity
    }

    /// Allocate humidity fields (and zeroed source buffers) on every
    /// loaded chunk. Seeds cells to `ambient_humidity`.
    pub fn enable_humidity_fields(&mut self) {
        self.humidity_fields_enabled = true;
        let sea = self.sea_level;
        let ambient = self.ambient_humidity.clamp(0.0, 1.0);
        let coords: Vec<i32> = self.chunks.keys().copied().collect();
        for coord in coords {
            let Some(chunk) = self.chunks.get_mut(&coord) else {
                continue;
            };
            if chunk.humidity.is_some() {
                continue;
            }
            let mut field = HumidityField::new_for_chunk(coord, chunk.bedrock_y, sea, ambient);
            let w = field.0.width_cells as usize;
            let h = field.0.height_cells as usize;
            field.0.halo = wk_field::FieldHalo::zeros(field.0.width_cells, field.0.height_cells);
            for cy in 0..h {
                field.0.halo.left[cy] = ambient;
                field.0.halo.right[cy] = ambient;
            }
            for cx in 0..w {
                field.0.halo.bottom[cx] = 0.0;
                field.0.halo.top[cx] = ambient;
            }
            let source = field.0.zeros_like();
            chunk.humidity = Some(field);
            chunk.humidity_source = Some(source);
        }
    }

    /// Horizontal/vertical wind (m/s) at a world point. Samples the
    /// chunk wind field when present; otherwise returns climate wind
    /// as `(wind_speed · SAMPLE_WIDTH_M, 0)`.
    pub fn wind_at_point(&self, world_x: i32, y_m: f32) -> (f32, f32) {
        let coord = Self::chunk_coord_for_world_x(world_x);
        if let Some(chunk) = self.chunks.get(&coord) {
            if let Some(wind) = &chunk.wind {
                let x_m = world_x as f32 * wk_material::SAMPLE_WIDTH_M;
                let h = wind.vx.height_cells as usize;
                let max_y = wind.vx.origin_y_m
                    + (h.saturating_sub(1) as f32 - 0.5) * wind.vx.cell_size_m;
                let y = y_m.min(max_y);
                return (
                    wind.vx.sample_bilinear(x_m, y),
                    wind.vy.sample_bilinear(x_m, y),
                );
            }
        }
        (
            self.climate.wind_speed * wk_material::SAMPLE_WIDTH_M,
            0.0,
        )
    }

    /// Allocate pressure + wind fields on every loaded chunk. Seeds
    /// pressure with a mild hydrostatic gradient and wind from the
    /// climate horizontal speed in air cells.
    pub fn enable_pressure_wind_fields(&mut self) {
        self.pressure_wind_fields_enabled = true;
        let sea = self.sea_level;
        let ambient = self.ambient_pressure;
        let base_vx = self.climate.wind_speed * wk_material::SAMPLE_WIDTH_M;
        let coords: Vec<i32> = self.chunks.keys().copied().collect();
        for coord in coords {
            let Some(chunk) = self.chunks.get_mut(&coord) else {
                continue;
            };
            if chunk.pressure.is_some() {
                continue;
            }
            let mut pressure = PressureField::new_for_chunk(coord, chunk.bedrock_y, sea, ambient);
            let mut wind = WindField::new_for_chunk(coord, chunk.bedrock_y, sea);
            let w = pressure.0.width_cells as usize;
            let h = pressure.0.height_cells as usize;
            let origin_y = pressure.0.origin_y_m;
            let extent = (h as f32) * pressure.0.cell_size_m;
            // Hydrostatic: slightly higher pressure deeper.
            const HYDRO_DELTA: f32 = 0.15;
            for cy in 0..h {
                for cx in 0..w {
                    let (_, y) = pressure.0.cell_center(cx, cy);
                    let depth_frac = if extent > 0.0 {
                        1.0 - ((y - origin_y) / extent).clamp(0.0, 1.0)
                    } else {
                        0.0
                    };
                    pressure.0.set_cell(cx, cy, ambient + HYDRO_DELTA * depth_frac);
                    let local = ((pressure.0.cell_center(cx, cy).0
                        / wk_material::SAMPLE_WIDTH_M)
                        .floor() as i32
                        - chunk.world_x_base())
                    .clamp(0, wk_material::CHUNK_W as i32 - 1)
                        as usize;
                    let surface = chunk.columns[local].surface_y;
                    if y >= surface {
                        wind.vx.set_cell(cx, cy, base_vx);
                    } else {
                        wind.vx.set_cell(cx, cy, 0.0);
                    }
                    wind.vy.set_cell(cx, cy, 0.0);
                }
            }
            pressure.0.halo =
                wk_field::FieldHalo::zeros(pressure.0.width_cells, pressure.0.height_cells);
            wind.vx.halo =
                wk_field::FieldHalo::zeros(wind.vx.width_cells, wind.vx.height_cells);
            wind.vy.halo =
                wk_field::FieldHalo::zeros(wind.vy.width_cells, wind.vy.height_cells);
            for cy in 0..h {
                pressure.0.halo.left[cy] = pressure.0.cell_at(0, cy);
                pressure.0.halo.right[cy] = pressure.0.cell_at(w - 1, cy);
                wind.vx.halo.left[cy] = wind.vx.cell_at(0, cy);
                wind.vx.halo.right[cy] = wind.vx.cell_at(w - 1, cy);
                wind.vy.halo.left[cy] = 0.0;
                wind.vy.halo.right[cy] = 0.0;
            }
            for cx in 0..w {
                pressure.0.halo.bottom[cx] = ambient + HYDRO_DELTA;
                pressure.0.halo.top[cx] = ambient;
                wind.vx.halo.bottom[cx] = 0.0;
                wind.vx.halo.top[cx] = base_vx;
                wind.vy.halo.bottom[cx] = 0.0;
                wind.vy.halo.top[cx] = 0.0;
            }
            chunk.pressure = Some(pressure);
            chunk.wind = Some(wind);
        }
    }

    /// Add a relative-humidity source term (RH per second) at a world
    /// point. No-op when humidity fields are disabled.
    pub fn inject_humidity_source(&mut self, world_x: i32, y_m: f32, amount: f32) {
        if !self.humidity_fields_enabled || amount == 0.0 {
            return;
        }
        let coord = Self::chunk_coord_for_world_x(world_x);
        let Some(chunk) = self.chunks.get_mut(&coord) else {
            return;
        };
        let x_m = world_x as f32 * wk_material::SAMPLE_WIDTH_M;
        let (cx, cy) = {
            let Some(humidity) = &chunk.humidity else {
                return;
            };
            humidity.0.world_to_cell(x_m, y_m)
        };
        let Some(source) = chunk.humidity_source.as_mut() else {
            return;
        };
        let prev = source.cell_at(cx, cy);
        source.set_cell(cx, cy, prev + amount);
    }

    /// Hydraulic head (m) at a world point. Samples the groundwater head
    /// field when present; otherwise returns the column water-table
    /// elevation (Dupuit / unconfined approximation).
    pub fn groundwater_head_at_point(&self, world_x: i32, y_m: f32) -> f32 {
        let coord = Self::chunk_coord_for_world_x(world_x);
        if let Some(chunk) = self.chunks.get(&coord) {
            if let Some(gw) = &chunk.gw_head {
                let x_m = world_x as f32 * wk_material::SAMPLE_WIDTH_M;
                return gw.0.sample_bilinear(x_m, y_m);
            }
            let lx = Self::local_x(world_x);
            return chunk.columns[lx].water_table_y();
        }
        self.sea_level
    }

    /// Allocate groundwater head fields on every loaded chunk, seeded
    /// from each column's water-table elevation (constant with depth —
    /// Dupuit approximation).
    pub fn enable_groundwater_head_fields(&mut self) {
        self.gw_head_fields_enabled = true;
        let sea = self.sea_level;
        let coords: Vec<i32> = self.chunks.keys().copied().collect();
        for coord in coords {
            let Some(chunk) = self.chunks.get_mut(&coord) else {
                continue;
            };
            if chunk.gw_head.is_some() {
                continue;
            }
            let mut field = GroundwaterHeadField::new_for_chunk(coord, chunk.bedrock_y, sea, sea);
            let w = field.0.width_cells as usize;
            let h = field.0.height_cells as usize;
            let base = chunk.world_x_base();
            for cy in 0..h {
                for cx in 0..w {
                    let (x_m, _) = field.0.cell_center(cx, cy);
                    let local = ((x_m / wk_material::SAMPLE_WIDTH_M).floor() as i32 - base)
                        .clamp(0, CHUNK_W as i32 - 1) as usize;
                    let head = chunk.columns[local].water_table_y();
                    field.0.set_cell(cx, cy, head);
                }
            }
            field.0.halo =
                wk_field::FieldHalo::zeros(field.0.width_cells, field.0.height_cells);
            for cy in 0..h {
                field.0.halo.left[cy] = field.0.cell_at(0, cy);
                field.0.halo.right[cy] = field.0.cell_at(w - 1, cy);
            }
            for cx in 0..w {
                field.0.halo.bottom[cx] = field.0.cell_at(cx, 0);
                field.0.halo.top[cx] = field.0.cell_at(cx, h - 1);
            }
            chunk.gw_head = Some(field);
        }
    }

    /// Cell volume (m³) for a dissolved-field cell — side-view cell of
    /// side `cell_size_m` with unit depth into the screen.
    pub fn dissolved_cell_volume_m3(cell_size_m: f32) -> f32 {
        cell_size_m * cell_size_m
    }

    /// Integrate dissolved concentration fields to total mineral mass (kg).
    pub fn dissolved_mass_kg(&self) -> i64 {
        let mut total = 0.0f64;
        for chunk in self.chunks.values() {
            let Some(d) = &chunk.dissolved else {
                continue;
            };
            let vol = Self::dissolved_cell_volume_m3(d.0.cell_size_m) as f64;
            for &c in &d.0.cells {
                total += c.max(0.0) as f64 * vol;
            }
        }
        total.round() as i64
    }

    /// Allocate zeroed dissolved-concentration fields on every loaded chunk.
    pub fn enable_dissolved_fields(&mut self) {
        self.dissolved_fields_enabled = true;
        let sea = self.sea_level;
        let coords: Vec<i32> = self.chunks.keys().copied().collect();
        for coord in coords {
            let Some(chunk) = self.chunks.get_mut(&coord) else {
                continue;
            };
            if chunk.dissolved.is_some() {
                continue;
            }
            chunk.dissolved = Some(DissolvedField::new_for_chunk(
                coord,
                chunk.bedrock_y,
                sea,
            ));
        }
        self.mass_audit.dissolved_total = self.dissolved_mass_kg();
    }

    /// Inject `mass_kg` of dissolved mineral at a world point (spreads into
    /// one cell as concentration). Used by tests today; karst dissolution
    /// (stage 7) will write through the same path / source buffer.
    pub fn inject_dissolved_mass(&mut self, world_x: i32, y_m: f32, mass_kg: f32) {
        if !self.dissolved_fields_enabled || mass_kg <= 0.0 {
            return;
        }
        let coord = Self::chunk_coord_for_world_x(world_x);
        let Some(chunk) = self.chunks.get_mut(&coord) else {
            return;
        };
        let Some(dissolved) = chunk.dissolved.as_mut() else {
            return;
        };
        let x_m = world_x as f32 * wk_material::SAMPLE_WIDTH_M;
        let (cx, cy) = dissolved.0.world_to_cell(x_m, y_m);
        let vol = Self::dissolved_cell_volume_m3(dissolved.0.cell_size_m).max(1e-6);
        let prev = dissolved.0.cell_at(cx, cy);
        dissolved.0.set_cell(cx, cy, prev + mass_kg / vol);
        self.mass_audit.dissolved_total = self.dissolved_mass_kg();
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
            dissolved_out_total: self.mass_audit.dissolved_out_total,
            dissolved_return_total: self.mass_audit.dissolved_return_total,
            // Preserve parked dissolved mass when fields are off; overwritten
            // below from the field integral when enabled.
            dissolved_total: self.mass_audit.dissolved_total,
            biomass_grow_total: self.mass_audit.biomass_grow_total,
            biomass_decay_total: self.mass_audit.biomass_decay_total,
            tick: self.mass_audit.tick,
            ..Default::default()
        };

        let mut biomass = 0i64;
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
                audit.by_material[MaterialId::Water.index()] += col.void_water_total();
                if col.sediment.total > 0 {
                    audit.by_material[col.sediment.dominant.index()] += col.sediment.total;
                }
                biomass += col.ecology.biomass_total();
            }
        }
        self.mass_audit.by_material = audit.by_material;
        self.mass_audit.dissolved_out_total = audit.dissolved_out_total;
        self.mass_audit.dissolved_return_total = audit.dissolved_return_total;
        self.mass_audit.biomass_grow_total = audit.biomass_grow_total;
        self.mass_audit.biomass_decay_total = audit.biomass_decay_total;
        self.mass_audit.biomass_total = biomass;
        if self.dissolved_fields_enabled {
            self.mass_audit.dissolved_total = self.dissolved_mass_kg();
        } else {
            self.mass_audit.dissolved_total = audit.dissolved_total;
        }
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
    /// Sparse cavities for the renderer (ceiling, height, water, light).
    pub voids: Vec<(f32, f32, i64, u8)>,
    /// Leaf-area proxy 0..1 for a subtle vegetation tint.
    pub leaf_area: f32,
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
    /// Relative humidity at the column surface (0..1).
    pub humidity_rh: f32,
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
    /// Colour-ramp of relative humidity at the column surface
    /// (dry brown → wet cyan).
    HumidityField,
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
                let voids = col
                    .voids
                    .iter()
                    .map(|v| (v.top_y, v.height_m, v.water_mass, v.light))
                    .collect();
                columns.push(ColumnView {
                    world_x: wx,
                    surface_y: col.surface_y,
                    bedrock_y: chunk.bedrock_y,
                    layers,
                    voids,
                    leaf_area: col.ecology.leaf_area,
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
                    humidity_rh: self.humidity_at_point(wx, col.surface_y),
                    biome: self.biome_at(col.climate_elevation()),
                });
            } else {
                columns.push(ColumnView {
                    world_x: wx,
                    surface_y: self.sea_level,
                    bedrock_y: 0.0,
                    layers: vec![],
                    voids: vec![],
                    leaf_area: 0.0,
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
                    humidity_rh: self.humidity_at_point(wx, self.sea_level),
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
