//! Automatic drifting-cloud weather layer.

use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::world::World;

use crate::buffer::WorldTransferScratch;

use super::shared::split_precipitation;

/// Spawns, advances, and rains from drifting clouds — the weather layer on
/// top of the manual rain toggle. Clouds are not particles: each is just an
/// x position, half-width, and remaining moisture, advected by wind
/// (columns/tick, sign = direction) and consumed as it rains.
///
/// When pressure/wind fields are enabled, each cloud samples horizontal
/// wind at its position (sky band); otherwise it uses `climate.wind_speed`.
pub fn run_weather(world: &mut World, scratch: &mut WorldTransferScratch, tick: u64) {
    let Some((x_min, x_max)) = world.world_x_bounds() else {
        return;
    };

    let climate_wind = world.climate.wind_speed;
    if world.weather.weather_enabled
        && world.clouds.len() < world.weather.max_clouds
        && tick >= world.next_cloud_spawn_tick
    {
        let seed = world.seed;
        let half_width = 20.0 + wk_world::terrain::hash_f32(seed, tick as i64, 401) * 40.0;
        // Needs enough budget to cross a wide stretch of ocean/shelf, then
        // continue raining intermittently across a wide stretch of land,
        // without running dry long before reaching terrain far inland
        // (like tall mountains) — see rain_chance_per_tick for the other
        // half of that budget (making rain intermittent, not constant).
        let moisture = 10_000.0 + wk_world::terrain::hash_f32(seed, tick as i64, 402) * 18_000.0;
        let spawn_x = if climate_wind >= 0.0 {
            x_min as f32 - half_width
        } else {
            x_max as f32 + half_width
        };
        world.clouds.push(wk_world::weather::Cloud {
            x: spawn_x,
            half_width,
            moisture,
        });
        world.next_cloud_spawn_tick = tick + world.weather.cloud_spawn_interval_ticks;
    }

    let sea = world.sea_level;
    let wind_cols: Vec<f32> = if world.pressure_wind_fields_enabled {
        world
            .clouds
            .iter()
            .map(|c| {
                let wx = c.x.round() as i32;
                let surface = world
                    .column_at(wx)
                    .map(|col| col.surface_y)
                    .unwrap_or(sea);
                let y = surface.max(sea) + 15.0;
                let (vx_m_s, _) = world.wind_at_point(wx, y);
                vx_m_s / SAMPLE_WIDTH_M
            })
            .collect()
    } else {
        vec![climate_wind; world.clouds.len()]
    };
    for (cloud, &w) in world.clouds.iter_mut().zip(wind_cols.iter()) {
        cloud.x += w;
    }

    if world.weather.weather_enabled {
        let climate = world.climate.clone();
        let sea = world.sea_level;
        let clouds = world.clouds.clone();
        // Whether each cloud is *actively* precipitating this tick. Without
        // this, a cloud continuously raining at full intensity every tick
        // it's over any land drains its whole moisture budget within a few
        // dozen ticks — nowhere near enough real distance (at a believable
        // drift speed) to ever reach terrain far from the coast, like the
        // mountains. Intermittent rain spreads a fixed moisture budget over
        // a much longer stretch of travel, and matches "sometimes make it
        // rain" much better than a constant drizzle under every cloud.
        let raining_now: Vec<bool> = clouds
            .iter()
            .enumerate()
            .map(|(idx, _)| {
                wk_world::terrain::hash_f32(world.seed, tick as i64, 900 + idx as u64)
                    < world.weather.rain_chance_per_tick
            })
            .collect();
        let coords: Vec<i32> = world.chunks.keys().copied().collect();
        for coord in coords {
            let base = coord * CHUNK_W as i32;
            let mut touched = false;
            for i in 0..CHUNK_W {
                let wx = base + i as i32;
                // Coverage (x/half_width) is checked against the tick-start
                // snapshot (those don't change mid-tick), but moisture must
                // be read live: many columns can drain the same cloud within
                // one tick, and checking the stale snapshot's moisture let
                // a cloud go deeply negative before finally despawning.
                let Some(cloud_idx) = clouds.iter().position(|c| c.covers(wx)) else {
                    continue;
                };
                if world.clouds[cloud_idx].moisture <= 0.0 || !raining_now[cloud_idx] {
                    continue;
                }
                let (climate_elev, existing_snow) = {
                    let col = &world.chunks.get(&coord).unwrap().columns[i];
                    (col.climate_elevation(), col.top_snow_mass())
                };
                let amount = world.weather.cloud_rain_rate;
                let (rain_component, snow_component) = split_precipitation(
                    sea,
                    amount,
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
                if rain_component > 0 || snow_component > 0 {
                    // Deplete by the actual kg delivered, not a flat amount
                    // per rained-on column — otherwise a cloud crossing the
                    // wide coastal blend zone (many columns each getting a
                    // light trickle) exhausts itself before ever reaching
                    // the mountains further inland.
                    world.clouds[cloud_idx].moisture -= (rain_component + snow_component) as f32;
                    touched = true;
                }
            }
            if touched {
                if let Some(chunk) = world.chunks.get_mut(&coord) {
                    chunk.set_all_active();
                }
            }
        }
    }

    // Despawn clouds that are spent or have drifted well off the map.
    let margin = 5.0;
    world.clouds.retain(|c| {
        c.moisture > 0.0
            && c.x + c.half_width > x_min as f32 - margin
            && c.x - c.half_width < x_max as f32 + margin
    });
}
