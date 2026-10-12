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

/// Climate days per lunar cycle (new → full → new).
pub const MOON_CYCLE_DAYS: u64 = 8;

/// Climate days per year cycle (spring → summer → autumn → winter).
pub const SEASON_CYCLE_DAYS: u64 = 32;

/// Lunar phase in `[0, 1)`: 0 ≈ new moon, 0.5 ≈ full, wraps over
/// [`MOON_CYCLE_DAYS`] climate days.
pub fn moon_phase_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let period = cfg.total_ticks().saturating_mul(MOON_CYCLE_DAYS).max(1);
    (tick % period) as f32 / period as f32
}

/// Illumination 0 (new) → 1 (full) from [`moon_phase_cfg`].
pub fn moon_illumination(phase: f32) -> f32 {
    // Cosine: new at 0/1, full at 0.5.
    let ang = phase.clamp(0.0, 1.0) * std::f32::consts::TAU;
    ((1.0 - ang.cos()) * 0.5).clamp(0.0, 1.0)
}

/// Season in `[0, 1)`: 0 ≈ spring, 0.25 ≈ summer, 0.5 ≈ autumn, 0.75 ≈ winter.
pub fn season_fraction_cfg(tick: u64, cfg: &ClimateConfig) -> f32 {
    let period = cfg.total_ticks().saturating_mul(SEASON_CYCLE_DAYS).max(1);
    (tick % period) as f32 / period as f32
}

/// Remap base day/night lengths for a season (same total ticks).
///
/// Summer (~0.25) lengthens day; winter (~0.75) lengthens night. Intended for
/// **sky / celestial draw** — keep the base config for sim/organism clocks.
pub fn climate_with_season(base: &ClimateConfig, season: f32) -> ClimateConfig {
    let total = base.total_ticks().max(2);
    // Peak summer day at season 0.25: cos(0) = 1 → day_frac high.
    let ang = (season.clamp(0.0, 1.0) - 0.25) * std::f32::consts::TAU;
    let day_frac = (0.50 + 0.16 * ang.cos()).clamp(0.32, 0.68);
    let day = ((total as f32) * day_frac).round().clamp(1.0, (total - 1) as f32) as u64;
    ClimateConfig {
        day_ticks: day,
        night_ticks: total.saturating_sub(day).max(1),
    }
}

/// Soft seasonal wash on a sky RGB sample (Phase 7 C).
///
/// Strength scales with daylight so night stays deep; winter still cools a
/// little after dusk for a longer blue hour.
pub fn apply_season_sky_tint(rgb: [u8; 3], season: f32, day_night: f32) -> [u8; 3] {
    let s = season.clamp(0.0, 1.0);
    // Blend weights for spring / summer / autumn / winter centers.
    let spring = smooth01(1.0 - ((s - 0.0).abs().min((s - 1.0).abs()) / 0.28));
    let summer = smooth01(1.0 - ((s - 0.25).abs() / 0.28));
    let autumn = smooth01(1.0 - ((s - 0.50).abs() / 0.28));
    let winter = smooth01(1.0 - ((s - 0.75).abs() / 0.28));
    let sum = (spring + summer + autumn + winter).max(1e-3);
    let (spring, summer, autumn, winter) =
        (spring / sum, summer / sum, autumn / sum, winter / sum);

    let day = ((day_night + 1.0) * 0.5).clamp(0.0, 1.0);
    // Night keeps most of the palette; winter cools a bit more.
    let mix = (0.10 + 0.22 * day + 0.08 * winter * (1.0 - day)).clamp(0.0, 0.40);

    let tint = [
        (spring * 210.0 + summer * 255.0 + autumn * 235.0 + winter * 150.0) as u8,
        (spring * 225.0 + summer * 235.0 + autumn * 175.0 + winter * 175.0) as u8,
        (spring * 200.0 + summer * 200.0 + autumn * 130.0 + winter * 220.0) as u8,
    ];
    [
        (rgb[0] as f32 * (1.0 - mix) + tint[0] as f32 * mix) as u8,
        (rgb[1] as f32 * (1.0 - mix) + tint[1] as f32 * mix) as u8,
        (rgb[2] as f32 * (1.0 - mix) + tint[2] as f32 * mix) as u8,
    ]
}

/// Sky RGB for a day/night factor in `[-1, 1]`.
///
/// Smooth shoulders: night → dusk/dawn → morning/evening → day.
/// Day is a soft pale blue (not saturated “underwater” cyan).
/// Phase 7: richer dusk/dawn core stops.
pub fn sky_rgb(day_night: f32) -> [u8; 3] {
    sky_rgb_phased(day_night, 0.0)
}

/// Like [`sky_rgb`], with `phase` in `[0, 1)` (noon→dusk→midnight→dawn)
/// so dusk leans amber/red and dawn leans rose.
pub fn sky_rgb_phased(day_night: f32, phase: f32) -> [u8; 3] {
    let day = [0xA6u8, 0xC2, 0xD0]; // pale daylight
    let morning = [0xB8u8, 0xC4, 0xC8]; // soft morning grey-blue
    let dusk = [0xE0u8, 0x6A, 0x32]; // richer sunset orange-red
    let evening = [0xE8u8, 0x9A, 0x58];
    let dawn_core = [0xD8u8, 0x7A, 0x7A]; // rose dawn
    let night_edge = [0x1Au8, 0x22, 0x44];
    let night = [0x06u8, 0x08, 0x18];
    let t = day_night.clamp(-1.0, 1.0);
    let mut rgb = if t >= 0.55 {
        let u = smooth01((t - 0.55) / 0.45);
        lerp_rgb(morning, day, u)
    } else if t >= 0.12 {
        let u = smooth01((t - 0.12) / 0.43);
        lerp_rgb(evening, morning, u)
    } else if t >= -0.12 {
        // Dawn / dusk core — pick rose vs amber from climate phase.
        let u = smooth01((t + 0.12) / 0.24);
        let dusk_side = smooth01(1.0 - ((phase - 0.25).abs() / 0.22));
        let dawn_side = smooth01(1.0 - ((phase - 0.75).abs() / 0.22));
        let core = if dawn_side > dusk_side {
            lerp_rgb(dawn_core, evening, 0.35)
        } else {
            dusk
        };
        lerp_rgb(core, evening, u)
    } else if t >= -0.45 {
        let u = smooth01((-t - 0.12) / 0.33);
        lerp_rgb(dusk, night_edge, u)
    } else {
        let u = smooth01((-t - 0.45) / 0.55);
        lerp_rgb(night_edge, night, u)
    };
    // Extra dawn rose / dusk ember when near twilight.
    let twilight = ((0.22 - t.abs()) / 0.22).clamp(0.0, 1.0);
    if twilight > 0.01 {
        let dusk_side = smooth01(1.0 - ((phase - 0.25).abs() / 0.22));
        let dawn_side = smooth01(1.0 - ((phase - 0.75).abs() / 0.22));
        let w = twilight * twilight;
        if dawn_side > dusk_side {
            rgb[0] = rgb[0].saturating_add((16.0 * w * dawn_side) as u8);
            rgb[2] = rgb[2].saturating_add((18.0 * w * dawn_side) as u8);
        } else if dusk_side > 0.05 {
            rgb[0] = rgb[0].saturating_add((20.0 * w * dusk_side) as u8);
            rgb[2] = (rgb[2] as f32 * (1.0 - 0.14 * w * dusk_side)) as u8;
        }
    }
    rgb
}

/// Vertical sky sample: `height_01` = 0 at zenith (top), 1 at horizon.
pub fn sky_rgb_at_height(day_night: f32, height_01: f32) -> [u8; 3] {
    sky_rgb_at_height_phased(day_night, height_01, 0.0)
}

/// Vertical sky sample with climate `phase` for dawn/dusk split.
pub fn sky_rgb_at_height_phased(day_night: f32, height_01: f32, phase: f32) -> [u8; 3] {
    let base = sky_rgb_phased(day_night, phase);
    let h = height_01.clamp(0.0, 1.0);
    let zenith_darken = if day_night >= 0.0 {
        1.0 - 0.08 * (1.0 - h)
    } else {
        1.0 - 0.22 * (1.0 - h)
    };
    // Warm horizon band near twilight (Phase 7).
    let twilight = ((0.28 - day_night.abs()) / 0.28).clamp(0.0, 1.0);
    let horizon_warm = twilight * twilight * h * h;
    let mut rgb = [
        (base[0] as f32 * zenith_darken) as u8,
        (base[1] as f32 * zenith_darken) as u8,
        (base[2] as f32 * zenith_darken) as u8,
    ];
    if horizon_warm > 0.01 {
        rgb[0] = rgb[0].saturating_add((28.0 * horizon_warm) as u8);
        rgb[1] = rgb[1].saturating_add((12.0 * horizon_warm) as u8);
        rgb[2] = (rgb[2] as f32 * (1.0 - 0.10 * horizon_warm)) as u8;
    }
    rgb
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

    #[test]
    fn moon_phase_cycles_and_illumination_peaks_at_full() {
        let cfg = ClimateConfig::default();
        let period = cfg.total_ticks() * MOON_CYCLE_DAYS;
        let new_p = moon_phase_cfg(0, &cfg);
        let full_p = moon_phase_cfg(period / 2, &cfg);
        assert!(new_p < 0.05, "new={new_p}");
        assert!((full_p - 0.5).abs() < 0.05, "full={full_p}");
        assert!(moon_illumination(0.0) < 0.05);
        assert!(moon_illumination(0.5) > 0.95);
        assert!(moon_illumination(0.25) > 0.4 && moon_illumination(0.25) < 0.6);
    }

    #[test]
    fn dusk_sky_is_warmer_than_dawn_sky() {
        // phase 0.25 ≈ dusk, 0.75 ≈ dawn; same day_night near twilight.
        let dusk = sky_rgb_phased(0.0, 0.25);
        let dawn = sky_rgb_phased(0.0, 0.75);
        // Dusk leans amber (higher R relative to B); dawn leans rose (more B).
        let dusk_warm = dusk[0] as i32 - dusk[2] as i32;
        let dawn_warm = dawn[0] as i32 - dawn[2] as i32;
        assert!(
            dusk_warm > dawn_warm,
            "dusk={dusk:?} dawn={dawn:?} warm_d={dusk_warm} warm_a={dawn_warm}"
        );
    }

    #[test]
    fn horizon_warms_at_twilight() {
        let zenith = sky_rgb_at_height_phased(0.0, 0.0, 0.25);
        let horizon = sky_rgb_at_height_phased(0.0, 1.0, 0.25);
        assert!(
            horizon[0] >= zenith[0],
            "zenith={zenith:?} horizon={horizon:?}"
        );
    }

    #[test]
    fn summer_lengthens_day_winter_lengthens_night() {
        let base = ClimateConfig::default();
        let summer = climate_with_season(&base, 0.25);
        let winter = climate_with_season(&base, 0.75);
        assert_eq!(summer.total_ticks(), base.total_ticks());
        assert_eq!(winter.total_ticks(), base.total_ticks());
        assert!(
            summer.day_ticks > winter.day_ticks,
            "summer_day={} winter_day={}",
            summer.day_ticks,
            winter.day_ticks
        );
        assert!(winter.night_ticks > summer.night_ticks);
    }

    #[test]
    fn winter_sky_tint_is_cooler_than_summer() {
        let base = [160u8, 180, 200];
        let summer = apply_season_sky_tint(base, 0.25, 0.8);
        let winter = apply_season_sky_tint(base, 0.75, 0.8);
        let summer_warm = summer[0] as i32 - summer[2] as i32;
        let winter_warm = winter[0] as i32 - winter[2] as i32;
        assert!(
            summer_warm > winter_warm,
            "summer={summer:?} winter={winter:?}"
        );
    }
}
