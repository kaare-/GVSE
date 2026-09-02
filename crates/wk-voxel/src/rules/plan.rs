//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Active-region planning helpers for standalone rule calls.

use crate::active::{plan_active, ActiveChunk};
use crate::chunk::{ChunkCoord, STANDING_AIR_SAT};
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

/// Loaded chunks that can host a confined rise.
///
/// Rain-film sky (`has_wet_air` only) and mid-ocean water with no solid
/// are skipped — those cells already reject per-cell (`open_air_both_sides`
/// / uncased hole) but walking every rainy 64×64 was the leftover
/// period-16 cost. Bootstrap (no flags set yet) falls back to
/// [`regions_wet_air_loaded`].
pub(crate) fn regions_confined_loaded(world: &World) -> Vec<ActiveChunk> {
    let ready = world
        .chunks
        .values()
        .any(|c| c.has_solid || c.has_standing_air);
    if !ready {
        return regions_wet_air_loaded(world);
    }
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_solid && (c.has_standing_air || c.has_wet_pores))
        .map(|(&coord, _)| coord)
        .collect();
    if coords.is_empty() && !world.chunks.is_empty() {
        return regions_wet_air_loaded(world);
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

/// Loaded chunks that may have a standing-water bed or a wet pore front.
///
/// Rain-film sky (sat &lt; 160, no pores) is skipped. Bootstrap falls
/// back to [`regions_wet_loaded`].
pub(crate) fn regions_lake_bed_loaded(world: &World) -> Vec<ActiveChunk> {
    let ready = world
        .chunks
        .values()
        .any(|c| c.has_standing_air || c.has_wet_pores);
    if !ready {
        return regions_wet_loaded(world);
    }
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_standing_air || c.has_wet_pores)
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

/// Refresh sticky standing-air / solid flags after a full-chunk water scan.
pub(crate) fn refresh_chunk_water_flags(world: &mut World, coord: ChunkCoord) {
    let Some(chunk) = world.chunks.get(&coord) else {
        return;
    };
    let mut standing = false;
    let mut solid = false;
    for cell in &chunk.cells {
        if cell.material != wk_material::MaterialId::Air {
            solid = true;
        } else if cell.sat.0 >= STANDING_AIR_SAT {
            standing = true;
        }
        if standing && solid {
            break;
        }
    }
    if let Some(chunk) = world.chunks.get_mut(&coord) {
        chunk.has_standing_air = standing;
        chunk.has_solid = solid;
    }
}

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
        // cx=1: standing water next to rock — confined + lake-bed.
        w.set_cell(65, 1, Cell::solid(MaterialId::Bedrock));
        w.set_cell(66, 2, Cell::water());
        // cx=2: mid-ocean water, no solid — lake-bed only.
        w.set_cell(130, 2, Cell::water());

        let confined: Vec<_> = regions_confined_loaded(&w)
            .into_iter()
            .map(|ac| ac.coord)
            .collect();
        assert_eq!(confined, vec![ChunkCoord::new(1, 0)]);

        let lake: Vec<_> = regions_lake_bed_loaded(&w)
            .into_iter()
            .map(|ac| ac.coord)
            .collect();
        assert_eq!(lake, vec![ChunkCoord::new(1, 0), ChunkCoord::new(2, 0)]);
    }
}
