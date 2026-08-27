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
pub mod atmosphere_metrics;
pub mod audit;
pub mod blueprint;
pub mod carbon;
pub mod cell;
pub mod chunk;
pub mod climate;
pub mod clouds;
pub mod competent_probe;
pub mod event_log;
pub mod failure;
pub mod fasthash;
pub mod fungi;
pub mod geotech_map;
pub mod grid;
pub mod heatmap;
pub mod humidity;
pub mod mineral;
pub mod sediment;
pub mod organism;
pub mod parallel;
pub mod phase;
pub mod plant;
pub mod rules;
pub mod save;
pub mod shade;
pub mod sim_preset;
pub mod spore_bank;
pub mod support_map;
pub mod displace;
pub mod landscape_body;
pub mod symbiosis;
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
    falls_through_empty_air, grain_max_stable_step, hosts_mycelium, is_flow_erodible, is_grain,
    is_repose_grain, permeability_cell, water_capacity, water_capacity_cell, water_capacity_with,
    Cell, CellFlags, Sat,
};
pub use chunk::{
    material_is_loose, Chunk, ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W,
};
pub use fasthash::{FxBuildHasher, FxHashMap, FxHashSet, FxHasher};
pub use blueprint::{
    ensure_symbiont_inherited, mutate_body, Blueprint, Genome, LaneId, PlacedModule, BLUEPRINT_DIR,
    BODY_MUTATION_MAX_EDITS,
    BODY_MUTATION_MAX_MODULES,
};
pub use atmosphere_metrics::{
    apply_weather_rgb, carbon_ratio, cloud_sky_transmit, column_surface_lee, humidity_mean_norm,
    humidity_norm_at, lit_sky_at, precip_cover_fraction, sky_rgb_at_height_weather, sky_transmit,
    sky_transmit_at, sun_sky_transmit, SkyWeatherParams, CLOUD_COVER_MAX, CLOUD_TRANSMIT_FLOOR,
    HUMIDITY_SKY_ATTEN, SUN_TRANSMIT_FLOOR,
};
pub use climate::{
    celestial_local, celestial_local_cfg, celestial_moon_local_cfg, celestial_moon_screen_pos_cfg,
    celestial_screen_pos, celestial_screen_pos_cfg, celestial_sun_local_cfg,
    celestial_sun_screen_pos_cfg, day_factor, day_factor_cfg, day_night_factor, day_night_factor_cfg,
    is_daytime, is_daytime_cfg, phase_fraction, phase_fraction_cfg, sky_rgb, sky_rgb_at_height,
    ClimateConfig, DEMO_DAY_TICKS,
};
pub use clouds::{
    cloud_floor_y, CloudConfig, CloudParcel, CloudStore, DOWNPOUR_MASS, MAX_CLOUD_PARCELS,
};
pub use event_log::{
    habit_label, SimEvent, SimEventKind, SimLog, SimSample, SIM_LOG_DEFAULT_CAP,
    SIM_LOG_DEFAULT_SAMPLE_PERIOD,
};
pub use failure::{
    apply_compaction, apply_failure, apply_roof_collapse, apply_shear_weaken, compaction_load_ok,
    effective_cohesion, face_shear_demand, grain_repose_max_step, pore_wetness, pore_wetness_with,
    roof_collapse_debris, roof_span_cells, roof_span_limit_cells, shear_weaken_debris,
    wet_repose_loosens, FailureConfig, FailureStats,
    COMPACTION_SIGMA_MIN,
};
pub use carbon::{
    gate_algae_photo, gate_plant_photo, step_carbon_budget, CarbonBudget, CarbonConfig,
    AMBIENT_ATM_C, AMBIENT_DISSOLVED_C, PLANT_PHOTO_C_FLOOR,
};
pub use spore_bank::{
    spore_bank_len, DormantSpore, SporeBank, SporeBankConfig, SporeKind, SPORE_BANK_PERIOD,
};
pub use fungi::{
    add_soft_litter, bind_strain_lineage, compost_organic_to_soil, infect_mycelium_at,
    infect_mycelium_with_lineage, is_fungus, is_surface_stalk, lineage_for_strain_at,
    max_mycelium_near, move_mycelium_meta, mycelium_energy_at, mycelium_shares_at,
    mycelium_shares_overlay_rgba, mycelium_strain_at, mycelium_strain_rgb,
    nearest_mycelium_lineage, pull_mycelium_cargo_to, seed_mycelium_near,
    sip_mycelium_energy_near, soft_litter_at, stamp_mycelium_lineage, step_mycelium_field,
    step_mycelium_field_cfg, strain_lineage,
    swap_cells_preserving_mycelium, swap_mycelium_meta, FungiConfig, MyceliumLineage,
    MyceliumLineageMap, FUNGUS_STALK_SPORE_MAX_DIST, FUNGUS_STALK_SPORE_MIN_DIST,
    MYCELIUM_ENERGY_CAP,
};
pub use geotech_map::{
    face_strength_wetness, geotech_map_due, relative_overburden, shear_score_c_threshold,
    wet_air_column_beside, FaceStress, GeotechMap, GeotechOverlayMode, GEOTECH_MAP_PERIOD,
    GEOTECH_MAP_PHASE,
};
pub use support_map::{
    column_supported, hanging_landscape_cluster, hanging_landscape_cluster_with, support_map_due,
    void_below_competent_seeds, void_below_competent_seeds_with, ChunkMask, SupportMap,
    COLUMN_SUPPORT_MAX_WALK, SUPPORT_MAP_PERIOD, SUPPORT_MAP_PHASE,
};
pub use landscape_body::{
    apply_landscape_fall, detach_landscape_bodies, detach_landscape_bodies_with, force_stamp_all,
    step_landscape_bodies, LandscapeBody, LandscapeBodyStore, LandscapeFallStats,
    LANDSCAPE_GRAVITY_ONLY_MIN, MAX_LANDSCAPE_BODIES, MIN_LANDSCAPE_BODY_CELLS,
};
pub use grid::{ChunkMap, World};
// HydroOverrides is defined in wk-material; re-export for app convenience.
pub use wk_material::{HydroOverrides, HydroSlot};
pub use organism::{
    bake_tip_into_body, column_sky_light, fallen_body_offset, resolve_organism_draw_cells,
    rigid_tip_offset, Atom, BodyModule, Corpse, ModuleId, OrganismPassTimings, OrganismStepOutcome,
    OrganismStepStats, OrganismStore, SpawnFail, SporeRelease, CORPSE_SETTLE_LAND_TICKS,
    CORPSE_SETTLE_WATER_TICKS, MAX_ATOMS, MAX_CORPSES, MAX_FALLEN_WATERLINE_EXTENT,
    SUBMERGED_STEM_URGE_LIGHT, WATER_LIGHT_TRANSMIT, WATER_SURFACE_TRANSMIT,
};
pub use symbiosis::{
    body_has_symbiont, clear_plant_sym_flow_lasts, clear_sym_net_flow_lasts,
    plant_sym_energy_reserve, plant_sym_sugar_spendable, probe_cream_link,
    probe_cream_link_preferring, probe_plant_link, probe_strain_frontier, step as step_symbiosis,
    step_strain_trade, treaty_match, SymBias, SymFrontierProbe, SymNetFlow, SymProbe, SymTradeMode,
    SYM_MATCH_MIN, SYM_NET_SUGAR_PAY_RESERVE, SYM_REPRO_RESERVE_FRAC,
};
pub use plant::{
    collect_live_photo_world_cells, collect_live_root_world_cells, collect_plant_sail_tops,
    find_fungus_slot, find_plant_slot, find_surface_air_slot, is_land_plant,
    plant_resists_rock_shove, plant_rooted_in_loose,
    sail_plants_on_wind_rafts, sail_plants_on_wind_rafts_cfg, sync_alloc_to_body,
    PlantGrowthCaps, MAX_PHOTO_MODULES,
    MAX_ROOT_MODULES, MAX_STEM_MODULES,
};
pub use shade::{
    build_canopy_index, build_canopy_index_posed, effective_photo_light, group_posed_by_atom,
    posed_canopy_sample, posed_canopy_sample_of, shade_transmit, shade_transmit_column,
    sum_posed_photo_light, sum_posed_photo_light_of, CanopyIndex, PosedModule,
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
    apply_cold_avalanche, apply_cold_avalanche_bound, apply_condensation_rain,
    apply_condensation_rain_phased, apply_condensation_rain_with_orographic, apply_evaporation,
    apply_evaporation_into_humidity, apply_evaporation_into_humidity_climate, apply_flow_erosion,
    apply_flow_erosion_bound,
    apply_grain_fall, apply_grain_fall_regions, apply_grain_repose, apply_grain_repose_bound,
    apply_snow_wind_drift,
    apply_grain_repose_regions, apply_gravity_fall, apply_gravity_fall_regions,
    apply_karst_dissolution, apply_lateral_spill, apply_rain, apply_rain_with_temp,
    apply_seepage, apply_seepage_contact_regions, apply_seepage_regions, apply_water_flow,
    apply_water_flow_regions,
    collect_floating_organic_columns, collect_floating_organic_columns_near,
    floating_organic_column_at, drift_floating_organic, drift_floating_organic_cfg,
    drift_floating_organic_columns, drift_floating_organic_columns_cfg,
    shove_floating_organic_with_current,
    active_has_unsupported_grain, is_standing_water, punch_through_floating_rafts,
    rise_and_soak_buoyant_litter, rise_and_soak_buoyant_litter_cfg, rise_buoyant_litter,
    settle_loose_grains, settle_loose_grains_regions, soak_floating_litter,
    soak_floating_litter_cfg,
    tick, tick_with_configs, tick_with_configs_and_geotech, tick_with_life,
    tick_with_life_profiled, tick_with_perf, tick_with_perf_profiled,
    wake_confined_head, wake_lake_bed_pores, wake_pore_weep_into_air,
    wake_vertical_chunk_seam_pores, apply_seepage_seam_coupling, wake_grains_for_settle, wake_grains_for_settle_coords,
    GrainWake,
    wake_unsupported_grains, wake_unstable_slopes,
    apply_competent_fall_regions, wake_competent_bodies, wake_competent_bodies_all,
    wake_floating_competent,
    CompetentFallConfig, CompetentFallStats, COMPETENT_FALL_PASSES, COMPETENT_FALL_PASSES_FPS,
    COMPETENT_TOPOLOGY_PASSES,
    CondensationConfig, EvapConfig, GrainConfig,
    KarstConfig,
    OrographicConfig, PerfConfig, PhysicsTimings, RainConfig, FLOW_QUIET_AREA, FLOW_SUBSTEPS,
    FLOW_SUBSTEPS_EO_AFTER, FLOW_SUBSTEPS_MIN, SEEPAGE_EVERY,
    GRAIN_REPOSE_HAZE_MAX, GRAIN_REPOSE_LAKE_MIN, GRAIN_SETTLE_PASSES,
    GRAIN_SETTLE_PASSES_SHALLOW, MYCELIUM_EROSION_BIND,
    MYCELIUM_RAFT_BIND_MIN,
    MYCELIUM_REPOSE_STEP_BONUS, ROOT_EROSION_BIND, ROOT_REPOSE_STEP_BONUS,
};
pub use temperature::{
    temperature_step_due, TempConfig, Temperature, TEMP_STEP_PERIOD, TEMP_STEP_PHASE,
};
pub use wind::Wind;
pub use save::{SimSnapshot, SIM_SAVE_DIR, SIM_SAVE_EXT, SIM_SCHEMA_VERSION};
pub use sim_preset::{
    builtin_preset_names, list_all_presets, list_disk_presets, load_builtin_preset, load_preset,
    preset_path, save_preset, sanitize_preset_name, PlantGenePreset, SimPreset, PRESET_DIR,
    PRESET_EXT, PRESET_SCHEMA_VERSION,
};
pub use worldgen::is_karst_zone_x;
pub use worldgen::{
    airborne_loose_at, continental_surface_y, live_surface_at, live_surface_y, stamp_world,
    WorldgenParams,
    LIVE_SURFACE_SEARCH,
};
