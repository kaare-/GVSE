//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Limestone dissolution.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::Cell;
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

use super::util::hash_prob;

/// Karst dissolution parameters for [`apply_karst_dissolution`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
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
///
/// Chunks without [`Chunk::has_limestone`] are skipped; the flag is
/// sticky on write and cleared here when a scan finds no limestone
/// left (empty sky / pure-stone slabs stay cheap).
pub fn apply_karst_dissolution(world: &mut World, cfg: &KarstConfig) {
    let mut converts: Vec<(i32, i32, Cell)> = Vec::new();
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_limestone)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    let seed = world.seed.0;
    let tick_no = world.tick;

    for coord in coords {
        let mut still_lime = false;
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
                still_lime = true;
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
        if !still_lime {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                chunk.has_limestone = false;
            }
        }
    }
    for (gx, gy, cell) in converts {
        world.set_cell(gx, gy, cell);
    }
}
