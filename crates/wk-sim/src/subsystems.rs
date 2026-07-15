use wk_material::{CHUNK_W, MaterialId, MaterialRegistry};
use wk_world::column::{Activity, SedimentLoad};
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
/// Fraction of suspended sediment that settles out of the water column
/// onto the bed each tick, regardless of flow speed. Continuous Stokes-
/// like settling — without this term sediment carried by a permanently
/// tilted river never actually deposits: capacity-based rules only fire
/// when the flow stops entirely, which for a steady stream it never
/// does. Real particles fall at their terminal velocity even in fast
/// flow; fast flow just keeps re-eroding what fell. Rate is small
/// enough that the sediment still visibly travels a fair distance
/// downstream before it's fully out of the water.
const SEDIMENT_SETTLE_FRACTION: f32 = 0.008;
/// Groundwater moves far slower than surface water in reality (limited by
/// how fast water can actually squeeze through pore space, not just
/// gravity), but slower than 0.02 becomes imperceptible at play speed —
/// waterlogged saturation should visibly spread between neighbouring
/// columns within a handful of seconds, not minutes.
const GROUNDWATER_FLOW_COEFF: f32 = 0.02;
/// Fraction of the "just enough to bring the slope back to the angle of
/// repose" transfer applied each tick when a granular top layer sits
/// steeper than its material allows. Small enough that a large slump
/// takes several ticks to fully unfold (avoiding the classic explicit-
/// diffusion oscillation), large enough to visibly settle within a
/// second or two at 1× play speed.
const SLUMP_RELAXATION: f32 = 0.35;
/// Cap on how much snow can pile up on a single column (~a few metres
/// depth) — a safety net independent of the climate_elevation fix, since
/// even without the feedback loop a permanently sub-freezing spot would
/// otherwise accumulate snow forever under constant precipitation.
const MAX_SNOW_MASS_KG: i64 = 6000;

pub struct SimParams {
    pub rain_rate: f32,
    pub rain_enabled: bool,
    pub sea_level: f32,
}

/// Splits a potential precipitation `amount` falling on one column into
/// a liquid rain or snow component (by local temperature). No more
/// "sea top-up" pump: the world is a closed rain ↔ evaporation loop
/// now, and the ocean level maintains itself through that cycle just
/// like every other water body. Ocean columns still receive rain when
/// clouds pass over them; they just don't get a special hidden refill.
fn split_precipitation(
    sea: f32,
    amount: f32,
    climate_elev: f32,
    tick: u64,
    climate: &wk_world::climate::ClimateSettings,
    existing_snow: i64,
) -> (i64, i64) {
    // (water_component, snow_component)
    // `amount` stays a float all the way through so a fractional rate
    // (e.g. cloud_rain_rate = 1.5) doesn't get chopped to a whole
    // number before rounding into an i64 kg count.
    let precip_component = amount.round() as i64;
    if precip_component <= 0 {
        return (0, 0);
    }
    // Uses climate_elevation (excludes any snow already piled up), not
    // raw surface_y — otherwise snow raising the surface would make
    // the column read as colder, causing still more snow: a runaway
    // feedback loop.
    let temp = wk_world::climate::temperature_at(climate_elev, sea, tick, climate);
    if temp <= climate.freeze_point_c && existing_snow < MAX_SNOW_MASS_KG {
        // Capped so a permanently-frozen spot doesn't accumulate an
        // unbounded snow tower; beyond the cap it falls as rain/slush
        // runoff instead (a crude stand-in for avalanche transport).
        (0, precip_component)
    } else {
        (precip_component, 0)
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

/// Fraction of the top phase-changing layer's mass that transitions per
/// tick when the temperature crosses its threshold (scaled by how far
/// past the threshold we are, capped at 10C to avoid runaway rates on
/// extreme days). One number covers snow→water, water→ice, and ice→water
/// because the physics is symmetric enough at this fidelity.
const PHASE_CHANGE_COEFF: f32 = 0.03;

pub fn run_surface_water(world: &World, scratch: &mut WorldTransferScratch) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        if !chunk.any_hydrology_active() {
            continue;
        }

        let mut flux_record = [0i64; CHUNK_W];
        // (out_left, out_right, net_water,
        //  sed_left, sed_right, sed_material)
        let mut deltas =
            [(0i64, 0i64, 0i64, 0i64, 0i64, MaterialId::Sand); CHUNK_W];

        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            // Flow considers any Water in the fluid cap — even water
            // sitting under a Snow or Ice cap can drain sideways, since
            // those caps float on the water rather than sealing it in.
            // Without this a snowfall on a pool would build an unbounded
            // water tower: rain deposits Water on top, settle sinks it
            // below the snow, and (under the old top-only rule) flow
            // then couldn't touch it.
            let Some((water_top_y, water_here)) = col.flowable_water() else {
                continue;
            };
            if col.activity == Activity::Dormant || water_here <= 0 {
                continue;
            }

            let head_here = water_top_y;
            let head_left = chunk.water_top_neighbor(i as i32 - 1);
            let head_right = chunk.water_top_neighbor(i as i32 + 1);

            let grad_left = head_here - head_left;
            let grad_right = head_here - head_right;

            let mut out_left = 0i64;
            let mut out_right = 0i64;

            if grad_left > 0.0 {
                let equalizing = grad_left * WATER_MASS_PER_METRE_DEPTH * 0.5;
                out_left = ((equalizing * FLOW_RELAXATION) as i64).min(water_here);
            }
            if grad_right > 0.0 {
                let remaining = (water_here - out_left).max(0);
                let equalizing = grad_right * WATER_MASS_PER_METRE_DEPTH * 0.5;
                out_right = ((equalizing * FLOW_RELAXATION) as i64).min(remaining);
            }

            // Sediment travels with water proportionally. If half the
            // column's water flows left, half its suspended sediment
            // goes with it — that's how a river carries silt downstream
            // rather than leaving it stranded at the erosion site.
            let (sed_left, sed_right) = if col.sediment.total > 0 {
                let sed = col.sediment.total;
                let sl = (out_left as i128 * sed as i128 / water_here as i128) as i64;
                let sr = (out_right as i128 * sed as i128 / water_here as i128) as i64;
                // Never send more sediment than the column has.
                let (sl, sr) = if sl + sr > sed {
                    let total = sl + sr;
                    let sl2 = (sl as i128 * sed as i128 / total as i128) as i64;
                    let sr2 = sed - sl2;
                    (sl2, sr2)
                } else {
                    (sl, sr)
                };
                (sl, sr)
            } else {
                (0, 0)
            };

            deltas[i] = (
                out_left,
                out_right,
                -(out_left + out_right),
                sed_left,
                sed_right,
                col.sediment.dominant,
            );
            flux_record[i] = out_left + out_right;
        }

        let mut left_water_outbox = 0i64;
        let mut right_water_outbox = 0i64;
        let mut left_sed_outbox = SedimentLoad::default();
        let mut right_sed_outbox = SedimentLoad::default();

        for i in 0..CHUNK_W {
            let (out_left, out_right, net, sed_left, sed_right, sed_mat) = deltas[i];
            let buf = scratch.buffer_mut(coord);
            buf.water_delta[i] += net;
            buf.sediment_delta[i] -= sed_left + sed_right;

            if i == 0 {
                left_water_outbox += out_left;
                if sed_left > 0 {
                    left_sed_outbox.add(sed_mat, sed_left);
                }
            } else {
                buf.water_delta[i - 1] += out_left;
                if sed_left > 0 {
                    buf.sediment_inflow[i - 1].add(sed_mat, sed_left);
                }
            }

            if i == CHUNK_W - 1 {
                right_water_outbox += out_right;
                if sed_right > 0 {
                    right_sed_outbox.add(sed_mat, sed_right);
                }
            } else {
                buf.water_delta[i + 1] += out_right;
                if sed_right > 0 {
                    buf.sediment_inflow[i + 1].add(sed_mat, sed_right);
                }
            }
        }
        let outbox = scratch.outbox_mut(coord);
        outbox.left_water += left_water_outbox;
        outbox.right_water += right_water_outbox;
        if left_sed_outbox.total > 0 {
            outbox
                .left_sediment
                .add(left_sed_outbox.dominant, left_sed_outbox.total);
        }
        if right_sed_outbox.total > 0 {
            outbox
                .right_sediment
                .add(right_sed_outbox.dominant, right_sed_outbox.total);
        }
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

            // Continuous Stokes-like settling: some fraction of any
            // suspended sediment falls onto the bed each tick, whether
            // or not the flow is fast. This is what makes eroded sand
            // actually accumulate as a deposit downstream instead of
            // circulating in suspension forever on a permanently-tilted
            // stream (fast flow will then re-erode most of it back into
            // suspension, matching how a real river bed exchanges
            // sediment continuously with its water column).
            if col.sediment.total > 0 && col.top_water_mass() > 0 {
                let settle = (col.sediment.total as f32 * SEDIMENT_SETTLE_FRACTION) as i64;
                let settle = settle.max(1).min(col.sediment.total);
                dep_req[i] += settle;
                dep_mat[i] = col.sediment.dominant;
            }

            let y_here = col.surface_y;
            let y_left = chunk.surface_y_neighbor(i as i32 - 1);
            let y_right = chunk.surface_y_neighbor(i as i32 + 1);
            let flux_indicator =
                ((y_here - y_left).max(0.0) + (y_here - y_right).max(0.0)).sqrt();

            // Under the unified model the top layer might be Water
            // (a river / pond). Erosion still targets the erodible bed
            // *underneath* that water, so we look for the top erodible
            // layer here rather than blindly taking `top_material`.
            let Some(bed_idx) = (0..col.layer_count as usize).find(|&j| {
                let m = col.layers[j].material;
                m.is_erodible() && MaterialRegistry::props(m).erosion_resistance < 150
            }) else {
                continue;
            };
            let material = col.layers[bed_idx].material;
            let props = MaterialRegistry::props(material);

            let water = col.top_water_mass().max(0) as f32;
            if water < 1.0 || flux_indicator < 0.01 {
                continue;
            }

            // Waterlogged instability: if the erodible bed *is* the
            // column's aquifer layer (top porous layer, the one
            // accumulating pore-water in `col.moisture`), its effective
            // cohesion collapses as saturation approaches 1. Physically
            // this is the pore-pressure collapse behind a sand dam or a
            // river bank slumping under a saturated shore — dry sand
            // holds its angle of repose; saturated sand doesn't.
            //
            // At full saturation resistance drops to ~30% of dry.
            let saturation = if col.top_porous_layer_index() == Some(bed_idx) {
                let cap = col.moisture_cap().max(1) as f32;
                (col.moisture as f32 / cap).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let effective_resistance =
                (props.erosion_resistance as f32) * (1.0 - saturation * 0.7);
            let erosion_rate =
                water * flux_indicator * EROSION_FLUX_COEFF / effective_resistance.max(1.0);
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
                // Permeability comes from the porous *substrate* that
                // will absorb the water, not the material sitting on
                // top (which is Water itself, permeability 0, which
                // would incorrectly block all infiltration under a
                // puddle).
                let perm = col
                    .top_porous_layer()
                    .map(|l| MaterialRegistry::props(l.material).permeability as f32 / 255.0)
                    .unwrap_or(0.0);
                (
                    col.activity,
                    col.top_water_mass(),
                    col.moisture,
                    col.moisture_cap(),
                    perm,
                )
            };
            if activity == Activity::Dormant || available <= 0 || perm <= 0.0 {
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
/// metre — depends on the porous layer's porosity and thickness. Under
/// the unified model this looks at the topmost *porous* layer, skipping
/// any water/ice/snow cap above it, so a puddle-covered sand bed still
/// has a well-defined aquifer capacity.
fn aquifer_mass_per_metre(col: &wk_world::column::Column) -> f32 {
    let cap = col.moisture_cap().max(1) as f32;
    let Some(layer) = col.top_porous_layer() else {
        return f32::INFINITY;
    };
    let layer_height_m = col.mass_to_height_delta(layer.material, layer.thickness);
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

            let perm = col
                .top_porous_layer()
                .map(|l| MaterialRegistry::props(l.material).permeability as f32 / 255.0)
                .unwrap_or(0.0);
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
                (col.activity, col.top_water_mass(), col.moisture)
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
            // Consider *any* water in the fluid cap — including water
            // that's sitting under a floating snow cap. Otherwise a
            // half-snow-covered lake fails hydrostatic equalization on
            // exactly the columns where the snow floated over, leaving
            // an uneven water surface right where the snow line sits.
            let (water_top_y, water) =
                col.flowable_water().unwrap_or((col.surface_y, 0));
            let bed_y = water_top_y - col.mass_to_height_delta(MaterialId::Water, water);
            cells.push(LakeCell {
                coord,
                local,
                ground: bed_y,
                water,
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

    // Rewrite each cell's total-flowable-water mass to the newly
    // computed value. We read `flowable_water` (not `top_water_mass`)
    // so a snow-covered pool sees its actual water total, not the
    // zero it would show if only the top layer counted — that
    // mismatch would double the water on write-back.
    for cell in cells {
        if let Some(chunk) = world.chunks.get_mut(&cell.coord) {
            let col = &mut chunk.columns[cell.local];
            let current = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            let delta = cell.water - current;
            col.adjust_top_water(delta, 0);
            col.settle_by_density(0);
            col.recompute_surface_y(chunk.bedrock_y);
        }
    }
}

/// Unified phase-change subsystem. Walks every column's top layer and,
/// if that material has a `phase_change` property, converts a fraction
/// of its mass into the target material based on temperature crossing
/// the threshold. This one function replaces `run_snow_melt` and
/// `run_freeze_thaw` — snow melting into water, water freezing into ice,
/// and ice thawing back into water are all instances of the same rule
/// driven by different property rows.
///
/// Direct-mutation subsystem: transitions only ever move mass within
/// the same column, no cross-column conflict.
pub fn run_phase_change(world: &mut World, tick: u64) {
    let climate = world.climate.clone();
    let sea_level = world.sea_level;
    for chunk in world.chunks.values_mut() {
        for col in &mut chunk.columns {
            let Some(top) = col.top_layer() else { continue; };
            let Some(pc) = MaterialRegistry::props(top.material).phase_change else {
                continue;
            };
            let mass_here = top.thickness;
            if mass_here <= 0 {
                continue;
            }
            let temp = wk_world::climate::temperature_at(
                col.climate_elevation(),
                sea_level,
                tick,
                &climate,
            );
            let target = if temp > pc.threshold_c {
                pc.above
            } else {
                pc.below
            };
            let Some(target) = target else { continue; };
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

/// Angle-of-repose slumping: loose granular material on a slope steeper
/// than its material's stable angle slides toward the lower neighbour.
///
/// Post-barrier direct mutation, same shape as `run_phase_change` /
/// `run_lake_level`, so it operates on already-committed layer state
/// (no interference with the buffered water/sediment deltas). For each
/// column pair, if the *solid* top surfaces differ by more than the
/// top solid material's `repose_rise_m`, transfer part of the excess
/// mass from the higher column's top solid layer to the lower one.
///
/// This is what turns a jagged terrain generation output — or a fresh
/// sediment deposit dropped straight onto one column — into a smoothly
/// graded slope over a handful of ticks, without any renderer-side
/// smoothing hack.
pub fn run_slumping(world: &mut World, tick: u64) {
    // Two passes per invocation to accelerate convergence for wide
    // repose fronts (excess piles up until it hits repose, then flows).
    for _ in 0..2 {
        slumping_pass(world, tick);
    }
}

fn slumping_pass(world: &mut World, tick: u64) {
    // Snapshot every column's top-solid state, INCLUDING columns whose
    // top solid can't slump itself (Stone / Bedrock outcrops). Those
    // still have to appear in the snapshot as valid *destinations* — a
    // sand column right next to an exposed bedrock outcrop should be
    // able to shed material onto the outcrop's shoulder, otherwise its
    // sand just piles up against an invisible wall and the visible
    // cliff never levels out.
    #[derive(Clone, Copy)]
    struct TopSolid {
        coord: i32,
        local: usize,
        material: MaterialId,
        thickness: i64,
        // Elevation of the top of this solid layer (excludes fluid cap above).
        top_y: f32,
        repose_rise: f32,
        // Kg per metre of layer height for this material — for
        // converting height differences back into transferable mass.
        // Zero for non-slumpable materials (they aren't sources).
        mass_per_m: f32,
        /// The top layer of the *entire* column, before skipping the
        /// fluid cap — used at deposit-time to decide whether to melt
        /// snow on contact with a water body.
        column_top_material: MaterialId,
    }

    let mut snapshot: Vec<TopSolid> = Vec::new();
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = &world.chunks[&coord];
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            let Some(idx) = (0..col.layer_count as usize).find(|&j| {
                // Skip fluids (Water flows via surface flow, Ice is rigid).
                // Include Snow — it's granular, has an angle of repose.
                !matches!(col.layers[j].material, MaterialId::Water | MaterialId::Ice)
            }) else {
                continue;
            };
            let layer = col.layers[idx];
            let props = MaterialRegistry::props(layer.material);
            let density = props.density.max(1) as f32;
            // Elevation of the top of this solid layer.
            let mut y = col.surface_y;
            for j in 0..idx {
                y -= col.mass_to_height_delta(col.layers[j].material, col.layers[j].thickness);
            }
            snapshot.push(TopSolid {
                coord,
                local,
                material: layer.material,
                thickness: layer.thickness,
                top_y: y,
                repose_rise: props.repose_rise_m,
                mass_per_m: density * wk_material::SAMPLE_WIDTH_M,
                column_top_material: col.top_material(),
            });
        }
    }

    // Sort by (coord, local) so we can index by world_x quickly.
    // World-x = coord * CHUNK_W + local. Use a BTreeMap keyed on that.
    use std::collections::BTreeMap;
    let mut by_wx: BTreeMap<i32, usize> = BTreeMap::new();
    for (i, s) in snapshot.iter().enumerate() {
        let wx = s.coord * CHUNK_W as i32 + s.local as i32;
        by_wx.insert(wx, i);
    }

    // For each column, compute a signed transfer against left and right
    // neighbours. Positive = mass leaving this column. Sources must
    // themselves be slumpable material (finite repose_rise); destinations
    // can be anything, including bedrock/stone outcrops that just
    // happen to sit lower — sand piling against a rock wall should still
    // shed material onto the shoulder of that wall.
    let mut transfers: Vec<(i32, i32, MaterialId, i64, MaterialId)> = Vec::new(); // (from_wx, to_wx, source_material, mass, dest_column_top)
    let wx_list: Vec<i32> = by_wx.keys().copied().collect();
    for wx in wx_list {
        let idx = by_wx[&wx];
        let s = snapshot[idx];
        if !s.repose_rise.is_finite() {
            continue; // Immovable source (Stone / Bedrock / Ice).
        }
        for &neighbour_wx in &[wx - 1, wx + 1] {
            let Some(&nidx) = by_wx.get(&neighbour_wx) else {
                continue;
            };
            let n = snapshot[nidx];
            let dy = s.top_y - n.top_y;
            if dy <= s.repose_rise {
                continue;
            }
            let excess = dy - s.repose_rise;
            let height_to_move = 0.5 * excess * SLUMP_RELAXATION;
            let mass_to_move = (height_to_move * s.mass_per_m) as i64;
            let mass_to_move = mass_to_move.min(s.thickness / 2).max(0);
            if mass_to_move > 0 {
                transfers.push((wx, neighbour_wx, s.material, mass_to_move, n.column_top_material));
            }
        }
    }

    // Apply transfers. Mass conservation is critical here: whatever
    // we actually manage to remove from the source is exactly what
    // gets deposited on the destination.
    //
    // Snow sliding into a water body melts on contact — an avalanche
    // reaching a lake doesn't leave a persistent floating slush ring,
    // it just adds equivalent water volume. Deposit as Water instead
    // of Snow whenever the destination column's *top layer* is Water.
    for (from_wx, to_wx, source_material, mass, dest_top) in transfers {
        let from_coord = World::chunk_coord_for_world_x(from_wx);
        let from_local = World::local_x(from_wx);
        let mut actual_take = 0i64;
        if let Some(chunk) = world.chunks.get_mut(&from_coord) {
            let col = &mut chunk.columns[from_local];
            if let Some(j) = (0..col.layer_count as usize)
                .find(|&j| col.layers[j].material == source_material)
            {
                let take = mass.min(col.layers[j].thickness);
                if take > 0 {
                    let dh = col.mass_to_height_delta(source_material, take);
                    col.layers[j].thickness -= take;
                    col.surface_y -= dh;
                    col.activity = Activity::HydrologyActive;
                    actual_take = take;
                }
            }
        }
        if actual_take <= 0 {
            continue;
        }
        let deposit_material = if source_material == MaterialId::Snow
            && dest_top == MaterialId::Water
        {
            // Snow sliding into open water melts on contact.
            MaterialId::Water
        } else {
            source_material
        };
        if let Some(chunk) = world.chunks.get_mut(&World::chunk_coord_for_world_x(to_wx)) {
            let local = World::local_x(to_wx);
            let col = &mut chunk.columns[local];
            col.deposit_to_top(deposit_material, actual_take, tick);
        }
    }

    // Clean up: settle+clamp per touched column so any empty layer
    // slots collapse, densities restack correctly, and surface_y
    // stays consistent with total layer heights.
    let touched_coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in touched_coords {
        let bedrock_y = world.chunks[&coord].bedrock_y;
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            for col in &mut chunk.columns {
                col.clamp_state();
                col.recompute_surface_y(bedrock_y);
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
