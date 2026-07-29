//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Differential biological decay (Wave L): Muscle → Organic (fast),
//! Skin → Organic (mid), Bone → Sand (slow). Opt-in pass.

use wk_material::MaterialId;

use crate::cell::Cell;
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

use super::util::hash_prob;

/// Tunables for [`apply_biological_decay`].
#[derive(Debug, Clone, Copy)]
pub struct BiologicalDecayConfig {
    /// Per-tick decay probability for Muscle → Organic (~100-tick half-life).
    pub muscle_prob: f32,
    /// Per-tick decay probability for Skin → Organic (~300-tick half-life).
    pub skin_prob: f32,
    /// Per-tick decay probability for Bone → Sand (~5000-tick half-life).
    pub bone_prob: f32,
    /// Salt mixed into the per-cell tick hash.
    pub seed_salt: u64,
}

impl Default for BiologicalDecayConfig {
    fn default() -> Self {
        Self {
            // 1 - 0.5^(1/N) ≈ ln(2)/N
            muscle_prob: 0.00693,
            skin_prob: 0.00231,
            bone_prob: 0.000139,
            seed_salt: 0xDEC1_A1_B10_u64,
        }
    }
}

/// Convert Bone / Muscle / Skin cells toward their decay products.
///
/// Deterministic given `(world.seed, gx, gy, world.tick, cfg.seed_salt)`.
/// Chunks without [`crate::chunk::Chunk::has_biomaterial`] are skipped;
/// the flag is sticky on write and cleared here when a scan finds none left.
pub fn apply_biological_decay(world: &mut World, cfg: &BiologicalDecayConfig) {
    let mut converts: Vec<(i32, i32, Cell)> = Vec::new();
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_biomaterial)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    let seed = world.seed.0;
    let tick_no = world.tick;

    for coord in coords {
        let mut still_bio = false;
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                let (prob, into) = match cur.material {
                    MaterialId::Muscle => (cfg.muscle_prob, MaterialId::Organic),
                    MaterialId::Skin => (cfg.skin_prob, MaterialId::Organic),
                    MaterialId::Bone => (cfg.bone_prob, MaterialId::Sand),
                    _ => continue,
                };
                still_bio = true;
                if prob <= 0.0 {
                    continue;
                }
                let roll = hash_prob(
                    seed,
                    gx.wrapping_mul(73_856_093).wrapping_add(gy),
                    tick_no,
                    cfg.seed_salt,
                );
                if roll >= prob {
                    continue;
                }
                let mut next = Cell::solid(into);
                let cap = crate::cell::water_capacity_with(into, &world.hydro);
                next.sat.0 = if cap > 0 { cur.sat.0.min(cap) } else { 0 };
                next.flags = cur.flags;
                converts.push((gx, gy, next));
            }
        }
        if !still_bio {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                chunk.has_biomaterial = false;
            }
        }
    }

    for (gx, gy, cell) in converts {
        world.set_cell(gx, gy, cell);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chunk::ChunkCoord;

    fn stamp_bio(world: &mut World, gx: i32, gy: i32, mat: MaterialId) {
        world.ensure_chunk(ChunkCoord::new(0, 0));
        world.set_cell(gx, gy, Cell::solid(mat));
    }

    #[test]
    fn muscle_decays_to_organic_within_deterministic_budget() {
        let mut w = World::new(42);
        stamp_bio(&mut w, 4, 4, MaterialId::Muscle);
        let cfg = BiologicalDecayConfig {
            muscle_prob: 1.0, // force convert
            skin_prob: 0.0,
            bone_prob: 0.0,
            ..BiologicalDecayConfig::default()
        };
        apply_biological_decay(&mut w, &cfg);
        assert_eq!(
            w.get_cell(4, 4).map(|c| c.material),
            Some(MaterialId::Organic)
        );
    }

    #[test]
    fn bone_survives_many_ticks_under_default_decay() {
        let mut w = World::new(7);
        stamp_bio(&mut w, 3, 3, MaterialId::Bone);
        let cfg = BiologicalDecayConfig::default();
        for _ in 0..200 {
            apply_biological_decay(&mut w, &cfg);
            w.tick = w.tick.wrapping_add(1);
        }
        assert_eq!(
            w.get_cell(3, 3).map(|c| c.material),
            Some(MaterialId::Bone),
            "bone should usually survive a short window under default rate"
        );
    }
}
