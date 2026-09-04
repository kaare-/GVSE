//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Karst dissolution — surface film on limestone, slower pore-water
//! dissolve of limestone and stone underground.

use serde::{Deserialize, Serialize};
use wk_material::{HydroOverrides, MaterialId};

use crate::cell::{water_capacity_cell, Cell};
#[cfg(test)]
use crate::cell::water_capacity_with;
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::parallel::map_chunk_coords_parallel;

use super::util::hash_prob;

fn default_karst_period_ticks() -> u64 {
    32
}

fn default_pore_scale() -> f32 {
    0.2
}

fn default_stone_scale() -> f32 {
    0.125
}

/// Karst dissolution parameters for [`apply_karst_dissolution`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct KarstConfig {
    /// Base probability per on-period tick that a soluble cell
    /// dissolves, multiplied by the contact weight (wet Air count on
    /// the surface path; pore / damp-cave contacts × [`Self::pore_scale`]
    /// underground).
    pub prob_per_wet_neighbour: f32,
    /// A neighbouring Air cell counts as "wet" for the *surface* path
    /// once its sat is at or above this threshold. Prevents faint rain
    /// droplets from dissolving whole cliffs. Underground wetness uses
    /// the same number as a fraction of the neighbour's capacity
    /// (`sat / cap >= min / 255`).
    pub min_wet_neighbour_sat: u8,
    /// Salt mixed into the per-cell tick hash so callers can run
    /// different karst regimes side-by-side.
    pub seed_salt: u64,
    /// Only run when `world.tick % period_ticks == 0`. Geology is slow —
    /// Super-Server demo paid ~1 ms/tick scanning limestone every frame.
    #[serde(default = "default_karst_period_ticks")]
    pub period_ticks: u64,
    /// Underground (own pore sat, wet solid neighbour, or damp cave
    /// Air) contact weight as a fraction of a surface wet-Air contact.
    /// Geology is slower than a waterfall on a cliff face.
    #[serde(default = "default_pore_scale")]
    pub pore_scale: f32,
    /// **Retired.** Silicate stone no longer dissolves: it fed the same
    /// dissolved load that precipitates as flowstone, so the sim was turning
    /// granite into carbonate. Stone widens mechanically under throughput
    /// instead (`mineral::widen_aperture`).
    ///
    /// Kept so existing presets and saves still deserialize. Has no effect.
    #[serde(default = "default_stone_scale")]
    pub stone_scale: f32,
}

impl Default for KarstConfig {
    fn default() -> Self {
        // Tuned so a limestone body under constant water exposure
        // dissolves visibly over a few thousand ticks — game-scale,
        // not real karst-formation-scale. Underground is a fraction
        // of that; stone is a fraction of limestone.
        Self {
            prob_per_wet_neighbour: 0.001,
            min_wet_neighbour_sat: 200,
            seed_salt: 0xCAFE_D155_01F0_D000_u64,
            period_ticks: default_karst_period_ticks(),
            pore_scale: default_pore_scale(),
            stone_scale: default_stone_scale(),
        }
    }
}

/// True when `cell.sat` is at least `min_sat / 255` of the material's
/// water capacity. Air is not a pore — callers handle it separately.
fn pore_is_wet(cell: Cell, hydro: &HydroOverrides, min_sat: u8) -> bool {
    if cell.material == MaterialId::Air || cell.sat.0 == 0 {
        return false;
    }
    let cap = water_capacity_cell(cell, hydro);
    if cap == 0 {
        return false;
    }
    (cell.sat.0 as u32) * 255 >= (min_sat as u32) * (cap as u32)
}

/// Open-sky drizzle must not count as a cave. A damp Air neighbour
/// only feeds the underground path when it sits under a solid roof.
fn air_is_roofed(world: &World, ax: i32, ay: i32) -> bool {
    match world.get_cell(ax, ay + 1) {
        Some(c) if c.material != MaterialId::Air => true,
        _ => false,
    }
}

/// Contact weight for one carbonate cell.
///
/// Surface wet-Air is unscaled; underground contacts (self-sat, wet solid
/// neighbour, roofed damp cave Air) are scaled by `pore_scale`, because geology
/// is slower than a waterfall on a cliff face.
fn contact_weight(world: &World, gx: i32, gy: i32, cur: Cell, cfg: &KarstConfig) -> f32 {
    let hydro = &world.hydro;
    let mut wet_air = 0u32;
    let mut wet_pore = 0u32;
    let mut damp_cave = 0u32;
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let nx = gx + dx;
        let ny = gy + dy;
        let Some(n) = world.get_cell(nx, ny) else {
            continue;
        };
        if n.material == MaterialId::Air {
            if n.sat.0 >= cfg.min_wet_neighbour_sat {
                wet_air += 1;
            } else if n.sat.0 > 0 && air_is_roofed(world, nx, ny) {
                // Dissolved pore water left in a roofed void — not a
                // surface waterfall, but enough to keep a conduit growing.
                damp_cave += 1;
            }
        } else if pore_is_wet(n, hydro, cfg.min_wet_neighbour_sat) {
            wet_pore += 1;
        }
    }
    let self_wet = u32::from(pore_is_wet(cur, hydro, cfg.min_wet_neighbour_sat));
    let underground = (wet_pore + damp_cave + self_wet) as f32 * cfg.pore_scale.max(0.0);
    if is_soluble(cur.material) {
        wet_air as f32 + underground
    } else {
        0.0
    }
}

/// Carbonate, and nothing else.
///
/// Silicate stone used to dissolve here at `stone_scale`, which fed the same
/// dissolved load that precipitates as flowstone — the sim was converting
/// granite into carbonate. Stone widens mechanically under throughput instead
/// (`mineral::widen_aperture`) and erodes to loose rock at the surface.
///
/// Flowstone is included: it is the same carbonate, so a sealed passage can
/// dissolve open again. It was missing from the old hardcoded pair.
fn is_soluble(material: MaterialId) -> bool {
    crate::mineral::is_soluble_rock(material)
}

/// Karst dissolution: soluble cells in contact with water become Air,
/// keeping whatever pore saturation they held.
///
/// Two contact paths:
///
/// - **Surface** — limestone only, 4-connected wet Air
///   (`sat >= min_wet_neighbour_sat`). Unchanged from the original
///   cliff-face rule.
/// - **Underground** — limestone and stone. A cell dissolves slowly
///   when it is itself near-saturated, when a porous neighbour is, or
///   when a *roofed* damp cave Air cell (sat > 0 but below the
///   surface threshold, solid immediately above) sits next to it.
///   Open-sky drizzle does not count. Stone is slower than limestone.
///   Dry stone next to a lake film does *not* count — that stays
///   mechanical erosion.
///
/// Deterministic given `(world.seed, gx, gy, world.tick,
/// cfg.seed_salt)`.
///
/// Compute-then-apply so the sweep order doesn't affect the outcome.
/// Chunk scans use rayon when [`crate::parallel::parallel_enabled`]
/// (frame-shell Phase 1).
///
/// Chunks without [`Chunk::has_soluble`] are skipped. The flag is
/// sticky on any soluble write (limestone, flowstone, sandstone,
/// conglomerate) and cleared here when a scan finds none left. Rain-
/// soaked sand / soil used to enter via `has_wet_pores` and pay a
/// full-chunk walk with nothing to dissolve — that is the soak-age
/// leftover. Buried saturated carbonate still wakes: worldgen writes
/// raise `has_soluble` the same way they raise `has_limestone`.
pub fn apply_karst_dissolution(world: &mut World, cfg: &KarstConfig) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_soluble)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    let seed = world.seed.0;
    let tick_no = world.tick;

    let per_chunk = map_chunk_coords_parallel(&coords, |coord| {
        let mut converts: Vec<(i32, i32, Cell)> = Vec::new();
        let mut still_lime = false;
        let mut still_soluble = false;
        for y in 0..CHUNK_CELLS_H {
            let gy = coord.cy * CHUNK_CELLS_H as i32 + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + x as i32;
                let Some(cur) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cur.material == MaterialId::Limestone {
                    still_lime = true;
                }
                if is_soluble(cur.material) {
                    still_soluble = true;
                } else {
                    continue;
                }
                let weight = contact_weight(world, gx, gy, cur, cfg);
                if weight <= 0.0 {
                    continue;
                }
                let effective_prob = (cfg.prob_per_wet_neighbour * weight).clamp(0.0, 1.0);
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
                //
                // A clastic rock leaves its grains behind: only the carbonate
                // matrix is soluble, so sandstone becomes sand, not a void.
                let becomes =
                    crate::mineral::loose_parent(cur.material).unwrap_or(MaterialId::Air);
                converts.push((
                    gx,
                    gy,
                    Cell {
                        material: becomes,
                        sat: cur.sat,
                        flags: cur.flags,
                        _pad: cur._pad,
                        pore: cur.pore,
                    },
                ));
            }
        }
        (coord, still_lime, still_soluble, converts)
    });

    let mut converts = Vec::new();
    for (coord, still_lime, still_soluble, local) in per_chunk {
        if !still_lime || !still_soluble {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                if !still_lime {
                    chunk.has_limestone = false;
                }
                if !still_soluble {
                    chunk.has_soluble = false;
                }
            }
        }
        converts.extend(local);
    }
    // Stable apply order (parallel scan may return per-chunk vecs already
    // in cy/cx order; sort cells so outcome matches serial history).
    converts.sort_by(|a, b| a.1.cmp(&b.1).then(a.0.cmp(&b.0)));
    for (gx, gy, cell) in converts {
        // Rock does not vanish: record what it was so the mineral becomes
        // dissolved load in the water now standing where it used to be. The
        // pre-dissolve cell matters — one already widened toward full aperture
        // has released most of its mineral incrementally.
        let was = world.get_cell(gx, gy);
        world.set_cell(gx, gy, cell);
        if let Some(prev) = was {
            crate::mineral::emit_from_dissolved_rock(world, gx, gy, prev);
        }
    }
}

#[cfg(test)]
mod unit {
    use super::*;
    use crate::cell::Sat;

    #[test]
    fn pore_wetness_is_relative_to_capacity() {
        let hydro = HydroOverrides::default();
        let cap = water_capacity_with(MaterialId::Limestone, &hydro);
        let mut cell = Cell::solid(MaterialId::Limestone);
        cell.sat = Sat(cap);
        assert!(pore_is_wet(cell, &hydro, 200));
        cell.sat = Sat((cap / 4).max(1));
        assert!(!pore_is_wet(cell, &hydro, 200));
        let air = Cell::water();
        assert!(!pore_is_wet(air, &hydro, 200));
    }
}
