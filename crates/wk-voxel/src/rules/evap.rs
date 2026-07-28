//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Standing-water evaporation into humidity.

use std::collections::HashMap;

use wk_material::MaterialId;

use crate::cell::{water_capacity, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Surface-evaporation parameters for [`apply_evaporation`].
#[derive(Debug, Clone, Copy)]
pub struct EvapConfig {
    /// Sat removed per qualifying tick from each surface cell.
    pub rate_per_tick: u8,
    /// A cell only evaporates when the cell above it is `Air` with
    /// `sat ≤ dry_above_max`. That keeps sub-surface lake cells from
    /// evaporating — only the top exposed water layer loses mass.
    pub dry_above_max: u8,
    /// Only run on ticks where `world.tick % period_ticks == 0`.
    /// Higher values slow the water→humidity pump so basins linger.
    pub period_ticks: u64,
}

impl Default for EvapConfig {
    fn default() -> Self {
        Self {
            rate_per_tick: 1,
            dry_above_max: 200,
            period_ticks: 1,
        }
    }
}

/// Bleed sat out of **standing** surface water (ocean film, puddles).
///
/// A cell qualifies when:
/// - It's `Air` with `sat > 0`.
/// - The cell directly above is `Air` with `sat ≤ cfg.dry_above_max`
///   OR the above chunk isn't loaded (open sky).
/// - It rests on solid ground **or** on wetter standing water below
///   (so mid-air rain / falling droplets are not re-evaporated before
///   they can reach the ground).
///
/// Compute-then-apply so evap is order-independent.
pub fn apply_evaporation(world: &mut World, cfg: &EvapConfig) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
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
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let deltas = collect_evap_deltas(world, cfg);
    apply_evap_deltas(world, deltas, Some(humidity));
}

/// True when wet Air is a free surface of a pool / ocean / land film,
/// not a suspended rain droplet with empty sky below it.
fn rests_on_evap_surface(world: &World, gx: i32, gy: i32, cfg: &EvapConfig) -> bool {
    match world.get_cell(gx, gy - 1) {
        None => false,
        Some(below) if below.material != MaterialId::Air => true,
        Some(below) => below.sat.0 > cfg.dry_above_max,
    }
}

fn collect_evap_deltas(world: &mut World, cfg: &EvapConfig) -> HashMap<(i32, i32), i32> {
    let mut deltas: HashMap<(i32, i32), i32> = HashMap::new();
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| c.has_wet_air)
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));
    for coord in coords {
        let mut still_wet = false;
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
                still_wet = true;
                let sky_above = match world.get_cell(gx, gy + 1) {
                    None => true, // above chunk absent → open sky
                    Some(above) => {
                        above.material == MaterialId::Air && above.sat.0 <= cfg.dry_above_max
                    }
                };
                if !sky_above || !rests_on_evap_surface(world, gx, gy, cfg) {
                    continue;
                }
                let mut rate = cfg.rate_per_tick as i32;
                // Orphaned crest film: no Air neighbour anywhere on the
                // surface (same-y or diagonal-down) → evaporate hard so
                // a single ridge pixel doesn't linger for hours.
                if is_orphan_surface_film(world, gx, gy) {
                    rate = (rate * 8).max(4);
                }
                *deltas.entry((gx, gy)).or_insert(0) -= rate;
            }
        }
        if !still_wet {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                chunk.has_wet_air = false;
            }
        }
    }
    deltas
}

/// True when a wet Air cell on solid has no Air neighbour on any of the
/// six surface directions — nothing lateral flow can couple it to.
fn is_orphan_surface_film(world: &World, gx: i32, gy: i32) -> bool {
    for (dx, dy) in [(-1_i32, 0), (1, 0), (-1, -1), (1, -1), (-1, 1), (1, 1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if matches!(
            world.get_cell(nx, ny),
            Some(c) if c.material == MaterialId::Air
        ) {
            return false;
        }
    }
    true
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
        let want_new = (cell.sat.0 as i32 + delta).clamp(0, cap);
        let want_removed = cell.sat.0 as i32 - want_new;
        if want_removed <= 0 {
            continue;
        }
        // Only lift what the atmosphere can still hold (per-tile cap).
        let accepted = if let Some(h) = humidity.as_deref_mut() {
            h.try_add(gx, gy, want_removed as f32).round() as i32
        } else {
            want_removed
        };
        if accepted <= 0 {
            continue;
        }
        let new_sat = (cell.sat.0 as i32 - accepted).clamp(0, cap);
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
