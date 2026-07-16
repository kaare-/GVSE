//! Simulation subsystems: one module per scheduled pass.
//!
//! Public API is re-exported here so `crate::subsystems::{...}` and
//! `crate::subsystems::exchange_outboxes` keep working after the
//! former monolithic `subsystems.rs` was split.

mod activity;
mod evaporation;
pub mod fields;
mod groundwater;
mod halos;
mod infiltration;
mod lake_level;
mod layer_merge;
mod phase_change;
mod rain;
mod sediment;
mod shared;
mod slumping;
mod surface_water;
mod weather;

pub use activity::run_activity;
pub use evaporation::run_evaporation;
pub use fields::{
    run_dissolved_field, run_groundwater_head_field, run_humidity_field, run_pressure_field,
    run_thermal_field, run_wind_field,
};
pub use groundwater::run_groundwater_flow;
pub use halos::{exchange_outboxes, update_halos};
pub use infiltration::run_infiltration;
pub use lake_level::run_lake_level;
pub use layer_merge::run_layer_merge;
pub use phase_change::run_phase_change;
pub use rain::run_rain_inject;
pub use sediment::run_sediment;
pub use shared::SimParams;
pub use slumping::run_slumping;
pub use surface_water::run_surface_water;
pub use weather::run_weather;
