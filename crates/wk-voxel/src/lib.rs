//! wk-voxel — greenfield 2D cellular-automata simulation.
//!
//! wk-voxel is an isolated experimental sim built alongside column-based
//! GVSE (`wk-world` / `wk-sim` / `wk-agents` / `wk-app`). It MUST NOT
//! import from wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app.
//! See docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! The only crate shared with the column-based stack is [`wk_material`]
//! (material IDs + property tables — pure data with no coupling).

pub mod active;
pub mod blueprint;
pub mod cell;
pub mod chunk;
pub mod climate;
pub mod clouds;
pub mod fungi;
pub mod grid;
pub mod heatmap;
pub mod humidity;
pub mod organism;
pub mod parallel;
pub mod phase;
pub mod plant;
pub mod rules;
pub mod shade;
pub mod temperature;
pub mod wind;
pub mod worldgen;

pub use active::{
    checkerboard_phase, clear_all_dirty, partition_checkerboard, plan_active, ActiveChunk,
};
pub use parallel::{parallel_enabled, set_parallel_enabled};
pub use cell::{
    falls_through_empty_air, grain_max_stable_step, is_flow_erodible, is_grain, is_repose_grain,
    water_capacity, Cell, CellFlags, Sat,
};
pub use chunk::{Chunk, ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
pub use blueprint::{Blueprint, Genome, LaneId, PlacedModule, BLUEPRINT_DIR};
pub use climate::{
    celestial_local, celestial_local_cfg, celestial_screen_pos, celestial_screen_pos_cfg, day_factor,
    day_factor_cfg, day_night_factor, day_night_factor_cfg, is_daytime, is_daytime_cfg, phase_fraction,
    phase_fraction_cfg, sky_rgb, sky_rgb_at_height, ClimateConfig, DEMO_DAY_TICKS,
};
pub use clouds::{
    cloud_floor_y, CloudConfig, CloudParcel, CloudStore, DOWNPOUR_MASS, MAX_CLOUD_PARCELS,
};
pub use fungi::{is_fungus, soft_litter_at, add_soft_litter};
pub use grid::World;
pub use organism::{
    Atom, BodyModule, Corpse, ModuleId, OrganismStore, CORPSE_SETTLE_LAND_TICKS,
    CORPSE_SETTLE_WATER_TICKS, MAX_ATOMS, MAX_CORPSES,
};
pub use plant::{find_plant_slot, is_land_plant, sync_alloc_to_body};
pub use shade::{build_canopy_index, effective_photo_light, CanopyIndex};
pub use heatmap::Heatmap;
pub use humidity::{
    humidity_diffuse_due, Humidity, TileBounds, HUMIDITY_DIFFUSE_PHASE, HUMIDITY_DIFFUSE_PERIOD,
};
pub use phase::{
    apply_freeze, apply_phase, deposit_condensate_on_surface, deposit_precip_on_surface,
    ice_lid_thickness, precip_forms_snow_at_air, PhaseConfig,
};
pub use rules::{
    apply_cold_avalanche, apply_condensation_rain, apply_condensation_rain_phased,
    apply_condensation_rain_with_orographic, apply_evaporation, apply_evaporation_into_humidity,
    apply_flow_erosion, apply_grain_fall, apply_grain_fall_regions, apply_grain_repose,
    apply_grain_repose_regions, apply_gravity_fall, apply_gravity_fall_regions,
    apply_karst_dissolution, apply_lateral_spill, apply_rain, apply_rain_with_temp,
    apply_seepage, apply_seepage_regions, apply_water_flow, apply_water_flow_regions,
    deposit_water_on_surface, hydraulic_head, is_standing_water, tick, tick_with_perf,
    CondensationConfig, EvapConfig, GrainConfig, KarstConfig, OrographicConfig, PerfConfig,
    RainConfig, FLOW_QUIET_AREA, FLOW_SUBSTEPS, FLOW_SUBSTEPS_MIN,
};
pub use temperature::{
    temperature_step_due, TempConfig, Temperature, TEMP_STEP_PERIOD, TEMP_STEP_PHASE,
};
pub use wind::Wind;
pub use worldgen::is_karst_zone_x;
pub use worldgen::{continental_surface_y, stamp_world, WorldgenParams};
