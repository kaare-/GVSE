//! Per-column hydrology activity flags (dormant vs active).

use wk_world::column::Activity;
use wk_world::world::World;

pub fn run_activity(world: &mut World) {
    let keep = world.agent_keep_awake.clone();
    for (&coord, chunk) in world.chunks.iter_mut() {
        let base = coord * wk_material::CHUNK_W as i32;
        for (i, col) in chunk.columns.iter_mut().enumerate() {
            let wx = base + i as i32;
            let cap = col.moisture_cap();
            let active = col.top_water_mass() > 0
                || col.flowable_water().map(|(_, m)| m).unwrap_or(0) > 0
                || col.top_snow_mass() > 0
                || col.top_ice_mass() > 0
                || col.sediment.total > 0
                || col.moisture > cap / 4
                || keep.binary_search(&wx).is_ok();
            col.activity = if active {
                Activity::HydrologyActive
            } else {
                Activity::Dormant
            };
        }
    }
}
