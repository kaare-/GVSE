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

use crate::cell::{is_grain, water_capacity, Cell, Sat};
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

/// One-cell-per-pass grain fall.
///
/// For every cell whose material is granular ([`is_grain`]) and whose
/// direct below neighbour is `Air`, swap the two `Cell`s. Whatever
/// water saturation the Air cell had comes up into the newly-emptied
/// upper cell, so a grain sinking through water displaces exactly the
/// water it walks through — mass is conserved and the water column
/// doesn't teleport.
///
/// Traversal is the same bottom-up sweep as [`apply_gravity_fall`],
/// with chunks ordered by ascending `cy` first so cross-chunk falls
/// land on already-processed cells and don't drop multiple rows per
/// pass.
///
/// V1 kept simple: grains fall through Air *any* saturation and stop
/// on anything else. Density-ordered stacking between grain species
/// (heavy sinks under light) and buoyancy interactions with less-
/// dense fluids are follow-up rules.
pub fn apply_grain_fall(world: &mut World) {
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
                if !is_grain(cur.material) {
                    continue;
                }
                let Some(below) = world.get_cell(gx, gy - 1) else {
                    continue;
                };
                if below.material != MaterialId::Air {
                    continue;
                }
                // Full-cell swap: grain moves down, air cell (with
                // whatever sat it held) rises. This is the density-
                // swap step from classic falling-sand simulations,
                // and it's the mechanism by which a dropped sand
                // grain sinks through a pond.
                world.set_cell(gx, gy, below);
                world.set_cell(gx, gy - 1, cur);
            }
        }
    }
}

/// Rain source parameters for [`apply_rain`].
#[derive(Debug, Clone, Copy)]
pub struct RainConfig {
    /// World-y row where droplets appear.
    pub top_y: i32,
    /// Inclusive `(x0, x1)` world-x range over which rain can fall.
    pub x_range: (i32, i32),
    /// Chance per column per tick of receiving a droplet.
    pub prob_per_col_per_tick: f32,
    /// Sat delta added per droplet (clamped so a cell can't exceed
    /// `u8::MAX`).
    pub droplet_sat: u8,
    /// Salt mixed into the per-column tick hash so callers can run
    /// multiple independent rain streams (mist vs storm) without
    /// them colliding.
    pub seed_salt: u64,
}

impl Default for RainConfig {
    fn default() -> Self {
        Self {
            top_y: 0,
            x_range: (0, 0),
            prob_per_col_per_tick: 0.02,
            droplet_sat: 64,
            seed_salt: 0xC10D,
        }
    }
}

/// Cheap deterministic 32-bit hash → f32 in `[0, 1)` — same mixer
/// used by [`crate::worldgen::continental_surface_y`].
fn hash_prob(seed: u64, gx: i32, tick_no: u64, salt: u64) -> f32 {
    let mut h = seed
        .wrapping_add(salt.wrapping_mul(0x9E37_79B9_7F4A_7C15))
        .wrapping_add(tick_no.wrapping_mul(0xBF58_476D_1CE4_E5B9))
        .wrapping_add(gx as u64);
    h ^= h.wrapping_shr(30);
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h.wrapping_shr(27);
    h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
    h ^= h.wrapping_shr(31);
    (h as u32 as f32) / (u32::MAX as f32 + 1.0)
}

/// Inject water into the sky row of `world` under a stochastic per-
/// column rule.
///
/// For each column `gx ∈ cfg.x_range`, roll a deterministic
/// pseudo-random probability seeded by `(world.seed, gx, world.tick,
/// cfg.seed_salt)`. When the roll passes `cfg.prob_per_col_per_tick`,
/// add `cfg.droplet_sat` to the cell at `(gx, cfg.top_y)` — provided
/// that cell is `Air`. Sat is saturated at `u8::MAX`.
///
/// Determinism: same world.seed + same tick + same config = same
/// droplet placements. That's what makes rain reproducible in
/// scenario tests.
pub fn apply_rain(world: &mut World, cfg: &RainConfig) {
    let (x0, x1) = cfg.x_range;
    if x0 > x1 {
        return;
    }
    let seed = world.seed.0;
    let tick_no = world.tick;
    for gx in x0..=x1 {
        let roll = hash_prob(seed, gx, tick_no, cfg.seed_salt);
        if roll >= cfg.prob_per_col_per_tick {
            continue;
        }
        let Some(cell) = world.get_cell(gx, cfg.top_y) else {
            continue;
        };
        if cell.material != MaterialId::Air {
            continue;
        }
        let new_sat = cell.sat.0.saturating_add(cfg.droplet_sat);
        world.set_cell(
            gx,
            cfg.top_y,
            Cell {
                sat: Sat(new_sat),
                ..cell
            },
        );
    }
}

/// Surface-evaporation parameters for [`apply_evaporation`].
#[derive(Debug, Clone, Copy)]
pub struct EvapConfig {
    /// Sat removed per tick from each qualifying cell.
    pub rate_per_tick: u8,
    /// A cell only evaporates when the cell above it is `Air` with
    /// `sat ≤ dry_above_max`. That keeps sub-surface lake cells from
    /// evaporating — only the top exposed water layer loses mass.
    pub dry_above_max: u8,
}

impl Default for EvapConfig {
    fn default() -> Self {
        Self {
            rate_per_tick: 1,
            dry_above_max: 200,
        }
    }
}

/// Bleed sat out of surface water cells.
///
/// A cell qualifies when:
/// - It's `Air` with `sat > 0`.
/// - The cell directly above is `Air` with `sat ≤ cfg.dry_above_max`
///   OR the above chunk isn't loaded (open sky).
///
/// Water leaves the world here — this is a boundary loss, not a
/// conservative transfer. When we add a humidity heatmap in a follow-
/// up PR the same helper will route the mass through it instead.
///
/// Compute-then-apply so evap is order-independent.
pub fn apply_evaporation(world: &mut World, cfg: &EvapConfig) {
    let deltas = collect_evap_deltas(world, cfg);
    apply_evap_deltas(world, deltas, None);
}

/// Mass-conservative variant of [`apply_evaporation`]. Instead of
/// deleting sat, the removed mass is deposited into the supplied
/// [`crate::humidity::Humidity`] heatmap at the cell's tile.
pub fn apply_evaporation_into_humidity(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &EvapConfig,
) {
    let deltas = collect_evap_deltas(world, cfg);
    apply_evap_deltas(world, deltas, Some(humidity));
}

fn collect_evap_deltas(world: &World, cfg: &EvapConfig) -> HashMap<(i32, i32), i32> {
    let mut deltas: HashMap<(i32, i32), i32> = HashMap::new();
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
                if cur.material != MaterialId::Air || cur.sat.is_empty() {
                    continue;
                }
                let sky_above = match world.get_cell(gx, gy + 1) {
                    None => true, // above chunk absent → open sky
                    Some(above) => {
                        above.material == MaterialId::Air && above.sat.0 <= cfg.dry_above_max
                    }
                };
                if !sky_above {
                    continue;
                }
                *deltas.entry((gx, gy)).or_insert(0) -= cfg.rate_per_tick as i32;
            }
        }
    }
    deltas
}

fn apply_evap_deltas(
    world: &mut World,
    deltas: HashMap<(i32, i32), i32>,
    mut humidity: Option<&mut crate::humidity::Humidity>,
) {
    for ((gx, gy), delta) in deltas {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        let cap = water_capacity(cell.material) as i32;
        let new_sat = (cell.sat.0 as i32 + delta).clamp(0, cap);
        let actually_removed = cell.sat.0 as i32 - new_sat;
        if actually_removed > 0 {
            if let Some(h) = humidity.as_deref_mut() {
                h.add(gx, gy, actually_removed as f32);
            }
        }
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

/// Karst dissolution parameters for [`apply_karst_dissolution`].
#[derive(Debug, Clone, Copy)]
pub struct KarstConfig {
    /// Base probability per tick that a Limestone cell dissolves
    /// *per* wet neighbour it has. Effective probability is
    /// `min(1, prob_per_wet_neighbour × wet_count)`.
    pub prob_per_wet_neighbour: f32,
    /// A neighbouring Air cell counts as "wet" once its sat is at
    /// or above this threshold. Prevents faint rain droplets from
    /// dissolving whole cliffs.
    pub min_wet_neighbour_sat: u8,
    /// Salt mixed into the per-cell tick hash so callers can run
    /// different karst regimes side-by-side.
    pub seed_salt: u64,
}

impl Default for KarstConfig {
    fn default() -> Self {
        // Tuned so a limestone body under constant water exposure
        // dissolves visibly over a few thousand ticks — game-scale,
        // not real karst-formation-scale.
        Self {
            prob_per_wet_neighbour: 0.001,
            min_wet_neighbour_sat: 200,
            seed_salt: 0xCAFE_D155_01F0_D000_u64,
        }
    }
}

/// Karst dissolution: Limestone cells with wet Air neighbours
/// probabilistically dissolve into Air, freeing their pore
/// saturation into the new Air cell.
///
/// Deterministic given `(world.seed, gx, gy, world.tick,
/// cfg.seed_salt)`.
///
/// Compute-then-apply so the sweep order doesn't affect the outcome.
pub fn apply_karst_dissolution(world: &mut World, cfg: &KarstConfig) {
    let mut converts: Vec<(i32, i32, Cell)> = Vec::new();
    let mut coords: Vec<ChunkCoord> = world.chunks.keys().copied().collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    let seed = world.seed.0;
    let tick_no = world.tick;

    for coord in coords {
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cur.material != MaterialId::Limestone {
                    continue;
                }
                // Count wet Air neighbours (4-connected).
                let mut wet = 0u32;
                for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
                    if let Some(n) = world.get_cell(gx + dx, gy + dy) {
                        if n.material == MaterialId::Air && n.sat.0 >= cfg.min_wet_neighbour_sat {
                            wet += 1;
                        }
                    }
                }
                if wet == 0 {
                    continue;
                }
                let effective_prob =
                    (cfg.prob_per_wet_neighbour * wet as f32).clamp(0.0, 1.0);
                // Bake gy into the hash so cells at different y
                // levels get independent rolls even though tick is
                // shared.
                let roll = hash_prob(
                    seed,
                    gx.wrapping_mul(73_856_093).wrapping_add(gy),
                    tick_no,
                    cfg.seed_salt,
                );
                if roll >= effective_prob {
                    continue;
                }
                // Dissolve — keep whatever pore water this cell held.
                converts.push((
                    gx,
                    gy,
                    Cell {
                        material: MaterialId::Air,
                        sat: cur.sat,
                        flags: cur.flags,
                        _pad: cur._pad,
                    },
                ));
            }
        }
    }
    for (gx, gy, cell) in converts {
        world.set_cell(gx, gy, cell);
    }
}

/// Advance the sim by one tick.
///
/// Runs the sub-passes in a fixed order:
///
/// 1. Gravity fall — every wet cell tries to move one cell downward.
/// 2. Lateral spill — pairs of horizontally-adjacent Air cells
///    equalise.
/// 3. Grain fall — granular materials sink into the Air cell below.
///
/// Gravity + spill first means water settles onto the current
/// terrain before grains drop through it; each tick the grain then
/// takes one step down (possibly through freshly-settled water) and
/// the next tick's water pass repacks around the new grain position.
///
/// Rain and evaporation are **opt-in**: callers wire
/// [`apply_rain`] and [`apply_evaporation`] into their per-frame
/// loop themselves. Scenario tests pass `tick(world)` alone and
/// stay deterministic without weather.
pub fn tick(world: &mut World) {
    apply_gravity_fall(world);
    apply_lateral_spill(world);
    apply_grain_fall(world);
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

    // ------------ grain fall ------------

    #[test]
    fn grain_falls_through_empty_air() {
        let mut w = setup_column_world();
        // Sand at y=5, everything below is empty Air, bedrock at y=0.
        w.set_cell(4, 5, Cell::solid(MaterialId::Sand));
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(4, 4).map(|c| c.material),
            Some(MaterialId::Sand)
        );
        assert_eq!(
            w.get_cell(4, 5).map(|c| c.material),
            Some(MaterialId::Air)
        );
    }

    #[test]
    fn grain_stops_on_competent_rock() {
        let mut w = setup_column_world();
        w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
        w.set_cell(4, 3, Cell::solid(MaterialId::Sand));
        apply_grain_fall(&mut w);
        // Below Stone is not Air → no swap.
        assert_eq!(w.get_cell(4, 3).unwrap().material, MaterialId::Sand);
        assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Stone);
    }

    #[test]
    fn grain_stops_on_another_grain() {
        let mut w = setup_column_world();
        w.set_cell(4, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(4, 2, Cell::solid(MaterialId::Gravel));
        apply_grain_fall(&mut w);
        // y=1 is Sand (not Air); Gravel at y=2 has nowhere to swap.
        // Sand at y=1 has bedrock at y=0 (not Air), also stays.
        assert_eq!(w.get_cell(4, 1).unwrap().material, MaterialId::Sand);
        assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Gravel);
    }

    #[test]
    fn grain_sinks_through_water_swap_conserves_mass() {
        // Water column at y=1..=4 (all Air with sat=full); sand at
        // y=5. After one grain pass, sand moves to y=4 and the water
        // that was at y=4 rises into y=5.
        let mut w = setup_column_world();
        for y in 1..=4 {
            w.set_cell(4, y, Cell::water());
        }
        w.set_cell(4, 5, Cell::solid(MaterialId::Sand));
        let start_water: i32 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i32)
            .sum();

        apply_grain_fall(&mut w);

        let end_water: i32 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i32)
            .sum();
        assert_eq!(end_water, start_water, "water sat is conserved by swap");
        assert_eq!(w.get_cell(4, 4).unwrap().material, MaterialId::Sand);
        // Sand carries its own sat (0) up... wait, the Air cell's
        // water rises. The newly-vacated cell at y=5 receives the
        // sat that was in the old below-cell (y=4 water full).
        assert_eq!(w.get_cell(4, 5).unwrap().material, MaterialId::Air);
        assert!(w.get_cell(4, 5).unwrap().sat.is_full());
    }

    #[test]
    fn grain_falls_across_chunk_boundary() {
        // Sand at gy=64 (chunk (0,1) local (7,0)); Air at gy=63
        // (chunk (0,0) local (7,63)). Sand should end at gy=63.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        w.set_cell(7, 64, Cell::solid(MaterialId::Sand));
        assert_eq!(
            w.get_cell(7, 64).unwrap().material,
            MaterialId::Sand
        );
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(7, 63).unwrap().material,
            MaterialId::Sand,
            "grain should have crossed the seam"
        );
        assert_eq!(
            w.get_cell(7, 64).unwrap().material,
            MaterialId::Air,
            "vacated cell above must be Air"
        );
    }

    #[test]
    fn grain_falls_one_cell_per_pass_through_empty_column() {
        // Multi-pass check that grain fall obeys the 1 cell / pass rule.
        let mut w = setup_column_world();
        w.set_cell(20, 10, Cell::solid(MaterialId::Sand));
        for expected in (1..=9).rev() {
            apply_grain_fall(&mut w);
            assert_eq!(
                w.get_cell(20, expected).map(|c| c.material),
                Some(MaterialId::Sand),
                "grain should be at y={expected}"
            );
            assert_eq!(
                w.get_cell(20, expected + 1).map(|c| c.material),
                Some(MaterialId::Air)
            );
        }
        // One more pass: bedrock below at y=0, no swap.
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(20, 1).unwrap().material,
            MaterialId::Sand
        );
    }

    // ------------ rain ------------

    fn setup_sky_row(y: i32) -> World {
        // Chunk (0, 0) with a full row of Air at `y`. Air is the
        // default cell, so we don't need to write anything — just
        // instantiate the chunk.
        let mut w = World::new(11);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        assert!((0..CHUNK_CELLS_H as i32).contains(&y));
        w
    }

    #[test]
    fn rain_is_deterministic_for_seed_and_tick() {
        let mut a = setup_sky_row(30);
        let mut b = setup_sky_row(30);
        let cfg = RainConfig {
            top_y: 30,
            x_range: (0, 63),
            prob_per_col_per_tick: 0.5,
            droplet_sat: 32,
            seed_salt: 0xF00,
        };
        apply_rain(&mut a, &cfg);
        apply_rain(&mut b, &cfg);
        for x in 0..64 {
            assert_eq!(
                a.get_cell(x, 30).map(|c| c.sat.0),
                b.get_cell(x, 30).map(|c| c.sat.0),
                "identical worlds must produce identical rain (x={x})"
            );
        }
    }

    #[test]
    fn rain_respects_x_range() {
        let mut w = setup_sky_row(30);
        let cfg = RainConfig {
            top_y: 30,
            x_range: (5, 20),
            prob_per_col_per_tick: 1.0, // always
            droplet_sat: 40,
            seed_salt: 1,
        };
        apply_rain(&mut w, &cfg);
        for x in 0..64 {
            let sat = w.get_cell(x, 30).unwrap().sat.0;
            if (5..=20).contains(&x) {
                assert!(sat > 0, "x={x} in range should have rain");
            } else {
                assert_eq!(sat, 0, "x={x} outside range should stay dry");
            }
        }
    }

    #[test]
    fn rain_droplet_saturates_at_full() {
        let mut w = setup_sky_row(30);
        w.set_cell(3, 30, Cell::water()); // already full
        let cfg = RainConfig {
            top_y: 30,
            x_range: (3, 3),
            prob_per_col_per_tick: 1.0,
            droplet_sat: 40,
            seed_salt: 2,
        };
        apply_rain(&mut w, &cfg);
        // Sat is clamped at u8::MAX — no overflow past FULL.
        assert_eq!(w.get_cell(3, 30).unwrap().sat.0, u8::MAX);
    }

    #[test]
    fn rain_skips_non_air_cells() {
        let mut w = setup_sky_row(30);
        // A stone cell at (10, 30) should not receive rain.
        w.set_cell(10, 30, Cell::solid(MaterialId::Stone));
        let cfg = RainConfig {
            top_y: 30,
            x_range: (10, 10),
            prob_per_col_per_tick: 1.0,
            droplet_sat: 40,
            seed_salt: 3,
        };
        apply_rain(&mut w, &cfg);
        assert_eq!(w.get_cell(10, 30).unwrap().sat.0, 0);
        assert_eq!(w.get_cell(10, 30).unwrap().material, MaterialId::Stone);
    }

    // ------------ evaporation ------------

    #[test]
    fn evap_removes_from_surface_water_only() {
        // Water column at gy=1..=5, dry air above. Only the topmost
        // wet cell (gy=5) has dry Air above and should lose sat.
        let mut w = setup_column_world();
        for y in 1..=5 {
            w.set_cell(4, y, Cell::water());
        }
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w, &cfg);
        for y in 1..=4 {
            assert!(
                w.get_cell(4, y).unwrap().sat.is_full(),
                "sub-surface cell y={y} should not evaporate"
            );
        }
        // Top wet cell lost a tiny bit.
        let top = w.get_cell(4, 5).unwrap().sat.0;
        assert!(top < u8::MAX);
        assert!(top >= u8::MAX - cfg.rate_per_tick);
    }

    #[test]
    fn evap_drains_a_droplet_to_zero_over_time() {
        // Small saturation should tick down to zero over many passes.
        let mut w = setup_column_world();
        let mut c = Cell::air();
        c.sat = Sat(20);
        w.set_cell(4, 5, c);
        let cfg = EvapConfig {
            rate_per_tick: 5,
            dry_above_max: 200,
        };
        for _ in 0..10 {
            apply_evaporation(&mut w, &cfg);
        }
        assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 0);
    }

    #[test]
    fn evap_leaves_dry_cells_alone() {
        let mut w = setup_column_world();
        // Air at y=5 with sat=0. No writes should occur.
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w, &cfg);
        assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 0);
    }

    #[test]
    fn evap_into_humidity_conserves_mass() {
        // Sat leaving cells lands as humidity mass. Sum should stay
        // constant across a single evap pass.
        use crate::humidity::Humidity;
        let mut w = setup_column_world();
        for y in 1..=5 {
            w.set_cell(4, y, Cell::water());
        }
        let mut h = Humidity::new(4);
        let cfg = EvapConfig {
            rate_per_tick: 3,
            dry_above_max: 200,
        };
        let cell_sat_before: i64 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i64)
            .sum();
        let hum_before = h.total_mass();

        apply_evaporation_into_humidity(&mut w, &mut h, &cfg);

        let cell_sat_after: i64 = (1..=5)
            .map(|y| w.get_cell(4, y).unwrap().sat.0 as i64)
            .sum();
        let hum_after = h.total_mass();
        assert!(cell_sat_after < cell_sat_before, "some water must have left");
        let removed = (cell_sat_before - cell_sat_after) as f32;
        let gained = hum_after - hum_before;
        assert!(
            (removed - gained).abs() < 1e-3,
            "removed sat ({removed}) should equal humidity gain ({gained})"
        );
    }

    #[test]
    fn evap_into_humidity_matches_bare_evap_cell_state() {
        // The cell-side effect must be identical to `apply_evaporation`
        // — humidity routing is purely an additive record of the
        // removed mass, not a different eligibility rule.
        use crate::humidity::Humidity;
        let mut w_bare = setup_column_world();
        let mut w_hum = setup_column_world();
        for y in 1..=5 {
            w_bare.set_cell(4, y, Cell::water());
            w_hum.set_cell(4, y, Cell::water());
        }
        let mut h = Humidity::new(4);
        let cfg = EvapConfig::default();
        apply_evaporation(&mut w_bare, &cfg);
        apply_evaporation_into_humidity(&mut w_hum, &mut h, &cfg);
        for y in 1..=5 {
            assert_eq!(
                w_bare.get_cell(4, y).map(|c| c.sat.0),
                w_hum.get_cell(4, y).map(|c| c.sat.0),
                "cell y={y} should evaporate identically"
            );
        }
    }

    // ------------ karst dissolution ------------

    fn setup_limestone_world() -> World {
        // Chunk (0, 0). Solid Limestone at y=1..=10, Bedrock at y=0,
        // Air above.
        let mut w = World::new(999);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..CHUNK_CELLS_W as i32 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=10 {
                w.set_cell(x, y, Cell::solid(MaterialId::Limestone));
            }
        }
        w
    }

    #[test]
    fn dry_limestone_never_dissolves() {
        let mut w = setup_limestone_world();
        // No wet neighbours anywhere — just dry Air above.
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 1,
        };
        for _ in 0..50 {
            apply_karst_dissolution(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        // No cell converted.
        for x in 0..(CHUNK_CELLS_W as i32) {
            for y in 1..=10 {
                assert_eq!(
                    w.get_cell(x, y).unwrap().material,
                    MaterialId::Limestone,
                    "dry limestone at ({x},{y}) must not dissolve"
                );
            }
        }
    }

    #[test]
    fn wet_limestone_eventually_dissolves() {
        let mut w = setup_limestone_world();
        // Put water full on top of a specific limestone cell.
        w.set_cell(10, 11, Cell::water());
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 42,
        };
        // With prob 1.0 the top-most limestone under the puddle
        // should convert on the first tick.
        apply_karst_dissolution(&mut w, &cfg);
        let after = w.get_cell(10, 10).unwrap();
        assert_eq!(after.material, MaterialId::Air, "wet limestone must dissolve");
    }

    #[test]
    fn karst_is_deterministic_for_seed_and_tick() {
        let mut a = setup_limestone_world();
        let mut b = setup_limestone_world();
        // Same puddle placement on both.
        for x in 5..=15 {
            a.set_cell(x, 11, Cell::water());
            b.set_cell(x, 11, Cell::water());
        }
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 0.5,
            min_wet_neighbour_sat: 200,
            seed_salt: 7,
        };
        for _ in 0..10 {
            apply_karst_dissolution(&mut a, &cfg);
            apply_karst_dissolution(&mut b, &cfg);
            a.tick = a.tick.wrapping_add(1);
            b.tick = b.tick.wrapping_add(1);
        }
        for x in 0..(CHUNK_CELLS_W as i32) {
            for y in 1..=10 {
                assert_eq!(
                    a.get_cell(x, y).map(|c| c.material),
                    b.get_cell(x, y).map(|c| c.material),
                    "seed-determinism failed at ({x},{y})"
                );
            }
        }
    }

    #[test]
    fn karst_ignores_non_limestone_solids() {
        // Stone cell adjacent to water — should never dissolve.
        let mut w = World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
        w.set_cell(5, 6, Cell::water());
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 3,
        };
        for _ in 0..20 {
            apply_karst_dissolution(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Stone);
    }

    #[test]
    fn karst_low_sat_neighbour_does_not_dissolve() {
        // Air cell above limestone has sat below threshold → no
        // dissolution.
        let mut w = setup_limestone_world();
        let mut wet_ish = Cell::air();
        wet_ish.sat = Sat(50); // below threshold 200
        w.set_cell(10, 11, wet_ish);
        let cfg = KarstConfig {
            prob_per_wet_neighbour: 1.0,
            min_wet_neighbour_sat: 200,
            seed_salt: 4,
        };
        for _ in 0..10 {
            apply_karst_dissolution(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(
            w.get_cell(10, 10).unwrap().material,
            MaterialId::Limestone,
            "damp-but-not-wet neighbour must not dissolve karst"
        );
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
