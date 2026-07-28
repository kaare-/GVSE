//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Active-region planning helpers for standalone rule calls.

use crate::active::{plan_active, ActiveChunk};
use crate::chunk::ChunkCoord;
use crate::grid::World;

/// Resolve the scan plan for a standalone rule call. Uses current
/// dirty rects; if nothing is dirty, falls back to a full-world scan
/// so unit tests that forget an intermediate dirty still work when
/// the chunk exists. Prefer [`tick`], which plans once and clears.
pub(crate) fn regions_for_standalone(world: &World) -> Vec<ActiveChunk> {
    let planned = plan_active(world);
    if !planned.is_empty() {
        return planned;
    }
    // Full scan fallback — only loaded chunks.
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: crate::chunk::Rect::full(),
        })
        .collect()
}
