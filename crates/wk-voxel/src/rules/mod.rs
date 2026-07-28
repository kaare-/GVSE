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
mod gravity;
mod water_flow;
mod spill;
mod seepage;
mod grain;
mod rain;
mod evap;
mod condensation;
mod karst;
mod tick;

#[cfg(test)]
mod tests;

pub use condensation::{
    apply_condensation_rain, apply_condensation_rain_phased,
    apply_condensation_rain_with_orographic, CondensationConfig, OrographicConfig,
};
pub use evap::{apply_evaporation, apply_evaporation_into_humidity, EvapConfig};
pub use grain::{
    apply_cold_avalanche, apply_flow_erosion, apply_grain_fall, apply_grain_fall_regions,
    apply_grain_repose, apply_grain_repose_regions, GrainConfig,
};
pub use gravity::{apply_gravity_fall, apply_gravity_fall_regions};
pub use karst::{apply_karst_dissolution, KarstConfig};
pub(crate) use rain::deposit_water_on_surface;
pub use rain::{apply_rain, apply_rain_with_temp, is_standing_water, RainConfig};
pub use seepage::{apply_seepage, apply_seepage_regions};
pub use spill::{apply_lateral_spill, apply_lateral_spill_regions};
pub use tick::{
    tick, tick_with_configs, tick_with_configs_and_geotech, tick_with_perf, PerfConfig,
    FLOW_QUIET_AREA, FLOW_SUBSTEPS, FLOW_SUBSTEPS_MIN,
};
pub use water_flow::{apply_water_flow, apply_water_flow_regions};
