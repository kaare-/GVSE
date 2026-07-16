//! Thermal field: diffusion with geothermal bottom + sky top boundaries.

use wk_field::{explicit_diffusion, FieldPatch};
use wk_material::{MaterialId, MaterialRegistry, CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::column::Column;
use wk_world::world::World;

/// Game seconds advanced per thermal field step. Paired with the
/// subsystem's schedule period (10 ticks) so one field step ≈ 10 ticks.
const DT_SECONDS: f32 = 10.0;

/// Diffusivity used for open air above the column surface.
const AIR_DIFFUSIVITY: f32 = 0.004;

/// Material occupying elevation `y_m` in a column (Air above the
/// surface, Bedrock below the deepest layer).
fn material_at_y(col: &Column, y_m: f32) -> MaterialId {
    if y_m >= col.surface_y {
        return MaterialId::Air;
    }
    let mut top = col.surface_y;
    for i in 0..col.layer_count as usize {
        let layer = &col.layers[i];
        let h = col.mass_to_height_delta(layer.material, layer.thickness);
        let bottom = top - h;
        if y_m >= bottom {
            return layer.material;
        }
        top = bottom;
    }
    MaterialId::Bedrock
}

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
            let mat = material_at_y(col, y_m);
            let a = if mat == MaterialId::Air {
                AIR_DIFFUSIVITY
            } else {
                MaterialRegistry::props(mat).thermal_diffusivity
            };
            alpha.set_cell(cx, cy, a);
        }
    }
    alpha
}

/// Refresh left/right thermal halos from neighbouring chunks' edge
/// columns. Top/bottom remain Dirichlet (sky / geothermal) and are
/// set each step inside the diffusion pass.
fn update_thermal_halos(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for &coord in &coords {
        let left_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord - 1))
            .and_then(|c| c.thermal.as_ref())
            .map(|t| {
                let w = t.0.width_cells as usize;
                let h = t.0.height_cells as usize;
                (0..h).map(|cy| t.0.cell_at(w - 1, cy)).collect()
            });
        let right_edge: Option<Vec<f32>> = world
            .chunks
            .get(&(coord + 1))
            .and_then(|c| c.thermal.as_ref())
            .map(|t| {
                let h = t.0.height_cells as usize;
                (0..h).map(|cy| t.0.cell_at(0, cy)).collect()
            });
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let Some(thermal) = chunk.thermal.as_mut() else {
            continue;
        };
        let h = thermal.0.height_cells as usize;
        if let Some(edge) = left_edge {
            for cy in 0..h.min(edge.len()) {
                thermal.0.halo.left[cy] = edge[cy];
            }
        } else {
            for cy in 0..h {
                thermal.0.halo.left[cy] = thermal.0.cell_at(0, cy);
            }
        }
        if let Some(edge) = right_edge {
            for cy in 0..h.min(edge.len()) {
                thermal.0.halo.right[cy] = edge[cy];
            }
        } else {
            let w = thermal.0.width_cells as usize;
            for cy in 0..h {
                thermal.0.halo.right[cy] = thermal.0.cell_at(w - 1, cy);
            }
        }
    }
}

/// Diffuse each chunk's thermal field, then re-impose Dirichlet
/// boundaries: bottom = geothermal, top = sky temperature from climate.
pub fn run_thermal_field(world: &mut World, tick: u64) {
    if !world.thermal_fields_enabled {
        return;
    }
    update_thermal_halos(world);

    let climate = world.climate.clone();
    let sea = world.sea_level;
    let geo = world.geothermal_bottom_c;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();

    for coord in coords {
        let out = {
            let Some(chunk) = world.chunks.get(&coord) else {
                continue;
            };
            let Some(thermal) = &chunk.thermal else {
                continue;
            };
            let field = &thermal.0;
            let alpha = build_alpha(chunk, field);
            let source = field.zeros_like();
            let w = field.width_cells as usize;
            let h = field.height_cells as usize;

            let mut field_with_bc = field.clone();
            let sky = wk_world::climate::temperature_at(sea, sea, tick, &climate);
            for cx in 0..w {
                field_with_bc.halo.top[cx] = sky;
                field_with_bc.halo.bottom[cx] = geo;
                field_with_bc.set_cell(cx, h - 1, sky);
                field_with_bc.set_cell(cx, 0, geo);
            }

            let mut out = field.zeros_like();
            explicit_diffusion(&field_with_bc, &alpha, &source, DT_SECONDS, &mut out);

            for cx in 0..w {
                out.set_cell(cx, h - 1, sky);
                out.set_cell(cx, 0, geo);
                out.halo.top[cx] = sky;
                out.halo.bottom[cx] = geo;
            }
            out.halo.left = field_with_bc.halo.left;
            out.halo.right = field_with_bc.halo.right;
            out
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(thermal) = chunk.thermal.as_mut() {
                thermal.0 = out;
            }
        }
    }
}
