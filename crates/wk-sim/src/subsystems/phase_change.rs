//! Temperature-driven material transitions (snow/water/ice).

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::MAX_FROZEN_SURFACE_MASS_KG;

/// Fraction of the top phase-changing layer's mass that transitions per
/// tick when the temperature crosses its threshold (scaled by how far
/// past the threshold we are, capped at 10C). Kept small so a sudden
/// base-temp cold snap cannot flash-freeze metres of lake in a single
/// tick and kick lake-level into flood/oscillation.
const PHASE_CHANGE_COEFF: f32 = 0.01;

/// Absolute kg cap on how much mass one column may convert per tick.
/// Independent of layer thickness so a deep pool cannot lose thousands
/// of kg of free surface in one step when the top layer is pure water.
const MAX_PHASE_CONVERT_KG: i64 = 60;

/// Total Ice already sitting in the fluid cap (above the first solid).
fn ice_in_cap(col: &wk_world::column::Column) -> i64 {
    let mut total = 0i64;
    for j in 0..col.layer_count as usize {
        match col.layers[j].material {
            MaterialId::Ice => total += col.layers[j].thickness,
            MaterialId::Snow | MaterialId::Water => {}
            _ => break,
        }
    }
    total
}

/// Free-surface temperature driving freeze/thaw.
///
/// - Climate path uses `climate_elevation` (solid ground under any
///   water/ice/snow). Using raw `surface_y` would make deep lakes read
///   colder via the lapse rate just because they have a tall water
///   column — that falsely freezes rivers/deltas and breaks mass
///   bookkeeping under heavy rain.
/// - Thermal path samples the field at the free surface. Sampling the
///   bed (climate elevation) made deep water columns read the warm
///   geothermal gradient while the air was freezing, melting ice and
///   dumping the melt through lake-level as flash floods.
/// - `min(climate, thermal_skin)` so the base-temp slider applies a
///   cold snap immediately even before the thermal top cell catches up.
fn phase_temp_c(
    chunk: &wk_world::chunk::Chunk,
    local: usize,
    base: i32,
    sea_level: f32,
    tick: u64,
    climate: &wk_world::climate::ClimateSettings,
) -> f32 {
    let col = &chunk.columns[local];
    let climate_skin =
        wk_world::climate::temperature_at(col.climate_elevation(), sea_level, tick, climate);
    if let Some(thermal) = &chunk.thermal {
        let x_m = (base + local as i32) as f32 * SAMPLE_WIDTH_M;
        let thermal_skin = thermal.0.sample_bilinear(x_m, col.surface_y);
        climate_skin.min(thermal_skin)
    } else {
        climate_skin
    }
}

pub fn run_phase_change(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    for chunk in world.chunks.values_mut() {
        let base = chunk.world_x_base();
        for i in 0..chunk.columns.len() {
            let temp = phase_temp_c(chunk, i, base, sea_level, tick, &climate);
            let col = &mut chunk.columns[i];
            let Some(top) = col.top_layer() else {
                continue;
            };
            let top_mat = top.material;
            let Some(pc) = MaterialRegistry::props(top_mat).phase_change else {
                continue;
            };
            let mass_here = top.thickness;
            if mass_here <= 0 {
                continue;
            }
            let target = if temp > pc.threshold_c {
                pc.above
            } else {
                pc.below
            };
            let Some(target) = target else {
                continue;
            };

            let overshoot = (temp - pc.threshold_c).abs().min(10.0);
            let mut convert =
                (mass_here as f32 * PHASE_CHANGE_COEFF * overshoot.max(1.0)) as i64;
            convert = convert.max(1).min(mass_here).min(MAX_PHASE_CONVERT_KG);

            // Water→Ice: stop growing ice once the column already holds a
            // thick frozen skin. Without this, any water briefly exposed
            // on top of ice (lake-level / rain before settle) freezes and
            // pumps the ice tower upward forever in a hard freeze.
            if target == MaterialId::Ice {
                let room = (MAX_FROZEN_SURFACE_MASS_KG - ice_in_cap(col)).max(0);
                if room <= 0 {
                    continue;
                }
                convert = convert.min(room);
            }

            let (removed, _) = col.take_from_top_layer(convert);
            if removed > 0 {
                col.deposit_to_top(target, removed, tick);
                col.activity = Activity::HydrologyActive;
            }
            // Density settle brings the fluid cap back into canonical
            // order after a phase change (ice floats on water, snow on
            // ice, rocks sink through).
            col.settle_by_density(tick);
        }
    }
}
