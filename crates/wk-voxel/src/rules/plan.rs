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
pub(crate) fn regions_all_loaded(world: &World) -> Vec<ActiveChunk> {
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk::new(coord, crate::chunk::Rect::full()))
        .collect()
}

/// Loaded chunks that can host a confined rise.
///
/// Needs standing water next to rock. Rain-film sky, mid-ocean water with
/// no solid, and groundwater-only crust (wet pores, no free surface) are
/// skipped — those cells already reject per-cell, and walking them was
/// the leftover period-16 cost. Occupancy is the source of truth: an
/// empty match means no shaft this tick.
pub(crate) fn regions_confined_loaded(world: &World) -> Vec<ActiveChunk> {
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_solid && c.has_standing_air)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk::new(coord, crate::chunk::Rect::full()))
        .collect()
}

/// True when standing water in `coord` can still wet a pore — this
/// chunk has a dry-loose or unsaturated bed, or the chunk below does.
/// Mid-ocean and a known-full table over inert rock are a no-op
/// (time-coarsen exact skip).
fn standing_can_still_infiltrate(
    world: &World,
    coord: ChunkCoord,
    chunk: &crate::chunk::Chunk,
) -> bool {
    if chunk.has_unsaturated_pores {
        return true;
    }
    // Dry sand / soil in this chunk has never raised the wet-pore flag.
    if chunk.has_loose && !chunk.has_wet_pores {
        return true;
    }
    let below = ChunkCoord::new(coord.cx, coord.cy - 1);
    match world.chunks.get(&below) {
        None => false,
        Some(b) => {
            if b.has_unsaturated_pores {
                return true;
            }
            if b.has_wet_pores && !b.has_unsaturated_pores {
                return false;
            }
            // Never wetted. Loose grains can drink; bedrock cannot.
            b.has_loose
        }
    }
}

/// Loaded chunks that may have a standing-water bed or a wetting front.
///
/// A quiet saturated water table (`has_wet_pores` only) is skipped.
/// Standing water walks down into a dry-loose or unsaturated bed
/// (this chunk or `cy-1`). Mid-ocean and a full table over inert
/// rock drop out — exact skip, not a frozen chunk. Rain-film sky is
/// skipped. Occupancy is the source of truth.
pub(crate) fn regions_lake_bed_loaded(world: &World) -> Vec<ActiveChunk> {
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(&coord, c)| {
            c.has_unsaturated_pores
                || (c.has_standing_air && standing_can_still_infiltrate(world, coord, c))
        })
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk::new(coord, crate::chunk::Rect::full()))
        .collect()
}

/// Loaded chunks with sticky [`crate::chunk::Chunk::has_loose`], plus
/// Moore neighbours (repose / cold-avalanche write into adjacent Air).
///
/// Occupancy is the source of truth — no loose flag means no walk.
pub(crate) fn regions_loose_moore(world: &World) -> Vec<ActiveChunk> {
    use std::collections::HashSet;
    let mut coords: HashSet<ChunkCoord> = HashSet::new();
    for (&coord, c) in &world.chunks {
        if !c.has_loose {
            continue;
        }
        for dy in -1..=1 {
            for dx in -1..=1 {
                let n = ChunkCoord::new(coord.cx + dx, coord.cy + dy);
                if world.chunks.contains_key(&n) {
                    coords.insert(n);
                }
            }
        }
    }
    let mut coords: Vec<ChunkCoord> = coords.into_iter().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    coords
        .into_iter()
        .map(|coord| ActiveChunk::new(coord, crate::chunk::Rect::full()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::grid::World;
    use wk_material::MaterialId;

    fn rain_film() -> Cell {
        let mut c = Cell::air();
        c.sat = Sat(33);
        c
    }

    #[test]
    fn confined_and_lake_bed_skip_rain_film_sky() {
        let mut w = World::new(8);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        w.ensure_chunk(ChunkCoord::new(2, 0));
        // cx=0: drizzle only — must not enter either insurance walk.
        w.set_cell(2, 2, rain_film());
        // cx=1: standing water next to rock — confined; lake-bed is
        // exact-skip (bedrock cannot drink).
        w.set_cell(65, 1, Cell::solid(MaterialId::Bedrock));
        w.set_cell(66, 2, Cell::water());
        // cx=2: mid-ocean water, no solid — exact-skip lake-bed.
        w.set_cell(130, 2, Cell::water());
        // cx=3: groundwater-only crust — lake-bed soak, not a confined shaft.
        w.ensure_chunk(ChunkCoord::new(3, 0));
        let mut pore = Cell::solid(MaterialId::Sand);
        pore.sat = Sat(20);
        w.set_cell(194, 1, pore);

        let confined: Vec<_> = regions_confined_loaded(&w)
            .into_iter()
            .map(|ac| ac.coord)
            .collect();
        assert_eq!(confined, vec![ChunkCoord::new(1, 0)]);

        let lake: Vec<_> = regions_lake_bed_loaded(&w)
            .into_iter()
            .map(|ac| ac.coord)
            .collect();
        assert_eq!(
            lake,
            vec![ChunkCoord::new(3, 0)],
            "mid-ocean and standing-on-bedrock are exact-skip; unsat sand stays"
        );

        // Quiet saturated table: after a lake-bed scan clears the unsat
        // flag, the chunk must drop out. Standing water still walks down
        // into a bed from the chunk above.
        w.ensure_chunk(ChunkCoord::new(4, 0));
        let cap = crate::cell::water_capacity(MaterialId::Stone);
        w.set_cell(
            258,
            1,
            Cell {
                material: MaterialId::Stone,
                sat: Sat(cap),
                ..Cell::default()
            },
        );
        crate::rules::wake_lake_bed_pores(&mut w);
        let lake: Vec<_> = regions_lake_bed_loaded(&w)
            .into_iter()
            .map(|ac| ac.coord)
            .collect();
        assert!(
            !lake.contains(&ChunkCoord::new(4, 0)),
            "full water-table chunk must leave the lake-bed walk"
        );
        assert!(
            !lake.contains(&ChunkCoord::new(1, 0)),
            "standing-on-bedrock stays exact-skip"
        );
        assert!(lake.contains(&ChunkCoord::new(3, 0)));
    }

    #[test]
    fn lake_bed_keeps_standing_above_dry_sand() {
        // cy=1 standing water, cy=0 dry loose — the wake must still
        // walk down. Skipping the ocean chunk is only legal when the
        // bed is known-full or inert.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.set_cell(2, 2, Cell::solid(MaterialId::Sand));
        w.set_cell(2, 66, Cell::water());
        let lake: Vec<_> = regions_lake_bed_loaded(&w)
            .into_iter()
            .map(|ac| ac.coord)
            .collect();
        assert!(
            lake.contains(&ChunkCoord::new(0, 1)),
            "standing above dry sand must stay on the lake-bed walk"
        );
    }
}
