//! Headless scenario experiments E1–E8, E20+.

mod e1_rain_hill;
mod e2_basin;
mod e3_river;
mod e4_delta;
mod e5_wet_dry;
mod e6_chunk_seam;
mod e7_soak;
mod e8_save_load;
mod e20_geothermal_steady_state;
mod e22_humidity_near_water_body;
mod e23_convection_cell;
mod e24_darcy_pressure_equilibration;
mod e25_dissolved_plume_diffusion;
mod helpers;

pub use helpers::*;
