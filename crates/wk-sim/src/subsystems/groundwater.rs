//! Slow lateral groundwater flow between neighbouring water tables.
//!
//! When `gw_head_fields_enabled`, hydraulic-head gradients come from the
//! Darcy-diffused groundwater head field (stage 6.5). Otherwise the
//! pre-field column-neighbour water-table path is used unchanged.

use wk_material::{CHUNK_W, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;

/// Groundwater moves far slower than surface water in reality (limited by
/// how fast water can actually squeeze through pore space, not just
/// gravity), but slower than 0.02 becomes imperceptible at play speed —
/// waterlogged saturation should visibly spread between neighbouring
/// columns within a handful of seconds, not minutes.
const GROUNDWATER_FLOW_COEFF: f32 = 0.02;

/// kg of moisture needed to raise this column's own water table by one
/// metre — depends on the porous layer's porosity and thickness. Under
/// the unified model this looks at the topmost *porous* layer, skipping
/// any water/ice/snow cap above it, so a puddle-covered sand bed still
/// has a well-defined aquifer capacity.
fn aquifer_mass_per_metre(col: &wk_world::column::Column) -> f32 {
    let cap = col.moisture_cap().max(1) as f32;
    let Some(layer) = col.top_porous_layer() else {
        return f32::INFINITY;
    };
    let layer_height_m = col.mass_to_height_delta(layer.material, layer.thickness);
    if layer_height_m <= 0.0 {
        return f32::INFINITY;
    }
    cap / layer_height_m
}

fn head_at_column(world: &World, coord: i32, local: usize) -> f32 {
    let chunk = world.chunks.get(&coord).unwrap();
    let col = &chunk.columns[local];
    if world.gw_head_fields_enabled {
        if let Some(gw) = &chunk.gw_head {
            let x_m = (chunk.world_x_base() + local as i32) as f32 * SAMPLE_WIDTH_M;
            // Sample mid-aquifer (bedrock → sediment bed), not mid-water
            // column — ocean surface_y sits at sea level far above the bed.
            let bed = col.climate_elevation();
            let y = 0.5 * (chunk.bedrock_y + bed);
            return gw.0.sample_bilinear(x_m, y);
        }
    }
    col.water_table_y()
}

fn head_neighbor(world: &World, coord: i32, local: i32) -> f32 {
    let chunk = world.chunks.get(&coord).unwrap();
    if local < 0 {
        if world.gw_head_fields_enabled {
            if let Some(left) = world.chunks.get(&(coord - 1)).and_then(|c| c.gw_head.as_ref()) {
                let w = left.0.width_cells as usize;
                let h = left.0.height_cells as usize;
                let cy = h / 2;
                return left.0.cell_at(w - 1, cy);
            }
        }
        return chunk.water_table_neighbor(-1);
    }
    if local >= CHUNK_W as i32 {
        if world.gw_head_fields_enabled {
            if let Some(right) = world.chunks.get(&(coord + 1)).and_then(|c| c.gw_head.as_ref()) {
                let h = right.0.height_cells as usize;
                let cy = h / 2;
                return right.0.cell_at(0, cy);
            }
        }
        return chunk.water_table_neighbor(CHUNK_W as i32);
    }
    head_at_column(world, coord, local as usize)
}

/// Slow lateral groundwater flow between neighbouring columns' water tables.
/// This is what lets a saturated aquifer act as a reservoir: water seeping
/// underground from a wet area can migrate toward a drier one (or toward a
/// lake bed sitting below the local table), separately from — and far more
/// slowly than — surface water flow. Discharge back into surface water when
/// a column's table would exceed its capacity is handled at commit time
/// (see commit_chunk_buffer), matching how real springs/seeps work.
pub fn run_groundwater_flow(world: &World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        if !chunk.any_hydrology_active() {
            continue;
        }

        let mut deltas = [(0i64, 0i64, 0i64); CHUNK_W]; // out_left, out_right, net

        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            if col.activity == Activity::Dormant || col.moisture <= 0 {
                continue;
            }
            // Free-surface water bodies already equalise hydrostatically.
            // Pore-space lateral flow under them, driven by tiny surface
            // ripples, ratchets aquifer mass into overflow faster than
            // infiltration can return it — emptying the water table under
            // oceans and flat ponds. Coastal land can still discharge
            // *into* wet columns (receive path below).
            if col.top_water_mass() > 0 {
                continue;
            }

            let head_here = head_at_column(world, coord, i);
            let head_left = head_neighbor(world, coord, i as i32 - 1);
            let head_right = head_neighbor(world, coord, i as i32 + 1);

            let grad_left = head_here - head_left;
            let grad_right = head_here - head_right;

            let perm = col
                .top_porous_layer()
                .map(|l| MaterialRegistry::props(l.material).permeability as f32 / 255.0)
                .unwrap_or(0.0);
            let mass_per_metre = aquifer_mass_per_metre(col);

            let mut out_left = 0i64;
            let mut out_right = 0i64;

            if grad_left > 0.0 && mass_per_metre.is_finite() {
                let transfer = grad_left * mass_per_metre * GROUNDWATER_FLOW_COEFF * perm;
                out_left = (transfer as i64).min(col.moisture);
            }
            if grad_right > 0.0 && mass_per_metre.is_finite() {
                let remaining = (col.moisture - out_left).max(0);
                let transfer = grad_right * mass_per_metre * GROUNDWATER_FLOW_COEFF * perm;
                out_right = (transfer as i64).min(remaining);
            }

            deltas[i] = (out_left, out_right, -(out_left + out_right));
        }

        let mut left_outbox = 0i64;
        let mut right_outbox = 0i64;

        for i in 0..CHUNK_W {
            let (out_left, out_right, net) = deltas[i];
            let buf = scratch.buffer_mut(coord);
            buf.moisture_delta[i] += net;

            if i == 0 {
                left_outbox += out_left;
            } else {
                buf.moisture_delta[i - 1] += out_left;
            }

            if i == CHUNK_W - 1 {
                right_outbox += out_right;
            } else {
                buf.moisture_delta[i + 1] += out_right;
            }
        }
        scratch.outbox_mut(coord).left_moisture += left_outbox;
        scratch.outbox_mut(coord).right_moisture += right_outbox;
    }
}
