//! Muscle / Bone / Neural Test Studio — shared types.
//!
//! Isolation: depends on `wk-voxel` + `wk-material` only.
//! See `docs/organism/STUDIO.md`.

mod arena;
mod export;
mod palette;
mod tissue;

pub use arena::{ArenaConfig, StudioArena};
pub use export::{export_body, ExportError, BODY_SCHEMA_VERSION};
pub use palette::{tissue_rgb, JOINT_SYMBOL};
pub use tissue::{
    ForceSensor, JointLimit, StudioBody, TissueKind, TissuePaint, FIXTURE_RGB, MUSCLE_RGB,
    NERVE_RGB, NEURON_BLOB_RGB, SKIN_RGB,
};
