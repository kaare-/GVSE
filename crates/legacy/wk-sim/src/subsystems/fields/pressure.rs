//! Pressure field: hydrostatic base + thermal buoyancy source.
//!
//! Hot air (sampled from the thermal field when present) lowers local
//! pressure; the wind subsystem turns the resulting gradient into a
//! circulation. Bottom Dirichlet = ambient + hydro delta; top = ambient.

use wk_field::{explicit_diffusion, FieldPatch};
use wk_material::{SAMPLE_WIDTH_M};
use wk_world::{CHUNK_W};
use wk_world::world::World;

const DT_SECONDS: f32 = 30.0;
/// Mild diffusion — strong enough to smooth noise, weak enough that a
/// sustained buoyancy low isn't erased within one period.
const AIR_DIFFUSIVITY: f32 = 0.008;
const GROUND_DIFFUSIVITY: f32 = 0.0005;
/// How strongly a °C above the sky reference lowers pressure (per second).
/// Tuned so a ~40 °C anomaly shifts pressure by a few hundredths per
/// step without slamming into the clamp floor.
const BUOYANCY_PER_C: f32 = 0.00002;
const HYDRO_DELTA: f32 = 0.15;

fn build_alpha(chunk: &wk_world::chunk::Chunk, field: &FieldPatch) -> FieldPatch {
    let mut alpha = field.zeros_like();
    let w = field.width_cells as usize;
    let h = field.height_cells as usize;
    let base_x = chunk.world_x_base();
    for cy in 0..h {
        for cx in 0..w {
            let (x_m, y_m) = field.cell_center(cx, cy);
            let local = ((x_m / SAMPLE_WIDTH_M).floor() as i32 - base_x)
                .clamp(0, CHUNK_W as i32 - 1) as usize;
            let surface = chunk.columns[local].surface_y;
            let a = if y_m >= surface {
                AIR_DIFFUSIVITY
            } else {
                GROUND_DIFFUSIVITY
            };
            alpha.set_cell(cx, cy, a);
        }
    }
    alpha
}

fn build_buoyancy_source(chunk: &wk_world::chunk::Chunk, field: &FieldPatch, sky_t: f32) -> FieldPatch {
    let mut source = field.zeros_like();
    let w = field.width_cells as usize;
    let h = field.height_cells as usize;
    let base_x = chunk.world_x_base();
    for cy in 0..h {
        for cx in 0..w {
            let (x_m, y_m) = field.cell_center(cx, cy);
            let local = ((x_m / SAMPLE_WIDTH_M).floor() as i32 - base_x)
                .clamp(0, CHUNK_W as i32 - 1) as usize;
            if y_m < chunk.columns[local].surface_y {
                continue;
            }
            let temp = chunk
                .thermal
                .as_ref()
                .map(|t| t.0.sample_bilinear(x_m, y_m))
                .unwrap_or(sky_t);
            // Hotter than sky → negative pressure source (buoyant low).
            let anomaly = (temp - sky_t).max(0.0);
            source.set_cell(cx, cy, -BUOYANCY_PER_C * anomaly);
        }
    }
    source
}

fn update_pressure_halos(world: &mut World) {
    let ambient = world.ambient_pressure;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for &coord in &coords {
        let (left_c, right_c) = world.neighbor_chunks(coord);
        let left_edge: Option<Vec<f32>> = world
            .chunks
            .get(&left_c)
            .and_then(|c| c.pressure.as_ref())
            .map(|p| {
                let w = p.0.width_cells as usize;
                let h = p.0.height_cells as usize;
                (0..h).map(|cy| p.0.cell_at(w - 1, cy)).collect()
            });
        let right_edge: Option<Vec<f32>> = world
            .chunks
            .get(&right_c)
            .and_then(|c| c.pressure.as_ref())
            .map(|p| {
                let h = p.0.height_cells as usize;
                (0..h).map(|cy| p.0.cell_at(0, cy)).collect()
            });
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(pressure) = chunk.pressure.as_mut() else {
            continue;
        };
        let h = pressure.0.height_cells as usize;
        if let Some(edge) = left_edge {
            for cy in 0..h.min(edge.len()) {
                pressure.0.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                pressure.0.halo.left[cy] = pressure.0.cell_at(0, cy);
            }
        }
        if let Some(edge) = right_edge {
            for cy in 0..h.min(edge.len()) {
                pressure.0.halo.right[cy] = edge[cy];
            }
        } else {
            let w = pressure.0.width_cells as usize;
            for cy in 0..h {
                pressure.0.halo.right[cy] = pressure.0.cell_at(w - 1, cy);
            }
        }
        let w = pressure.0.width_cells as usize;
        for cx in 0..w {
            pressure.0.halo.top[cx] = ambient;
            pressure.0.halo.bottom[cx] = ambient + HYDRO_DELTA;
        }
    }
}

pub fn run_pressure_field(world: &mut World, tick: u64) {
    if !world.pressure_wind_fields_enabled {
        return;
    }
    update_pressure_halos(world);

    let ambient = world.ambient_pressure;
    let climate = world.climate.clone();
    let sea = world.sea_level;
    let sky_t = wk_world::climate::temperature_at(sea, sea, tick, &climate);
    let coords: Vec<i32> = world.chunks.keys().copied().collect();

    for coord in coords {
        let out = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(pressure) = &chunk.pressure else {
                continue;
            };
            let field = &pressure.0;
            let alpha = build_alpha(chunk, field);
            let source = build_buoyancy_source(chunk, field, sky_t);
            let w = field.width_cells as usize;
            let h = field.height_cells as usize;

            let mut field_with_bc = field.clone();
            for cx in 0..w {
                field_with_bc.halo.top[cx] = ambient;
                field_with_bc.halo.bottom[cx] = ambient + HYDRO_DELTA;
                field_with_bc.set_cell(cx, h - 1, ambient);
                field_with_bc.set_cell(cx, 0, ambient + HYDRO_DELTA);
            }

            let mut out = field.zeros_like();
            explicit_diffusion(&field_with_bc, &alpha, &source, DT_SECONDS, &mut out);
            let lo = ambient - 0.2;
            let hi = ambient + HYDRO_DELTA + 0.05;
            for cy in 0..h {
                for cx in 0..w {
                    let v = out.cell_at(cx, cy).clamp(lo, hi);
                    out.set_cell(cx, cy, v);
                }
            }
            for cx in 0..w {
                out.set_cell(cx, h - 1, ambient);
                out.set_cell(cx, 0, ambient + HYDRO_DELTA);
                out.halo.top[cx] = ambient;
                out.halo.bottom[cx] = ambient + HYDRO_DELTA;
            }
            out.halo.left = field_with_bc.halo.left;
            out.halo.right = field_with_bc.halo.right;
            out
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(pressure) = chunk.pressure.as_mut() {
                pressure.0 = out;
            }
        }
    }
}
