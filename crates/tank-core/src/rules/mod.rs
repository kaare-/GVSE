//! Tank-scale physics passes.
//!
//! These are the five rules the handoff keeps from GvsE (`gravity`,
//! `water_flow`, `head`, `grain`, `seepage`), rewritten for a single
//! fixed grid. The world-scale originals pull in a thread pool, the wall
//! clock, mycelium and karst, which this core does not link. Behaviour the
//! tank needs — water falls and levels, sand sinks and slumps, pores
//! wet, stone stays put — is covered by the crate tests.

mod grain;
mod gravity;
mod head;
mod plan;
mod seepage;
mod tick;
mod water_flow;

pub use tick::step;
