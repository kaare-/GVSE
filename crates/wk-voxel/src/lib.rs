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
pub mod grid;
pub mod heatmap;
pub mod humidity;
pub mod organism;
pub mod parallel;
pub mod rules;
pub mod wind;
pub mod worldgen;

pub use active::{
    checkerboard_phase, clear_all_dirty, partition_checkerboard, plan_active, ActiveChunk,
};
pub use parallel::{parallel_enabled, set_parallel_enabled};
pub use cell::{is_grain, water_capacity, Cell, CellFlags, Sat};
pub use chunk::{Chunk, ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
pub use blueprint::{Blueprint, Genome, LaneId, PlacedModule, BLUEPRINT_DIR};
pub use grid::World;
pub use organism::{day_factor, Atom, BodyModule, ModuleId, OrganismStore, DEMO_DAY_TICKS, MAX_ATOMS};
pub use heatmap::Heatmap;
pub use humidity::{
    humidity_diffuse_due, Humidity, TileBounds, HUMIDITY_DIFFUSE_PHASE, HUMIDITY_DIFFUSE_PERIOD,
};
pub use rules::{
    apply_condensation_rain, apply_condensation_rain_with_orographic, apply_evaporation,
    apply_evaporation_into_humidity, apply_grain_fall, apply_gravity_fall, apply_karst_dissolution,
    apply_lateral_spill, apply_rain, apply_seepage, hydraulic_head, tick, CondensationConfig,
    EvapConfig, KarstConfig, OrographicConfig, RainConfig,
};
pub use wind::Wind;
pub use worldgen::is_karst_zone_x;
pub use worldgen::{continental_surface_y, stamp_world, WorldgenParams};
