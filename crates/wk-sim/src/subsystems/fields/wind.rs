//! Wind field: derived from −∇pressure, zero underground, climate bias.
//!
//! Runs one schedule phase after pressure so it samples the freshly
//! committed pressure field. Weather reads horizontal wind via
//! [`World::wind_at_point`].

use wk_field::gradient;
use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::world::World;

/// Convert pressure gradient → wind speed (m/s per unit pressure / m).
const WIND_GAIN: f32 = 4.0;
/// Blend toward the climate horizontal wind each step (air cells).
const CLIMATE_BLEND: f32 = 0.1;
/// Blend factor toward the freshly derived −∇P wind (rest is previous).
const UPDATE_BLEND: f32 = 0.35;
/// Cap |wind| so a stiff gradient can't throw clouds across the map.
const WIND_MAX_M_S: f32 = 2.0;

fn update_wind_halos(world: &mut World) {
    let base_vx = world.climate.wind_speed * SAMPLE_WIDTH_M;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for &coord in &coords {
        let (left_c, right_c) = world.neighbor_chunks(coord);
        let left_vx: Option<Vec<f32>> = world
            .chunks
            .get(&left_c)
            .and_then(|c| c.wind.as_ref())
            .map(|w| {
                let width = w.vx.width_cells as usize;
                let h = w.vx.height_cells as usize;
                (0..h).map(|cy| w.vx.cell_at(width - 1, cy)).collect()
            });
        let left_vy: Option<Vec<f32>> = world
            .chunks
            .get(&left_c)
            .and_then(|c| c.wind.as_ref())
            .map(|w| {
                let width = w.vy.width_cells as usize;
                let h = w.vy.height_cells as usize;
                (0..h).map(|cy| w.vy.cell_at(width - 1, cy)).collect()
            });
        let right_vx: Option<Vec<f32>> = world
            .chunks
            .get(&right_c)
            .and_then(|c| c.wind.as_ref())
            .map(|w| {
                let h = w.vx.height_cells as usize;
                (0..h).map(|cy| w.vx.cell_at(0, cy)).collect()
            });
        let right_vy: Option<Vec<f32>> = world
            .chunks
            .get(&right_c)
            .and_then(|c| c.wind.as_ref())
            .map(|w| {
                let h = w.vy.height_cells as usize;
                (0..h).map(|cy| w.vy.cell_at(0, cy)).collect()
            });
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(wind) = chunk.wind.as_mut() else {
            continue;
        };
        let h = wind.vx.height_cells as usize;
        let w = wind.vx.width_cells as usize;
        if let Some(edge) = left_vx {
            for cy in 0..h.min(edge.len()) {
                wind.vx.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                wind.vx.halo.left[cy] = wind.vx.cell_at(0, cy);
            }
        }
        if let Some(edge) = left_vy {
            for cy in 0..h.min(edge.len()) {
                wind.vy.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                wind.vy.halo.left[cy] = 0.0;
            }
        }
        if let Some(edge) = right_vx {
            for cy in 0..h.min(edge.len()) {
                wind.vx.halo.right[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                wind.vx.halo.right[cy] = wind.vx.cell_at(w - 1, cy);
            }
        }
        if let Some(edge) = right_vy {
            for cy in 0..h.min(edge.len()) {
                wind.vy.halo.right[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                wind.vy.halo.right[cy] = 0.0;
            }
        }
        for cx in 0..w {
            wind.vx.halo.top[cx] = base_vx;
            wind.vx.halo.bottom[cx] = 0.0;
            wind.vy.halo.top[cx] = 0.0;
            wind.vy.halo.bottom[cx] = 0.0;
        }
    }
}

pub fn run_wind_field(world: &mut World, _tick: u64) {
    if !world.pressure_wind_fields_enabled {
        return;
    }
    update_wind_halos(world);

    let base_vx = world.climate.wind_speed * SAMPLE_WIDTH_M;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();

    for coord in coords {
        let (out_vx, out_vy) = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let (Some(pressure), Some(wind)) = (&chunk.pressure, &chunk.wind) else {
                continue;
            };
            let w = pressure.0.width_cells as usize;
            let h = pressure.0.height_cells as usize;
            let base_x = chunk.world_x_base();
            let mut out_vx = wind.vx.zeros_like();
            let mut out_vy = wind.vy.zeros_like();
            out_vx.halo = wind.vx.halo.clone();
            out_vy.halo = wind.vy.halo.clone();

            for cy in 0..h {
                for cx in 0..w {
                    let (x_m, y_m) = pressure.0.cell_center(cx, cy);
                    let local = ((x_m / SAMPLE_WIDTH_M).floor() as i32 - base_x)
                        .clamp(0, CHUNK_W as i32 - 1) as usize;
                    let surface = chunk.columns[local].surface_y;
                    if y_m < surface {
                        out_vx.set_cell(cx, cy, 0.0);
                        out_vy.set_cell(cx, cy, 0.0);
                        continue;
                    }
                    let (dpx, dpy) = gradient(&pressure.0, cx, cy);
                    let mut vx = -WIND_GAIN * dpx;
                    let mut vy = -WIND_GAIN * dpy;
                    // Climate bias on the horizontal component.
                    vx = vx * (1.0 - CLIMATE_BLEND) + base_vx * CLIMATE_BLEND;
                    let prev_vx = wind.vx.cell_at(cx, cy);
                    let prev_vy = wind.vy.cell_at(cx, cy);
                    vx = prev_vx * (1.0 - UPDATE_BLEND) + vx * UPDATE_BLEND;
                    vy = prev_vy * (1.0 - UPDATE_BLEND) + vy * UPDATE_BLEND;
                    vx = vx.clamp(-WIND_MAX_M_S, WIND_MAX_M_S);
                    vy = vy.clamp(-WIND_MAX_M_S, WIND_MAX_M_S);
                    out_vx.set_cell(cx, cy, vx);
                    out_vy.set_cell(cx, cy, vy);
                }
            }
            (out_vx, out_vy)
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(wind) = chunk.wind.as_mut() {
                wind.vx = out_vx;
                wind.vy = out_vy;
            }
        }
    }
}
