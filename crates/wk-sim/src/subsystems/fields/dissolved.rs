//! Dissolved-mineral concentration field (kg/m³).
//!
//! Diffuses through wet cells (pore moisture / surface water). Dry rock
//! and open air have near-zero diffusivity so solute stays in the
//! aquifer. Solid→dissolved injection is owned by `run_karst` (flux-
//! driven); this pass only diffuses the concentration field. Tests may
//! also inject via [`World::inject_dissolved_mass`].

use wk_field::{explicit_diffusion, FieldPatch};
use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::world::World;

/// Game seconds per dissolved-field step (matches schedule period 6).
const DT_SECONDS: f32 = 6.0;

/// Diffusivity in fully saturated pore space (m²/s, game-tuned).
const WET_DIFFUSIVITY: f32 = 0.01;

/// Tiny floor so the field doesn't hard-freeze at exact zeros.
const DRY_DIFFUSIVITY: f32 = 1e-5;

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
            let col = &chunk.columns[local];
            let wet = if y_m >= col.surface_y {
                // Free air / above surface: only wet if standing water
                // reaches this elevation (approximate with top water).
                col.top_water_mass() > 0 || col.void_water_total() > 0
            } else {
                let cap = col.moisture_cap().max(1) as f32;
                (col.moisture as f32 / cap) > 0.05
                    || col.top_water_mass() > 0
                    || col.void_water_total() > 0
            };
            alpha.set_cell(
                cx,
                cy,
                if wet {
                    WET_DIFFUSIVITY
                } else {
                    DRY_DIFFUSIVITY
                },
            );
        }
    }
    alpha
}

fn update_dissolved_halos(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for &coord in &coords {
        let left_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord - 1))
            .and_then(|c| c.dissolved.as_ref())
            .map(|d| {
                let w = d.0.width_cells as usize;
                let h = d.0.height_cells as usize;
                (0..h).map(|cy| d.0.cell_at(w - 1, cy)).collect()
            });
        let right_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord + 1))
            .and_then(|c| c.dissolved.as_ref())
            .map(|d| {
                let h = d.0.height_cells as usize;
                (0..h).map(|cy| d.0.cell_at(0, cy)).collect()
            });
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(dissolved) = chunk.dissolved.as_mut() else {
            continue;
        };
        let h = dissolved.0.height_cells as usize;
        let w = dissolved.0.width_cells as usize;
        if let Some(edge) = left_edge {
            for cy in 0..h.min(edge.len()) {
                dissolved.0.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                dissolved.0.halo.left[cy] = dissolved.0.cell_at(0, cy);
            }
        }
        if let Some(edge) = right_edge {
            for cy in 0..h.min(edge.len()) {
                dissolved.0.halo.right[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                dissolved.0.halo.right[cy] = dissolved.0.cell_at(w - 1, cy);
            }
        }
        for cx in 0..w {
            dissolved.0.halo.bottom[cx] = dissolved.0.cell_at(cx, 0);
            dissolved.0.halo.top[cx] = dissolved.0.cell_at(cx, h - 1);
        }
    }
}

pub fn run_dissolved_field(world: &mut World, _tick: u64) {
    if !world.dissolved_fields_enabled {
        return;
    }
    update_dissolved_halos(world);

    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let out = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(dissolved) = &chunk.dissolved else {
                continue;
            };
            let field = &dissolved.0;
            let alpha = build_alpha(chunk, field);
            let source = field.zeros_like();
            let mut out = field.zeros_like();
            explicit_diffusion(field, &alpha, &source, DT_SECONDS, &mut out);
            // Concentration can't go negative.
            let w = out.width_cells as usize;
            let h = out.height_cells as usize;
            for cy in 0..h {
                for cx in 0..w {
                    let v = out.cell_at(cx, cy).max(0.0);
                    out.set_cell(cx, cy, v);
                }
            }
            out.halo = field.halo.clone();
            out
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(dissolved) = chunk.dissolved.as_mut() {
                dissolved.0 = out;
            }
        }
    }
    world.mass_audit.dissolved_total = world.dissolved_mass_kg();
}
