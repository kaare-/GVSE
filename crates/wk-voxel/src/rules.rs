//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cellular-automaton rules.
//!
//! One rule per tick sub-pass. Rules always read the world at the
//! start of the pass and write cells via [`World::set_cell`] so
//! chunk dirty rectangles stay coherent for whatever runs next.

use crate::cell::{water_capacity, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Bottom-up single-step gravity fall for water saturation.
///
/// For every cell that holds any water (`sat > 0`), try to move as
/// much of it as possible into the cell directly below, up to that
/// cell's water capacity. Behaviour:
///
/// - Traversal is **bottom-up** within each chunk (`y = 0 → H-1`),
///   matching Petri Purho's Noita rule: a column of water on solid
///   ground stays put (each cell's below neighbour is already full or
///   impermeable), while a lone droplet migrates down exactly one
///   cell per invocation (each cell's below neighbour was empty and
///   is now wet, but its own iteration already passed).
/// - **Cross-chunk**: when the below cell falls in another chunk, we
///   dispatch through [`World::get_cell`] / [`World::set_cell`] so
///   the neighbouring chunk materialises lazily.
/// - **Missing chunk**: below cell in a never-loaded chunk is treated
///   as impermeable (no fall). Follow-up worldgen rules can decide
///   whether to spawn a below-world chunk on demand.
///
/// This is intentionally the simplest possible fall model — one cell
/// per invocation, no lateral spread, no density swap. Free-fall
/// acceleration and the density-swap rule are follow-up PRs.
pub fn apply_gravity_fall(world: &mut World) {
    // Chunks are iterated bottom-up (`cy` ascending). Combined with
    // the bottom-up sweep inside each chunk this means every cell is
    // visited from world-bottom to world-top exactly once per pass —
    // water that crosses into a lower chunk from above lands on a
    // cell that has already been processed and therefore rests for
    // the remainder of this tick, giving the expected one-cell-per-
    // tick fall speed across chunk seams.
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    for coord in coords {
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cur.sat.is_empty() {
                    continue;
                }
                // Water can't leave a Water/Ice/Snow *material* cell in
                // this v1 rule — those materials are impermeable to the
                // gravity pass. `water_capacity` returns 0 for them, but
                // that's about the *receiving* end. The source-side
                // guard would let the fall rule empty a lake by moving
                // its sat down forever; we don't want that.
                if water_capacity(cur.material) == 0 {
                    continue;
                }
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                let cap_below = water_capacity(below.material);
                if cap_below == 0 {
                    continue;
                }
                let free_below = cap_below.saturating_sub(below.sat.0);
                if free_below == 0 {
                    continue;
                }
                let move_amt = cur.sat.0.min(free_below);
                if move_amt == 0 {
                    continue;
                }
                let new_cur = Cell {
                    sat: Sat(cur.sat.0 - move_amt),
                    ..cur
                };
                let new_below = Cell {
                    sat: Sat(below.sat.0 + move_amt),
                    ..below
                };
                world.set_cell(gx, gy, new_cur);
                world.set_cell(gx, gy - 1, new_below);
            }
        }
    }
}

/// Advance the sim by one tick.
///
/// Currently runs a single gravity-fall pass and bumps counters.
/// Future rules (lateral spill, density swap, porosity absorb) slot
/// in ahead of the dirty-rectangle clear once they land.
pub fn tick(world: &mut World) {
    apply_gravity_fall(world);
    world.tick = world.tick.wrapping_add(1);
    for chunk in world.chunks.values_mut() {
        chunk.tick = chunk.tick.wrapping_add(1);
        // Clearing the dirty rectangle *after* rules run means the
        // next tick starts from a clean baseline; any cell writes
        // during the rule pass have already extended the rect for
        // this tick and will do so again on the next.
        chunk.clear_dirty();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;
    use wk_material::MaterialId;

    fn setup_column_world() -> World {
        // One chunk. Row y=0 is a solid Bedrock floor; every other
        // cell is Air (empty).
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..(CHUNK_CELLS_W as i32) {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    #[test]
    fn droplet_falls_one_cell_per_pass() {
        let mut w = setup_column_world();
        w.set_cell(4, 10, Cell::water());
        assert!(w.get_cell(4, 10).unwrap().sat.is_full());
        assert!(w.get_cell(4, 9).unwrap().sat.is_empty());

        apply_gravity_fall(&mut w);
        assert!(w.get_cell(4, 10).unwrap().sat.is_empty());
        assert!(w.get_cell(4, 9).unwrap().sat.is_full());
    }

    #[test]
    fn droplet_stops_on_bedrock() {
        let mut w = setup_column_world();
        w.set_cell(2, 1, Cell::water());
        apply_gravity_fall(&mut w);
        // Bedrock capacity is 0 — no move.
        assert!(w.get_cell(2, 1).unwrap().sat.is_full());
        assert!(w
            .get_cell(2, 0)
            .unwrap()
            .sat
            .is_empty());
    }

    #[test]
    fn resting_column_does_not_compress() {
        let mut w = setup_column_world();
        // Water in y=1..4 (four cells), solid bedrock at y=0.
        for y in 1..=4 {
            w.set_cell(2, y, Cell::water());
        }
        apply_gravity_fall(&mut w);
        // All four cells should still be full — each already sits on
        // full water or bedrock and has nowhere to go.
        for y in 1..=4 {
            assert!(
                w.get_cell(2, y).unwrap().sat.is_full(),
                "y={y} lost water"
            );
        }
    }

    #[test]
    fn water_saturates_porous_solid_up_to_capacity() {
        let mut w = setup_column_world();
        // Sand cell sits above bedrock at y=1; water above at y=2.
        w.set_cell(3, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(3, 2, Cell::water());

        // One pass: as much as fits transfers into the sand up to its
        // porosity capacity.
        apply_gravity_fall(&mut w);
        let sand = w.get_cell(3, 1).unwrap();
        let above = w.get_cell(3, 2).unwrap();
        let sand_cap = water_capacity(MaterialId::Sand);
        assert_eq!(sand.sat.0, sand_cap);
        assert_eq!(above.sat.0, u8::MAX - sand_cap);

        // A second pass: sand is at capacity → no more water moves in.
        apply_gravity_fall(&mut w);
        let sand2 = w.get_cell(3, 1).unwrap();
        let above2 = w.get_cell(3, 2).unwrap();
        assert_eq!(sand2.sat.0, sand_cap);
        assert_eq!(above2.sat.0, u8::MAX - sand_cap);
    }

    #[test]
    fn does_not_leak_through_stone() {
        // Stone porosity is small but > 0. Ensure the pass never over-fills
        // and no water disappears.
        let mut w = World::new(2);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..(CHUNK_CELLS_W as i32) {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 2, Cell::water());
        let cap = water_capacity(MaterialId::Stone);
        let start_mass: i32 =
            w.get_cell(5, 2).unwrap().sat.0 as i32 + w.get_cell(5, 1).unwrap().sat.0 as i32;

        apply_gravity_fall(&mut w);

        let stone = w.get_cell(5, 1).unwrap();
        let above = w.get_cell(5, 2).unwrap();
        assert_eq!(stone.sat.0, cap);
        assert_eq!(above.sat.0 as i32 + stone.sat.0 as i32, start_mass);
    }

    #[test]
    fn droplet_falls_across_chunk_boundary() {
        // Chunk (0, 1) at y=64..127; chunk (0, 0) at y=0..63.
        // Drop a water cell at gy=64 (bottom row of chunk (0,1)),
        // expect it in gy=63 (top row of chunk (0,0)) after one pass.
        let mut w = World::new(3);
        // Instantiate both chunks so `get_cell` returns Some for both
        // sides of the seam.
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.set_cell(7, 64, Cell::water());
        assert!(w.get_cell(7, 64).unwrap().sat.is_full());
        assert!(w.get_cell(7, 63).unwrap().sat.is_empty());

        apply_gravity_fall(&mut w);

        assert!(w.get_cell(7, 64).unwrap().sat.is_empty());
        assert!(w.get_cell(7, 63).unwrap().sat.is_full());
    }

    #[test]
    fn missing_below_chunk_stops_fall() {
        // Chunk (0, 0) exists; chunk (0, -1) does not. A water cell
        // at gy=0 (bottom of chunk 0,0) has no below chunk — it must
        // stay put rather than pour into the void.
        let mut w = World::new(4);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(1, 0, Cell::water());
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(1, 0).unwrap().sat.is_full());
        // Below chunk still doesn't exist.
        assert_eq!(w.get_cell(1, -1), None);
    }

    #[test]
    fn tick_advances_and_drains_droplet_over_ticks() {
        let mut w = setup_column_world();
        // Water at y=5, everything below is air, bedrock at y=0.
        w.set_cell(6, 5, Cell::water());

        // After N ticks (N < 5), water should be N rows lower.
        tick(&mut w);
        assert!(w.get_cell(6, 4).unwrap().sat.is_full());
        assert!(w.get_cell(6, 5).unwrap().sat.is_empty());
        tick(&mut w);
        assert!(w.get_cell(6, 3).unwrap().sat.is_full());
        // After enough ticks to reach the bedrock (y=1 is one above),
        // water rests on top of bedrock at y=1.
        tick(&mut w);
        tick(&mut w);
        assert!(
            w.get_cell(6, 1).unwrap().sat.is_full(),
            "water should be resting on bedrock"
        );
        tick(&mut w);
        // Another tick doesn't move it — bedrock is impermeable.
        assert!(w.get_cell(6, 1).unwrap().sat.is_full());
        assert!(w.get_cell(6, 0).unwrap().sat.is_empty()); // bedrock sat stays 0
    }
}
