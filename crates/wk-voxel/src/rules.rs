//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Rule engine (stub).
//!
//! The full rule set (gravity fall, lateral spill, density swap,
//! porosity absorb) lands in a follow-up PR. Today's job is to
//! establish the interface — one entry point that advances the world
//! by one tick and clears dirty rectangles that had no cell writes.
//!
//! Design notes for the eventual implementation (see
//! `docs/VOXEL_MIGRATION.md` § "Update order sketch"):
//!
//! - Update chunks in a 4-pass checkerboard so future multithreading
//!   can partition without locks (Noita: Purho 2019). Even quadrant →
//!   odd column → even column → odd quadrant.
//! - Walk each chunk **bottom-up** within a pass so gravity moves a
//!   cell only once per tick (Purho's "falling sand from the bottom
//!   up" rule).
//! - Use the chunk's `dirty` rectangle to skip quiescent regions.
//! - Per-chunk RNG seeded by `(world.seed, coord, world.tick /
//!   period)` so rules stay deterministic across replays.

use crate::grid::World;

/// Advance the sim by one tick.
///
/// Today this is a no-op — real cell rules are in the follow-up PR.
/// It still bumps `world.tick` and every chunk's `tick`, and clears
/// dirty rectangles so tests can observe the tick loop without a
/// physics implementation.
pub fn tick(world: &mut World) {
    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
        // Dirty rectangle would normally be cleared *after* the rule
        // pass consumed it. With no rules yet, clearing here just
        // keeps the counter honest for smoke tests.
        chunk.clear_dirty();
    }
}
