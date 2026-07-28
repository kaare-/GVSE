//! World spatial model: columns, chunks, terrain generation, markers.
//!
//! Column-based GVSE. MUST NOT import from wk-voxel. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".

/// Max solid layers per column (column stack).
pub const MAX_LAYERS: usize = 8;
/// Columns per chunk (column stack).
pub const CHUNK_W: usize = 64;
/// Raised for grand ring maps (~192–256 chunks). Streaming can lower this later.
pub const MAX_LOADED_CHUNKS: usize = 256;
pub const MAX_MARKERS: usize = 64;
pub const MERGE_GAP: u64 = 100;
pub const MERGE_MAX_THICKNESS: i64 = 1_000_000;

pub mod climate;
pub mod column;
pub mod chunk;
pub mod dig;
pub mod fields;
pub mod marker;
pub mod terrain;
pub mod weather;
pub mod world;
pub mod worldgen;

pub use climate::*;
pub use column::*;
pub use chunk::*;
pub use dig::*;
pub use fields::*;
pub use marker::*;
pub use terrain::*;
pub use weather::*;
pub use world::*;
pub use worldgen::*;
