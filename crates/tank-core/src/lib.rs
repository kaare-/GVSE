//! MakeAcreature simulation core.
//!
//! Phase 0 of the handoff: a 128×256 physics tank, a 384×768 pixel
//! layer, a 20/20 tank clock, postcard saves, and a telemetry queue.
//!
//! The physics passes are named for the GvsE rules they replace
//! (`gravity`, `water_flow`, `head`, `grain`, `seepage`). They are a
//! tank-scale rewrite, not a line copy. The originals are built for a
//! chunked world and depend on a thread pool, the wall clock, mycelium
//! and karst, which this crate does not use. `wk-material` is the shared
//! property table. Randomness is [`hash_u64`]. This crate does not read
//! the wall clock and does not touch the filesystem.

pub mod archive;
pub mod bio;
pub mod cell;
pub mod clock;
pub mod consts;
pub mod font;
pub mod grid;
pub mod hash;
pub mod enclosure;
pub mod humidity;
pub mod milestones;
pub mod pixels;
pub mod rocks;
pub mod rules;
pub mod tank;
pub mod telemetry;
pub mod thermal;
pub mod tools;

pub use archive::{
    portrait, starter_genome, starter_name, SpeciesNode, Specimen, SpecimenStats,
};
pub use cell::Cell;
pub use clock::{day_factor, is_day, phase, phase_name};
pub use font::{glyph_bits, glyph_rows};
pub use hash::{hash_bytes, hash_u64};
pub use tank::{Carried, Inspect, LineageCard, ReleaseError, SimEvent, Tank, TankError, TankInput, ToolEvent};
pub use telemetry::{format_batch, TelEvent, Telemetry};
pub use tools::set_fill_level;

#[cfg(test)]
mod tests;
