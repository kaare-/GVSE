//! Per-column hydrology activity flags (dormant vs active).

use wk_world::column::Activity;
use wk_world::world::World;

pub fn run_activity(world: &mut World) {
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            let cap = col.moisture_cap();
            let active = col.top_water_mass() > 0
                || col.top_snow_mass() > 0
                || col.top_ice_mass() > 0
                || col.sediment.total > 0
                || col.moisture > cap / 4;
            col.activity = if active {
                Activity::HydrologyActive
            } else {
                Activity::Dormant
            };
        }
    }
}
