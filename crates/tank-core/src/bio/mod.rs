//! Phase 2 biology. B0: free algae cells, interned genomes, glass film.
#![allow(clippy::too_many_arguments)]

pub mod genome;
pub mod step;
pub mod store;

pub use genome::{GenomeStore, GenomeV0};
pub use step::{step, swipe_film, BioStepResult};
pub use store::CellStore;
