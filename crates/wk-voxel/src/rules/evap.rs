//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Standing-water evaporation into humidity.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::cell::{water_capacity_cell, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W, STANDING_AIR_SAT};
use crate::grid::World;
use crate::parallel::map_chunk_coords_parallel;

/// Surface-evaporation parameters for [`apply_evaporation`].
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
/// Rain-film sky chunks (`has_wet_air` only) are not scanned — airborne
/// droplets already fail `rests`. Chunk scans use rayon when
/// [`crate::parallel::parallel_enabled`] (frame-shell Phase 1).
pub fn apply_evaporation(world: &mut World, cfg: &EvapConfig) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    let (deltas, occupancy) = collect_evap_deltas(world, cfg, None);
    apply_evap_occupancy_flags(world, &occupancy);
    apply_evap_deltas(world, deltas, None, None);
}

/// Mass-conservative variant of [`apply_evaporation`]. Instead of
/// deleting sat, the removed mass is deposited into the supplied
/// [`crate::humidity::Humidity`] heatmap at the cell's tile.
pub fn apply_evaporation_into_humidity(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &EvapConfig,
) {
    apply_evaporation_into_humidity_climate(world, humidity, cfg, None, 0.0);
}

/// [`apply_evaporation_into_humidity`] with temperature + wind.
///
/// Same wet-chunk scan (CPU-safe). Rate scales with warm air, wind,
/// and local humidity deficit so a cold still night barely pumps and
/// a hot breeze dries films. `wind_speed` is |tiles/tick|.
pub fn apply_evaporation_into_humidity_climate(
    world: &mut World,
    humidity: &mut crate::humidity::Humidity,
    cfg: &EvapConfig,
    temp: Option<&crate::temperature::Temperature>,
    wind_speed: f32,
) {
    let period = cfg.period_ticks.max(1);
    if world.tick % period != 0 {
        return;
    }
    // Skip the ocean-surface scan once the sky is already over budget —
    // otherwise a long soak keeps walking every wet chunk for no gain.
    if humidity.atmosphere_overfull() {
        return;
    }
    let (deltas, occupancy) = {
        let climate = temp.map(|t| (t, wind_speed.abs(), humidity as &crate::humidity::Humidity));
        collect_evap_deltas(world, cfg, climate)
    };
    apply_evap_occupancy_flags(world, &occupancy);
    apply_evap_deltas(world, deltas, Some(humidity), temp);
}

/// Reference `rate_per_tick` is ~18 °C, light breeze, dry air.
pub(crate) fn evap_climate_rate(base: i32, temp_c: f32, wind_abs: f32, humidity_mass: f32) -> i32 {
    if base <= 0 {
        return 0;
    }
    let sat = crate::humidity::Humidity::saturation_mass_at_temp(temp_c);
    let t_scale = (crate::humidity::Humidity::sat_vapor_pressure_hpa(temp_c)
        / crate::humidity::Humidity::sat_vapor_pressure_hpa(18.0))
    .clamp(0.08, 4.0);
    let w_scale = (0.62 + wind_abs * 10.0).clamp(0.50, 2.0);
    let rh = (humidity_mass / sat.max(1.0)).clamp(0.0, 1.0);
    let deficit = (1.0 - rh).clamp(0.20, 1.0);
    let cap = (base * 4).max(1);
    ((base as f32) * t_scale * w_scale * deficit)
        .round()
        .clamp(0.0, cap as f32) as i32
}

/// Per-chunk scan result: local sat deltas + wet / standing occupancy.
fn collect_evap_deltas(
    world: &World,
    cfg: &EvapConfig,
    climate: Option<(
        &crate::temperature::Temperature,
        f32,
        &crate::humidity::Humidity,
    )>,
) -> (
    HashMap<(i32, i32), i32>,
    Vec<(ChunkCoord, bool, bool, u8, u8)>,
) {
    // Surface films only. Rain-film sky (`has_wet_air` without solid or
    // standing water) cannot rest on a bed, so the per-cell `rests`
    // check always fails — walking those 64×64s was the leftover evap
    // cost on a tall box. Bootstrap (no flags yet) keeps wet-air.
    let ready = world
        .chunks
        .values()
        .any(|c| c.has_standing_air || (c.has_wet_air && c.has_solid));
    let mut coords: Vec<ChunkCoord> = world
        .chunks
        .iter()
        .filter(|(_, c)| {
            if ready {
                c.has_standing_air || (c.has_wet_air && c.has_solid)
            } else {
                c.has_wet_air
            }
        })
        .map(|(&coord, _)| coord)
        .collect();
    coords.sort_by(|a, b| a.cy.cmp(&b.cy).then(a.cx.cmp(&b.cx)));

    let per_chunk = map_chunk_coords_parallel(&coords, |coord| {
        let Some(chunk) = world.chunks.get(&coord) else {
            return (coord, false, false, 255, 0, Vec::new());
        };
        let above = world.chunks.get(&ChunkCoord::new(coord.cx, coord.cy + 1));
        let below = world.chunks.get(&ChunkCoord::new(coord.cx, coord.cy - 1));
        let base_gx = coord.cx * CHUNK_CELLS_W as i32;
        let base_gy = coord.cy * CHUNK_CELLS_H as i32;
        let mut local: Vec<((i32, i32), i32)> = Vec::new();
        let mut still_wet = false;
        let mut still_standing = false;
        let mut stand_lo = 255u8;
        let mut stand_hi = 0u8;
        for y in 0..CHUNK_CELLS_H {
            let gy = base_gy + y as i32;
            for x in 0..CHUNK_CELLS_W {
                let gx = world.wrap_x(base_gx + x as i32);
                let cur = chunk.get(x, y);
                if cur.material != MaterialId::Air || cur.sat.is_empty() {
                    continue;
                }
                still_wet = true;
                if cur.sat.0 >= STANDING_AIR_SAT {
                    still_standing = true;
                    let yu = y as u8;
                    stand_lo = stand_lo.min(yu);
                    stand_hi = stand_hi.max(yu);
                }
                let sky_above = if y + 1 < CHUNK_CELLS_H {
                    let above_c = chunk.get(x, y + 1);
                    above_c.material == MaterialId::Air && above_c.sat.0 <= cfg.dry_above_max
                } else {
                    match above {
                        None => true, // above chunk absent → open sky
                        Some(c) => {
                            let a = c.get(x, 0);
                            a.material == MaterialId::Air && a.sat.0 <= cfg.dry_above_max
                        }
                    }
                };
                let rests = if y > 0 {
                    let below_c = chunk.get(x, y - 1);
                    below_c.material != MaterialId::Air || below_c.sat.0 > cfg.dry_above_max
                } else {
                    match below {
                        None => false,
                        Some(c) => {
                            let b = c.get(x, CHUNK_CELLS_H - 1);
                            b.material != MaterialId::Air || b.sat.0 > cfg.dry_above_max
                        }
                    }
                };
                if !sky_above || !rests {
                    continue;
                }
                let mut rate = cfg.rate_per_tick as i32;
                if let Some((temp, wind_abs, hum)) = climate {
                    rate = evap_climate_rate(
                        rate,
                        temp.at_cell(gx, gy),
                        wind_abs,
                        hum.at_cell(gx, gy),
                    );
                }
                // Orphaned crest film: no Air neighbour anywhere on the
                // surface (same-y or diagonal-down) → evaporate hard so
                // a single ridge pixel doesn't linger for hours.
                if is_orphan_surface_film(world, gx, gy) {
                    rate = (rate * 8).max(4);
                }
                if rate <= 0 {
                    continue;
                }
                local.push(((gx, gy), -rate));
            }
        }
        (coord, still_wet, still_standing, stand_lo, stand_hi, local)
    });

    let mut deltas: HashMap<(i32, i32), i32> = HashMap::new();
    let mut occupancy = Vec::new();
    for (coord, still_wet, still_standing, stand_lo, stand_hi, local) in per_chunk {
        occupancy.push((coord, still_wet, still_standing, stand_lo, stand_hi));
        for (key, delta) in local {
            *deltas.entry(key).or_insert(0) += delta;
        }
    }
    (deltas, occupancy)
}

fn apply_evap_occupancy_flags(
    world: &mut World,
    occupancy: &[(ChunkCoord, bool, bool, u8, u8)],
) {
    for &(coord, still_wet, still_standing, stand_lo, stand_hi) in occupancy {
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            if !still_wet {
                chunk.has_wet_air = false;
            }
            if still_standing {
                chunk.has_standing_air = true;
                chunk.standing_air_y0 = stand_lo;
                chunk.standing_air_y1 = stand_hi;
            } else {
                chunk.clear_standing_air();
            }
        }
    }
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
    temp: Option<&crate::temperature::Temperature>,
) {
    for ((gx, gy), delta) in deltas {
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        let cap = water_capacity_cell(cell, &world.hydro) as i32;
        let want_new = (cell.sat.0 as i32 + delta).clamp(0, cap);
        let want_removed = cell.sat.0 as i32 - want_new;
        if want_removed <= 0 {
            continue;
        }
        // Only lift what the atmosphere can still hold (per-tile cap).
        let accepted = if let Some(h) = humidity.as_deref_mut() {
            if h.column_near_saturated(gx, gy) {
                0
            } else {
                match temp {
                    Some(t) => h
                        .try_add_at_temp(gx, gy, want_removed as f32, t.at_cell(gx, gy))
                        .round() as i32,
                    None => h.try_add(gx, gy, want_removed as f32).round() as i32,
                }
            }
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
        // Vapour leaves its mineral behind. This is what builds a mound at an
        // evaporating spring outlet: water arrives carrying load, departs
        // without it. Concentration first, then the whole load once dry.
        crate::mineral::precipitate_at(world, gx, gy);
        if new_sat == 0 {
            crate::mineral::precipitate_dry_cell(world, gx, gy);
        }
    }
}
