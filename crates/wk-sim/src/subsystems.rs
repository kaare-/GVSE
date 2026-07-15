use wk_material::{CHUNK_W, MaterialId, MaterialRegistry};
use wk_world::column::Activity;
use wk_world::world::World;

use crate::buffer::{ChunkBoundaryOutbox, WorldTransferScratch};
use crate::residual::ResidualAccumulator;

/// kg of water per metre of standing depth on one column (density 1000
/// kg/m^3 * SAMPLE_WIDTH_M 0.25 m cross-section).
const WATER_MASS_PER_METRE_DEPTH: f32 = 250.0;
/// Fraction of the "exact equalizing transfer" actually applied each tick.
/// This is mathematically equivalent to the diffusion parameter r in the
/// standard explicit scheme d' = d + r*(neighbour - 2*d + other_neighbour),
/// with r = FLOW_RELAXATION/2. Von Neumann stability requires r <= 0.5, i.e.
/// FLOW_RELAXATION <= 1.0 — at exactly 1.0 the worst-case (checkerboard)
/// mode is damped to zero in a single tick, the fastest possible speed with
/// no oscillation. Above ~1.0 it starts ringing again (the earlier bug);
/// staying at 0.9 keeps a small safety margin for the asymmetric real-world
/// coupling (unequal neighbours, ground slope) that a pure symmetric
/// analysis doesn't capture exactly.
const FLOW_RELAXATION: f32 = 0.97;
const EROSION_FLUX_COEFF: f32 = 0.004;
const SEDIMENT_CAPACITY_COEFF: f32 = 0.02;
// Slow enough that standing water lingers as a visible pool for a while
// instead of soaking away within a few dozen ticks, but fast enough that a
// basin collecting runoff from a whole mountainside still reaches a real
// equilibrium below the surrounding peaks instead of climbing forever.
const INFILTRATION_COEFF: f32 = 0.01;
const EVAPORATION_COEFF: f32 = 0.035;
const HUMIDITY: f32 = 0.4;
/// Groundwater moves far slower than surface water in reality (limited by
/// how fast water can actually squeeze through pore space, not just
/// gravity), so this is deliberately tiny compared to FLOW_RELAXATION.
const GROUNDWATER_FLOW_COEFF: f32 = 0.004;
/// Cap on how much snow can pile up on a single column (~a few metres
/// depth) — a safety net independent of the climate_elevation fix, since
/// even without the feedback loop a permanently sub-freezing spot would
/// otherwise accumulate snow forever under constant precipitation.
const MAX_SNOW_MASS_KG: i64 = 6000;

fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if edge0 == edge1 {
        return if x >= edge1 { 1.0 } else { 0.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Standing water depth (metres) for a given mass — used so flow gradients
/// consider the actual water surface, not just bare ground elevation.
fn water_depth_m(mass: i64) -> f32 {
    if mass <= 0 {
        return 0.0;
    }
    let density = MaterialRegistry::props(MaterialId::Water).density.max(1) as f32;
    (mass as f32 / density) / wk_material::SAMPLE_WIDTH_M
}

pub struct SimParams {
    pub rain_rate: f32,
    pub rain_enabled: bool,
    pub sea_level: f32,
}

/// Splits a potential precipitation `amount` falling on one column into a
/// sea top-up, liquid rain, or snow component — shared by both the manual
/// global rain toggle and cloud-driven weather so they behave identically
/// at the coastline (smooth land/sea blend, no hard branch) and around the
/// freeze point (snow instead of rain when cold).
fn split_precipitation(
    surface_y: f32,
    sea: f32,
    surface_water: i64,
    amount: f32,
    climate_elev: f32,
    tick: u64,
    climate: &wk_world::climate::ClimateSettings,
    existing_snow: i64,
) -> (i64, i64, i64) {
    // (sea_component, water_component, snow_component)
    // Narrow band: this only needs to smooth away the single-column color/
    // material flicker right at the shoreline (the original bug it fixed).
    // A wide band means a large stretch of the still-mostly-submerged shelf
    // partially "counts" as land for precipitation purposes, which was
    // draining cloud moisture over open water long before a cloud ever
    // reached real land.
    let land_frac = smoothstep(-0.5, 0.5, surface_y - sea);
    let sea_deficit = 120i64.saturating_sub(surface_water).max(0);
    let sea_component = (sea_deficit as f32 * (1.0 - land_frac)).round() as i64;
    // `amount` stays a float all the way through so a fractional rate (e.g.
    // cloud_rain_rate = 1.5) doesn't get chopped to a whole number before
    // land_frac scaling even gets a chance to apply — otherwise any rate
    // below 2 effectively rounds to "0 or 1 kg, no in-between" everywhere.
    let precip_component = (amount * land_frac).round() as i64;
    if precip_component <= 0 {
        return (sea_component, 0, 0);
    }
    // Uses climate_elevation (excludes any snow already piled up), not raw
    // surface_y — otherwise snow raising the surface would make the column
    // read as colder, causing still more snow: a runaway feedback loop.
    let temp = wk_world::climate::temperature_at(climate_elev, sea, tick, climate);
    if temp <= climate.freeze_point_c && existing_snow < MAX_SNOW_MASS_KG {
        // Capped so a permanently-frozen spot doesn't accumulate an
        // unbounded snow tower; beyond the cap it falls as rain/slush
        // runoff instead (a crude stand-in for avalanche transport).
        (sea_component, 0, precip_component)
    } else {
        (sea_component, precip_component, 0)
    }
}

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
        // Rain/sea top-up is an external forcing — it must reach every
        // column including currently Dormant ones. Otherwise a fully dried
        // out region can never receive rain again (nothing else would ever
        // set it back to active), permanently deadlocking re-hydration.
        for i in 0..CHUNK_W {
            let (surface_y, surface_water, climate_elev, existing_snow) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                (
                    col.surface_y,
                    col.surface_water,
                    col.climate_elevation(),
                    if col.top_material() == MaterialId::Snow {
                        col.top_layer().map(|l| l.thickness).unwrap_or(0)
                    } else {
                        0
                    },
                )
            };
            let (sea_component, rain_component, snow_component) = split_precipitation(
                surface_y,
                sea,
                surface_water,
                inject_per_col,
                climate_elev,
                tick,
                &climate,
                existing_snow,
            );
            let buf = scratch.buffer_mut(coord);
            if sea_component > 0 {
                buf.water_delta[i] += sea_component;
                world.mass_audit.sea_inject_total += sea_component;
            }
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

/// Spawns, advances, and rains from drifting clouds — the weather layer on
/// top of the manual rain toggle. Clouds are not particles: each is just an
/// x position, half-width, and remaining moisture, advected by a constant
/// wind speed (columns/tick, sign = direction) and consumed as it rains.
pub fn run_weather(world: &mut World, scratch: &mut WorldTransferScratch, tick: u64) {
    let Some((x_min, x_max)) = world.world_x_bounds() else {
        return;
    };

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
        let moisture = 6000.0 + wk_world::terrain::hash_f32(seed, tick as i64, 402) * 12000.0;
        let spawn_x = if world.climate.wind_speed >= 0.0 {
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

    let wind = world.climate.wind_speed;
    for cloud in &mut world.clouds {
        cloud.x += wind;
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
                let (surface_y, surface_water, climate_elev, existing_snow) = {
                    let col = &world.chunks.get(&coord).unwrap().columns[i];
                    (
                        col.surface_y,
                        col.surface_water,
                        col.climate_elevation(),
                        if col.top_material() == MaterialId::Snow {
                            col.top_layer().map(|l| l.thickness).unwrap_or(0)
                        } else {
                            0
                        },
                    )
                };
                let amount = world.weather.cloud_rain_rate;
                let (sea_component, rain_component, snow_component) = split_precipitation(
                    surface_y,
                    sea,
                    surface_water,
                    amount,
                    climate_elev,
                    tick,
                    &climate,
                    existing_snow,
                );
                let buf = scratch.buffer_mut(coord);
                if sea_component > 0 {
                    buf.water_delta[i] += sea_component;
                    world.mass_audit.sea_inject_total += sea_component;
                }
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

/// Fraction of a snow layer's mass that melts per degree above freezing,
/// per tick. Direct-mutation subsystem (like layer merge / activity) rather
/// than buffer-based: melting only ever affects the column it happens in,
/// no cross-column conflict to resolve.
const SNOW_MELT_COEFF: f32 = 0.02;

pub fn run_snow_melt(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            if col.top_material() != MaterialId::Snow {
                continue;
            }
            let snow_mass = col.top_layer().map(|l| l.thickness).unwrap_or(0);
            if snow_mass <= 0 {
                continue;
            }
            let temp =
                wk_world::climate::temperature_at(col.climate_elevation(), sea_level, tick, &climate);
            let above_freeze = temp - climate.freeze_point_c;
            if above_freeze <= 0.0 {
                continue;
            }
            let melt = (snow_mass as f32 * SNOW_MELT_COEFF * above_freeze.min(10.0)) as i64;
            let melt = melt.max(1).min(snow_mass);
            let (removed, mat) = col.erode_from_top(melt);
            if removed > 0 && mat == MaterialId::Snow {
                col.surface_water += removed;
                col.activity = Activity::HydrologyActive;
            }
        }
    }
}

pub fn run_surface_water(world: &World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        if !chunk.any_hydrology_active() {
            continue;
        }

        let mut flux_record = [0i64; CHUNK_W];
        let mut deltas = [(0i64, 0i64, 0i64); CHUNK_W]; // out_left, out_right, net

        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            if col.activity == Activity::Dormant || col.surface_water <= 0 {
                continue;
            }

            // Compare total water-surface head (ground + standing water), not
            // just bare ground, so ponds level out across flat/low terrain
            // instead of only ever moving when the ground itself slopes.
            let head_here = col.surface_y + water_depth_m(col.surface_water);
            let head_left =
                chunk.surface_y_neighbor(i as i32 - 1) + water_depth_m(chunk.water_neighbor(i as i32 - 1));
            let head_right =
                chunk.surface_y_neighbor(i as i32 + 1) + water_depth_m(chunk.water_neighbor(i as i32 + 1));

            let grad_left = head_here - head_left;
            let grad_right = head_here - head_right;

            // Transfer only a damped fraction of the mass that would exactly
            // equalize heads with this neighbour (never more) — this is
            // unconditionally stable: it can approach equality but can't
            // overshoot into oscillation the way a gradient-proportional
            // flux with a high coefficient did before (that caused a
            // checkerboard: alternating columns locking into two different
            // stable water levels instead of leveling out).
            let mut out_left = 0i64;
            let mut out_right = 0i64;

            if grad_left > 0.0 {
                let equalizing = grad_left * WATER_MASS_PER_METRE_DEPTH * 0.5;
                out_left = ((equalizing * FLOW_RELAXATION) as i64).min(col.surface_water);
            }
            if grad_right > 0.0 {
                let remaining = (col.surface_water - out_left).max(0);
                let equalizing = grad_right * WATER_MASS_PER_METRE_DEPTH * 0.5;
                out_right = ((equalizing * FLOW_RELAXATION) as i64).min(remaining);
            }

            deltas[i] = (out_left, out_right, -(out_left + out_right));
            flux_record[i] = out_left + out_right;
        }

        let mut left_outbox = 0i64;
        let mut right_outbox = 0i64;

        for i in 0..CHUNK_W {
            let (out_left, out_right, net) = deltas[i];
            {
                let buf = scratch.buffer_mut(coord);
                buf.water_delta[i] += net;

                if i == 0 {
                    left_outbox += out_left;
                } else {
                    buf.water_delta[i - 1] += out_left;
                }

                if i == CHUNK_W - 1 {
                    right_outbox += out_right;
                } else {
                    buf.water_delta[i + 1] += out_right;
                }
            }
        }
        scratch.outbox_mut(coord).left_water += left_outbox;
        scratch.outbox_mut(coord).right_water += right_outbox;
        scratch.last_water_flux.insert(coord, flux_record);
    }
}

pub fn run_sediment(world: &World, scratch: &mut WorldTransferScratch, tick: u64) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        if !chunk.any_hydrology_active() {
            continue;
        }

        let mut erosion_record = [0i64; CHUNK_W];
        let mut erode_req = [0i64; CHUNK_W];
        let sed_delta = [0i64; CHUNK_W];
        let mut dep_req = [0i64; CHUNK_W];
        let mut dep_mat = [MaterialId::Sand; CHUNK_W];

        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            if col.activity == Activity::Dormant {
                continue;
            }

            let y_here = col.surface_y;
            let y_left = chunk.surface_y_neighbor(i as i32 - 1);
            let y_right = chunk.surface_y_neighbor(i as i32 + 1);
            let flux_indicator =
                ((y_here - y_left).max(0.0) + (y_here - y_right).max(0.0)).sqrt();

            let material = col.top_material();
            let props = MaterialRegistry::props(material);
            if !material.is_erodible() || props.erosion_resistance >= 150 {
                continue;
            }

            let water = col.surface_water.max(0) as f32;
            if water < 1.0 || flux_indicator < 0.01 {
                continue;
            }

            let erosion_rate = water * flux_indicator * EROSION_FLUX_COEFF
                / (props.erosion_resistance as f32).max(1.0);
            let erode_mass = erosion_rate as i64;
            if erode_mass <= 0 {
                continue;
            }

            erode_req[i] += erode_mass;
            erosion_record[i] = erode_mass;

            let capacity = (water * flux_indicator * SEDIMENT_CAPACITY_COEFF * 1000.0) as i64;
            let current_sed = col.sediment.total + erode_req[i] + sed_delta[i];
            if current_sed > capacity {
                let excess = current_sed - capacity;
                dep_req[i] += excess;
                dep_mat[i] = material;
            }

            if flux_indicator < 0.05 && col.sediment.total > 50 {
                let deposit = (col.sediment.total / 16).max(1);
                dep_req[i] += deposit;
                dep_mat[i] = col.sediment.dominant;
            }
        }

        let buf = scratch.buffer_mut(coord);
        for i in 0..CHUNK_W {
            buf.erosion_request[i] += erode_req[i];
            buf.sediment_delta[i] += sed_delta[i];
            buf.deposit_request[i] += dep_req[i];
            if dep_req[i] > 0 {
                buf.deposit_material[i] = dep_mat[i];
            }
        }
        scratch.last_erosion_flux.insert(coord, erosion_record);
        let _ = tick;
    }
}

pub fn run_infiltration(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let (activity, available, moisture, cap, perm) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                (
                    col.activity,
                    col.surface_water,
                    col.moisture,
                    col.moisture_cap(),
                    MaterialRegistry::props(col.top_material()).permeability as f32 / 255.0,
                )
            };
            if activity == Activity::Dormant || available <= 0 {
                continue;
            }
            let rate = available as f32 * perm * INFILTRATION_COEFF;
            let col = world.chunks.get_mut(&coord).unwrap();
            let transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.infiltration, rate);
            let actual = transfer.min(available).min(cap.saturating_sub(moisture));
            if actual > 0 {
                let buf = scratch.buffer_mut(coord);
                buf.water_delta[i] -= actual;
                buf.moisture_delta[i] += actual;
            }
        }
    }
}

/// kg of moisture needed to raise this column's own water table by one
/// metre — depends on the top layer's porosity and thickness, so it varies
/// per column (sand holds water differently than clay or stone).
fn aquifer_mass_per_metre(col: &wk_world::column::Column) -> f32 {
    let cap = col.moisture_cap().max(1) as f32;
    let Some(top) = col.top_layer() else {
        return f32::INFINITY;
    };
    let layer_height_m = col.mass_to_height_delta(top.material, top.thickness);
    if layer_height_m <= 0.0 {
        return f32::INFINITY;
    }
    cap / layer_height_m
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

            let head_here = col.water_table_y();
            let head_left = chunk.water_table_neighbor(i as i32 - 1);
            let head_right = chunk.water_table_neighbor(i as i32 + 1);

            let grad_left = head_here - head_left;
            let grad_right = head_here - head_right;

            let perm = MaterialRegistry::props(col.top_material()).permeability as f32 / 255.0;
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

pub fn run_evaporation(world: &mut World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        for i in 0..CHUNK_W {
            let (activity, surface_water, moisture) = {
                let col = &world.chunks.get(&coord).unwrap().columns[i];
                (col.activity, col.surface_water, col.moisture)
            };
            if activity == Activity::Dormant {
                continue;
            }
            let evap_factor = 1.0 - HUMIDITY;
            let from_surface =
                (surface_water as f32 * EVAPORATION_COEFF * evap_factor).max(0.0);
            let from_moisture =
                (moisture as f32 * EVAPORATION_COEFF * 0.5 * evap_factor).max(0.0);

            let col = world.chunks.get_mut(&coord).unwrap();
            let surf_transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.evaporation, from_surface);
            let moist_transfer =
                ResidualAccumulator::drain(&mut col.columns[i].residual.evaporation, from_moisture);

            let surf_actual = surf_transfer.min(surface_water);
            let moist_actual = moist_transfer.min(moisture);

            if surf_actual > 0 || moist_actual > 0 {
                let buf = scratch.buffer_mut(coord);
                buf.water_delta[i] -= surf_actual;
                buf.moisture_delta[i] -= moist_actual;
                world.mass_audit.evap_out_total += surf_actual + moist_actual;
            }
        }
    }
}

struct LakeCell {
    coord: i32,
    local: usize,
    ground: f32,
    water: i64,
}

/// Fraction of the way each application moves cells toward the exact flat
/// equilibrium. A full-strength instant snap (1.0) makes small/shallow
/// bodies "pop" — their footprint and total mass are still changing tick to
/// tick from rain/evaporation/infiltration, so jumping straight to a brand
/// new exact target every time looks like water appearing/disappearing out
/// of nowhere. Blending gradually (combined with running this more often,
/// see LakeLevel's schedule) still converges much faster than pure
/// neighbour-by-neighbour diffusion for a *wide* lake, but changes smoothly
/// enough tick-to-tick to not read as a glitch for small ponds.
const LAKE_LEVEL_BLEND: f32 = 0.1;
/// Minimum standing water (kg, ~10cm depth on one column) to count as part
/// of a "lake" for leveling purposes. Without this, a light rain sheen
/// sitting on every column across the whole map — including hilltops with
/// only a trace of water — would all register as nonzero and therefore
/// "connected", causing the leveling pass to treat the *entire visible map*
/// as one giant lake and dilute real pooled water down to nothing. Trace
/// amounts below this threshold are still governed by ordinary diffusion,
/// evaporation and infiltration; they just aren't part of a lake body.
const MIN_LAKE_WATER_KG: i64 = 25;

/// Binary search for the common water-surface elevation such that flooding
/// every cell in `cells` up to that level uses exactly `total_mass`.
fn solve_level(cells: &[LakeCell], total_mass: i64) -> f32 {
    let min_ground = cells.iter().map(|c| c.ground).fold(f32::MAX, f32::min);
    let mut lo = min_ground;
    let mut hi = min_ground + 1.0;

    let volume_at = |level: f32| -> f32 {
        cells
            .iter()
            .map(|c| (level - c.ground).max(0.0))
            .sum::<f32>()
            * WATER_MASS_PER_METRE_DEPTH
    };

    // Grow the upper bound until it can hold at least the total mass.
    for _ in 0..24 {
        if volume_at(hi) >= total_mass as f32 {
            break;
        }
        hi = min_ground + (hi - min_ground) * 2.0 + 1.0;
    }

    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if volume_at(mid) < total_mass as f32 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// Move each cell partway from its current value toward the level implied
/// by `total_mass` (mass-conserving either way, since blending two
/// distributions that both sum to `total_mass` still sums to `total_mass`).
fn level_segment(cells: &mut [LakeCell]) {
    let total_mass: i64 = cells.iter().map(|c| c.water).sum();
    if total_mass <= 0 {
        return;
    }
    let level = solve_level(cells, total_mass);

    let mut assigned = 0i64;
    let mut deepest_idx = 0usize;
    let mut deepest_val = i64::MIN;
    for (idx, c) in cells.iter_mut().enumerate() {
        let depth = (level - c.ground).max(0.0);
        let target_mass = (depth * WATER_MASS_PER_METRE_DEPTH) as i64;
        let blended =
            (c.water as f32 + (target_mass - c.water) as f32 * LAKE_LEVEL_BLEND) as i64;
        let blended = blended.max(0);
        c.water = blended;
        assigned += blended;
        if blended > deepest_val {
            deepest_val = blended;
            deepest_idx = idx;
        }
    }
    // Rounding can leave a tiny drift; dump it on the deepest cell so total
    // mass is preserved exactly.
    let drift = total_mass - assigned;
    if drift != 0 {
        cells[deepest_idx].water = (cells[deepest_idx].water + drift).max(0);
    }
}

/// Gradually flattens every connected run of "currently wet" columns across
/// the whole loaded world (not just within one chunk — lakes can span
/// several) toward a single hydrostatic level. Runs periodically, not every
/// tick: it's meant to model near-instant real water pressure equalization,
/// which the per-tick neighbour diffusion (run_surface_water) is too slow
/// to reproduce on its own for a wide lake.
pub fn run_lake_level(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    if coords.is_empty() {
        return;
    }

    let mut cells: Vec<LakeCell> = Vec::with_capacity(coords.len() * CHUNK_W);
    for &coord in &coords {
        let chunk = &world.chunks[&coord];
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            cells.push(LakeCell {
                coord,
                local,
                ground: col.surface_y,
                water: col.surface_water,
            });
        }
    }

    let n = cells.len();
    let mut i = 0usize;
    while i < n {
        if cells[i].water < MIN_LAKE_WATER_KG {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end + 1 < n && cells[end + 1].water >= MIN_LAKE_WATER_KG {
            end += 1;
        }
        if end > start {
            level_segment(&mut cells[start..=end]);
        }
        i = end + 1;
    }

    for cell in cells {
        if let Some(chunk) = world.chunks.get_mut(&cell.coord) {
            chunk.columns[cell.local].surface_water = cell.water;
        }
    }
}

/// Fraction of remaining liquid water that freezes per tick when below the
/// freeze point.
const FREEZE_RATE_FRAC: f32 = 0.03;
/// Fraction of ice that thaws per tick per degree above freezing.
const THAW_RATE_FRAC: f32 = 0.03;

/// Freezes standing liquid water into inert `ice` when cold, and thaws it
/// back when warm. Direct-mutation subsystem: freezing/thawing only ever
/// moves mass between two fields *within* the same column, no cross-column
/// conflict to resolve. Because evaporation/infiltration/lateral flow only
/// ever look at `surface_water`, frozen mass is automatically excluded from
/// all of them for free — no separate guards needed elsewhere.
pub fn run_freeze_thaw(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            if col.surface_water <= 0 && col.ice <= 0 {
                continue;
            }
            let temp = wk_world::climate::temperature_at(col.climate_elevation(), sea_level, tick, &climate);
            if temp <= climate.freeze_point_c {
                if col.surface_water > 0 {
                    let freeze = ((col.surface_water as f32) * FREEZE_RATE_FRAC) as i64;
                    let freeze = freeze.max(1).min(col.surface_water);
                    col.surface_water -= freeze;
                    col.ice += freeze;
                }
            } else if col.ice > 0 {
                let above_freeze = (temp - climate.freeze_point_c).min(10.0);
                let thaw = ((col.ice as f32) * THAW_RATE_FRAC * above_freeze) as i64;
                let thaw = thaw.max(1).min(col.ice);
                col.ice -= thaw;
                col.surface_water += thaw;
                col.activity = Activity::HydrologyActive;
            }
        }
    }
}

pub fn run_layer_merge(world: &mut World, tick: u64) {
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            col.merge_layers(true, tick);
        }
    }
}

pub fn run_activity(world: &mut World) {
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            let cap = col.moisture_cap();
            let active = col.surface_water > 0
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

pub fn update_halos(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let left = world.chunks.get(&(coord - 1)).cloned();
        let right = world.chunks.get(&(coord + 1)).cloned();
        let chunk = world.chunks.get_mut(&coord).unwrap();
        chunk.update_halos_from_neighbors(left.as_ref(), right.as_ref());
    }
}

pub fn exchange_outboxes(world: &mut World, scratch: &WorldTransferScratch) -> i64 {
    let mut boundary_out = 0i64;
    let pairs: Vec<(i32, ChunkBoundaryOutbox)> = scratch
        .outbox
        .iter()
        .map(|(&c, o)| (c, o.clone()))
        .collect();

    for (coord, outbox) in pairs {
        if let Some(right) = world.chunks.get_mut(&(coord + 1)) {
            right.inbox.water_in[0] += outbox.right_water;
            right.inbox.moisture_in[0] += outbox.right_moisture;
            right.inbox.sediment_in[0].add(
                outbox.right_sediment.dominant,
                outbox.right_sediment.total,
            );
        } else {
            boundary_out += outbox.right_water + outbox.right_sediment.total + outbox.right_moisture;
        }

        if let Some(left) = world.chunks.get_mut(&(coord - 1)) {
            let last = CHUNK_W - 1;
            left.inbox.water_in[last] += outbox.left_water;
            left.inbox.moisture_in[last] += outbox.left_moisture;
            left.inbox.sediment_in[last].add(
                outbox.left_sediment.dominant,
                outbox.left_sediment.total,
            );
        } else {
            boundary_out += outbox.left_water + outbox.left_sediment.total + outbox.left_moisture;
        }
    }
    boundary_out
}
