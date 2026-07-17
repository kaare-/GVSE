//! Ocean surface dynamics — **tide only**.
//!
//! Earlier iterations ran a 1-D shallow-water momentum step per tick.
//! Every version we tuned still built 2-cell zig-zag comb teeth on the
//! free surface (a well-known checkerboard mode of that scheme). We
//! give up on physical wave propagation and keep the one thing this
//! subsystem needs to do for gameplay:
//!
//! - **Tide** raises/lowers oceanic columns as a whole. Booked on
//!   `sea_inject_total` (E49b).
//!
//! Wind setup, surface diffusion and any other flow live entirely in
//! `run_lake_level` / `run_surface_water`. Deep oceans are now leveled
//! by `run_lake_level` too, so the surface reads as a proper flat sea
//! (± a slow tide) instead of a spiky wave grid.

use wk_material::CHUNK_W;
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

/// Fraction of the tidal target depth applied per tick.
const TIDE_BLEND: f32 = 0.04;
/// Minimum flowable water (kg) to participate.
const MIN_WATER_KG: i64 = 20;
/// Minimum depth (m) that participates.
const MIN_DEPTH_M: f32 = 0.08;
/// Mean depth (m) above which a wet column counts as oceanic for tide.
const OCEAN_MEAN_DEPTH_M: f32 = 1.25;

pub fn run_surface_waves(world: &mut World, tick: u64) {
    if !world.surface_waves_enabled || !world.tide_enabled {
        // No physics-side wave motion. Any residual `surface_u` is stale;
        // zero it so future saves don't carry non-zero velocities around.
        if !world.surface_waves_enabled {
            for chunk in world.chunks.values_mut() {
                for col in chunk.columns.iter_mut() {
                    if col.surface_u != 0.0 {
                        col.surface_u = 0.0;
                    }
                }
            }
        }
        return;
    }

    let tide = world.tide_eta_m(tick);
    let sea = world.sea_level;
    let still = sea + tide;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    let mut water_before = 0i64;
    let mut deltas: Vec<(i32, usize, i64)> = Vec::new();

    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        let base_x = chunk.world_x_base();
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            let Some((eta, mass)) = col.flowable_water() else {
                continue;
            };
            if mass < MIN_WATER_KG {
                continue;
            }
            let depth_m = mass as f32 / WATER_MASS_PER_METRE_DEPTH;
            if depth_m < MIN_DEPTH_M {
                continue;
            }
            let bed_y = eta - depth_m;
            // True ocean: bed submerged and deep enough for a tide signal.
            let oceanic = bed_y < sea - 0.25 && depth_m >= OCEAN_MEAN_DEPTH_M * 0.5;
            if !oceanic {
                continue;
            }
            water_before += mass;
            let target_depth = (still - bed_y).max(MIN_DEPTH_M);
            let target_mass = (target_depth * WATER_MASS_PER_METRE_DEPTH) as i64;
            let step = ((target_mass - mass) as f32 * TIDE_BLEND) as i64;
            if step != 0 {
                deltas.push((coord, local, step));
            }
            let _ = base_x;
        }
    }

    let mut water_after = water_before;
    for (coord, local, delta) in deltas {
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let bedrock = chunk.bedrock_y;
        let col = &mut chunk.columns[local];
        let before = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
        col.adjust_top_water(delta, 0);
        col.recompute_surface_y(bedrock);
        col.activity = Activity::HydrologyActive;
        let after = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
        water_after = water_after - before + after;
        col.surface_u = 0.0;
    }
    let net = water_after - water_before;
    if net != 0 {
        world.mass_audit.sea_inject_total += net;
    }
}
