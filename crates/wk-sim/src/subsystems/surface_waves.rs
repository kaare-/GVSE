//! Wind- and tide-driven free-surface dynamics.
//!
//! Replaces the old "fake waves" artifact (RainInject × LakeLevel beat
//! frequencies) with a 1-D shallow-water step on standing water:
//!
//! 1. Integrate horizontal velocity `Column::surface_u` from gravity
//!    restoring force (`−g ∂η/∂x`), wind stress, and linear drag.
//! 2. Advect water mass with that velocity (mass-conserving pairwise flux).
//! 3. Optionally nudge deep ocean columns toward `sea_level + tide_eta`
//!    (external shelf exchange, booked on `sea_inject_total`).
//!
//! Lake-level equalization still runs afterward for *shallow* ponds, but
//! is gated off deep wave-bearing water so it doesn't erase setup/seiche.

use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

/// Game-scaled gravity for free-surface waves (m/s²). Real 9.81 is unstable
/// at dt≈1s and 0.25 m columns; this yields basin seiches on O(10²) ticks.
const WAVE_G: f32 = 0.14;
/// Wind stress coefficient: `du += WIND_STRESS * wind_x / (depth + ε)`.
const WIND_STRESS: f32 = 2.8;
/// Linear drag on surface velocity per tick.
const LINEAR_DRAG: f32 = 0.10;
/// Hard cap on |u| (m/s) — keeps CFL comfortable with SAMPLE_WIDTH_M.
const MAX_U: f32 = 0.20;
/// Minimum depth (m) that carries momentum / wind stress.
const MIN_WAVE_DEPTH_M: f32 = 0.08;
/// Fraction of the tidal target depth applied per tick (smooth, not a snap).
const TIDE_BLEND: f32 = 0.05;
/// Mean depth (m) above which a wet run is "oceanic" for tide forcing.
const OCEAN_MEAN_DEPTH_M: f32 = 1.25;
/// Minimum flowable water (kg) to participate in the wave pass.
const MIN_WAVE_WATER_KG: i64 = 20;

#[derive(Clone, Copy)]
struct WaveCell {
    coord: i32,
    local: usize,
    world_x: i32,
    bed_y: f32,
    eta: f32,
    depth_m: f32,
    mass: i64,
    u: f32,
    oceanic: bool,
}

fn collect_cells(world: &World) -> Vec<WaveCell> {
    let mut cells = Vec::new();
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        let base = chunk.world_x_base();
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            let Some((eta, mass)) = col.flowable_water() else {
                continue;
            };
            if mass < MIN_WAVE_WATER_KG {
                continue;
            }
            let depth_m = mass as f32 / WATER_MASS_PER_METRE_DEPTH;
            if depth_m < MIN_WAVE_DEPTH_M {
                continue;
            }
            let bed_y = eta - depth_m;
            cells.push(WaveCell {
                coord,
                local,
                world_x: base + local as i32,
                bed_y,
                eta,
                depth_m,
                mass,
                u: col.surface_u,
                oceanic: bed_y < world.sea_level - 0.25 && depth_m >= OCEAN_MEAN_DEPTH_M * 0.5,
            });
        }
    }
    cells.sort_by_key(|c| c.world_x);
    cells
}

/// Integrate velocity, advect mass, apply tide. No-op when disabled.
pub fn run_surface_waves(world: &mut World, tick: u64) {
    if !world.surface_waves_enabled {
        return;
    }

    let cells = collect_cells(world);
    if cells.is_empty() {
        return;
    }

    let dx = SAMPLE_WIDTH_M.max(1e-3);
    let mut u_new = vec![0.0f32; cells.len()];

    for (i, cell) in cells.iter().enumerate() {
        let eta_l = if i > 0 {
            cells[i - 1].eta
        } else {
            cell.eta
        };
        let eta_r = if i + 1 < cells.len() {
            cells[i + 1].eta
        } else {
            cell.eta
        };
        // Only couple to contiguous wet neighbours (gap ⇒ reflecting wall).
        let left_ok = i > 0 && cells[i - 1].world_x + 1 == cell.world_x;
        let right_ok = i + 1 < cells.len() && cells[i + 1].world_x == cell.world_x + 1;
        let grad = match (left_ok, right_ok) {
            (true, true) => (eta_r - eta_l) / (2.0 * dx),
            (false, true) => (eta_r - cell.eta) / dx,
            (true, false) => (cell.eta - eta_l) / dx,
            (false, false) => 0.0,
        };

        let (wind_x, _) = world.wind_at_point(cell.world_x, cell.eta);
        let wind_accel = WIND_STRESS * wind_x / (cell.depth_m + 0.5);
        let mut u = cell.u - WAVE_G * grad + wind_accel;
        u *= 1.0 - LINEAR_DRAG;
        u_new[i] = u.clamp(-MAX_U, MAX_U);
    }

    // Pairwise mass fluxes from face velocities (conserving).
    let mut mass_delta = vec![0i64; cells.len()];
    for i in 0..cells.len().saturating_sub(1) {
        if cells[i + 1].world_x != cells[i].world_x + 1 {
            continue;
        }
        let u_face = 0.5 * (u_new[i] + u_new[i + 1]);
        let depth_face = 0.5 * (cells[i].depth_m + cells[i + 1].depth_m);
        let mut flux = (u_face * depth_face * WATER_MASS_PER_METRE_DEPTH).round() as i64;
        if flux > 0 {
            flux = flux.min(cells[i].mass / 4).max(0);
        } else if flux < 0 {
            flux = (-flux).min(cells[i + 1].mass / 4).max(0);
            flux = -flux;
        }
        mass_delta[i] -= flux;
        mass_delta[i + 1] += flux;
    }

    // Tide: pull oceanic free surfaces toward sea_level + η_tide.
    let tide = world.tide_eta_m(tick);
    let sea = world.sea_level;
    if world.tide_enabled {
        let target_eta = sea + tide;
        for (i, cell) in cells.iter().enumerate() {
            if !cell.oceanic {
                continue;
            }
            let target_depth = (target_eta - cell.bed_y).max(MIN_WAVE_DEPTH_M);
            let target_mass = (target_depth * WATER_MASS_PER_METRE_DEPTH) as i64;
            let step = ((target_mass - (cell.mass + mass_delta[i])) as f32 * TIDE_BLEND) as i64;
            mass_delta[i] += step;
        }
    }

    let water_before: i64 = cells.iter().map(|c| c.mass).sum();

    // Write back velocity + mass.
    for (i, cell) in cells.iter().enumerate() {
        let Some(chunk) = world.chunks.get_mut(&cell.coord) else {
            continue;
        };
        let col = &mut chunk.columns[cell.local];
        col.surface_u = u_new[i];
        let delta = mass_delta[i];
        if delta != 0 {
            col.adjust_top_water(delta, 0);
            col.settle_by_density(0);
            col.recompute_surface_y(chunk.bedrock_y);
            col.activity = Activity::HydrologyActive;
        }
        // Dry-out clears momentum.
        if col.flowable_water().map(|(_, m)| m).unwrap_or(0) < MIN_WAVE_WATER_KG {
            col.surface_u = 0.0;
        }
    }

    // Wave fluxes conserve; any net change is tidal shelf exchange.
    if world.tide_enabled {
        let mut water_after = 0i64;
        for cell in &cells {
            if let Some(chunk) = world.chunks.get(&cell.coord) {
                water_after += chunk.columns[cell.local]
                    .flowable_water()
                    .map(|(_, m)| m)
                    .unwrap_or(0);
            }
        }
        let net = water_after - water_before;
        if net != 0 {
            world.mass_audit.sea_inject_total += net;
        }
    }
}
