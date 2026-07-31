//! Headless wk-voxel scenario ports of legacy E-series product intents.
//!
//! Column-stack oracles live in `tests/scenarios/` (via wk-sim). This
//! harness never imports those crates — isolation guardrails in
//! `docs/VOXEL_MIGRATION.md`.

mod e1_rain_hill;
mod e2_basin;
mod e6_chunk_seam;
mod e8_save_load;
mod e10_sinkhole;
mod e11_cave_roof;
mod e15_roots_reduce_erosion;
mod e18_bone_persists_after_muscle_rots;
mod e19_bone_fragility;
mod e40_epiphyte_seat;
mod e42_standing_dead_stem_topples;
mod e42b_fungal_rot_accelerates_topple;
mod e45_live_stem_load_topple;
mod e43_host_leave_smother;
mod e41_stem_wetness_drink;
mod helpers;
