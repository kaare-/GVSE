//! Climate: biomes, elevation/day-night driven temperature.
//!
//! Temperature is deliberately *not* stored per column — it's a pure
//! function of elevation, biome, and the current tick, computed on demand.
//! That keeps it always consistent with the live climate settings (no stale
//! cached values) and adds nothing to the save format per column.

use serde::{Deserialize, Serialize};

/// A coarse climate zone used only as a stand-in for the latitude-driven
/// warmth variation a real planet would have (this world has no north/south
/// — it's a single east-west cross-section), *in addition to* elevation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Biome {
    Ocean,
    Shelf,
    Coast,
    Plains,
    Mountain,
}

impl Biome {
    /// Degrees C added on top of the base temperature, before elevation
    /// cooling and day/night are applied.
    pub fn heat_bias(self) -> f32 {
        match self {
            Biome::Ocean => 2.0,
            Biome::Shelf => 1.0,
            Biome::Coast => 3.0,
            Biome::Plains => 0.0,
            Biome::Mountain => 0.0,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Biome::Ocean => "ocean",
            Biome::Shelf => "shelf",
            Biome::Coast => "coast",
            Biome::Plains => "plains",
            Biome::Mountain => "mountain",
        }
    }
}

/// Classify purely by elevation relative to sea level — simple and doesn't
/// duplicate the terrain generator's macro-zone logic.
pub fn biome_for(surface_y: f32, sea_level: f32) -> Biome {
    let rel = surface_y - sea_level;
    if rel < -18.0 {
        Biome::Ocean
    } else if rel < 0.0 {
        Biome::Shelf
    } else if rel < 6.0 {
        Biome::Coast
    } else if rel > 40.0 {
        Biome::Mountain
    } else {
        Biome::Plains
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClimateSettings {
    pub day_length_ticks: u64,
    pub night_length_ticks: u64,
    pub base_temp_c: f32,
    /// Degrees C colder per metre above sea level. Real Earth's ~6.5C/1000m
    /// would be invisible on this world's ~85m peaks, so this is
    /// deliberately exaggerated by default — tune it in the settings panel.
    pub lapse_rate_c_per_m: f32,
    pub day_night_amplitude_c: f32,
    pub freeze_point_c: f32,
    /// Columns per tick that weather (clouds) drift; sign gives direction.
    pub wind_speed: f32,
}

impl Default for ClimateSettings {
    fn default() -> Self {
        Self {
            day_length_ticks: 12 * 60 * 60,
            night_length_ticks: 10 * 60 * 60,
            base_temp_c: 20.0,
            lapse_rate_c_per_m: 0.15,
            day_night_amplitude_c: 8.0,
            freeze_point_c: 0.0,
            // Fast enough to cross a ~3300-column map in a few minutes of
            // play at 1x speed, so clouds actually reach interesting terrain
            // (and rain/snow on it) within a normal session instead of
            // drifting for the better part of an hour.
            wind_speed: 0.25,
        }
    }
}

impl ClimateSettings {
    pub fn cycle_length_ticks(&self) -> u64 {
        (self.day_length_ticks + self.night_length_ticks).max(1)
    }

    pub fn is_daytime(&self, tick: u64) -> bool {
        (tick % self.cycle_length_ticks()) < self.day_length_ticks
    }

    /// 0.0 at the start of day, 1.0 at the start of the next day (for a
    /// clock-face style HUD readout).
    pub fn phase_fraction(&self, tick: u64) -> f32 {
        (tick % self.cycle_length_ticks()) as f32 / self.cycle_length_ticks() as f32
    }

    /// In [-1, 1]: 0 at sunrise/sunset, +1 at solar noon, -1 at the depth of
    /// night. Half-cosine easing so it doesn't jump abruptly at the day/
    /// night boundary, and day/night can have different lengths.
    pub fn day_night_factor(&self, tick: u64) -> f32 {
        let t = tick % self.cycle_length_ticks();
        if t < self.day_length_ticks {
            let day_len = self.day_length_ticks.max(1) as f32;
            let phase = t as f32 / day_len;
            (phase * std::f32::consts::PI).sin()
        } else {
            let night_len = self.night_length_ticks.max(1) as f32;
            let phase = (t - self.day_length_ticks) as f32 / night_len;
            -(phase * std::f32::consts::PI).sin()
        }
    }
}

pub fn temperature_at(surface_y: f32, sea_level: f32, tick: u64, settings: &ClimateSettings) -> f32 {
    let biome = biome_for(surface_y, sea_level);
    let elevation_above_sea = (surface_y - sea_level).max(0.0);
    settings.base_temp_c + biome.heat_bias() - settings.lapse_rate_c_per_m * elevation_above_sea
        + settings.day_night_amplitude_c * settings.day_night_factor(tick)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn higher_elevation_is_colder() {
        let settings = ClimateSettings::default();
        let low = temperature_at(12.0, 12.0, 0, &settings);
        let high = temperature_at(80.0, 12.0, 0, &settings);
        assert!(high < low, "mountain ({high}) should be colder than sea level ({low})");
    }

    #[test]
    fn day_is_warmer_than_night_at_same_elevation() {
        let settings = ClimateSettings::default();
        let noon_tick = settings.day_length_ticks / 2;
        let midnight_tick = settings.day_length_ticks + settings.night_length_ticks / 2;
        let noon = temperature_at(12.0, 12.0, noon_tick, &settings);
        let midnight = temperature_at(12.0, 12.0, midnight_tick, &settings);
        assert!(noon > midnight);
    }

    #[test]
    fn day_night_factor_bounded() {
        let settings = ClimateSettings::default();
        for tick in (0..settings.cycle_length_ticks()).step_by(37) {
            let f = settings.day_night_factor(tick);
            assert!((-1.0..=1.0).contains(&f), "factor {f} out of range at tick {tick}");
        }
    }
}
