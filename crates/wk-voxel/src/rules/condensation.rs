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
/// Warm air → liquid film. Cold air on cold ground → a **thin Ice glaze**
/// (frost / rime), not `Snow` packs and never taller than
/// [`crate::phase::PhaseConfig::frost_coat_depth`]. Cloud flakes still own
/// real snow accumulation.
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
    for (hx, hy) in tiles {
        let mass = humidity.at_tile(hx, hy);
        let (mut prob_mult, mut mass_mult, mut min_mass) = match oro {
            Some(o) => orographic_factors(o, hx, tile_cols, cfg.min_mass_to_rain),
            None => (1.0, 1.0, cfg.min_mass_to_rain),
        };
        let mut full_mass = cfg.full_mass;
        if let Some(th) = temp {
            let (tp, tm, tmin, sat) = thermal_rain_factors(th, hx, hy, tile_cols, cfg);
            prob_mult *= tp;
            mass_mult *= tm;
            min_mass = min_mass.min(tmin);
            full_mass = sat.max(min_mass + 1.0);
        }
        if mass < min_mass {
            continue;
        }
        // Linear scale from 0 at min_mass to max at thermal/orographic full.
        let t = ((mass - min_mass) / (full_mass - min_mass)).clamp(0.0, 1.0);
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
        let take_mass = (cfg.mass_per_droplet * mass_mult).min(mass);
        if take_mass <= 0.0 {
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
    for &(_mass, hx, hy, take_mass) in hits.iter().take(limit) {
        let mass = humidity.at_tile(hx, hy);
        if mass <= 0.0 {
            continue;
        }
        let take_mass = take_mass.min(mass);
        // Rain / frost lands on the ground / ocean under the tile centre.
        let centre_gx = hx * tile_cols + tile_cols / 2;
        let mut landed = crate::phase::deposit_condensate_on_surface(
            world,
            centre_gx,
            cfg.top_y,
            take_mass,
            temp,
            phase,
        );
        // Cold frost needs a full cell (255). Small drizzle budgets refuse;
        // retry once from the tile so rime can still form without underpaying.
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

fn orographic_factors(
    oro: &OrographicConfig,
    hx: i32,
    tile_cols: i32,
    base_min_mass: f32,
) -> (f32, f32, f32) {
    use crate::worldgen::continental_surface_y;
    let tc = tile_cols.max(1);
    let gx = hx * tc + tc / 2;
    let sign = if oro.wind_sign >= 0 { 1 } else { -1 };
    let gx_up = gx - sign * tc;
    let s_here = continental_surface_y(oro.seed, gx, oro.sea_level_y, oro.width_cols);
    let s_up = continental_surface_y(oro.seed, gx_up, oro.sea_level_y, oro.width_cols);
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
fn thermal_rain_factors(
    temp: &crate::temperature::Temperature,
    hx: i32,
    hy: i32,
    tile_cols: i32,
    cfg: &CondensationConfig,
) -> (f32, f32, f32, f32) {
    let air = temp.at_tile(hx, hy);
    let sat = crate::humidity::Humidity::saturation_mass_at_temp(air)
        * (cfg.full_mass / crate::humidity::Humidity::MAX_MASS_PER_TILE).clamp(0.15, 1.0);
    let sat = sat.max(12.0);
    let min_mass = cfg.min_mass_to_rain * (sat / cfg.full_mass.max(1.0)).clamp(0.30, 1.35);
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

    /// A rain budget smaller than one cell is *refused* by
    /// `phase::deposit_condensate_on_surface`, and a refused deposit drains no
    /// humidity at all — so small droplets do not make gentle drizzle, they
    /// stall the water cycle and let the atmosphere fill up behind them.
    ///
    /// Measured on the demo world: 40.0 held equilibrium humidity at 666k where
    /// 255.0 settles at 387k, and the bigger droplet is marginally *cheaper*
    /// because fewer events are wasted.
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
