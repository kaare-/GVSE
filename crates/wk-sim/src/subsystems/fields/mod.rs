//! Field subsystems (stage 6). One module per field family.

pub mod humidity;
pub mod pressure;
pub mod thermal;
pub mod wind;

pub use humidity::run_humidity_field;
pub use pressure::run_pressure_field;
pub use thermal::run_thermal_field;
pub use wind::run_wind_field;
