//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Shared day/night clock for light, temperature, and sky drawing.
//!
//! Phase convention matches the original Set A `day_factor`:
//! **tick 0 ≈ noon**. Day and night lengths are independently tunable.

use serde::{Deserialize, Serialize};

/// Default full cycle length (day + night) in ticks.
pub const DEMO_DAY_TICKS: u64 = 1_200;

/// Tunable day / night durations.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct ClimateConfig {
    /// Daylight duration in ticks (noon→dusk + dawn→noon).
    pub day_ticks: u64,
    /// Night duration in ticks (dusk→dawn).
    pub night_ticks: u64,
}

impl Default for ClimateConfig {
    fn default() -> Self {
        Self {
            day_ticks: DEMO_DAY_TICKS / 2,
            night_ticks: DEMO_DAY_TICKS / 2,
        }
    }
}

impl ClimateConfig {
    pub fn total_ticks(self) -> u64 {
        self.day_ticks.max(1) + self.night_ticks.max(1)
    }
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
    let day_frac = cfg.day_ticks.max(1) as f32 / cfg.total_ticks() as f32;
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
    if is_daytime_cfg(tick, cfg) {
        celestial_sun_local_cfg(tick, cfg)
    } else {
        celestial_moon_local_cfg(tick, cfg)
    }
}

/// Sun arc local 0→1 (dawn→dusk). Outside daytime clamps to the nearest horizon.
pub fn celestial_sun_local_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let phase = phase_fraction_cfg(tick, cfg);
    let day_frac = cfg.day_ticks.max(1) as f32 / cfg.total_ticks() as f32;
    let half_day = (day_frac * 0.5).clamp(1e-4, 0.499);
    if phase >= 1.0 - half_day {
        // dawn → noon: 0 → 0.5
        let t = (phase - (1.0 - half_day)) / half_day;
        (t * 0.5).clamp(0.0, 0.5)
    } else if phase <= half_day {
        // noon → dusk: 0.5 → 1
        let t = phase / half_day;
        (0.5 + t * 0.5).clamp(0.5, 1.0)
    } else {
        // Night — sun stays set on the dusk side.
        1.0
    }
}

/// Moon arc local 0→1 (dusk→dawn). Outside nighttime clamps to horizons.
pub fn celestial_moon_local_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let phase = phase_fraction_cfg(tick, cfg);
    let day_frac = cfg.day_ticks.max(1) as f32 / cfg.total_ticks() as f32;
    let half_day = (day_frac * 0.5).clamp(1e-4, 0.499);
    if phase > half_day && phase < 1.0 - half_day {
        let night_len = (1.0 - 2.0 * half_day).max(1e-4);
        ((phase - half_day) / night_len).clamp(0.0, 1.0)
    } else if phase <= half_day {
        // Still day afternoon — moon not up yet (pre-rise).
        0.0
    } else {
        // Past dawn — moon has set.
        1.0
    }
}

fn celestial_pos_from_local(local: f32, sw: f32, sh: f32) -> (f32, f32) {
    let local = local.clamp(0.0, 1.0);
    let x = 0.06 * sw + local * 0.88 * sw;
    // Apex ~8% from top; rise/set ~72% down the frame (horizon band).
    let y = 0.08 * sh + 0.64 * sh * (1.0 - (local * std::f32::consts::PI).sin());
    (x, y)
}

/// Screen-space arc for the active celestial body.
///
/// Rise/set sit low (near the horizon band) so the body can set behind
/// ridges instead of vanishing mid-sky when day/night flips.
pub fn celestial_screen_pos(tick: u64, sw: f32, sh: f32) -> (f32, f32) {
    celestial_screen_pos_cfg(tick, sw, sh, &ClimateConfig::default())
}

pub fn celestial_screen_pos_cfg(tick: u64, sw: f32, sh: f32, cfg: &ClimateConfig) -> (f32, f32) {
    celestial_pos_from_local(celestial_local_cfg(tick, cfg), sw, sh)
}

pub fn celestial_sun_screen_pos_cfg(
    tick: u64,
    sw: f32,
    sh: f32,
    cfg: &ClimateConfig,
) -> (f32, f32) {
    celestial_pos_from_local(celestial_sun_local_cfg(tick, cfg), sw, sh)
}

pub fn celestial_moon_screen_pos_cfg(
    tick: u64,
    sw: f32,
    sh: f32,
    cfg: &ClimateConfig,
) -> (f32, f32) {
    celestial_pos_from_local(celestial_moon_local_cfg(tick, cfg), sw, sh)
}

fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Sky RGB for a day/night factor in `[-1, 1]`.
///
/// Smooth shoulders: night → dusk/dawn → morning/evening → day.
/// Day is a soft pale blue (not saturated “underwater” cyan).
pub fn sky_rgb(day_night: f32) -> [u8; 3] {
    let day = [0xA6u8, 0xC2, 0xD0]; // pale daylight
    let morning = [0xB4u8, 0xC0, 0xC4]; // soft morning grey-blue
    let dusk = [0xC4u8, 0x6A, 0x3A];
    let evening = [0xD8u8, 0x8E, 0x58];
    let night_edge = [0x1Au8, 0x22, 0x44];
    let night = [0x06u8, 0x08, 0x18];
    let t = day_night.clamp(-1.0, 1.0);
    if t >= 0.55 {
        // Full day ← morning
        let u = smooth01((t - 0.55) / 0.45);
        lerp_rgb(morning, day, u)
    } else if t >= 0.12 {
        // Morning/evening shoulder ← dusk warmth
        let u = smooth01((t - 0.12) / 0.43);
        lerp_rgb(evening, morning, u)
    } else if t >= -0.12 {
        // Dawn / dusk core
        let u = smooth01((t + 0.12) / 0.24);
        lerp_rgb(dusk, evening, u)
    } else if t >= -0.45 {
        // Dusk → night edge
        let u = smooth01((-t - 0.12) / 0.33);
        lerp_rgb(dusk, night_edge, u)
    } else {
        // Deep night
        let u = smooth01((-t - 0.45) / 0.55);
        lerp_rgb(night_edge, night, u)
    }
}

/// Vertical sky sample: `height_01` = 0 at zenith (top), 1 at horizon.
pub fn sky_rgb_at_height(day_night: f32, height_01: f32) -> [u8; 3] {
    let base = sky_rgb(day_night);
    let h = height_01.clamp(0.0, 1.0);
    let zenith_darken = if day_night >= 0.0 {
        1.0 - 0.08 * (1.0 - h)
    } else {
        1.0 - 0.22 * (1.0 - h)
    };
    [
        (base[0] as f32 * zenith_darken) as u8,
        (base[1] as f32 * zenith_darken) as u8,
        (base[2] as f32 * zenith_darken) as u8,
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
        };
        let long_night = ClimateConfig {
            day_ticks: 200,
            night_ticks: 800,
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
        let cfg = ClimateConfig::default();
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
}
