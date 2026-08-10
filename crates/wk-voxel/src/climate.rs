//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Shared day/night clock for light, temperature, and sky drawing.
//!
//! Phase convention matches the original Set A `day_factor`:
//! **tick 0 ≈ noon**. Day and night lengths are independently tunable.
//!
//! Seasons, lunar phase, and sun/moon apparent-size (distance) cycles
//! are cosmetic + day-length drivers for the voxel demo sky. They do
//! not model real orbital mechanics.

use serde::{Deserialize, Serialize};

/// Default full day+night cycle length in ticks.
pub const DEMO_DAY_TICKS: u64 = 1_200;

/// Default year length (~28 demo days).
pub const DEMO_SEASON_TICKS: u64 = DEMO_DAY_TICKS * 28;

/// Default synodic month (~4 demo days).
pub const DEMO_LUNAR_TICKS: u64 = DEMO_DAY_TICKS * 4;

/// Tunable day / night / season / celestial presentation.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ClimateConfig {
    /// Daylight duration in ticks (noon→dusk + dawn→noon) at equinox.
    pub day_ticks: u64,
    /// Night duration in ticks (dusk→dawn) at equinox.
    pub night_ticks: u64,

    /// Full year length in ticks (season clock).
    #[serde(default = "default_season_ticks")]
    pub season_ticks: u64,
    /// Season phase offset in `[0, 1)` — scrub / start of year.
    /// 0 = winter solstice, 0.25 = spring, 0.5 = summer, 0.75 = autumn.
    #[serde(default = "default_season_offset")]
    pub season_offset: f32,
    /// When true, season stays fixed at [`Self::season_offset`].
    #[serde(default)]
    pub season_lock: bool,
    /// How strongly season stretches day vs night (0 = off, ~0.35 = bold).
    #[serde(default = "default_season_day_amp")]
    pub season_day_length_amp: f32,
    /// Seasonal sky tint strength (winter cool / summer warm / autumn gold).
    #[serde(default = "default_season_sky_tint")]
    pub season_sky_tint: f32,

    /// Synodic month length in ticks (new→new).
    #[serde(default = "default_lunar_ticks")]
    pub lunar_ticks: u64,
    /// Lunar phase offset in `[0, 1)` — 0 = new, 0.5 = full.
    #[serde(default)]
    pub lunar_offset: f32,
    /// When true, moon phase stays fixed at [`Self::lunar_offset`].
    #[serde(default)]
    pub lunar_lock: bool,
    /// Moon apparent-size wobble (perigee/apogee), 0 = fixed.
    #[serde(default = "default_moon_distance_amp")]
    pub moon_distance_amp: f32,
    /// Mean moon radius in screen pixels.
    #[serde(default = "default_moon_base_radius")]
    pub moon_base_radius: f32,

    /// Sun apparent-size wobble (perihelion), 0 = fixed.
    #[serde(default = "default_sun_distance_amp")]
    pub sun_distance_amp: f32,
    /// Mean sun radius in screen pixels.
    #[serde(default = "default_sun_base_radius")]
    pub sun_base_radius: f32,
    /// Night starfield opacity multiplier (0 = off).
    #[serde(default = "default_star_strength")]
    pub star_strength: f32,
}

fn default_season_ticks() -> u64 {
    DEMO_SEASON_TICKS
}
fn default_season_offset() -> f32 {
    0.35 // late spring — matches [`ClimateConfig::default`]
}
fn default_season_day_amp() -> f32 {
    0.28
}
fn default_season_sky_tint() -> f32 {
    0.55
}
fn default_lunar_ticks() -> u64 {
    DEMO_LUNAR_TICKS
}
fn default_moon_distance_amp() -> f32 {
    0.22
}
fn default_moon_base_radius() -> f32 {
    14.0
}
fn default_sun_distance_amp() -> f32 {
    0.10
}
fn default_sun_base_radius() -> f32 {
    16.0
}
fn default_star_strength() -> f32 {
    0.85
}

impl Default for ClimateConfig {
    fn default() -> Self {
        Self {
            day_ticks: DEMO_DAY_TICKS / 2,
            night_ticks: DEMO_DAY_TICKS / 2,
            season_ticks: DEMO_SEASON_TICKS,
            season_offset: default_season_offset(),
            season_lock: false,
            season_day_length_amp: default_season_day_amp(),
            season_sky_tint: default_season_sky_tint(),
            lunar_ticks: DEMO_LUNAR_TICKS,
            lunar_offset: 0.0,
            lunar_lock: false,
            moon_distance_amp: default_moon_distance_amp(),
            moon_base_radius: default_moon_base_radius(),
            sun_distance_amp: default_sun_distance_amp(),
            sun_base_radius: default_sun_base_radius(),
            star_strength: default_star_strength(),
        }
    }
}

impl ClimateConfig {
    pub fn total_ticks(self) -> u64 {
        self.day_ticks.max(1) + self.night_ticks.max(1)
    }
}

/// Season name for HUD / Tab labels.
pub fn season_name(season: f32) -> &'static str {
    let s = season.rem_euclid(1.0);
    if s < 0.125 || s >= 0.875 {
        "Winter"
    } else if s < 0.375 {
        "Spring"
    } else if s < 0.625 {
        "Summer"
    } else {
        "Autumn"
    }
}

/// Season fraction in `[0, 1)` — 0 winter solstice, 0.5 summer solstice.
pub fn season_fraction(tick: u64) -> f32 {
    season_fraction_cfg(tick, &ClimateConfig::default())
}

pub fn season_fraction_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    if cfg.season_lock {
        return cfg.season_offset.rem_euclid(1.0);
    }
    let period = cfg.season_ticks.max(1);
    let off = (cfg.season_offset.rem_euclid(1.0) * period as f32).round() as u64;
    ((tick.wrapping_add(off)) % period) as f32 / period as f32
}

/// Lunar phase in `[0, 1)` — 0 new moon, 0.5 full moon.
pub fn lunar_fraction(tick: u64) -> f32 {
    lunar_fraction_cfg(tick, &ClimateConfig::default())
}

pub fn lunar_fraction_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    if cfg.lunar_lock {
        return cfg.lunar_offset.rem_euclid(1.0);
    }
    let period = cfg.lunar_ticks.max(1);
    let off = (cfg.lunar_offset.rem_euclid(1.0) * period as f32).round() as u64;
    ((tick.wrapping_add(off)) % period) as f32 / period as f32
}

/// Lit fraction of the moon disk in `[0, 1]` (0 new, 1 full).
pub fn moon_illumination(tick: u64, cfg: &ClimateConfig) -> f32 {
    let phase = lunar_fraction_cfg(tick, cfg);
    0.5 * (1.0 - (phase * std::f32::consts::TAU).cos())
}

/// Apparent sun scale (~1 at mean distance). Larger near winter perihelion.
pub fn sun_apparent_scale(tick: u64, cfg: &ClimateConfig) -> f32 {
    let season = season_fraction_cfg(tick, cfg);
    // Earth perihelion ≈ early January → winter solstice (season 0).
    let wobble = (season * std::f32::consts::TAU).cos();
    (1.0 + cfg.sun_distance_amp.clamp(0.0, 0.6) * wobble).max(0.35)
}

/// Apparent moon scale (~1 at mean distance).
pub fn moon_apparent_scale(tick: u64, cfg: &ClimateConfig) -> f32 {
    let phase = lunar_fraction_cfg(tick, cfg);
    // Anomalistic wobble phase-shifted from synodic so size ≠ phase lock.
    let anom = phase * std::f32::consts::TAU + 1.7;
    let wobble = anom.cos();
    (1.0 + cfg.moon_distance_amp.clamp(0.0, 0.8) * wobble).max(0.35)
}

/// Equinox day/night lengths adjusted for the current season.
pub fn effective_day_night_ticks(tick: u64, cfg: &ClimateConfig) -> (u64, u64) {
    let total = cfg.total_ticks().max(2);
    let base_day_frac = cfg.day_ticks.max(1) as f32 / total as f32;
    let season = season_fraction_cfg(tick, cfg);
    // Winter (0): shorter day; summer (0.5): longer day.
    let cos_s = -(season * std::f32::consts::TAU).cos();
    let amp = cfg.season_day_length_amp.clamp(0.0, 0.6);
    let day_frac = (base_day_frac + amp * 0.5 * cos_s).clamp(0.12, 0.88);
    let day = (day_frac * total as f32).round().max(1.0) as u64;
    let night = total.saturating_sub(day).max(1);
    (day, night)
}

/// Phase in `[0, 1)` — 0 ≈ noon, progresses through dusk, midnight, dawn.
pub fn phase_fraction(tick: u64) -> f32 {
    phase_fraction_cfg(tick, &ClimateConfig::default())
}

pub fn phase_fraction_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let total = cfg.total_ticks();
    (tick % total) as f32 / total as f32
}

/// Raised cosine for organism light / upkeep: ~1 at noon, floor 0.08 at night.
pub fn day_factor(tick: u64) -> f32 {
    day_factor_cfg(tick, &ClimateConfig::default())
}

pub fn day_factor_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let dn = day_night_factor_cfg(tick, cfg);
    ((dn + 1.0) * 0.5).clamp(0.08, 1.0)
}

/// Signed day/night drive: +1 noon, −1 midnight, 0 at dawn/dusk.
pub fn day_night_factor(tick: u64) -> f32 {
    day_night_factor_cfg(tick, &ClimateConfig::default())
}

pub fn day_night_factor_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let phase = phase_fraction_cfg(tick, cfg);
    let (day_ticks, _night) = effective_day_night_ticks(tick, cfg);
    let day_frac = day_ticks as f32 / cfg.total_ticks() as f32;
    let half_day = (day_frac * 0.5).clamp(1e-4, 0.499);
    if phase <= half_day {
        // Noon → dusk: +1 → 0
        let t = phase / half_day;
        (t * std::f32::consts::FRAC_PI_2).cos()
    } else if phase >= 1.0 - half_day {
        // Dawn → noon: 0 → +1
        let t = (phase - (1.0 - half_day)) / half_day;
        (t * std::f32::consts::FRAC_PI_2).sin()
    } else {
        // Dusk → midnight → dawn: 0 → −1 → 0
        let u = (phase - half_day) / (1.0 - 2.0 * half_day).max(1e-4);
        -((u * std::f32::consts::PI).sin())
    }
}

pub fn is_daytime(tick: u64) -> bool {
    is_daytime_cfg(tick, &ClimateConfig::default())
}

pub fn is_daytime_cfg(tick: u64, cfg: &ClimateConfig) -> bool {
    // Treat exact dusk/dawn (factor ≈ 0) as day so the sun arc completes.
    day_night_factor_cfg(tick, cfg) >= -1e-3
}

/// Progress 0→1 along the current body's sky arc (rise→set).
pub fn celestial_local(tick: u64) -> f32 {
    celestial_local_cfg(tick, &ClimateConfig::default())
}

pub fn celestial_local_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let phase = phase_fraction_cfg(tick, cfg);
    let (day_ticks, _) = effective_day_night_ticks(tick, cfg);
    let day_frac = day_ticks as f32 / cfg.total_ticks() as f32;
    let half_day = (day_frac * 0.5).clamp(1e-4, 0.499);
    if is_daytime_cfg(tick, cfg) {
        if phase >= 1.0 - half_day {
            // dawn → noon: 0 → 0.5
            let t = (phase - (1.0 - half_day)) / half_day;
            (t * 0.5).clamp(0.0, 0.5)
        } else {
            // noon → dusk: 0.5 → 1
            let t = phase / half_day;
            (0.5 + t * 0.5).clamp(0.5, 1.0)
        }
    } else {
        let night_len = (1.0 - 2.0 * half_day).max(1e-4);
        let u = ((phase - half_day) / night_len).clamp(0.0, 1.0);
        u
    }
}

/// Screen-space arc for the active celestial body.
pub fn celestial_screen_pos(tick: u64, sw: f32, sh: f32) -> (f32, f32) {
    celestial_screen_pos_cfg(tick, sw, sh, &ClimateConfig::default())
}

pub fn celestial_screen_pos_cfg(tick: u64, sw: f32, sh: f32, cfg: &ClimateConfig) -> (f32, f32) {
    let local = celestial_local_cfg(tick, cfg);
    // Summer: slightly higher arc; winter: lower (axial tilt feel).
    let season = season_fraction_cfg(tick, cfg);
    let elev = 1.0 + 0.18 * -(season * std::f32::consts::TAU).cos();
    let x = 0.08 * sw + local * 0.84 * sw;
    let y = 0.08 * sh + 0.28 * sh * elev * (1.0 - (local * std::f32::consts::PI).sin());
    (x, y)
}

/// Sky RGB for a day/night factor in `[-1, 1]` (equinox palette).
pub fn sky_rgb(day_night: f32) -> [u8; 3] {
    sky_rgb_cfg(day_night, 0.35, 0.0)
}

/// Seasonal sky sample. `season` in `[0,1)`, `tint` in `[0,1]`.
pub fn sky_rgb_cfg(day_night: f32, season: f32, tint: f32) -> [u8; 3] {
    let day = [0x87u8, 0xCE, 0xEB];
    let dusk = [0xC4u8, 0x6A, 0x3A];
    let night_edge = [0x1Au8, 0x22, 0x44];
    let night = [0x06u8, 0x08, 0x18];
    let t = day_night.clamp(-1.0, 1.0);
    let mut base = if t >= 0.2 {
        let u = ((t - 0.2) / 0.8).clamp(0.0, 1.0);
        lerp_rgb(dusk, day, 0.35 + 0.65 * u)
    } else if t >= -0.15 {
        let u = ((t + 0.15) / 0.35).clamp(0.0, 1.0);
        lerp_rgb(dusk, day, u)
    } else {
        let u = ((-t - 0.15) / 0.85).clamp(0.0, 1.0);
        lerp_rgb(night_edge, night, u)
    };

    let strength = tint.clamp(0.0, 1.0);
    if strength > 1e-3 {
        let s = season.rem_euclid(1.0);
        // Seasonal wash: winter cool, spring soft mint, summer warm,
        // autumn amber — applied gently so dusk oranges still read.
        let wash = if s < 0.125 || s >= 0.875 {
            [0x70u8, 0x90, 0xC8] // winter
        } else if s < 0.375 {
            [0x9Au8, 0xD4, 0xB8] // spring
        } else if s < 0.625 {
            [0xF0u8, 0xD0, 0x88] // summer
        } else {
            [0xE0u8, 0x96, 0x58] // autumn
        };
        let wash_amt = strength
            * if t >= 0.0 {
                0.18 + 0.10 * t
            } else {
                0.10 + 0.08 * (-t)
            };
        base = lerp_rgb(base, wash, wash_amt);
    }
    base
}

/// Vertical sky sample: `height_01` = 0 at zenith (top), 1 at horizon.
pub fn sky_rgb_at_height(day_night: f32, height_01: f32) -> [u8; 3] {
    sky_rgb_at_height_cfg(day_night, height_01, 0.35, 0.0)
}

pub fn sky_rgb_at_height_cfg(
    day_night: f32,
    height_01: f32,
    season: f32,
    tint: f32,
) -> [u8; 3] {
    let base = sky_rgb_cfg(day_night, season, tint);
    let h = height_01.clamp(0.0, 1.0);
    // Horizon haze: warmer / brighter near the ground line at dusk,
    // deeper indigo at zenith at night.
    let zenith_darken = if day_night >= 0.0 {
        1.0 - 0.10 * (1.0 - h)
    } else {
        1.0 - 0.28 * (1.0 - h)
    };
    let horizon_lift = if day_night.abs() < 0.45 {
        // Dawn / dusk glow band near the horizon.
        let glow = (1.0 - (day_night.abs() / 0.45)).clamp(0.0, 1.0);
        1.0 + 0.12 * glow * h
    } else {
        1.0
    };
    let scale = zenith_darken * horizon_lift;
    [
        (base[0] as f32 * scale).clamp(0.0, 255.0) as u8,
        (base[1] as f32 * scale).clamp(0.0, 255.0) as u8,
        (base[2] as f32 * scale).clamp(0.0, 255.0) as u8,
    ]
}

fn lerp_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_midnight_is_dim() {
        let cfg = ClimateConfig::default();
        let noon = day_factor_cfg(0, &cfg);
        let midnight = day_factor_cfg(cfg.total_ticks() / 2, &cfg);
        assert!(noon > 0.9, "noon={noon}");
        assert!(midnight < 0.25, "midnight={midnight}");
        assert!(day_night_factor_cfg(0, &cfg) > 0.9);
        assert!(day_night_factor_cfg(cfg.total_ticks() / 2, &cfg) < -0.9);
    }

    #[test]
    fn longer_night_keeps_late_phase_dark() {
        let short_night = ClimateConfig {
            day_ticks: 800,
            night_ticks: 200,
            season_day_length_amp: 0.0,
            ..ClimateConfig::default()
        };
        let long_night = ClimateConfig {
            day_ticks: 200,
            night_ticks: 800,
            season_day_length_amp: 0.0,
            ..ClimateConfig::default()
        };
        // 80% through the cycle: short-night world is in dawn/day;
        // long-night world is still deep night.
        let t_short = (short_night.total_ticks() as f32 * 0.8) as u64;
        let t_long = (long_night.total_ticks() as f32 * 0.8) as u64;
        let late_short = day_night_factor_cfg(t_short, &short_night);
        let late_long = day_night_factor_cfg(t_long, &long_night);
        assert!(
            late_long < late_short,
            "short={late_short} long={late_long}"
        );
        assert!(late_long < 0.0);
        assert!(late_short > 0.0);
    }

    #[test]
    fn celestial_local_covers_arc() {
        let cfg = ClimateConfig {
            season_day_length_amp: 0.0,
            ..ClimateConfig::default()
        };
        let dusk_tick = cfg.day_ticks / 2;
        let dusk = celestial_local_cfg(dusk_tick, &cfg);
        let noon = celestial_local_cfg(0, &cfg);
        assert!(dusk > 0.9, "dusk={dusk}");
        assert!((noon - 0.5).abs() < 0.05, "noon local={noon}");
    }

    #[test]
    fn midnight_sky_is_darker_than_noon() {
        let day = sky_rgb(1.0);
        let night = sky_rgb(-1.0);
        let day_luma = day[0] as u32 + day[1] as u32 + day[2] as u32;
        let night_luma = night[0] as u32 + night[1] as u32 + night[2] as u32;
        assert!(night_luma < day_luma / 2, "day={day:?} night={night:?}");
    }

    #[test]
    fn summer_day_longer_than_winter() {
        let mut cfg = ClimateConfig::default();
        cfg.season_lock = true;
        cfg.season_day_length_amp = 0.35;
        cfg.season_offset = 0.0; // winter
        let (dw, _) = effective_day_night_ticks(0, &cfg);
        cfg.season_offset = 0.5; // summer
        let (ds, _) = effective_day_night_ticks(0, &cfg);
        assert!(ds > dw, "summer day={ds} winter day={dw}");
    }

    #[test]
    fn full_moon_brighter_than_new() {
        let mut cfg = ClimateConfig::default();
        cfg.lunar_lock = true;
        cfg.lunar_offset = 0.0;
        let new_i = moon_illumination(0, &cfg);
        cfg.lunar_offset = 0.5;
        let full_i = moon_illumination(0, &cfg);
        assert!(new_i < 0.05, "new={new_i}");
        assert!(full_i > 0.95, "full={full_i}");
    }

    #[test]
    fn moon_apparent_scale_varies_with_distance_amp() {
        let mut cfg = ClimateConfig::default();
        cfg.lunar_lock = true;
        cfg.moon_distance_amp = 0.3;
        cfg.lunar_offset = 0.0;
        let a = moon_apparent_scale(0, &cfg);
        cfg.lunar_offset = 0.5;
        let b = moon_apparent_scale(0, &cfg);
        assert!((a - b).abs() > 0.05, "a={a} b={b}");
    }

    #[test]
    fn sun_bigger_near_winter_perihelion() {
        let mut cfg = ClimateConfig::default();
        cfg.season_lock = true;
        cfg.sun_distance_amp = 0.2;
        cfg.season_offset = 0.0;
        let winter = sun_apparent_scale(0, &cfg);
        cfg.season_offset = 0.5;
        let summer = sun_apparent_scale(0, &cfg);
        assert!(winter > summer, "winter={winter} summer={summer}");
    }

    #[test]
    fn season_name_bands() {
        assert_eq!(season_name(0.0), "Winter");
        assert_eq!(season_name(0.3), "Spring");
        assert_eq!(season_name(0.5), "Summer");
        assert_eq!(season_name(0.8), "Autumn");
    }
}
