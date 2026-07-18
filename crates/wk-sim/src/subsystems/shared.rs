//! Shared subsystem parameters and cross-subsystem helpers.

use wk_world::climate::ClimateSettings;

/// kg of water per metre of standing depth on one column (density 1000
/// kg/m^3 * SAMPLE_WIDTH_M 0.25 m cross-section).
pub const WATER_MASS_PER_METRE_DEPTH: f32 = 250.0;

/// Cap on snow+ice in the weather cap of one column. Snow alone used to
/// be capped, but melt→refreeze (and snow landing on ice) converted that
/// budget into unbounded ice towers — mountains grew to megametres.
pub const MAX_FROZEN_SURFACE_MASS_KG: i64 = 10_000;

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
pub fn split_precipitation(
    sea: f32,
    amount: f32,
    climate_elev: f32,
    tick: u64,
    climate: &ClimateSettings,
    existing_frozen: i64,
) -> (i64, i64) {
    // (water_component, snow_component)
    // `amount` stays a float all the way through so a fractional rate
    // (e.g. cloud_rain_rate = 1.5) doesn't get chopped to a whole
    // number before rounding into an i64 kg count.
    let precip_component = amount.round() as i64;
    if precip_component <= 0 {
        return (0, 0);
    }
    // Uses climate_elevation (excludes any snow/ice already piled up), not
    // raw surface_y — otherwise snow raising the surface would make
    // the column read as colder, causing still more snow: a runaway
    // feedback loop.
    let temp = wk_world::climate::temperature_at(climate_elev, sea, tick, climate);
    if temp <= climate.freeze_point_c && existing_frozen < MAX_FROZEN_SURFACE_MASS_KG {
        // Capped so a permanently-frozen spot doesn't accumulate an
        // unbounded ice/snow tower; beyond the cap it falls as rain/slush
        // runoff instead (a crude stand-in for avalanche transport).
        (0, precip_component)
    } else {
        (precip_component, 0)
    }
}
