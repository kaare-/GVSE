//! Temperature-driven material transitions (snow/water/ice).

use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::MAX_FROZEN_SURFACE_MASS_KG;

/// Fraction of the top phase-changing layer's mass that transitions per
/// tick when the temperature crosses its threshold (scaled by how far
/// past the threshold we are, capped at 10C to avoid runaway rates on
/// extreme days). One number covers snow→water, water→ice, and ice→water
/// because the physics is symmetric enough at this fidelity.
const PHASE_CHANGE_COEFF: f32 = 0.03;

pub fn run_phase_change(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    let mut culled_frozen = 0i64;
    for chunk in world.chunks.values_mut() {
        let base = chunk.world_x_base();
        let bedrock = chunk.bedrock_y;
        for (i, col) in chunk.columns.iter_mut().enumerate() {
            // Cull runaway ice/snow towers (legacy saves / feedback bugs).
            // Excess mass is removed from the world — converting it all to
            // water in one tick would just replace an ice tower with a
            // water tower of the same height.
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
            // Sample a thin skin near the bed / sea — not ice-tower tops
            // and not abyssal geothermal clamps.
            let elev = col.ambient_elevation(sea_level);
            let temp = if let Some(thermal) = &chunk.thermal {
                let x_m = (base + i as i32) as f32 * SAMPLE_WIDTH_M;
                thermal.0.sample_bilinear(x_m, elev)
            } else {
                wk_world::climate::temperature_at(elev, sea_level, tick, &climate)
            };
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
            let convert = (mass_here as f32 * PHASE_CHANGE_COEFF * overshoot.max(1.0)) as i64;
            let mut convert = convert.max(1).min(mass_here);
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
            // order after a phase change (e.g. ice forming above water
            // is denser than snow above it, so snow floats back up).
            col.settle_by_density(tick);
        }
    }
    if culled_frozen > 0 {
        // Bookkeeping: runaway frozen mass leaves like evaporative loss.
        world.mass_audit.evap_out_total += culled_frozen;
    }
}
