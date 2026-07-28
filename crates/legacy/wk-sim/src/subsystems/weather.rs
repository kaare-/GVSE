//! Automatic drifting-cloud weather layer.

use wk_material::{SAMPLE_WIDTH_M};
use wk_world::{CHUNK_W};
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
        // Occasionally spawn a small pack so cover arrives as weather
        // systems, not one lonely puff every few minutes.
        let pack = 1
            + (wk_world::terrain::hash_f32(seed, tick as i64, 400) * 2.5).floor() as usize;
        let room = world.weather.max_clouds.saturating_sub(world.clouds.len());
        let n = pack.min(room).max(1);
        for k in 0..n {
            let salt = tick.wrapping_add(k as u64 * 17) as i64;
            let half_width = 10.0 + wk_world::terrain::hash_f32(seed, salt, 401) * 55.0;
            // Needs enough budget for one or two full rain bursts while
            // crossing ocean/shelf, then continuing inland.
            let moisture =
                14_000.0 + wk_world::terrain::hash_f32(seed, salt, 402) * 26_000.0;
            let spawn_x = if world.topology().is_ring() {
                x_min as f32
                    + wk_world::terrain::hash_f32(seed, salt, 403)
                        * (x_max - x_min).max(1) as f32
            } else if climate_wind >= 0.0 {
                x_min as f32 - half_width - k as f32 * half_width * 0.4
            } else {
                x_max as f32 + half_width + k as f32 * half_width * 0.4
            };
            world.clouds.push(wk_world::weather::Cloud {
                x: spawn_x,
                half_width,
                moisture,
                raining: false,
                rain_ticks_left: 0,
            });
        }
        world.next_cloud_spawn_tick = tick + world.weather.cloud_spawn_interval_ticks;
    }

    let sea = world.sea_level;
    let ring = world.topology().is_ring();
    let width_cols = world.topology().width_columns();
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
        if ring {
            if let Some(wcols) = width_cols {
                let w = wcols as f32;
                cloud.x = cloud.x.rem_euclid(w);
            }
        }
    }

    // Advance rain-burst state independently of hydrology delivery so the
    // renderer can show streaks under a cloud even when every column under
    // it is ocean (still "raining" visually).
    if world.weather.weather_enabled {
        let burst_min = world.weather.rain_burst_ticks_min.max(1);
        let burst_max = world.weather.rain_burst_ticks_max.max(burst_min);
        let start_p = world.weather.rain_chance_per_tick.clamp(0.0, 1.0);
        for (idx, cloud) in world.clouds.iter_mut().enumerate() {
            if cloud.rain_ticks_left > 0 {
                cloud.rain_ticks_left -= 1;
                cloud.raining = cloud.rain_ticks_left > 0 && cloud.moisture > 0.0;
                if cloud.rain_ticks_left == 0 {
                    cloud.raining = false;
                }
            } else {
                cloud.raining = false;
                let h = wk_world::terrain::hash_f32(
                    world.seed,
                    tick as i64,
                    900 + idx as u64,
                );
                if cloud.moisture > 0.0 && h < start_p {
                    let span = (burst_max - burst_min) as f32;
                    let len = burst_min
                        + (wk_world::terrain::hash_f32(
                            world.seed,
                            tick as i64,
                            910 + idx as u64,
                        ) * (span + 1.0))
                            .floor() as u16;
                    cloud.rain_ticks_left = len.max(1);
                    cloud.raining = true;
                }
            }
        }
    } else {
        for cloud in &mut world.clouds {
            cloud.raining = false;
            cloud.rain_ticks_left = 0;
        }
    }

    if world.weather.weather_enabled {
        let climate = world.climate.clone();
        let sea = world.sea_level;
        let clouds = world.clouds.clone();
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
                if world.clouds[cloud_idx].moisture <= 0.0 || !clouds[cloud_idx].raining {
                    continue;
                }
                let (climate_elev, existing_frozen) = {
                    let col = &world.chunks.get(&coord).unwrap().columns[i];
                    (col.climate_elevation(), col.frozen_surface_mass())
                };
                let amount = world.weather.cloud_rain_rate;
                let (rain_component, snow_component) = split_precipitation(
                    sea,
                    amount,
                    climate_elev,
                    tick,
                    &climate,
                    existing_frozen,
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

    // Despawn spent clouds; off-map cull only on open strips (rings wrap).
    let margin = 5.0;
    let ring = world.topology().is_ring();
    world.clouds.retain(|c| {
        if c.moisture <= 0.0 {
            return false;
        }
        if ring {
            true
        } else {
            c.x + c.half_width > x_min as f32 - margin
                && c.x - c.half_width < x_max as f32 + margin
        }
    });
}
