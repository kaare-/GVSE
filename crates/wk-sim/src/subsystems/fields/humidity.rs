//! Humidity field: atmospheric RH diffusion with open-water emission.
//!
//! Boundary conditions:
//! - top (sky) = `world.ambient_humidity`
//! - bottom (ground) = 0 (rock absorbs, doesn't emit air moisture)
//! - left/right = neighbour chunk edges (or ambient at domain edge)
//!
//! Sources:
//! - continuous emission from columns with standing surface water
//! - vapor injected by `run_evaporation` via `World::inject_humidity_source`

use wk_field::{explicit_diffusion, FieldPatch};
use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::world::World;

/// Game seconds advanced per humidity field step (matches period 10).
const DT_SECONDS: f32 = 10.0;

/// Diffusivity of water vapour in open air (game-tuned, m²/s).
const AIR_DIFFUSIVITY: f32 = 0.05;

/// Diffusivity inside solid ground (near-zero — RH is an air field).
const GROUND_DIFFUSIVITY: f32 = 0.0005;

/// Continuous RH/s emission from a wet surface toward saturation.
const OPEN_WATER_EMIT: f32 = 0.02;

/// Minimum surface-water mass (kg) to count as an open-water emitter.
const OPEN_WATER_MIN_KG: i64 = 50;

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

/// Merge continuous open-water emission into the chunk's source buffer.
fn accumulate_open_water_sources(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let emissions: Vec<(usize, usize, f32)> = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(humidity) = &chunk.humidity else {
                continue;
            };
            let base = chunk.world_x_base();
            let mut out = Vec::new();
            let cell = humidity.0.cell_size_m;
            let h = humidity.0.height_cells as usize;
            // Keep emission out of the top Dirichlet row (sky BC).
            let max_cy = h.saturating_sub(2);
            for i in 0..CHUNK_W {
                let col = &chunk.columns[i];
                if col.top_water_mass() < OPEN_WATER_MIN_KG {
                    continue;
                }
                let x_m = (base + i as i32) as f32 * SAMPLE_WIDTH_M;
                // Emit into the air cell just above the water surface.
                let y_emit = col.surface_y + 0.5 * cell;
                let (cx, cy) = humidity.0.world_to_cell(x_m, y_emit);
                let cy = cy.min(max_cy);
                let rh = humidity.0.cell_at(cx, cy).clamp(0.0, 1.0);
                // Drive toward near-saturation over open water.
                let emit = OPEN_WATER_EMIT * (0.95 - rh).max(0.0);
                if emit > 0.0 {
                    out.push((cx, cy, emit));
                }
            }
            out
        };
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(source) = chunk.humidity_source.as_mut() else {
            continue;
        };
        for (cx, cy, emit) in emissions {
            let prev = source.cell_at(cx, cy);
            source.set_cell(cx, cy, prev + emit);
        }
    }
}

fn update_humidity_halos(world: &mut World) {
    let ambient = world.ambient_humidity.clamp(0.0, 1.0);
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for &coord in &coords {
        let left_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord - 1))
            .and_then(|c| c.humidity.as_ref())
            .map(|h| {
                let w = h.0.width_cells as usize;
                let ht = h.0.height_cells as usize;
                (0..ht).map(|cy| h.0.cell_at(w - 1, cy)).collect()
            });
        let right_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord + 1))
            .and_then(|c| c.humidity.as_ref())
            .map(|h| {
                let ht = h.0.height_cells as usize;
                (0..ht).map(|cy| h.0.cell_at(0, cy)).collect()
            });
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(humidity) = chunk.humidity.as_mut() else {
            continue;
        };
        let h = humidity.0.height_cells as usize;
        if let Some(edge) = left_edge {
            for cy in 0..h.min(edge.len()) {
                humidity.0.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                humidity.0.halo.left[cy] = ambient;
            }
        }
        if let Some(edge) = right_edge {
            for cy in 0..h.min(edge.len()) {
                humidity.0.halo.right[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                humidity.0.halo.right[cy] = ambient;
            }
        }
    }
}

/// Diffuse each chunk's humidity field, apply Dirichlet BCs, clamp to
/// \[0, 1\], then clear the source buffer.
pub fn run_humidity_field(world: &mut World, _tick: u64) {
    if !world.humidity_fields_enabled {
        return;
    }
    accumulate_open_water_sources(world);
    update_humidity_halos(world);

    let ambient = world.ambient_humidity.clamp(0.0, 1.0);
    let coords: Vec<i32> = world.chunks.keys().copied().collect();

    for coord in coords {
        let out = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(humidity) = &chunk.humidity else {
                continue;
            };
            let field = &humidity.0;
            let alpha = build_alpha(chunk, field);
            let source = chunk
                .humidity_source
                .as_ref()
                .cloned()
                .unwrap_or_else(|| field.zeros_like());
            let w = field.width_cells as usize;
            let h = field.height_cells as usize;

            let mut field_with_bc = field.clone();
            for cx in 0..w {
                field_with_bc.halo.top[cx] = ambient;
                field_with_bc.halo.bottom[cx] = 0.0;
                field_with_bc.set_cell(cx, h - 1, ambient);
                field_with_bc.set_cell(cx, 0, 0.0);
            }

            let mut out = field.zeros_like();
            explicit_diffusion(&field_with_bc, &alpha, &source, DT_SECONDS, &mut out);

            for cy in 0..h {
                for cx in 0..w {
                    let v = out.cell_at(cx, cy).clamp(0.0, 1.0);
                    out.set_cell(cx, cy, v);
                }
            }
            for cx in 0..w {
                out.set_cell(cx, h - 1, ambient);
                out.set_cell(cx, 0, 0.0);
                out.halo.top[cx] = ambient;
                out.halo.bottom[cx] = 0.0;
            }
            out.halo.left = field_with_bc.halo.left;
            out.halo.right = field_with_bc.halo.right;
            out
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(humidity) = chunk.humidity.as_mut() {
                humidity.0 = out;
            }
            if let Some(source) = chunk.humidity_source.as_mut() {
                for v in source.cells.iter_mut() {
                    *v = 0.0;
                }
            }
        }
    }
}
