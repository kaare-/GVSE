//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Cellular-automaton rules.
//!
//! One rule per tick sub-pass. Rules always read the world at the
//! start of the pass and write cells via [`World::set_cell`] so
//! chunk dirty rectangles stay coherent for whatever runs next.

use std::collections::HashMap;

use wk_material::MaterialId;

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

/// Pairwise horizontal water spreading between adjacent `Air` cells.
///
/// Each pair `(gx, gy)` ↔ `(gx+1, gy)` transfers **half the
/// difference** in saturation from the higher-sat cell to the lower.
/// Two properties matter:
///
/// - **Compute-then-apply.** Every pair reads the *pre-pass* state,
///   accumulates a signed delta per cell, and the deltas are applied
///   once at the end. That way the result is independent of iteration
///   order and pairs never double-count.
/// - **Air-Air only in v1.** Cross-material flow (Air → porous solid,
///   solid → solid Darcy) is a follow-up. Today the rule handles the
///   pure "puddle spreads across a bowl" and "lake surface levels
///   itself out" cases.
///
/// One pass moves ~1 cell per tick along a chain of cells (the
/// virtual-pipes propagation speed limit). Standing water on a flat
/// bed with no forcing settles to a flat surface over ~N ticks for a
/// puddle N cells wide.
pub fn apply_lateral_spill(world: &mut World) {
    let mut deltas: HashMap<(i32, i32), i32> = HashMap::new();

    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    for coord in coords {
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(left) = world.get_cell(gx, gy) else {
                    continue;
                };
                if left.material != MaterialId::Air {
                    continue;
                }
                let Some(right) = world.get_cell(gx + 1, gy) else {
                    continue;
                };
                if right.material != MaterialId::Air {
                    continue;
                }
                let l = left.sat.0 as i32;
                let r = right.sat.0 as i32;
                if l == r {
                    continue;
                }
                // Half the difference — the classic virtual-pipes
                // symmetric filter. Positive means "move right",
                // negative means "move left".
                let move_amt = (l - r) / 2;
                if move_amt == 0 {
                    continue;
                }
                *deltas.entry((gx, gy)).or_insert(0) -= move_amt;
                *deltas.entry((gx + 1, gy)).or_insert(0) += move_amt;
            }
        }
    }

    for ((gx, gy), delta) in deltas {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        let cap = water_capacity(cell.material) as i32;
        // Applied deltas from Air-Air pairs never cross zero or cap
        // for a *single* cell (each pair contributes at most half the
        // gap), but multiple neighbours can add up. Clamp defensively.
        let new_sat = (cell.sat.0 as i32 + delta).clamp(0, cap);
        world.set_cell(
            gx,
            gy,
            Cell {
                sat: Sat(new_sat as u8),
                ..cell
            },
        );
    }
}

/// Advance the sim by one tick.
///
/// Runs the fluid sub-passes in a fixed order:
///
/// 1. Gravity fall — every wet cell tries to move one cell downward.
/// 2. Lateral spill — pairs of horizontally-adjacent Air cells
///    equalize.
///
/// Together those two rules cover the "falling sand → puddle
/// spreads" behaviour. Future rules (density swap, porosity absorb
/// refinements, evaporation) will slot in ahead of the dirty-rect
/// clear once they land.
pub fn tick(world: &mut World) {
    apply_gravity_fall(world);
    apply_lateral_spill(world);
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

    // ------------ lateral spill ------------

    fn setup_air_row(width: i32) -> World {
        // Bedrock floor at y=0, everything above y=0 is Air, in one
        // 64-wide chunk. `width` is how many columns to make bedrock.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..width {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w
    }

    fn total_sat(world: &World, xs: std::ops::Range<i32>, y: i32) -> i64 {
        xs.map(|x| world.get_cell(x, y).map(|c| c.sat.0 as i64).unwrap_or(0))
            .sum()
    }

    #[test]
    fn spill_equalizes_isolated_pair() {
        // Put stone walls on the outside so the water cell only has
        // one Air neighbour — this isolates a single pair.
        let mut w = setup_air_row(64);
        w.set_cell(9, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(10, 5, Cell::water());
        // (11, 5) starts as default Air with sat 0.
        let start_mass = w.get_cell(10, 5).unwrap().sat.0 as i32;

        apply_lateral_spill(&mut w);

        let l = w.get_cell(10, 5).unwrap().sat.0 as i32;
        let r = w.get_cell(11, 5).unwrap().sat.0 as i32;
        // Half the difference moved: 255/2 = 127.
        assert_eq!(l, 255 - 127);
        assert_eq!(r, 127);
        assert_eq!(l + r, start_mass, "mass conserved");
    }

    #[test]
    fn spill_is_symmetric_across_a_single_pass() {
        // Water at gx=10 with dry air on both sides. Rule must feed
        // both neighbours equally — the pair is symmetric.
        let mut w = setup_air_row(64);
        w.set_cell(10, 5, Cell::water());
        apply_lateral_spill(&mut w);
        let left = w.get_cell(9, 5).unwrap().sat.0;
        let right = w.get_cell(11, 5).unwrap().sat.0;
        assert_eq!(left, right, "L/R must be equal");
        assert!(left > 0);
        // Mass conserved across the three cells.
        let total = w.get_cell(9, 5).unwrap().sat.0 as i32
            + w.get_cell(10, 5).unwrap().sat.0 as i32
            + w.get_cell(11, 5).unwrap().sat.0 as i32;
        assert_eq!(total, 255);
    }

    #[test]
    fn spill_conserves_mass_over_a_long_chain() {
        let mut w = setup_air_row(64);
        // Puddle at columns 20..25 (5 water cells), rest dry.
        for x in 20..25 {
            w.set_cell(x, 3, Cell::water());
        }
        let start_mass = total_sat(&w, 0..64, 3);
        for _ in 0..30 {
            apply_lateral_spill(&mut w);
        }
        let end_mass = total_sat(&w, 0..64, 3);
        assert_eq!(start_mass, end_mass, "mass must be preserved");
    }

    #[test]
    fn spill_stops_at_a_solid_wall() {
        let mut w = setup_air_row(64);
        // Put a Stone cell at (5, 5); water at (4, 5). Water must not
        // cross a non-Air cell.
        w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 5, Cell::water());
        apply_lateral_spill(&mut w);
        // (5, 5) is Stone — spill rule skips non-Air pairs entirely.
        // Water is still bound to (4, 5) and can only travel via
        // (3, 5), which had sat=0 originally, so half went left.
        assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Stone);
        // No sat leaked into the Stone cell.
        assert_eq!(w.get_cell(5, 5).unwrap().sat.0, 0);
        // Half the original water is now at (3, 5).
        assert_eq!(w.get_cell(3, 5).unwrap().sat.0, 127);
        assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 255 - 127);
    }

    #[test]
    fn spill_propagates_one_cell_per_tick() {
        // Full-water cell at x=32 in an otherwise dry row. After N
        // ticks, non-zero sat should reach at least x=32-N and x=32+N.
        let mut w = setup_air_row(64);
        w.set_cell(32, 3, Cell::water());

        for tick_i in 1..=4 {
            apply_lateral_spill(&mut w);
            // Both sides should have some water by tick_i.
            let left = w.get_cell(32 - tick_i, 3).unwrap();
            let right = w.get_cell(32 + tick_i, 3).unwrap();
            assert!(
                left.sat.0 > 0,
                "tick={tick_i} left cell x={} should have water",
                32 - tick_i
            );
            assert!(
                right.sat.0 > 0,
                "tick={tick_i} right cell x={} should have water",
                32 + tick_i
            );
            // The frontier is exactly `tick_i` cells: no water yet
            // one further out.
            if 32 - tick_i - 1 >= 0 {
                assert_eq!(
                    w.get_cell(32 - tick_i - 1, 3).unwrap().sat.0,
                    0,
                    "tick={tick_i} frontier at x={}",
                    32 - tick_i - 1
                );
            }
            if 32 + tick_i + 1 < 64 {
                assert_eq!(
                    w.get_cell(32 + tick_i + 1, 3).unwrap().sat.0,
                    0,
                    "tick={tick_i} frontier at x={}",
                    32 + tick_i + 1
                );
            }
        }
    }

    #[test]
    fn spill_crosses_chunk_boundary() {
        // Full water cell at gx=63 in chunk (0, 0); empty air at
        // gx=64 in chunk (1, 0). Stone wall at gx=62 so only the
        // cross-boundary pair contributes.
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..128 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(62, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(63, 5, Cell::water());
        apply_lateral_spill(&mut w);
        assert_eq!(w.get_cell(63, 5).unwrap().sat.0, 255 - 127);
        assert_eq!(w.get_cell(64, 5).unwrap().sat.0, 127);
        // Mass conserved across the boundary.
        assert_eq!(
            w.get_cell(63, 5).unwrap().sat.0 as i32
                + w.get_cell(64, 5).unwrap().sat.0 as i32,
            255
        );
    }

    #[test]
    fn gravity_only_drains_droplet_over_passes() {
        // Verified without lateral spill so we can assert exact
        // per-tick positions of a single droplet.
        let mut w = setup_column_world();
        w.set_cell(6, 5, Cell::water());

        apply_gravity_fall(&mut w);
        assert!(w.get_cell(6, 4).unwrap().sat.is_full());
        assert!(w.get_cell(6, 5).unwrap().sat.is_empty());
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(6, 3).unwrap().sat.is_full());
        apply_gravity_fall(&mut w);
        apply_gravity_fall(&mut w);
        assert!(
            w.get_cell(6, 1).unwrap().sat.is_full(),
            "water should be resting on bedrock"
        );
        apply_gravity_fall(&mut w);
        assert!(w.get_cell(6, 1).unwrap().sat.is_full());
        assert!(w.get_cell(6, 0).unwrap().sat.is_empty()); // bedrock sat stays 0
    }

    #[test]
    fn tick_runs_gravity_then_spill_and_conserves_mass() {
        // Full tick pass: droplet falls one row, then spreads
        // sideways to its Air neighbours. Total sat is unchanged.
        let mut w = setup_column_world();
        w.set_cell(30, 5, Cell::water());
        let start_mass = 255i64;

        tick(&mut w);
        let after_mass: i64 = (0..64i32)
            .flat_map(|x| (0..64i32).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i64))
            .sum();
        assert_eq!(after_mass, start_mass, "tick must conserve total sat");

        // The droplet should no longer be at (30, 5) — it fell to
        // (30, 4) and then spread across (29, 4)..(31, 4).
        assert!(w.get_cell(30, 5).unwrap().sat.is_empty());
        // At least the centre-column landing cell + its two
        // neighbours in the next row down should have some water.
        let landed_row_wet: i32 = (28..=32)
            .filter(|&x| w.get_cell(x, 4).map(|c| c.sat.0 > 0).unwrap_or(false))
            .count() as i32;
        assert!(landed_row_wet >= 3, "landed row should have spread wet cells");
    }
}
