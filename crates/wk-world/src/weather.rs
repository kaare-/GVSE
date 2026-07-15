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
    /// Probability, per tick, that a given cloud is actively precipitating
    /// (rather than just drifting). A cloud that rained continuously every
    /// tick it had any land underneath would exhaust its whole moisture
    /// budget within a few dozen ticks — nowhere near enough real distance,
    /// at a believable drift speed, to ever reach terrain far from the
    /// coast. Intermittent rain spreads a fixed moisture budget over a much
    /// longer stretch of travel, and matches "sometimes rains" rather than
    /// a constant drizzle under every cloud.
    pub rain_chance_per_tick: f32,
}

impl Default for WeatherSettings {
    fn default() -> Self {
        Self {
            weather_enabled: true,
            cloud_spawn_interval_ticks: 3600,
            cloud_rain_rate: 1.5,
            max_clouds: 6,
            rain_chance_per_tick: 0.015,
        }
    }
}
