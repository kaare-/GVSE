use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use wk_material::{CHUNK_W, MaterialId, MAX_LOADED_CHUNKS, MAX_MARKERS, MATERIAL_COUNT};

use crate::chunk::Chunk;
use crate::climate::{biome_for, temperature_at, Biome, ClimateSettings};
use crate::column::{Activity, Column, MarkerId, ResidualBucket, SedimentLoad};
use crate::fields::{HumidityField, PressureField, ThermalField, WindField};
use crate::marker::Marker;
use crate::weather::{Cloud, WeatherSettings};
use crate::worldgen::{
    neighbor_chunk_coords, wrap_chunk_coord, wrap_world_x, WorldGenParams, WorldTopology,
};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MassAudit {
    pub by_material: [i64; MATERIAL_COUNT],
    pub evap_out_total: i64,
    pub sea_inject_total: i64,
    pub rain_inject_total: i64,
    pub boundary_out_total: i64,
    pub tick: u64,
    /// Cumulative kg dissolved out of solid rock (karst / solubility).
    /// Bookkeeping counter — not part of `total_tracked`.
    ///
    /// The old `dissolved_total` field-integral bucket is gone; solute
    /// no longer lives in a spatial grid. Speleogenesis draws from the
    /// difference `dissolved_out_total - dissolved_return_total` as an
    /// implicit "in transit" bank.
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
    /// Cumulative kg of alive biomass eaten by agents (stage 10 sink).
    #[serde(default)]
    pub biomass_eaten_total: i64,
}

impl MassAudit {
    pub fn total_tracked(&self) -> i64 {
        self.by_material.iter().sum::<i64>() + self.dissolved_bank() + self.biomass_total
    }

    pub fn bookkeeping_balance(&self) -> i64 {
        self.rain_inject_total + self.sea_inject_total + self.biomass_grow_total
            - self.evap_out_total
            - self.boundary_out_total
            - self.biomass_decay_total
            - self.biomass_eaten_total
    }

    /// Kg dissolved out of solid rock and not yet reprecipitated.
    /// Represents the implicit solute bank that speleogenesis draws
    /// from. Always non-negative by construction.
    pub fn dissolved_bank(&self) -> i64 {
        (self.dissolved_out_total - self.dissolved_return_total).max(0)
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
    /// When true, free-surface momentum + wind stress + tide run each tick
    /// (`run_surface_waves`). Default false so older hydro scenarios stay
    /// on pure lake-level / diffusion dynamics.
    pub surface_waves_enabled: bool,
    /// When true, ocean columns track a sinusoidal tide around `sea_level`.
    /// Mass exchange books through `sea_inject_total`.
    pub tide_enabled: bool,
    /// Peak tidal free-surface offset (metres).
    pub tide_amplitude_m: f32,
    /// Tide period in ticks (one full rise+fall cycle).
    pub tide_period_ticks: u64,
    /// Ring vs open strip + which terrain generator to use.
    pub gen: WorldGenParams,
    /// World-x columns that currently host agents. `run_activity` keeps
    /// these HydrologyActive so creature-bearing chunks don't go dormant.
    /// Rebuilt each agent step; not part of the save schema.
    pub agent_keep_awake: Vec<i32>,
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
    /// On a ring this is always `[0, width_columns - 1]` when fully loaded.
    pub fn world_x_bounds(&self) -> Option<(i32, i32)> {
        if let Some(w) = self.gen.topology.width_columns() {
            if !self.chunks.is_empty() {
                return Some((0, w - 1));
            }
        }
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
            surface_waves_enabled: false,
            tide_enabled: false,
            tide_amplitude_m: 0.45,
            tide_period_ticks: 1_800,
            gen: WorldGenParams::default(),
            agent_keep_awake: Vec::new(),
        }
    }

    pub fn topology(&self) -> WorldTopology {
        self.gen.topology
    }

    /// Resolve world-x through ring wrap (identity on open maps).
    pub fn resolve_world_x(&self, world_x: i32) -> i32 {
        wrap_world_x(self.gen.topology, world_x)
    }

    pub fn resolve_chunk_coord(&self, coord: i32) -> i32 {
        wrap_chunk_coord(self.gen.topology, coord)
    }

    /// Instantaneous tidal free-surface offset (metres) at `tick`.
    pub fn tide_eta_m(&self, tick: u64) -> f32 {
        if !self.tide_enabled || self.tide_amplitude_m.abs() < 1e-6 {
            return 0.0;
        }
        let period = self.tide_period_ticks.max(1) as f32;
        let phase = std::f32::consts::TAU * (tick as f32) / period;
        self.tide_amplitude_m * phase.sin()
    }

    /// Remove up to `max_kg` of living plant mass from the column's ecology.
    /// Returns kg actually eaten. Books `biomass_eaten_total`.
    pub fn eat_biomass(&mut self, world_x: i32, max_kg: i64) -> i64 {
        if max_kg <= 0 {
            return 0;
        }
        let Some(col) = self.column_at_mut(world_x) else {
            return 0;
        };
        let take = max_kg.min(col.ecology.alive_biomass).max(0);
        if take <= 0 {
            return 0;
        }
        col.ecology.alive_biomass -= take;
        // Canopy tracks biomass downward a little.
        let cover = ((col.ecology.alive_biomass as f32) / 800.0).clamp(0.0, 1.0);
        col.ecology.leaf_area = col.ecology.leaf_area.min(cover + 0.05);
        col.ecology.root_density = col.ecology.root_density.min(cover + 0.05);
        col.activity = Activity::HydrologyActive;
        self.mass_audit.biomass_eaten_total += take;
        take
    }

    /// Drink up to `max_kg` of water: prefer standing / flowable water,
    /// else pore moisture. Returns kg removed from the column.
    pub fn drink_water(&mut self, world_x: i32, max_kg: i64) -> i64 {
        if max_kg <= 0 {
            return 0;
        }
        let Some(col) = self.column_at_mut(world_x) else {
            return 0;
        };
        let mut remaining = max_kg;
        let from_cap = col.take_water_from_cap(remaining);
        remaining -= from_cap;
        let mut from_moist = 0i64;
        if remaining > 0 && col.moisture > 0 {
            from_moist = remaining.min(col.moisture);
            col.moisture -= from_moist;
        }
        let taken = from_cap + from_moist;
        if taken > 0 {
            col.activity = Activity::HydrologyActive;
            // Water left the world into the creature — count like evap.
            self.mass_audit.evap_out_total += taken;
        }
        taken
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
                // Keep near-surface samples off the geothermal Dirichlet
                // row (cy == 0). Out-of-range y clamps to the edge cell,
                // which made deep-ocean ambient read as exactly 55 °C.
                let cell = thermal.0.cell_size_m.max(1e-3);
                let y_lo = thermal.0.origin_y_m + cell * 1.5;
                let y_hi = thermal.0.origin_y_m
                    + cell * (thermal.0.height_cells as f32 - 1.5);
                let y_sample = y_m.clamp(y_lo, y_hi.max(y_lo));
                let t = thermal.0.sample_bilinear(x_m, y_sample);
                // Final guard: a "skin" sample must never report the
                // geothermal floor value (field bug / bad clamp).
                if y_m > self.sea_level - 40.0
                    && (t - self.geothermal_bottom_c).abs() < 2.0
                {
                    return temperature_at(y_m.max(self.sea_level), self.sea_level, tick, &self.climate);
                }
                return t;
            }
        }
        temperature_at(y_m, self.sea_level, tick, &self.climate)
    }

    /// Vertical temperature samples from `y_top` down to `y_bot` (inclusive).
    /// Depth and step are capped so the heatmap overlay stays cheap on
    /// deep ocean viewports.
    pub fn sample_temp_column(
        &self,
        world_x: i32,
        y_top: f32,
        y_bot: f32,
        tick: u64,
    ) -> Vec<(f32, f32)> {
        const MAX_DEPTH_M: f32 = 64.0;
        const STEP_M: f32 = 2.0;
        let top = y_top.max(y_bot);
        let bot = y_top.min(y_bot).max(top - MAX_DEPTH_M);
        let mut out = Vec::new();
        let mut y = top;
        while y > bot + 0.01 {
            out.push((y, self.temperature_at_point(world_x, y, tick)));
            y -= STEP_M;
        }
        out.push((bot, self.temperature_at_point(world_x, bot, tick)));
        out
    }

    /// Allocate and initialise a thermal field on every loaded chunk
    /// (no-op for chunks that already have one). Seeds a stratified
    /// water column (warm mixed layer → cool thermocline → geothermal
    /// at depth) with air at sky temperature.
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
            let sky = temperature_at(sea, sea, 0, &climate);
            for cy in 0..h {
                for cx in 0..w {
                    let (_, y) = field.0.cell_center(cx, cy);
                    let t = crate::fields::stratified_water_temp(y, sea, origin_y, sky, geo, 0.0);
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

    /// Hydraulic head (m) at a world point. Falls back to the column's
    /// water-table elevation (Dupuit / unconfined approximation).
    pub fn groundwater_head_at_point(&self, world_x: i32, y_m: f32) -> f32 {
        let _ = y_m;
        let coord = Self::chunk_coord_for_world_x(world_x);
        if let Some(chunk) = self.chunks.get(&coord) {
            let lx = Self::local_x(world_x);
            return chunk.columns[lx].water_table_y();
        }
        self.sea_level
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
        let world_x = self.resolve_world_x(world_x);
        let coord = Self::chunk_coord_for_world_x(world_x);
        let lx = Self::local_x(world_x);
        self.chunks.get_mut(&coord).map(|c| &mut c.columns[lx])
    }

    pub fn column_at(&self, world_x: i32) -> Option<&Column> {
        let world_x = self.resolve_world_x(world_x);
        let coord = Self::chunk_coord_for_world_x(world_x);
        let lx = Self::local_x(world_x);
        self.chunks.get(&coord).map(|c| &c.columns[lx])
    }

    pub fn world_x_to_chunk_local(&self, world_x: i32) -> Option<(i32, usize)> {
        let world_x = self.resolve_world_x(world_x);
        let coord = Self::chunk_coord_for_world_x(world_x);
        let lx = Self::local_x(world_x);
        self.chunks.get(&coord).map(|_| (coord, lx))
    }

    /// Left/right neighbour chunk coords (wrapped on a ring).
    pub fn neighbor_chunks(&self, coord: i32) -> (i32, i32) {
        neighbor_chunk_coords(self.gen.topology, coord)
    }

    pub fn recompute_mass_audit(&mut self) {
        let mut audit = MassAudit {
            evap_out_total: self.mass_audit.evap_out_total,
            sea_inject_total: self.mass_audit.sea_inject_total,
            rain_inject_total: self.mass_audit.rain_inject_total,
            boundary_out_total: self.mass_audit.boundary_out_total,
            dissolved_out_total: self.mass_audit.dissolved_out_total,
            dissolved_return_total: self.mass_audit.dissolved_return_total,
            biomass_grow_total: self.mass_audit.biomass_grow_total,
            biomass_decay_total: self.mass_audit.biomass_decay_total,
            biomass_eaten_total: self.mass_audit.biomass_eaten_total,
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
        self.mass_audit.biomass_eaten_total = audit.biomass_eaten_total;
        self.mass_audit.biomass_total = biomass;
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
    /// Mass (kg) of flowable Water in the fluid cap (includes water
    /// sitting under an ice/snow skin). Prefer this for HUD/debug when
    /// the top layer is ice — `surface_water` is then zero even though
    /// a lake still sits below.
    pub flowable_water: i64,
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
    /// Vertical temperature samples `(y_m, °C)` for the heatmap overlay.
    /// Empty unless [`OverlayMode::TemperatureField`] is active.
    pub temp_column: Vec<(f32, f32)>,
    /// Relative humidity at the column surface (0..1).
    pub humidity_rh: f32,
    /// Dissolved CO₂ in water when wet, else air CO₂ (relative units).
    pub co2: f32,
    /// Dissolved O₂ in water when wet, else air O₂ (relative units).
    pub o2: f32,
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
    /// Vertical thermal heatmap (cold blue → hot red) through the
    /// water column / near-surface rock — shows warm skin vs cool deep.
    TemperatureField,
    /// Colour-ramp of *atmospheric* relative humidity at the column
    /// surface (dry brown → wet cyan). Not soil moisture.
    HumidityField,
    /// Pore-water saturation heatmap (`moisture / moisture_cap`).
    /// Dry amber → saturated blue — distinct from air humidity.
    SoilMoisture,
    /// Dissolved / air CO₂ (low brown → high green).
    Co2Field,
    /// Dissolved / air O₂ (low purple → high cyan).
    O2Field,
}

impl OverlayMode {
    /// Short label for the bottom HUD (empty when no overlay).
    pub fn hud_label(self) -> &'static str {
        match self {
            OverlayMode::None => "",
            OverlayMode::WaterFlux => "water flux",
            OverlayMode::Erosion => "erosion",
            OverlayMode::Activity => "activity",
            OverlayMode::Conservation => "conservation",
            OverlayMode::TemperatureField => "temperature",
            OverlayMode::HumidityField => "air humidity",
            OverlayMode::SoilMoisture => "soil moisture",
            OverlayMode::Co2Field => "CO2",
            OverlayMode::O2Field => "O2",
        }
    }
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
    /// Instantaneous tidal free-surface offset for the render pass.
    /// Renderers snap oceanic water tops to `sea_level + tide_eta_m` so
    /// tiny per-column mass wobbles don't show as visible spikes.
    pub tide_eta_m: f32,
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
            let wx = self.resolve_world_x(viewport_x + i as i32);
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
                // Voids in this build are always dry; the 3rd tuple slot
                // is left as 0 to preserve the render/inspector ABI.
                let voids = col
                    .voids
                    .iter()
                    .map(|v| (v.top_y, v.height_m, 0i64, v.light))
                    .collect();
                let temperature_c = self.temperature_at_point(
                    wx,
                    col.ambient_elevation(self.sea_level),
                    tick,
                );
                let temp_column = if overlay.mode == OverlayMode::TemperatureField {
                    let (y_top, y_bot) = match col.flowable_water() {
                        Some((top, mass)) => {
                            let depth = mass as f32 / 250.0;
                            let bed = (top - depth).max(chunk.bedrock_y);
                            (top.max(self.sea_level), bed)
                        }
                        None => (
                            col.surface_y.max(self.sea_level),
                            (col.surface_y - 20.0).max(chunk.bedrock_y),
                        ),
                    };
                    self.sample_temp_column(wx, y_top, y_bot, tick)
                } else {
                    Vec::new()
                };
                columns.push(ColumnView {
                    world_x: wx,
                    surface_y: col.surface_y,
                    bedrock_y: chunk.bedrock_y,
                    layers,
                    voids,
                    leaf_area: col.ecology.leaf_area,
                    surface_water: col.top_water_mass(),
                    flowable_water: col.flowable_water().map(|(_, m)| m).unwrap_or(0),
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
                    temperature_c,
                    temp_column,
                    humidity_rh: self.humidity_at_point(wx, col.surface_y),
                    co2: if col.flowable_water().map(|(_, m)| m).unwrap_or(0) > 0 {
                        col.ecology.water_co2
                    } else {
                        col.ecology.air_co2
                    },
                    o2: if col.flowable_water().map(|(_, m)| m).unwrap_or(0) > 0 {
                        col.ecology.water_o2
                    } else {
                        col.ecology.air_o2
                    },
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
                    flowable_water: 0,
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
                    temp_column: Vec::new(),
                    humidity_rh: self.humidity_at_point(wx, self.sea_level),
                    co2: 1.0,
                    o2: 1.0,
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
            tide_eta_m: self.tide_eta_m(tick),
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
