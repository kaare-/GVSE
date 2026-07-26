//! Muscle / Bone / Neural Test Studio — shared types.
//!
//! Isolation: depends on `wk-voxel` + `wk-material` only.
//! See `docs/organism/STUDIO.md`.

mod arena;
mod body;
mod export;
mod ga;
mod neural;
mod palette;
mod physics;
mod scenarios;
mod tissue;
mod train;

pub use arena::{ArenaConfig, StudioArena, ARENA_MAX, ARENA_MIN};
pub use body::{
    activate, force_sensors_bridging_parts, script_muscles, step_body, ActivateError, AttachedTissue,
    BodyGraph, Joint, Muscle, MuscleFeedback, NerveStrand, NeuronCluster, PartKind, RigidPart,
};
pub use export::{
    decode_body, encode_body, export_body, export_body_with_net, import_body_paint, ExportError,
    ExportedBody, BODY_SCHEMA_VERSION,
};
pub use ga::{evolve_morphology, mutate_paint, GaIndividual};
pub use neural::StudioNet;
pub use palette::{tissue_rgb, JOINT_SYMBOL};
pub use physics::{tick_world_gated, StudioPhysicsConfig};
pub use scenarios::{
    fin_hydro_arena, paint_fin_bench, paint_rough_terrain, paint_vertical_arm, rough_walk_arena,
    vertical_arm_arena,
};
pub use tissue::{
    ForceSensor, JointLimit, StudioBody, TissueKind, TissuePaint, FIXTURE_RGB, JOINT_RGB,
    MUSCLE_RGB, NERVE_RGB, NEURON_BLOB_RGB, SKIN_RGB,
};
pub use train::{apply_net, evaluate_net, hill_climb, EpisodeResult, TrainingSession};
