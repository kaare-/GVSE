//! Weather: simplified drifting clouds that carry rain, driven by wind.
//!
//! Clouds are not simulated as particles — each is just a position, width,
//! and remaining moisture. They drift at a constant wind speed, rain on
//! whatever's underneath while they have moisture left, and despawn once
//! they're spent or drift off the loaded map.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Cloud {
    /// World-x of the cloud's centre (fractional — wind drift is sub-column
    /// per tick at realistic speeds, so this needs to accumulate smoothly).
    pub x: f32,
    /// Half-width in columns.
    pub half_width: f32,
    /// Remaining rain potential; depletes while actively raining, despawns
    /// at zero.
    pub moisture: f32,
    /// True while this cloud is in an active precipitation burst.
    /// Drives both hydrology and the rain-streak animation.
    #[serde(default)]
    pub raining: bool,
    /// Ticks left in the current rain burst (`0` when idle).
    #[serde(default)]
    pub rain_ticks_left: u16,
}

impl Cloud {
    pub fn covers(&self, world_x: i32) -> bool {
        let dx = world_x as f32 - self.x;
        dx.abs() <= self.half_width
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WeatherSettings {
    /// Automatic cloud-driven rain, independent of the manual rain override.
    pub weather_enabled: bool,
    pub cloud_spawn_interval_ticks: u64,
    pub cloud_rain_rate: f32,
    pub max_clouds: usize,
    /// Probability, per tick, that an idle cloud *starts* a rain burst.
    /// Bursts then rain continuously for `rain_burst_*` ticks so weather
    /// systems read as fronts rather than single-tick sprinkles.
    pub rain_chance_per_tick: f32,
    /// Minimum length of a precipitation burst (ticks).
    #[serde(default = "default_rain_burst_min")]
    pub rain_burst_ticks_min: u16,
    /// Maximum length of a precipitation burst (ticks).
    #[serde(default = "default_rain_burst_max")]
    pub rain_burst_ticks_max: u16,
}

fn default_rain_burst_min() -> u16 {
    50
}
fn default_rain_burst_max() -> u16 {
    140
}

impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            weather_enabled: true,
            // Dense enough that a continental shelf sees regular cover
            // and rain can offset open-water evaporative skin loss.
            cloud_spawn_interval_ticks: 280,
            cloud_rain_rate: 2.5,
            max_clouds: 28,
            // ~1% / tick to start a burst → frequent weather fronts without
            // every cloud raining at once.
            rain_chance_per_tick: 0.012,
            rain_burst_ticks_min: default_rain_burst_min(),
            rain_burst_ticks_max: default_rain_burst_max(),
        }
    }
}
