//! Headless scenario experiments E1–E17, E20+, E30+.

mod e1_rain_hill;
mod e2_basin;
mod e3_river;
mod e4_delta;
mod e5_wet_dry;
mod e6_chunk_seam;
mod e7_soak;
mod e8_save_load;
mod e9_karst_horizontal_passage;
mod e10_sinkhole_captures_river;
mod e11_cave_roof_collapses;
mod e13_burrow_produces_surface_tailings;
mod e14_ecology_grows_when_wet;
mod e15_roots_reduce_erosion;
mod e16_grazer_eats_biomass;
mod e17_reproduction_mutates;
mod e20_geothermal_steady_state;
mod e30_atom_bloom;
mod e31_messy_boom;
mod e33_day_float_night_sink;
mod e46_plankton_env;
mod e47_ocean_water_budget;
mod e48_gas_exchange;
mod e49_wind_tide_waves;
mod e50_ring_facies;
mod e51_default_atom_repro;
mod e52_cold_snap_water;
mod e54_rain_puddles;
mod perf_profile;
mod e22_humidity_near_water_body;
mod e23_convection_cell;
mod helpers;

pub use helpers::*;
