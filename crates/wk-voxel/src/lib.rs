//! wk-voxel — greenfield 2D cellular-automata simulation.
//!
//! wk-voxel is an isolated experimental sim built alongside column-based
//! GVSE (`wk-world` / `wk-sim` / `wk-agents` / `wk-app`). It MUST NOT
//! import from wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app.
//! See docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! The only crate shared with the column-based stack is [`wk_material`]
//! (material IDs + property tables — pure data with no coupling).

pub mod cell;
pub mod chunk;
pub mod grid;
pub mod heatmap;
pub mod rules;
pub mod worldgen;

pub use cell::{is_grain, water_capacity, Cell, CellFlags, Sat};
pub use chunk::{Chunk, ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
pub use grid::World;
pub use heatmap::Heatmap;
pub use rules::{
    apply_evaporation, apply_grain_fall, apply_gravity_fall, apply_lateral_spill, apply_rain,
    tick, EvapConfig, RainConfig,
};
pub use worldgen::{continental_surface_y, stamp_world, WorldgenParams};
