//! Phase 2 biology. B0: free algae cells, interned genomes, glass film.
//! B1: profiles, growth, stress. B2: mode-program bodies, realised map,
//! GLUE colonies, pace law, brood detach. B3: tiled keyed fields, contact
//! chemistry (enzyme / toxin / needle / signal / receptor), litter, minerals,
//! bone / shell secretions, cilia.
#![allow(clippy::too_many_arguments)]

pub mod chem;
pub mod fields;
pub mod genome;
pub mod minerals;
pub mod profile;
pub mod program;
pub mod species;
pub mod step;
pub mod store;

pub use genome::{Genome, GenomeStore, GenotypeNode, StressDef};
pub use species::{SpeciesRec, NO_SPECIES};
pub use profile::{Block, Entry, Mode, ModeCoeffs};
pub use program::{ChildKind, ChildSpec, Detach, Dir, Gate, HeadingRule, Scalar, Transition};
pub use step::{
    corpse_split, ev, log2_q8, max_age_ticks, pixel_to_phys, step, swipe_film, BioStepResult, FINES_SHARE_Q8,
};
pub use store::{CellRec, CellStore, OrgBundle, OrgRec, Slot};
