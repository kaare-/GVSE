//! Manual rain injection (toggle-driven precipitation).

use wk_material::CHUNK_W;
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;

use super::shared::{split_precipitation, SimParams};

pub fn run_rain_inject(
    world: &mut World,
    scratch: &mut WorldTransferScratch,
    params: &SimParams,
    tick: u64,
) {
    if !params.rain_enabled {
        return;
    }
    let inject_per_col = params.rain_rate;
    let climate = world.climate.clone();
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let sea = params.sea_level;
        // Rain is an external forcing — it must reach every column
        // including currently Dormant ones. Otherwise a fully dried
        // out region can never receive rain again (nothing else would
        // set it back to active), permanently deadlocking re-hydration.
        for i in 0..CHUNK_W {
            let (climate_elev, existing_snow) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                (col.climate_elevation(), col.top_snow_mass())
            };
            let (rain_component, snow_component) = split_precipitation(
                sea,
                inject_per_col,
                climate_elev,
                tick,
                &climate,
                existing_snow,
            );
            let buf = scratch.buffer_mut(coord);
            if rain_component > 0 {
                buf.water_delta[i] += rain_component;
                world.mass_audit.rain_inject_total += rain_component;
            }
            if snow_component > 0 {
                buf.snow_request[i] += snow_component;
                world.mass_audit.rain_inject_total += snow_component;
            }
        }
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            chunk.set_all_active();
        }
    }
}
