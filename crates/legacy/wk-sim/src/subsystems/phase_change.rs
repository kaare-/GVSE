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

pub fn run_phase_change(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    let mut culled_frozen = 0i64;
    for chunk in world.chunks.values_mut() {
        let base = chunk.world_x_base();
        let bedrock = chunk.bedrock_y;
        for i in 0..chunk.columns.len() {
            // Cull runaway ice/snow towers (legacy saves / feedback bugs).
            // Excess mass is removed from the world — converting it all to
            // water in one tick would just replace an ice tower with a
            // water tower of the same height.
            {
                let col = &mut chunk.columns[i];
                let frozen = col.frozen_surface_mass();
                if frozen > MAX_FROZEN_SURFACE_MASS_KG {
                    let mut left = frozen - MAX_FROZEN_SURFACE_MASS_KG;
                    let mut guard = 0;
                    while left > 0 && guard < 64 {
                        guard += 1;
                        let Some(top) = col.top_layer() else {
                            break;
                        };
                        if !matches!(top.material, MaterialId::Ice | MaterialId::Snow) {
                            break;
                        }
                        let take = left.min(top.thickness).max(1);
                        let (removed, _) = col.take_from_top_layer(take);
                        if removed <= 0 {
                            break;
                        }
                        culled_frozen += removed;
                        left -= removed;
                        col.activity = Activity::HydrologyActive;
                    }
                    col.recompute_surface_y(bedrock);
                }
            }

            let elev = chunk.columns[i].ambient_elevation(sea_level);
            let climate_skin =
                wk_world::climate::temperature_at(elev, sea_level, tick, &climate);
            // `min(climate, thermal)` so the base-temp slider applies a
            // cold snap immediately even before the thermal top cell
            // catches up. Sampling uses ambient_elevation (not abyssal
            // bed / ice-tower tops).
            let temp = if let Some(thermal) = &chunk.thermal {
                let x_m = (base + i as i32) as f32 * SAMPLE_WIDTH_M;
                let thermal_skin = thermal.0.sample_bilinear(x_m, elev);
                climate_skin.min(thermal_skin)
            } else {
                climate_skin
            };

            let col = &mut chunk.columns[i];
            let Some(top) = col.top_layer() else {
                continue;
            };
            let Some(pc) = MaterialRegistry::props(top.material).phase_change else {
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
            // Don't grow ice past the frozen-surface budget (melt→refreeze
            // used to bypass the snow-only precip cap).
            if target == MaterialId::Ice
                && col.frozen_surface_mass() >= MAX_FROZEN_SURFACE_MASS_KG
            {
                continue;
            }
            let overshoot = (temp - pc.threshold_c).abs().min(10.0);
            let mut convert =
                (mass_here as f32 * PHASE_CHANGE_COEFF * overshoot.max(1.0)) as i64;
            convert = convert.max(1).min(mass_here).min(MAX_PHASE_CONVERT_KG);
            if target == MaterialId::Ice {
                let room = (MAX_FROZEN_SURFACE_MASS_KG - col.frozen_surface_mass()).max(0);
                convert = convert.min(room);
                if convert <= 0 {
                    continue;
                }
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
    if culled_frozen > 0 {
        // Bookkeeping: runaway frozen mass leaves like evaporative loss.
        world.mass_audit.evap_out_total += culled_frozen;
    }
}
