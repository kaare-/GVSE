//! Phase 2 biology. B0: free algae cells, interned genomes, glass film.
//! B1: profiles, growth, stress. B2: mode-program children, GLUE colonies.
#![allow(clippy::too_many_arguments)]

pub mod fields;
pub mod genome;
pub mod profile;
pub mod program;
pub mod step;
pub mod store;

pub use genome::{GenomeStore, GenomeV0};
pub use profile::{Block, Mode};
pub use step::{step, swipe_film, BioStepResult};
pub use store::CellStore;
