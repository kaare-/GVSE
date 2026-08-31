//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cellular-automaton rules.
//!
//! One rule per tick sub-pass. Rules always read the world at the
//! start of the pass and write cells via [`World::set_cell`] so
//! chunk dirty rectangles stay coherent for whatever runs next.

mod plan;
mod head;
mod util;
pub(crate) use util::hash_prob;
mod gravity;
mod water_flow;
mod spill;
mod seepage;
mod competent_fall;
pub(crate) mod grain;
mod rain;
mod evap;
mod condensation;
mod karst;
mod tick;

#[cfg(test)]
mod tests;

pub use condensation::{
    apply_condensation_rain, apply_condensation_rain_phased,
    apply_condensation_rain_with_orographic, precipitate_thermal_surplus, CondensationConfig,
    OrographicConfig,
};
pub use evap::{
    apply_evaporation, apply_evaporation_into_humidity, apply_evaporation_into_humidity_climate,
    EvapConfig,
};
pub use competent_fall::{
    apply_competent_fall_regions, wake_competent_bodies, wake_competent_bodies_all,
    wake_floating_competent,
    CompetentFallConfig, CompetentFallStats, COMPETENT_FALL_PASSES, COMPETENT_FALL_PASSES_FPS,
    COMPETENT_TOPOLOGY_PASSES,
};
pub use grain::{
    apply_cold_avalanche, apply_cold_avalanche_bound, apply_flow_erosion, apply_flow_erosion_bound,
    apply_grain_fall, apply_grain_fall_regions, apply_grain_repose, apply_grain_repose_bound,
    apply_snow_wind_drift,
    apply_grain_repose_regions, settle_loose_grains, settle_loose_grains_regions,
    settle_loose_grains_regions_ex,
    collect_floating_organic_columns, collect_floating_organic_columns_near,
    floating_organic_column_at,     drift_floating_organic, drift_floating_organic_cfg,
    drift_floating_organic_columns, drift_floating_organic_columns_cfg,
    shove_floating_organic_with_current,
    punch_through_floating_rafts,
    rise_and_soak_buoyant_litter, rise_and_soak_buoyant_litter_cfg, rise_buoyant_litter,
    soak_floating_litter, soak_floating_litter_cfg,
    active_has_unsupported_grain, wake_grains_for_settle, wake_grains_for_settle_coords,
    GrainWake,
    wake_unsupported_grains, wake_unstable_slopes, GrainConfig, GRAIN_REPOSE_HAZE_MAX,
    GRAIN_REPOSE_LAKE_MIN, GRAIN_SETTLE_PASSES, GRAIN_SETTLE_PASSES_FPS_DEEP,
    GRAIN_SETTLE_PASSES_SHALLOW,
    MYCELIUM_EROSION_BIND,
    MYCELIUM_RAFT_BIND_MIN, MYCELIUM_REPOSE_STEP_BONUS, ROOT_EROSION_BIND, ROOT_REPOSE_STEP_BONUS,
};
pub use gravity::{apply_gravity_fall, apply_gravity_fall_regions};
pub use karst::{apply_karst_dissolution, KarstConfig};
pub(crate) use rain::{deposit_water_in_air, deposit_water_on_surface};
pub use rain::{apply_rain, apply_rain_with_temp, is_standing_water, RainConfig};
pub use seepage::{
    apply_seepage, apply_seepage_contact_regions, apply_seepage_regions,
    apply_seepage_seam_coupling, wake_lake_bed_pores,
    wake_pore_weep_into_air, wake_vertical_chunk_seam_pores,
};
pub use spill::{apply_lateral_spill, apply_lateral_spill_regions};
pub use tick::{
    tick, tick_with_configs, tick_with_configs_and_geotech, tick_with_life,
    tick_with_life_profiled, tick_with_perf, tick_with_perf_profiled, PerfConfig, PhysicsTimings,
    FLOW_QUIET_AREA, FLOW_SUBSTEPS, FLOW_SUBSTEPS_EO_AFTER, FLOW_SUBSTEPS_MIN, SEEPAGE_EVERY,
};
pub use water_flow::{apply_water_flow, apply_water_flow_regions, wake_confined_head};
