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
pub mod aggregate;
pub mod audit;
pub mod biology;
pub mod blueprint;
pub mod cell;
pub mod chunk;
pub mod climate;
pub mod clouds;
pub mod failure;
pub mod fungi;
pub mod geotech_map;
pub mod grid;
pub mod heatmap;
pub mod humidity;
pub mod organism;
pub mod parallel;
pub mod phase;
pub mod plant;
pub mod rules;
pub mod save;
pub mod shade;
pub mod temperature;
pub mod wind;
pub mod worldgen;

pub use active::{clear_all_dirty, partition_checkerboard, plan_active, ActiveChunk};
pub use audit::{
    assert_cell_sat_conserved, mass_audit_enabled, sat_totals, set_mass_audit_enabled,
    tracked_totals, SatTotals, CELL_SAT_TICK_TOLERANCE,
};
pub use parallel::{parallel_enabled, set_parallel_enabled};
pub use cell::{
    falls_through_empty_air, grain_max_stable_step, is_flow_erodible, is_grain, is_repose_grain,
    water_capacity, water_capacity_with, Cell, CellFlags, Sat,
};
pub use chunk::{Chunk, ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
pub use aggregate::{body_plan_from, body_plan_from_kinds, BodyPlan};
pub use biology::module_death_material;
pub use blueprint::{
    kind_swap_partners, modulate_module_rgb, paint_genome_onto_modules, paint_genome_onto_traits,
    Blueprint, Genome, LaneId, PixelTraits, PlacedModule,
    BLUEPRINT_DIR,
};
pub use climate::{
    celestial_local, celestial_local_cfg, celestial_screen_pos, celestial_screen_pos_cfg, day_factor,
    day_factor_cfg, day_night_factor, day_night_factor_cfg, is_daytime, is_daytime_cfg, phase_fraction,
    phase_fraction_cfg, sky_rgb, sky_rgb_at_height, ClimateConfig, DEMO_DAY_TICKS,
};
pub use clouds::{
    cloud_floor_y, CloudConfig, CloudParcel, CloudStore, DOWNPOUR_MASS, MAX_CLOUD_PARCELS,
};
pub use failure::{
    apply_bone_crush, apply_compaction, apply_failure, apply_roof_collapse, apply_shear_weaken,
    bone_crush_load_ok, compaction_load_ok, effective_cohesion, face_shear_demand, pore_wetness,
    pore_wetness_with, roof_collapse_debris, roof_span_cells, roof_span_limit_cells,
    shear_weaken_debris, wet_repose_loosens, FailureConfig, BONE_CRUSH_SIGMA_MIN,
    COMPACTION_SIGMA_MIN,
};
pub use fungi::{
    add_soft_litter, collect_fungus_tissue_world_cells, deposit_organic_on_surface, digest_labile,
    dissolve_corpse_to_organic, fill_ghost_root_voids, fungus_near, hypha_count, is_fungus,
    soft_litter_at, try_grow_hypha_into_dead_stem, try_seed_litter_bloom, HYPHA_GROW_COST,
    HYPHA_GROW_PERIOD, LITTER_BLOOM_THRESHOLD, MAX_HYPHA_MODULES,
};
pub use geotech_map::{
    face_strength_wetness, geotech_map_due, relative_overburden, shear_score_c_threshold,
    wet_air_column_beside, FaceStress, GeotechMap, GeotechOverlayMode, GEOTECH_MAP_PERIOD,
    GEOTECH_MAP_PHASE,
};
pub use grid::World;
// HydroOverrides is defined in wk-material; re-export for app convenience.
pub use wk_material::{HydroOverrides, HydroSlot};
pub use organism::{
    apply_living_stem_integrity, bone_column_capacity, collect_corpse_stem_world_cells,
    collect_epiphyte_load_on_stems, collect_stem_wetness_on_cells, epiphyte_stem_wetness,
    fracture_overloaded_bones, stem_integrity_failing_index, stem_weight_above, topple_stem_at,
    update_stem_wetness, Atom, BodyModule, Corpse, ModuleId, OrganismStore, SpawnFail,
    CORPSE_SETTLE_LAND_TICKS, CORPSE_SETTLE_WATER_TICKS, DEAD_DECAY_PER_TICK, EPI_STEM_DRINK,
    EPI_STEM_DRY_FRAC, FUNGAL_DECAY_PER_TICK, INTEGRITY_TOPPLE_THRESHOLD, MAX_ATOMS, MAX_CORPSES,
    STEM_FREE_LOAD, STEM_LOAD_DRAIN_PER_ABOVE, STEM_RECHARGE_ENERGY_PER_UNIT, STEM_RECHARGE_PER_TICK,
    STEM_WET_TRACK,
};
pub use plant::{
    attach_seek_radius, collect_live_root_world_cells, collect_live_stem_world_cells,
    find_fungus_slot, find_plant_slot, find_surface_air_slot, apply_genome, is_epiphyte,
    is_holdfast_anchored, is_land_plant, leave_dead_roots_in_place, sync_alloc_on_atom,
    sync_alloc_to_body, try_elongate_root, try_epiphyte_reseat, PlantGrowthCaps,
    MAX_PHOTO_MODULES, MAX_ROOT_MODULES, MAX_STEM_MODULES,
};
pub use shade::{
    build_canopy_index, effective_photo_light, epiphyte_rider_transmit, CanopyIndex,
};
pub use heatmap::Heatmap;
pub use humidity::{
    humidity_diffuse_due, Humidity, TileBounds, HUMIDITY_DIFFUSE_PHASE, HUMIDITY_DIFFUSE_PERIOD,
};
pub use phase::{
    apply_freeze, apply_phase, deposit_condensate_on_surface, deposit_precip_on_surface,
    ice_lid_thickness, precip_forms_snow_at_air, PhaseConfig,
};
pub use rules::{
    apply_biological_decay, apply_cold_avalanche, apply_cold_avalanche_bound,
    apply_condensation_rain, apply_condensation_rain_phased,
    apply_condensation_rain_with_orographic, apply_evaporation, apply_evaporation_into_humidity,
    apply_flow_erosion, apply_flow_erosion_bound, apply_grain_fall, apply_grain_fall_regions,
    apply_grain_repose, apply_grain_repose_bound, apply_grain_repose_regions, apply_gravity_fall,
    apply_gravity_fall_regions, apply_karst_dissolution, apply_lateral_spill, apply_rain,
    apply_rain_with_temp, apply_seepage, apply_seepage_regions, apply_water_flow,
    apply_water_flow_regions, is_standing_water, tick, tick_with_configs,
    tick_with_configs_and_geotech, tick_with_life, tick_with_perf, BiologicalDecayConfig,
    CondensationConfig, EvapConfig, GrainConfig, KarstConfig, OrographicConfig, PerfConfig,
    RainConfig, FLOW_QUIET_AREA, FLOW_SUBSTEPS, FLOW_SUBSTEPS_MIN, ROOT_EROSION_BIND,
    ROOT_REPOSE_STEP_BONUS,
};
pub use temperature::{
    temperature_step_due, TempConfig, Temperature, TEMP_STEP_PERIOD, TEMP_STEP_PHASE,
};
pub use wind::Wind;
pub use save::{SimSnapshot, SIM_SAVE_DIR, SIM_SAVE_EXT, SIM_SCHEMA_VERSION};
pub use worldgen::is_karst_zone_x;
pub use worldgen::{continental_surface_y, stamp_world, WorldgenParams};
