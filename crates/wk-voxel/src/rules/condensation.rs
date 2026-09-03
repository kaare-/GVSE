//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Humidity condensation drizzle (+ orographic boost).

use serde::{Deserialize, Serialize};

use crate::grid::World;

use super::util::hash_prob;

/// Condensation-rain parameters for [`apply_condensation_rain`].
///
/// The "cloud row" `top_y` is where droplets appear when a humidity
/// tile is wet enough to precipitate. Rain empties a bounded mass
/// from the tile, and the droplet's sat is proportional to the mass
/// removed (clamped by [`u8::MAX`]).
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct CondensationConfig {
    /// World-y row where droplets condense.
    pub top_y: i32,
    /// A tile only rains when its humidity mass is at or above this.
    /// Prevents faint air moisture from immediately raining back.
    pub min_mass_to_rain: f32,
    /// Chance-per-tick that a *saturated* tile rains at all. Actual
    /// per-tick probability scales linearly from 0 at `min_mass_to_rain`
    /// up to `max_prob_per_tick` at `full_mass`.
    pub max_prob_per_tick: f32,
    /// Humidity mass at which precipitation rate hits its cap.
    pub full_mass: f32,
    /// Mass removed from a tile per rain event.
    pub mass_per_droplet: f32,
    /// Salt mixed into the per-tile tick hash.
    pub seed_salt: u64,
    /// Cap drizzle events per tick (`0` = unlimited). A filled sky
    /// used to rain from every tile (~thousands of column walks → 7 FPS).
    #[serde(default = "default_cond_max_events")]
    pub max_events_per_tick: u32,
}

/// How far past saturation the rain response keeps climbing, as a multiple of
/// the min-to-saturation span.
///
/// Bounded rather than open-ended so a very wet tile cannot pin
/// `max_prob_per_tick` at its ceiling every tick, but wide enough that the
/// diurnal swing in saturation mass stays visible in the response.
const SUPERSATURATION_HEADROOM: f32 = 4.0;

/// Fraction of local saturation left in a raining tile.
///
/// Wiping to zero is what turned the H wash into a terrain-hugging fog
/// sheet: the first free-air tile hit sat, rained its whole mass, and
/// never had a leftover deck that convection could loft. A cloudy
/// remnant stays visible and keeps feeding rise.
const CLOUD_REMNANT_SAT_FRAC: f32 = 0.82;

/// Spare the first free-air tile and one row above it. Fog sits there
/// so thermals can lift it; rain harder aloft where a real cloud is.
const SURFACE_FILM_SPARE_TILES: i32 = 1;

fn default_cond_max_events() -> u32 {
    48
}

impl Default for CondensationConfig {
    fn default() -> Self {
        Self {
            top_y: 0,
            min_mass_to_rain: 64.0,
            max_prob_per_tick: 0.4,
            full_mass: 512.0,
            // One full cell: a sub-cell budget is refused outright by
            // `deposit_condensate_on_surface`, and a refused deposit drains no
            // humidity, so small droplets quietly stall the water cycle.
            mass_per_droplet: 255.0,
            seed_salt: 0xC10D_BA5E,
            max_events_per_tick: default_cond_max_events(),
        }
    }
}

/// Orographic rain boost — moist air dumps when climbing tall land.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct OrographicConfig {
    pub seed: u64,
    pub width_cols: i32,
    pub sea_level_y: i32,
    /// Surface must sit at least this many cells above sea to count
    /// as "tall" for forced release.
    pub tall_above_sea: i32,
    /// Ascent (cells) that reaches full probability multiplier.
    pub ascent_scale: f32,
    /// Max multiplier on rain probability when climbing hard.
    pub max_prob_mult: f32,
    /// Extra mass drained per event on a strong orographic hit.
    pub mass_mult: f32,
    /// Prevailing wind sign (+1 = +x) for upwind surface sampling.
    pub wind_sign: i32,
}

impl Default for OrographicConfig {
    fn default() -> Self {
        Self {
            seed: 0,
            width_cols: 256,
            sea_level_y: 0,
            tall_above_sea: 22,
            ascent_scale: 35.0,
            max_prob_mult: 3.0,
            mass_mult: 1.6,
            wind_sign: 1,
        }
    }
}

/// How many oversaturated tiles may nucleate water in one surplus pass.
///
/// Leftover tiles keep their vapour — this is a rate limit, not a clamp.
const THERMAL_SURPLUS_MAX_HITS: usize = 128;

/// One flake still costs a whole cell. Thaw is `Air+FULL`; a cheaper
/// seat would mint the shortfall. Snow does not carry sat today.
const FLAKE_MASS: f32 = 255.0;

/// Airborne snow is a solid grain. Surplus used to mint up to 128
/// flakes a tick. A light shower, not a blizzard. Hold at −0.1 °C is
/// ~206 tile mass; at −3 °C ~162 — we snow only when over that hold
/// and the parcel can pay a flake.
const SNOW_MAX_FLAKES_PER_TICK: u32 = 8;

/// How far up/down a column we look before seating another flake.
/// Stacking mid-air is what turned a shower into a hanging fog of snow.
const SNOW_COLUMN_CLEARANCE: i32 = 16;

/// True when this air must not become liquid rain.
///
/// Phase's freeze point wins when present; otherwise 0 °C. Below that
/// the C lottery and the surplus pass may snow (if they can pay a flake)
/// or hold — they never `deposit_water_in_air`.
fn air_is_freezing(air_t: f32, phase: Option<&crate::phase::PhaseConfig>) -> bool {
    let freeze = phase
        .map(|ph| ph.freeze_point_c)
        .unwrap_or(0.0);
    air_t <= freeze
}

/// Seat a flake and drain the local humidity parcel. Returns mass landed
/// (0 if the parcel cannot pay [`FLAKE_MASS`] or there is no empty air).
fn column_has_airborne_snow(world: &World, gx: i32, y: i32) -> bool {
    let jx = world.wrap_x(gx);
    for dy in -SNOW_COLUMN_CLEARANCE..=SNOW_COLUMN_CLEARANCE {
        if world
            .get_cell(jx, y + dy)
            .is_some_and(|c| c.material == wk_material::MaterialId::Snow)
        {
            return true;
        }
    }
    false
}

fn try_snow_from_parcel(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    gx: i32,
    gy: i32,
) -> f32 {
    if humidity.peek_around(gx, gy) + 1e-3 < FLAKE_MASS {
        return 0.0;
    }
    if column_has_airborne_snow(world, gx, gy) {
        return 0.0;
    }
    let snowed = crate::phase::deposit_snow_in_air(world, gx, gy, FLAKE_MASS);
    if snowed <= 0.0 {
        return 0.0;
    }
    humidity.take_around(gx, gy, snowed)
}

/// Turn vapour the local air can no longer hold into precip.
///
/// Clausius–Clapeyron shrinks the hold when a tile cools. Above freeze
/// that surplus is rain. At or below freeze it is a snowflake paid from
/// the local 3×3 humidity parcel, or a hold if the parcel cannot pay a
/// whole cell. It is **not** deleted, and it does **not** become liquid
/// rain below 0 °C.
///
/// Only humidity that actually lands is drained. A refused deposit
/// leaves the vapour where it is.
pub fn precipitate_thermal_surplus(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    temp: &crate::temperature::Temperature,
    phase: Option<&crate::phase::PhaseConfig>,
) {
    let tile_cols = humidity.tile_cols.max(1);
    let mut hits: Vec<(f32, i32, i32)> = Vec::new();
    for (&(hx, hy), &mass) in &humidity.cells {
        let sat = crate::humidity::Humidity::saturation_mass_at_temp(temp.at_tile(hx, hy));
        let surplus = mass - sat;
        if surplus >= 1.0 {
            hits.push((surplus, hx, hy));
        }
    }
    if hits.is_empty() {
        return;
    }
    hits.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut flakes = 0u32;
    for &(_, hx, hy) in hits.iter().take(THERMAL_SURPLUS_MAX_HITS) {
        let mass = humidity.at_tile(hx, hy);
        let sat = crate::humidity::Humidity::saturation_mass_at_temp(temp.at_tile(hx, hy));
        let take = (mass - sat).max(0.0);
        if take < 1.0 {
            continue;
        }
        let gx = hx * tile_cols + tile_cols / 2;
        let gy = hy * tile_cols + tile_cols / 2;
        let air_t = temp.at_tile(hx, hy);
        if air_is_freezing(air_t, phase) {
            // Below freeze: snow from the parcel, or hold. Never liquid.
            let snow_ok = phase.map(|ph| ph.enable_snow_precip).unwrap_or(true);
            if snow_ok && flakes < SNOW_MAX_FLAKES_PER_TICK {
                if try_snow_from_parcel(world, humidity, gx, gy) > 0.0 {
                    flakes += 1;
                }
            }
            continue;
        }
        let landed = super::deposit_water_in_air(world, gx, gy, take);
        if landed <= 0.0 {
            continue;
        }
        let entry = humidity.cells.entry((hx, hy)).or_insert(0.0);
        *entry -= landed.min(*entry);
        if *entry < 1e-6 {
            humidity.cells.remove(&(hx, hy));
        }
    }
}

/// Precipitation feedback: humidity tiles that hold enough
/// atmospheric water (especially when colder than the vapor, or
/// when a colder tile sits below — dew) drop droplets back into the
/// cell grid, draining the tile as they do.
///
/// Rain lands at the tile's centre column, in the cell at
/// `cfg.top_y` — provided that cell is currently `Air`. Sat and
/// tile mass are both bounded so the pass can't create or lose
/// mass beyond what's actually available.
///
/// Deterministic given `(world.seed, tile_coord, world.tick,
/// cfg.seed_salt)`.
pub fn apply_condensation_rain(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &CondensationConfig,
) {
    apply_condensation_rain_with_orographic(world, humidity, cfg, None);
}

/// Like [`apply_condensation_rain`], but moist tiles over tall /
/// upslope terrain rain more readily (orographic dump).
pub fn apply_condensation_rain_with_orographic(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &CondensationConfig,
    oro: Option<&OrographicConfig>,
) {
    apply_condensation_rain_phased(world, humidity, cfg, oro, None, None);
}

/// Condensation drizzle from leftover humidity.
///
/// Warm air over the T-hold → liquid rain. At or below freeze → a
/// falling flake paid from the local humidity parcel, or a hold if
/// that parcel cannot cover a whole cell. Never liquid rain below 0 °C.
pub fn apply_condensation_rain_phased(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &CondensationConfig,
    oro: Option<&OrographicConfig>,
    temp: Option<&crate::temperature::Temperature>,
    phase: Option<&crate::phase::PhaseConfig>,
) {
    if cfg.min_mass_to_rain >= cfg.full_mass || cfg.max_prob_per_tick <= 0.0 {
        return;
    }
    let seed = world.seed.0;
    let tick_no = world.tick;
    let tile_cols = humidity.tile_cols;
    // Snapshot tile keys so we can mutate humidity as we go.
    let tiles: Vec<(i32, i32)> = humidity.cells.keys().copied().collect();
    // Collect first, then apply the heaviest hits so a saturated sky
    // cannot walk every column every tick (~thousands → 7 FPS).
    let mut hits: Vec<(f32, i32, i32, f32)> = Vec::new(); // mass, hx, hy, take_mass
    // Lowest wet tile per column — hint for the floor walk. Built for
    // every key (cheap) so a high lottery winner cannot start 24 cells
    // above the crest, miss the ground, and cache "no floor".
    let mut col_lo: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    for &(hx, hy) in &tiles {
        col_lo
            .entry(hx)
            .and_modify(|m| *m = (*m).min(hy))
            .or_insert(hy);
    }
    let mut film_floor: std::collections::HashMap<i32, i32> = std::collections::HashMap::new();
    for (hx, hy) in tiles {
        // Cheap sat / min-mass gate, then lottery, then the column floor
        // walk. Floor does not change the roll, so losers must not pay
        // `first_free_air_hy`. Under-sat tiles skip the below-tile dew
        // read and orographic surface walks too.
        let mass = humidity.at_tile(hx, hy);
        let (mut prob_mult, mut mass_mult, mut min_mass) = (1.0, 1.0, cfg.min_mass_to_rain);
        let mut full_mass = cfg.full_mass;
        let mut leftover = 0.0f32;
        if let Some(th) = temp {
            let sat = thermal_sat_mass(th, hx, hy);
            if mass < sat {
                continue;
            }
            let (tp, tm, tmin, sat) = thermal_rain_factors(th, hx, hy, tile_cols);
            prob_mult *= tp;
            mass_mult *= tm;
            min_mass = tmin;
            full_mass = sat.max(min_mass + 1.0);
            leftover = (sat * CLOUD_REMNANT_SAT_FRAC).max(1.0);
            if let Some(o) = oro {
                let (op, om, _) = orographic_factors(world, o, hx, tile_cols, cfg.min_mass_to_rain);
                prob_mult *= op;
                mass_mult *= om;
            }
        } else {
            if let Some(o) = oro {
                let (op, om, omin) =
                    orographic_factors(world, o, hx, tile_cols, cfg.min_mass_to_rain);
                prob_mult = op;
                mass_mult = om;
                min_mass = omin;
            }
            if mass < min_mass {
                continue;
            }
        }
        // Linear from 0 at min_mass to 1 at thermal/orographic full — and it
        // keeps climbing past that, up to [`SUPERSATURATION_HEADROOM`].
        //
        // Clamping at 1.0 was why the world rained constantly instead of having
        // weather. `full_mass` is the saturation mass, which is what the
        // day/night cycle and orographic lift actually move; once humidity sat
        // above it the clamp pinned this term and neither could change the
        // answer. Measured before: 6 degrees of diurnal swing moved the rain
        // rate by four points (day 80%, night 76%), and hill condensation had
        // effectively stopped. Headroom above saturation gives both somewhere
        // to act, and makes a supersaturated tile rain harder — which is also
        // what pulls the equilibrium back down.
        let t = ((mass - min_mass) / (full_mass - min_mass))
            .clamp(0.0, SUPERSATURATION_HEADROOM);
        let effective_prob = (cfg.max_prob_per_tick * t * prob_mult).clamp(0.0, 0.95);
        // Hash uses tile coord + tick + salt for per-tile determinism.
        let roll = hash_prob(
            seed,
            hx.wrapping_mul(73_856_093).wrapping_add(hy),
            tick_no,
            cfg.seed_salt,
        );
        if roll >= effective_prob {
            continue;
        }
        let hint = col_lo.get(&hx).copied().unwrap_or(hy);
        let floor = first_free_air_hy(world, tile_cols, hx, hint, &mut film_floor);
        if hy <= floor.saturating_add(SURFACE_FILM_SPARE_TILES) {
            continue;
        }
        let surplus = (mass - leftover).max(0.0);
        let raw = (cfg.mass_per_droplet * mass_mult).min(mass);
        // Leftover sat used to shave a 140-mass tile down to ~23. Terrain
        // and the H shaft hide Air sat ≤ 32, so that event was invisible.
        let mut take_mass = raw.min(surplus);
        let air_t = temp.map(|t| t.at_tile(hx, hy));
        let freezing = air_t.is_some_and(|t| air_is_freezing(t, phase));
        // Play droplets are 255. Leftover sat must not shave them under
        // the haze band (invisible rain, no shaft). Snow is a whole cell
        // gathered from the parcel — do not bump a cold leftover to 33
        // and rain it.
        if !freezing
            && cfg.mass_per_droplet + 1e-3 >= crate::phase::PRECIP_IN_AIR_MIN
            && take_mass + 1e-3 < crate::phase::PRECIP_IN_AIR_MIN
            && mass >= crate::phase::PRECIP_IN_AIR_MIN
        {
            take_mass = crate::phase::PRECIP_IN_AIR_MIN;
        }
        if freezing {
            let snow_ok = phase.map(|ph| ph.enable_snow_precip).unwrap_or(true);
            let gx = hx * tile_cols + tile_cols / 2;
            let gy = hy * tile_cols + tile_cols / 2;
            if !snow_ok
                || humidity.peek_around(gx, gy) + 1e-3 < FLAKE_MASS
                || column_has_airborne_snow(world, gx, gy)
            {
                continue;
            }
            take_mass = FLAKE_MASS;
        } else if take_mass <= 0.0 {
            continue;
        }
        hits.push((mass, hx, hy, take_mass));
    }
    if hits.is_empty() {
        return;
    }
    hits.sort_unstable_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let limit = if cfg.max_events_per_tick == 0 {
        hits.len()
    } else {
        hits.len().min(cfg.max_events_per_tick as usize)
    };
    let mut flakes = 0u32;
    for &(_mass, hx, hy, take_mass) in hits.iter().take(limit) {
        let mass = humidity.at_tile(hx, hy);
        if mass <= 0.0 {
            continue;
        }
        let take_mass = take_mass.min(mass);
        if take_mass <= 0.0 {
            continue;
        }
        let centre_gx = hx * tile_cols + tile_cols / 2;
        // Nucleate **where the vapour is** and let gravity have it.
        //
        // This used to deposit on the ground under the tile, scanning up to 512
        // cells down from the sky ceiling — rain that teleported rather than
        // fell, which is why the falling drops had to be a cosmetic overlay
        // drawn over an event that had already finished. A droplet now appears in
        // the air cell that held the vapour and descends like any other water.
        let centre_gy = hy * tile_cols + tile_cols / 2;
        let air_t = temp.map(|t| t.at_tile(hx, hy));
        let freezing = air_t.is_some_and(|t| air_is_freezing(t, phase));
        if freezing {
            // Below freeze: snow from the parcel, or hold. Never liquid.
            let snow_ok = phase.map(|ph| ph.enable_snow_precip).unwrap_or(true);
            if snow_ok && flakes < SNOW_MAX_FLAKES_PER_TICK {
                if try_snow_from_parcel(world, humidity, centre_gx, centre_gy) > 0.0 {
                    flakes += 1;
                }
            }
            continue;
        }
        let mut landed = if take_mass >= crate::phase::PRECIP_IN_AIR_MIN {
            super::deposit_water_in_air(world, centre_gx, centre_gy, take_mass)
        } else {
            0.0
        };
        // Warm frost is unused; this retry is for a refused in-air seat
        // that still has a full cell on the tile (surface glaze).
        if landed <= 0.0 && mass >= u8::MAX as f32 {
            landed = crate::phase::deposit_condensate_on_surface(
                world,
                centre_gx,
                cfg.top_y,
                u8::MAX as f32,
                temp,
                phase,
            );
        }
        if landed <= 0.0 {
            continue;
        }
        // Drain the humidity tile by the mass that landed (clamp to tile).
        let entry = humidity.cells.entry((hx, hy)).or_insert(0.0);
        *entry -= landed.min(*entry);
        if *entry < 1e-6 {
            humidity.cells.remove(&(hx, hy));
        }
    }
}

fn occupies_humidity_film(c: &crate::cell::Cell) -> bool {
    c.material != wk_material::MaterialId::Air || c.sat.0 > crate::GRAIN_REPOSE_HAZE_MAX
}

/// First humidity-tile row whose centre sits in free air above the live crest.
///
/// Cached per `hx`. Walks **down** from the lowest wet tile in the column
/// (a few cells) instead of up from y=0 through the mountain — that 192-cell
/// climb per column was an FPS sink on a wet sky.
fn first_free_air_hy(
    world: &World,
    tile_cols: i32,
    hx: i32,
    hint_hy: i32,
    cache: &mut std::collections::HashMap<i32, i32>,
) -> i32 {
    if let Some(&hy) = cache.get(&hx) {
        return hy;
    }
    let tc = tile_cols.max(1);
    let gx = world.wrap_x(hx * tc + tc / 2);
    let mut y = hint_hy * tc + tc / 2;
    let mut ground = None;
    for _ in 0..24 {
        match world.get_cell(gx, y) {
            Some(c) if occupies_humidity_film(&c) => {
                ground = Some(y);
                break;
            }
            Some(_) => y -= 1,
            None => break,
        }
    }
    let hy = match ground {
        Some(gy) => (gy + 1).div_euclid(tc),
        None => i32::MIN / 4,
    };
    cache.insert(hx, hy);
    hy
}

fn orographic_factors(
    world: &crate::grid::World,
    oro: &OrographicConfig,
    hx: i32,
    tile_cols: i32,
    base_min_mass: f32,
) -> (f32, f32, f32) {
    use crate::worldgen::live_surface_at;
    let tc = tile_cols.max(1);
    let gx = hx * tc + tc / 2;
    let sign = if oro.wind_sign >= 0 { 1 } else { -1 };
    let gx_up = gx - sign * tc;
    let s_here = live_surface_at(world, oro.seed, gx, oro.sea_level_y, oro.width_cols);
    let s_up = live_surface_at(world, oro.seed, gx_up, oro.sea_level_y, oro.width_cols);
    let ascent = (s_here - s_up) as f32;
    let tall = s_here >= oro.sea_level_y + oro.tall_above_sea;
    if !tall && ascent <= 2.0 {
        return (1.0, 1.0, base_min_mass);
    }
    let climb = (ascent / oro.ascent_scale.max(1.0)).clamp(0.0, 1.0);
    // Tall peaks dump readily even without a steep local climb —
    // moist air that makes it inland tends to rain out over high land.
    let tall_f = if tall { 0.65 } else { 0.0 };
    let strength = (climb * 0.7 + tall_f).clamp(0.0, 1.0);
    let prob_mult = 1.0 + strength * (oro.max_prob_mult - 1.0);
    let mass_mult = 1.0 + strength * (oro.mass_mult - 1.0);
    // Tall / climbing air rains from thinner clouds too.
    let min_mass = base_min_mass * (1.0 - 0.55 * strength);
    (prob_mult, mass_mult, min_mass)
}

/// Warm moist air raining when it hits colder air / material.
///
/// Uses the existing temperature tiles only (no extra world walk).
/// Cold air holds less vapor, so the same mass is closer to rain;
/// a colder tile below (ridge, lake, night skin) adds dew.
fn thermal_sat_mass(temp: &crate::temperature::Temperature, hx: i32, hy: i32) -> f32 {
    crate::humidity::Humidity::saturation_mass_at_temp(temp.at_tile(hx, hy)).max(1.0)
}

fn thermal_rain_factors(
    temp: &crate::temperature::Temperature,
    hx: i32,
    hy: i32,
    tile_cols: i32,
) -> (f32, f32, f32, f32) {
    let air = temp.at_tile(hx, hy);
    // The Clausius–Clapeyron hold is the rain gate. The old 0.30 clamp
    // kept min_mass ≈ 19 even at −20 °C, so leftover vapour rained as
    // liquid. Below freeze the apply path refuses water anyway; above
    // freeze we only lottery when the tile is actually over sat.
    let sat = crate::humidity::Humidity::saturation_mass_at_temp(air).max(1.0);
    let min_mass = sat;
    let gx = hx * tile_cols + tile_cols / 2;
    let gy_below = hy * tile_cols - tile_cols;
    let below = temp.at_cell(gx, gy_below);
    let mut prob_mult = 1.0;
    let mut mass_mult = 1.0;
    if below < air - 1.5 {
        let d = ((air - below) / 10.0).clamp(0.0, 1.6);
        prob_mult += 0.65 * d;
        mass_mult += 0.30 * d;
    }
    (prob_mult, mass_mult, min_mass, sat)
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;

    /// A rain budget smaller than one cell is *refused* by
    /// `phase::deposit_condensate_on_surface`, and a refused deposit drains no
    /// humidity at all — so small droplets do not make gentle drizzle, they
    /// stall the water cycle and let the atmosphere fill up behind them.
    ///
    /// Measured on the demo world: 40.0 held equilibrium humidity at 666k where
    /// 255.0 settles at 387k, and the bigger droplet is marginally *cheaper*
    /// because fewer events are wasted.
    /// Cold condensation must make snow that *falls*, not rime on the ground.
    ///
    /// Snow is the frozen counterpart of the phase-1 change that stopped rain
    /// teleporting to the surface. Before this, cold precipitation only had the
    /// frost path, so nothing ever descended through the air.
    #[test]
    fn cold_precipitation_nucleates_snow_in_the_air() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        // A whole cell's worth of budget, into empty air.
        let paid = crate::phase::deposit_snow_in_air(&mut w, 4, 20, u8::MAX as f32);
        assert_eq!(paid, u8::MAX as f32, "a full cell of budget buys one flake");
        assert_eq!(
            w.get_cell(4, 20).unwrap().material,
            MaterialId::Snow,
            "snow should appear in the air, not on the ground"
        );
    }

    #[test]
    fn a_partial_budget_buys_no_snowflake() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // A flake is a whole cell. Part-paying mints the rest on thaw.
        let paid = crate::phase::deposit_snow_in_air(&mut w, 4, 20, 200.0);
        assert_eq!(paid, 0.0, "under a whole cell must be refused, not part-paid");
        assert_ne!(w.get_cell(4, 20).unwrap().material, MaterialId::Snow);
        let _ = Cell::air();
    }

    #[test]
    fn snow_does_not_seed_into_wet_air() {
        use crate::cell::{Cell, Sat};
        use crate::chunk::ChunkCoord;
        use crate::grid::World;

        let mut w = World::new(6);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut wet = Cell::air();
        wet.sat = Sat(200);
        for y in 20..=24 {
            w.set_cell(4, y, wet);
        }
        let paid = crate::phase::deposit_snow_in_air(&mut w, 4, 20, u8::MAX as f32);
        assert_eq!(
            paid, 0.0,
            "seeding into wet air would strand that water inside a cell that does \
             not carry sat"
        );
        assert_eq!(w.get_cell(4, 20).unwrap().sat.0, 200, "its water is untouched");
    }

    #[test]
    fn deposit_snow_in_air_lands_on_first_empty_air_above_solid() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;

        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(16, 16, Cell::solid(MaterialId::Stone));
        w.set_cell(16, 17, Cell::air());
        assert_eq!(
            crate::phase::deposit_snow_in_air(&mut w, 16, 16, u8::MAX as f32),
            u8::MAX as f32
        );
        assert_eq!(w.get_cell(16, 16).unwrap().material, MaterialId::Stone);
        assert_eq!(w.get_cell(16, 17).unwrap().material, MaterialId::Snow);
    }

    #[test]
    fn cold_full_tile_nucleates_a_falling_flake() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(12);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let mut h = Humidity::new(4);
        // Aloft of the spared surface film. 255 is exactly one flake;
        // leftover sat used to shave this to ~238 and refuse.
        h.cells.insert((1, 8), 400.0);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        temp.config.base_temp_c = -12.0;
        for v in temp.cells.values_mut() {
            *v = -12.0;
        }
        let cfg = CondensationConfig {
            top_y: 32,
            max_prob_per_tick: 1.0,
            min_mass_to_rain: 64.0,
            full_mass: 512.0,
            mass_per_droplet: 255.0,
            max_events_per_tick: 8,
            ..CondensationConfig::default()
        };
        let phase = crate::phase::PhaseConfig::default();
        apply_condensation_rain_phased(&mut w, &mut h, &cfg, None, Some(&temp), Some(&phase));
        let flake = (0..8).any(|x| {
            (1..40).any(|y| {
                w.get_cell(x, y)
                    .is_some_and(|c| c.material == MaterialId::Snow)
            })
        });
        assert!(flake, "a full-cell cold tile must seat a falling flake");
        assert!(
            h.at_tile(1, 8) <= 400.0 - 250.0,
            "a paid flake drains a whole cell, left {}",
            h.at_tile(1, 8)
        );
    }

    #[test]
    fn condensation_leaves_a_cloudy_remnant() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let mut h = Humidity::new(4);
        // Aloft of the spared surface film (hy 0–1). Must sit over the
        // 18 °C hold (~700) or the T-gate will not lottery.
        let start = 900.0;
        h.cells.insert((1, 8), start);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = 18.0;
        }
        let cfg = CondensationConfig {
            top_y: 32,
            max_prob_per_tick: 1.0,
            min_mass_to_rain: 64.0,
            full_mass: 512.0,
            mass_per_droplet: 255.0,
            max_events_per_tick: 8,
            ..CondensationConfig::default()
        };
        for _ in 0..8 {
            apply_condensation_rain_phased(&mut w, &mut h, &cfg, None, Some(&temp), None);
            w.tick = w.tick.wrapping_add(1);
            if h.at_tile(1, 8) < start - 1.0 {
                break;
            }
        }
        let left = h.at_tile(1, 8);
        let sat = Humidity::saturation_mass_at_temp(18.0);
        assert!(
            left > sat * 0.70,
            "raining tile must keep a cloudy remnant, left {left} sat={sat:.0}"
        );
        assert!(
            left < start - 1.0,
            "surplus above the remnant must still rain, left {left}"
        );
    }

    #[test]
    fn condensation_spares_the_surface_fog_film() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let mut h = Humidity::new(4);
        h.cells.insert((1, 0), 400.0);
        h.cells.insert((1, 8), 400.0);
        let cfg = CondensationConfig {
            top_y: 32,
            max_prob_per_tick: 1.0,
            min_mass_to_rain: 64.0,
            mass_per_droplet: 255.0,
            max_events_per_tick: 8,
            ..CondensationConfig::default()
        };
        apply_condensation_rain_phased(&mut w, &mut h, &cfg, None, None, None);
        assert!(
            (h.at_tile(1, 0) - 400.0).abs() < 1e-3,
            "surface film must stay so thermals can loft it, left {}",
            h.at_tile(1, 0)
        );
        assert!(
            h.at_tile(1, 8) < 400.0 - 1.0,
            "aloft surplus must still rain, left {}",
            h.at_tile(1, 8)
        );
    }

    #[test]
    fn cold_air_surplus_becomes_snow_not_rain() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        let mut h = Humidity::new(4);
        // Tile (1, 8) → world (6, 34). Load that band of air.
        for y in 32i32..40 {
            w.ensure_chunk(ChunkCoord::new(0, y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32)));
        }
        let start = 400.0;
        h.cells.insert((1, 8), start);
        w.set_cell(6, 34, Cell::air());
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = -20.0;
        }
        temp.rebuild_row_means();
        let sat = Humidity::saturation_mass_at_temp(-20.0);
        assert!(
            start > sat + 50.0,
            "precondition: tile is well over the cold hold (sat={sat:.1})"
        );
        let hum_before = h.total_mass();
        precipitate_thermal_surplus(&mut w, &mut h, &temp, None);
        let hum_after = h.total_mass();
        let drained = hum_before - hum_after;
        assert!(
            (drained - FLAKE_MASS).abs() < 1.0,
            "oversaturated cold air should pay one flake (drained={drained:.1})"
        );
        let cell = w.get_cell(6, 34).expect("air seat");
        assert_eq!(
            cell.material,
            MaterialId::Snow,
            "below freeze surplus must snow, not rain (sat={})",
            cell.sat.0
        );
        assert_eq!(cell.sat.0, 0, "a flake does not carry sat");
    }

    #[test]
    fn below_freezing_leftover_does_not_rain() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 32i32..40 {
            w.ensure_chunk(ChunkCoord::new(0, y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32)));
        }
        w.set_cell(6, 34, Cell::air());
        let mut h = Humidity::new(4);
        // Over the −20 °C hold (~35) but short of a flake, even with neighbours.
        h.cells.insert((1, 8), 80.0);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = -20.0;
        }
        let cfg = CondensationConfig {
            top_y: 32,
            max_prob_per_tick: 1.0,
            min_mass_to_rain: 64.0,
            full_mass: 512.0,
            mass_per_droplet: 255.0,
            max_events_per_tick: 8,
            ..CondensationConfig::default()
        };
        let phase = crate::phase::PhaseConfig::default();
        apply_condensation_rain_phased(&mut w, &mut h, &cfg, None, Some(&temp), Some(&phase));
        precipitate_thermal_surplus(&mut w, &mut h, &temp, Some(&phase));
        assert!(
            (h.at_tile(1, 8) - 80.0).abs() < 1e-3,
            "underpaid cold leftover must hold, left {}",
            h.at_tile(1, 8)
        );
        let cell = w.get_cell(6, 34).expect("air seat");
        assert_eq!(cell.material, MaterialId::Air);
        assert_eq!(cell.sat.0, 0, "must not rain at −20 °C");
    }

    #[test]
    fn snow_gathers_neighbour_tiles_for_a_full_flake() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0i32..16 {
            w.ensure_chunk(ChunkCoord::new(
                x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                0,
            ));
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 32i32..40 {
            w.ensure_chunk(ChunkCoord::new(0, y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32)));
            w.ensure_chunk(ChunkCoord::new(1, y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32)));
        }
        w.set_cell(6, 34, Cell::air());
        let mut h = Humidity::new(4);
        // Centre tile is short of a flake; the neighbour covers it.
        // Surplus walks heaviest-first, so put the shortfall on the
        // tile we nucleate (1, 8) → world (6, 34).
        h.cells.insert((1, 8), 200.0);
        h.cells.insert((2, 8), 80.0);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = -12.0;
        }
        let phase = crate::phase::PhaseConfig::default();
        precipitate_thermal_surplus(&mut w, &mut h, &temp, Some(&phase));
        assert!(
            (h.total_mass() - (280.0 - FLAKE_MASS)).abs() < 1.0,
            "parcel should pay one flake, left {}",
            h.total_mass()
        );
        assert_eq!(
            w.get_cell(6, 34).map(|c| c.material),
            Some(MaterialId::Snow),
            "gathered neighbours must seat a flake on the nucleating tile"
        );
    }

    #[test]
    fn surplus_caps_airborne_flakes_per_tick() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        for x in 0i32..64 {
            w.ensure_chunk(ChunkCoord::new(
                x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                0,
            ));
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 32i32..40 {
            for cx in 0..2 {
                w.ensure_chunk(ChunkCoord::new(
                    cx,
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
            }
        }
        let mut h = Humidity::new(4);
        for hx in 1..16 {
            h.cells.insert((hx, 8), 400.0);
            w.set_cell(hx * 4 + 2, 34, Cell::air());
        }
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = -12.0;
        }
        let phase = crate::phase::PhaseConfig::default();
        precipitate_thermal_surplus(&mut w, &mut h, &temp, Some(&phase));
        let mut flakes = 0usize;
        for x in 0..64 {
            for y in 1..40 {
                if w.get_cell(x, y)
                    .is_some_and(|c| c.material == MaterialId::Snow)
                {
                    flakes += 1;
                }
            }
        }
        assert!(
            flakes > 0 && flakes <= SNOW_MAX_FLAKES_PER_TICK as usize,
            "surplus must shower, not blizzard (flakes={flakes})"
        );
    }

    #[test]
    fn snow_skips_a_column_that_already_has_a_flake() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        for y in 32i32..40 {
            w.ensure_chunk(ChunkCoord::new(0, y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32)));
        }
        w.set_cell(6, 34, Cell::solid(MaterialId::Snow));
        let mut h = Humidity::new(4);
        h.cells.insert((1, 8), 400.0);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = -12.0;
        }
        let phase = crate::phase::PhaseConfig::default();
        precipitate_thermal_surplus(&mut w, &mut h, &temp, Some(&phase));
        let flakes = (30..40)
            .filter(|&y| {
                w.get_cell(6, y)
                    .is_some_and(|c| c.material == MaterialId::Snow)
            })
            .count();
        assert_eq!(
            flakes, 1,
            "must not stack another flake on an occupied column"
        );
        assert!(
            (h.at_tile(1, 8) - 400.0).abs() < 1e-3,
            "occupied column must hold the vapour, left {}",
            h.at_tile(1, 8)
        );
    }

    #[test]
    fn refused_thermal_surplus_stays_in_the_air() {
        use crate::cell::Cell;
        use crate::chunk::ChunkCoord;
        use crate::grid::World;
        use crate::humidity::Humidity;
        use crate::temperature::Temperature;

        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Tile centre is solid — deposit_water_in_air refuses.
        for x in 0i32..8 {
            for y in 0i32..40 {
                w.ensure_chunk(ChunkCoord::new(
                    x.div_euclid(crate::chunk::CHUNK_CELLS_W as i32),
                    y.div_euclid(crate::chunk::CHUNK_CELLS_H as i32),
                ));
                w.set_cell(x, y, Cell::solid(MaterialId::Stone));
            }
        }
        let mut h = Humidity::new(4);
        h.cells.insert((1, 8), 400.0);
        let mut temp = Temperature::with_world_bounds(4, 0, 0, 64, 64, 1, 64, 8, false);
        for v in temp.cells.values_mut() {
            *v = -20.0;
        }
        temp.rebuild_row_means();
        precipitate_thermal_surplus(&mut w, &mut h, &temp, None);
        assert!(
            (h.at_tile(1, 8) - 400.0).abs() < 1e-3,
            "a refused deposit must leave the vapour (left {})",
            h.at_tile(1, 8)
        );
    }

    #[test]
    fn shipped_configs_use_whole_cell_droplets() {
        let full = u8::MAX as f32;
        for (name, cond) in [
            ("default", CondensationConfig::default()),
            (
                "tab_defaults",
                crate::sim_preset::SimPreset::tab_defaults().cond,
            ),
            (
                "soak_survival",
                crate::sim_preset::SimPreset::soak_survival().cond,
            ),
        ] {
            assert!(
                cond.mass_per_droplet >= full,
                "{name}: mass_per_droplet {} is under one cell ({full}), so its \
                 deposits are refused and drain no humidity",
                cond.mass_per_droplet
            );
        }
    }
}
