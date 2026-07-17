//! Wind- and tide-driven free-surface dynamics.
//!
//! Replaces the old "fake waves" artifact (RainInject × LakeLevel beat
//! frequencies) with a 1-D shallow-water step on standing water:
//!
//! 1. Integrate horizontal velocity `Column::surface_u` from gravity
//!    restoring force (`−g ∂η/∂x`), wind stress, and linear drag.
//! 2. Advect water mass with that velocity (mass-conserving pairwise flux).
//! 3. Soften single-column "comb" teeth with a conserving neighbour blend.
//! 4. Optionally nudge deep ocean columns toward `sea_level + tide_eta`.
//!
//! **Performance / amplitude:** flux uses a capped *active layer* depth
//! (not the full abyssal water column). Using full ocean depth made every
//! tick shove thousands of kg between neighbours, which looked like huge
//! comb teeth and forced `settle_by_density` across the whole ring.

use wk_material::{CHUNK_W, SAMPLE_WIDTH_M};
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

/// Game-scaled gravity for free-surface waves (m/s²).
const WAVE_G: f32 = 0.04;
/// Wind stress coefficient: `du += WIND_STRESS * wind_x / (depth_eff + ε)`.
const WIND_STRESS: f32 = 1.4;
/// Linear drag on surface velocity per wave step.
const LINEAR_DRAG: f32 = 0.14;
/// Hard cap on |u| (m/s).
const MAX_U: f32 = 0.06;
/// Only the top of the water column participates in wave flux. Full-depth
/// oceans (100–400 m) otherwise produce absurd η spikes.
const WAVE_ACTIVE_DEPTH_M: f32 = 3.0;
/// Clamp |η − still| after the step (metres of free-surface departure).
const MAX_ETA_AMP_M: f32 = 0.55;
/// Minimum depth (m) that carries momentum / wind stress.
const MIN_WAVE_DEPTH_M: f32 = 0.08;
/// Fraction of the tidal target depth applied per wave step.
const TIDE_BLEND: f32 = 0.04;
/// Mean depth (m) above which a wet run is "oceanic" for tide forcing.
const OCEAN_MEAN_DEPTH_M: f32 = 1.25;
/// Minimum flowable water (kg) to participate in the wave pass.
const MIN_WAVE_WATER_KG: i64 = 20;
/// Max fraction of the *active-layer* mass that may leave in one flux.
const MAX_FLUX_FRAC: i64 = 6;
/// Conserving neighbour blend — only kills short teeth, not basin setup.
const SURFACE_SMOOTH: f32 = 0.12;
/// Only smooth neighbour pairs whose depth differ by less than this (m).
const SMOOTH_MAX_JUMP_M: f32 = 0.35;

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

fn active_depth(depth_m: f32) -> f32 {
    depth_m.min(WAVE_ACTIVE_DEPTH_M).max(MIN_WAVE_DEPTH_M)
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
    // Climate wind is enough for setup/seiche and avoids per-column
    // bilinear samples of the wind field on every wet column.
    let wind_x = world.climate.wind_speed * SAMPLE_WIDTH_M;
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
        let left_ok = i > 0 && cells[i - 1].world_x + 1 == cell.world_x;
        let right_ok = i + 1 < cells.len() && cells[i + 1].world_x == cell.world_x + 1;
        let grad = match (left_ok, right_ok) {
            (true, true) => {
                let near = (eta_r - eta_l) / (2.0 * dx);
                let far_l = if i > 1 && cells[i - 2].world_x + 2 == cell.world_x {
                    cells[i - 2].eta
                } else {
                    eta_l
                };
                let far_r = if i + 2 < cells.len() && cells[i + 2].world_x == cell.world_x + 2
                {
                    cells[i + 2].eta
                } else {
                    eta_r
                };
                let wide = (far_r - far_l) / (4.0 * dx);
                0.25 * near + 0.75 * wide
            }
            (false, true) => (eta_r - cell.eta) / dx,
            (true, false) => (cell.eta - eta_l) / dx,
            (false, false) => 0.0,
        };

        // Wind stress uses *full* column depth (deep water barely tilts).
        // Flux below still uses the active-layer cap so abyssal columns
        // don't exchange their entire water mass each tick.
        let wind_accel = WIND_STRESS * wind_x / (cell.depth_m + 0.5);
        let mut u = cell.u - WAVE_G * grad + wind_accel;
        u *= 1.0 - LINEAR_DRAG;
        u_new[i] = u.clamp(-MAX_U, MAX_U);
    }

    let mut mass_delta = vec![0i64; cells.len()];
    for i in 0..cells.len().saturating_sub(1) {
        if cells[i + 1].world_x != cells[i].world_x + 1 {
            continue;
        }
        let u_face = 0.5 * (u_new[i] + u_new[i + 1]);
        let depth_eff = 0.5
            * (active_depth(cells[i].depth_m) + active_depth(cells[i + 1].depth_m));
        let active_mass_i =
            (active_depth(cells[i].depth_m) * WATER_MASS_PER_METRE_DEPTH) as i64;
        let active_mass_j =
            (active_depth(cells[i + 1].depth_m) * WATER_MASS_PER_METRE_DEPTH) as i64;
        let mut flux = (u_face * depth_eff * WATER_MASS_PER_METRE_DEPTH).round() as i64;
        if flux > 0 {
            flux = flux.min(active_mass_i / MAX_FLUX_FRAC).max(0);
        } else if flux < 0 {
            flux = (-flux).min(active_mass_j / MAX_FLUX_FRAC).max(0);
            flux = -flux;
        }
        mass_delta[i] -= flux;
        mass_delta[i + 1] += flux;
    }

    for i in 0..cells.len().saturating_sub(1) {
        if cells[i + 1].world_x != cells[i].world_x + 1 {
            continue;
        }
        if !(cells[i].oceanic && cells[i + 1].oceanic) {
            continue;
        }
        let mi = cells[i].mass + mass_delta[i];
        let mj = cells[i + 1].mass + mass_delta[i + 1];
        let di = mi as f32 / WATER_MASS_PER_METRE_DEPTH;
        let dj = mj as f32 / WATER_MASS_PER_METRE_DEPTH;
        // Skip large jumps — those are wind setup / seiche slopes.
        if (di - dj).abs() > SMOOTH_MAX_JUMP_M {
            continue;
        }
        let xfer = ((di - dj) * 0.5 * SURFACE_SMOOTH * WATER_MASS_PER_METRE_DEPTH).round() as i64;
        if xfer == 0 {
            continue;
        }
        if xfer > 0 {
            let take = xfer.min(mi.max(0) / 6);
            mass_delta[i] -= take;
            mass_delta[i + 1] += take;
        } else {
            let take = (-xfer).min(mj.max(0) / 6);
            mass_delta[i + 1] -= take;
            mass_delta[i] += take;
        }
    }

    let tide = world.tide_eta_m(tick);
    let sea = world.sea_level;
    let still = sea + tide;
    if world.tide_enabled {
        for (i, cell) in cells.iter().enumerate() {
            if !cell.oceanic {
                continue;
            }
            let target_depth = (still - cell.bed_y).max(MIN_WAVE_DEPTH_M);
            let target_mass = (target_depth * WATER_MASS_PER_METRE_DEPTH) as i64;
            let step = ((target_mass - (cell.mass + mass_delta[i])) as f32 * TIDE_BLEND) as i64;
            mass_delta[i] += step;
        }
    }

    // Amplitude clamp on oceans only — keep |η − (sea+tide)| modest so
    // deep columns can't grow metre-scale comb teeth. Ponds/basins are
    // left alone so wind setup (E49a) can still pile water.
    for (i, cell) in cells.iter().enumerate() {
        if !cell.oceanic {
            continue;
        }
        let new_mass = cell.mass + mass_delta[i];
        let new_eta = cell.bed_y + new_mass as f32 / WATER_MASS_PER_METRE_DEPTH;
        let over = new_eta - still;
        if over.abs() <= MAX_ETA_AMP_M {
            continue;
        }
        let capped_eta = still + over.signum() * MAX_ETA_AMP_M;
        let capped_mass = ((capped_eta - cell.bed_y).max(MIN_WAVE_DEPTH_M)
            * WATER_MASS_PER_METRE_DEPTH) as i64;
        mass_delta[i] = capped_mass - cell.mass;
    }

    let water_before: i64 = cells.iter().map(|c| c.mass).sum();

    // Write back: adjust top water only — no density settle. Water is
    // already the free surface; settle was the ring-wide cost spike.
    for (i, cell) in cells.iter().enumerate() {
        let Some(chunk) = world.chunks.get_mut(&cell.coord) else {
            continue;
        };
        let bedrock = chunk.bedrock_y;
        let col = &mut chunk.columns[cell.local];
        col.surface_u = u_new[i];
        let delta = mass_delta[i];
        if delta != 0 {
            col.adjust_top_water(delta, 0);
            col.recompute_surface_y(bedrock);
            col.activity = Activity::HydrologyActive;
        }
        if col.flowable_water().map(|(_, m)| m).unwrap_or(0) < MIN_WAVE_WATER_KG {
            col.surface_u = 0.0;
        }
    }

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

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;
    use wk_world::terrain::generate_flat_sand;

    #[test]
    fn wind_setup_from_wave_pass_alone() {
        let mut world = World::new(1);
        world.sea_level = 0.0;
        world.surface_waves_enabled = true;
        world.tide_enabled = false;
        world.climate.wind_speed = 1.5;
        for c in -1..=1 {
            world.insert_chunk(generate_flat_sand(c, 0.0, 8.0));
        }
        for x in -64..128 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.deposit_to_top(MaterialId::Water, 2_500, 0);
            }
        }
        world.wake_all();

        let depth = |w: &World, a: i32, b: i32| {
            let mut s = 0.0f32;
            let mut n = 0u32;
            for x in a..=b {
                if let Some(col) = w.column_at(x) {
                    if let Some((_, m)) = col.flowable_water() {
                        s += m as f32 / WATER_MASS_PER_METRE_DEPTH;
                        n += 1;
                    }
                }
            }
            s / n as f32
        };
        for t in 0..600u64 {
            run_surface_waves(&mut world, t);
        }
        let left1 = depth(&world, 4, 20);
        let right1 = depth(&world, 44, 60);
        assert!(
            right1 > left1 + 0.1,
            "wave pass alone should pile water downwind (L={left1:.3} R={right1:.3})"
        );
    }
}
