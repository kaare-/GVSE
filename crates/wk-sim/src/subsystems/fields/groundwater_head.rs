//! Groundwater hydraulic-head field (Dupuit / unconfined aquifer).
//!
//! Each vertical column of cells carries the column's water-table
//! elevation as head. Darcy diffusion with permeability-based α smooths
//! head laterally; `run_groundwater_flow` samples the field for
//! gradients when `gw_head_fields_enabled`.

use wk_field::{explicit_diffusion, FieldPatch};
use wk_material::{CHUNK_W, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::world::World;

/// Game seconds per head-field step (matches schedule period 30).
const DT_SECONDS: f32 = 30.0;

/// Base diffusivity scale (m²/s). Multiplied by material permeability
/// fraction so clay barely conducts and sand/gravel equalise quickly.
const DARCY_DIFFUSIVITY_SCALE: f32 = 0.02;

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
            // No conduction in free air above the ground surface.
            let a = if y_m >= col.surface_y {
                0.0
            } else {
                let perm = col
                    .top_porous_layer()
                    .map(|l| MaterialRegistry::props(l.material).permeability as f32 / 255.0)
                    .unwrap_or(0.0);
                DARCY_DIFFUSIVITY_SCALE * perm
            };
            alpha.set_cell(cx, cy, a);
        }
    }
    alpha
}

/// Push each column's current water-table elevation into the head field
/// (constant with depth — Dupuit).
pub fn sync_head_from_columns(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let base = chunk.world_x_base();
        let heads: Vec<f32> = chunk.columns.iter().map(|c| c.water_table_y()).collect();
        let Some(gw) = chunk.gw_head.as_mut() else {
            continue;
        };
        let w = gw.0.width_cells as usize;
        let h = gw.0.height_cells as usize;
        for cy in 0..h {
            for cx in 0..w {
                let (x_m, _) = gw.0.cell_center(cx, cy);
                let local = ((x_m / SAMPLE_WIDTH_M).floor() as i32 - base)
                    .clamp(0, CHUNK_W as i32 - 1) as usize;
                gw.0.set_cell(cx, cy, heads[local]);
            }
        }
    }
}

fn update_gw_halos(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for &coord in &coords {
        let left_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord - 1))
            .and_then(|c| c.gw_head.as_ref())
            .map(|g| {
                let w = g.0.width_cells as usize;
                let h = g.0.height_cells as usize;
                (0..h).map(|cy| g.0.cell_at(w - 1, cy)).collect()
            });
        let right_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord + 1))
            .and_then(|c| c.gw_head.as_ref())
            .map(|g| {
                let h = g.0.height_cells as usize;
                (0..h).map(|cy| g.0.cell_at(0, cy)).collect()
            });
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(gw) = chunk.gw_head.as_mut() else {
            continue;
        };
        let h = gw.0.height_cells as usize;
        let w = gw.0.width_cells as usize;
        if let Some(edge) = left_edge {
            for cy in 0..h.min(edge.len()) {
                gw.0.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                gw.0.halo.left[cy] = gw.0.cell_at(0, cy);
            }
        }
        if let Some(edge) = right_edge {
            for cy in 0..h.min(edge.len()) {
                gw.0.halo.right[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                gw.0.halo.right[cy] = gw.0.cell_at(w - 1, cy);
            }
        }
        for cx in 0..w {
            gw.0.halo.bottom[cx] = gw.0.cell_at(cx, 0);
            gw.0.halo.top[cx] = gw.0.cell_at(cx, h - 1);
        }
    }
}

/// Sync column water tables into the head field, then Darcy-diffuse.
pub fn run_groundwater_head_field(world: &mut World, _tick: u64) {
    if !world.gw_head_fields_enabled {
        return;
    }
    sync_head_from_columns(world);
    update_gw_halos(world);

    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let out = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(gw) = &chunk.gw_head else {
                continue;
            };
            let field = &gw.0;
            let alpha = build_alpha(chunk, field);
            let source = field.zeros_like();
            let mut out = field.zeros_like();
            explicit_diffusion(field, &alpha, &source, DT_SECONDS, &mut out);
            out.halo = field.halo.clone();
            out
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(gw) = chunk.gw_head.as_mut() {
                gw.0 = out;
            }
        }
    }
}
