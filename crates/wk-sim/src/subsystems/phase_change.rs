//! Temperature-driven material transitions (snow/water/ice).

use wk_material::{MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

/// Fraction of the top phase-changing layer's mass that transitions per
/// tick when the temperature crosses its threshold (scaled by how far
/// past the threshold we are, capped at 10C to avoid runaway rates on
/// extreme days). One number covers snow→water, water→ice, and ice→water
/// because the physics is symmetric enough at this fidelity.
const PHASE_CHANGE_COEFF: f32 = 0.03;

pub fn run_phase_change(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    for chunk in world.chunks.values_mut() {
        let base = chunk.world_x_base();
        for (i, col) in chunk.columns.iter_mut().enumerate() {
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
            // Sample the free surface / ice skin — not the solid bed.
            // Deep-ocean climate_elevation sits below the thermal-field
            // floor and would clamp to geothermal (~55°C), so water never
            // froze next to an iced shelf column that read as cold.
            let elev = col.ambient_elevation();
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
            let overshoot = (temp - pc.threshold_c).abs().min(10.0);
            let convert = (mass_here as f32 * PHASE_CHANGE_COEFF * overshoot.max(1.0)) as i64;
            let convert = convert.max(1).min(mass_here);
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
}
