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
    regions_all_loaded(world)
}

/// All loaded chunks as full-rect active regions (ignores dirty halo).
///
/// Used by confined-head wake so ocean evaporation cannot starve a quiet
/// pipe shaft of scans.
pub(crate) fn regions_all_loaded(world: &World) -> Vec<ActiveChunk> {
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

/// Loaded chunks with sticky wet occupancy (surface Air and/or pore fill).
///
/// Skipping dry stone / empty sky cuts the insurance scan on demos while
/// still visiting quiet groundwater columns after lakes settle.
pub(crate) fn regions_wet_loaded(world: &World) -> Vec<ActiveChunk> {
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air || c.has_wet_pores)
        .map(|(&coord, _)| coord)
        .collect();
    // Bootstrap: old saves / stamps that never raised the flag.
    if coords.is_empty() && !world.chunks.is_empty() {
        return regions_all_loaded(world);
    }
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: crate::chunk::Rect::full(),
        })
        .collect()
}

/// Loaded chunks that currently hold wet Air.
///
/// Confined-head insurance only needs standing water / pipe films.
/// Groundwater-only chunks (`has_wet_pores` without wet Air) cannot
/// host a rising column, and after a drizzle soak they are most of
/// the map. Bootstrap (no flag ever set) falls back to
/// [`regions_wet_loaded`].
pub(crate) fn regions_wet_air_loaded(world: &World) -> Vec<ActiveChunk> {
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air)
        .map(|(&coord, _)| coord)
        .collect();
    if coords.is_empty() && !world.chunks.is_empty() {
        return regions_wet_loaded(world);
    }
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: crate::chunk::Rect::full(),
        })
        .collect()
}

/// Loaded chunks with sticky [`crate::chunk::Chunk::has_loose`], plus
/// Moore neighbours (repose / cold-avalanche write into adjacent Air).
///
/// Bootstrap (no flag ever set) falls back to all loaded chunks.
pub(crate) fn regions_loose_moore(world: &World) -> Vec<ActiveChunk> {
    use std::collections::HashSet;
    let mut coords: HashSet<ChunkCoord> = HashSet::new();
    let mut any = false;
    for (&coord, c) in &world.chunks {
        if !c.has_loose {
            continue;
        }
        any = true;
        for dy in -1..=1 {
            for dx in -1..=1 {
                let n = ChunkCoord::new(coord.cx + dx, coord.cy + dy);
                if world.chunks.contains_key(&n) {
                    coords.insert(n);
                }
            }
        }
    }
    if !any {
        return regions_all_loaded(world);
    }
    let mut coords: Vec<ChunkCoord> = coords.into_iter().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk {
            coord,
            rect: crate::chunk::Rect::full(),
        })
        .collect()
}
