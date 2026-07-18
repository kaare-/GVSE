//! Thermal field: diffusion with geothermal bottom + sky top boundaries,
//! plus a restored warm mixed layer / cool thermocline in free water.

use wk_field::{explicit_diffusion, FieldPatch};
use wk_material::{MaterialId, MaterialRegistry, CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::column::Column;
use wk_world::fields::{
    stratified_water_temp, MIXED_LAYER_M, MIXED_LAYER_SOLAR_C, THERMOCLINE_M,
};
use wk_world::world::World;

/// Game seconds advanced per thermal field step. Kept at 10 s for
/// explicit-diffusion CFL even though the schedule period is 20 ticks
/// (heat just advances a bit slower in game time — fine for gameplay).
const DT_SECONDS: f32 = 10.0;

/// Diffusivity used for open air above the column surface.
const AIR_DIFFUSIVITY: f32 = 0.004;

/// Blend strength restoring the stratified water profile each step.
const STRATIFY_BLEND: f32 = 0.06;

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
        let (left_c, right_c) = world.neighbor_chunks(coord);
        let left_edge: Option<Vec<f32>> = world
            .chunks
            .get(&left_c)
            .and_then(|c| c.thermal.as_ref())
            .map(|t| {
                let w = t.0.width_cells as usize;
                let h = t.0.height_cells as usize;
                (0..h).map(|cy| t.0.cell_at(w - 1, cy)).collect()
            });
        let right_edge: Option<Vec<f32>> = world
            .chunks
            .get(&right_c)
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
/// boundaries and gently restore the warm-skin / cool-deep profile in
/// free water so solar heating + thermocline survive diffusion.
pub fn run_thermal_field(world: &mut World, tick: u64) {
    if !world.thermal_fields_enabled {
        return;
    }
    update_thermal_halos(world);

    let climate = world.climate.clone();
    let sea = world.sea_level;
    let geo = world.geothermal_bottom_c;
    let day = climate.day_night_factor(tick).max(0.0);
    let solar = MIXED_LAYER_SOLAR_C * day;
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
            let origin_y = field.origin_y_m;
            let base_x = chunk.world_x_base();

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

            // Restore stratification in the free-water column so the
            // mixed layer stays warm (solar) and the thermocline cool.
            let strat_bot = sea - MIXED_LAYER_M - THERMOCLINE_M;
            for cx in 0..w {
                let local = {
                    let (x_m, _) = out.cell_center(cx, 0);
                    ((x_m / SAMPLE_WIDTH_M).floor() as i32 - base_x)
                        .clamp(0, CHUNK_W as i32 - 1) as usize
                };
                let col = &chunk.columns[local];
                let water_top = col
                    .flowable_water()
                    .map(|(top, _)| top)
                    .unwrap_or(col.surface_y);
                for cy in 1..h.saturating_sub(1) {
                    let (_, y) = out.cell_center(cx, cy);
                    if y >= water_top || y < strat_bot.min(water_top) {
                        continue;
                    }
                    if material_at_y(col, y) != MaterialId::Water {
                        continue;
                    }
                    let target = stratified_water_temp(y, sea, origin_y, sky, geo, solar);
                    let t = out.cell_at(cx, cy);
                    out.set_cell(cx, cy, t * (1.0 - STRATIFY_BLEND) + target * STRATIFY_BLEND);
                }
            }
            out
        };
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if let Some(thermal) = chunk.thermal.as_mut() {
                thermal.0 = out;
            }
        }
    }
}
