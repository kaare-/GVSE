//! Hydrostatic lake-level equalization across connected wet segments.

use wk_material::{CHUNK_W, MaterialId};
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

struct LakeCell {
    coord: i32,
    local: usize,
    ground: f32,
    water: i64,
}

/// Fraction of the way each application moves cells toward the exact flat
/// equilibrium. A full-strength instant snap (1.0) makes small/shallow
/// bodies "pop" — their footprint and total mass are still changing tick to
/// tick from rain/evaporation/infiltration, so jumping straight to a brand
/// new exact target every time looks like water appearing/disappearing out
/// of nowhere. Blending gradually (combined with running this more often,
/// see LakeLevel's schedule) still converges much faster than pure
/// neighbour-by-neighbour diffusion for a *wide* lake, but changes smoothly
/// enough tick-to-tick to not read as a glitch for small ponds.
const LAKE_LEVEL_BLEND: f32 = 0.1;
/// When free-surface waves are enabled, flatten much more gently so wind
/// setup / seiches aren't erased every tick by the hydrostatic blender.
const LAKE_LEVEL_BLEND_WITH_WAVES: f32 = 0.02;
/// Mean water depth (m) above which a wet run is left to `run_surface_waves`
/// instead of being force-flattened (oceans / deep lakes).
const DEEP_WAVE_DEPTH_M: f32 = 1.0;
/// Minimum standing water (kg, ~10cm depth on one column) to count as part
/// of a "lake" for leveling purposes. Without this, a light rain sheen
/// sitting on every column across the whole map — including hilltops with
/// only a trace of water — would all register as nonzero and therefore
/// "connected", causing the leveling pass to treat the *entire visible map*
/// as one giant lake and dilute real pooled water down to nothing. Trace
/// amounts below this threshold are still governed by ordinary diffusion,
/// evaporation and infiltration; they just aren't part of a lake body.
const MIN_LAKE_WATER_KG: i64 = 25;

/// Binary search for the common water-surface elevation such that flooding
/// every cell in `cells` up to that level uses exactly `total_mass`.
fn solve_level(cells: &[LakeCell], total_mass: i64) -> f32 {
    let min_ground = cells.iter().map(|c| c.ground).fold(f32::MAX, f32::min);
    let mut lo = min_ground;
    let mut hi = min_ground + 1.0;

    let volume_at = |level: f32| -> f32 {
        cells
            .iter()
            .map(|c| (level - c.ground).max(0.0))
            .sum::<f32>()
            * WATER_MASS_PER_METRE_DEPTH
    };

    // Grow the upper bound until it can hold at least the total mass.
    for _ in 0..24 {
        if volume_at(hi) >= total_mass as f32 {
            break;
        }
        hi = min_ground + (hi - min_ground) * 2.0 + 1.0;
    }

    for _ in 0..40 {
        let mid = (lo + hi) * 0.5;
        if volume_at(mid) < total_mass as f32 {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    hi
}

/// Move each cell partway from its current value toward the level implied
/// by `total_mass` (mass-conserving either way, since blending two
/// distributions that both sum to `total_mass` still sums to `total_mass`).
fn level_segment(cells: &mut [LakeCell], blend: f32) {
    let total_mass: i64 = cells.iter().map(|c| c.water).sum();
    if total_mass <= 0 {
        return;
    }
    let level = solve_level(cells, total_mass);

    let mut assigned = 0i64;
    let mut deepest_idx = 0usize;
    let mut deepest_val = i64::MIN;
    for (idx, c) in cells.iter_mut().enumerate() {
        let depth = (level - c.ground).max(0.0);
        let target_mass = (depth * WATER_MASS_PER_METRE_DEPTH) as i64;
        let blended =
            (c.water as f32 + (target_mass - c.water) as f32 * blend) as i64;
        let blended = blended.max(0);
        c.water = blended;
        assigned += blended;
        if blended > deepest_val {
            deepest_val = blended;
            deepest_idx = idx;
        }
    }
    // Rounding can leave a tiny drift; dump it on the deepest cell so total
    // mass is preserved exactly.
    let drift = total_mass - assigned;
    if drift != 0 {
        cells[deepest_idx].water = (cells[deepest_idx].water + drift).max(0);
    }
}

/// Gradually flattens every connected run of "currently wet" columns across
/// the whole loaded world (not just within one chunk — lakes can span
/// several) toward a single hydrostatic level. Runs periodically, not every
/// tick: it's meant to model near-instant real water pressure equalization,
/// which the per-tick neighbour diffusion (run_surface_water) is too slow
/// to reproduce on its own for a wide lake.
pub fn run_lake_level(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    if coords.is_empty() {
        return;
    }
    let blend = if world.surface_waves_enabled {
        LAKE_LEVEL_BLEND_WITH_WAVES
    } else {
        LAKE_LEVEL_BLEND
    };
    let skip_deep = world.surface_waves_enabled;

    let mut cells: Vec<LakeCell> = Vec::with_capacity(coords.len() * CHUNK_W);
    for &coord in &coords {
        let chunk = &world.chunks[&coord];
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            // Consider *any* water in the fluid cap — including water
            // that's sitting under a floating snow cap. Otherwise a
            // half-snow-covered lake fails hydrostatic equalization on
            // exactly the columns where the snow floated over, leaving
            // an uneven water surface right where the snow line sits.
            let (water_top_y, water) =
                col.flowable_water().unwrap_or((col.surface_y, 0));
            let bed_y = water_top_y - col.mass_to_height_delta(MaterialId::Water, water);
            cells.push(LakeCell {
                coord,
                local,
                ground: bed_y,
                water,
            });
        }
    }

    let n = cells.len();
    let mut i = 0usize;
    while i < n {
        if cells[i].water < MIN_LAKE_WATER_KG {
            i += 1;
            continue;
        }
        let start = i;
        let mut end = i;
        while end + 1 < n && cells[end + 1].water >= MIN_LAKE_WATER_KG {
            end += 1;
        }
        if end > start {
            let segment = &mut cells[start..=end];
            // Deep connected water is wave/tide territory — hydrostatic
            // flattening there was the old "fake ripple" eraser.
            if skip_deep {
                let mean_depth = segment
                    .iter()
                    .map(|c| c.water as f32 / WATER_MASS_PER_METRE_DEPTH)
                    .sum::<f32>()
                    / segment.len() as f32;
                if mean_depth >= DEEP_WAVE_DEPTH_M {
                    i = end + 1;
                    continue;
                }
            }
            level_segment(segment, blend);
        }
        i = end + 1;
    }

    // Rewrite each cell's total-flowable-water mass to the newly
    // computed value. We read `flowable_water` (not `top_water_mass`)
    // so a snow-covered pool sees its actual water total, not the
    // zero it would show if only the top layer counted — that
    // mismatch would double the water on write-back.
    for cell in cells {
        if let Some(chunk) = world.chunks.get_mut(&cell.coord) {
            let col = &mut chunk.columns[cell.local];
            let current = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            let delta = cell.water - current;
            col.adjust_top_water(delta, 0);
            col.settle_by_density(0);
            col.recompute_surface_y(chunk.bedrock_y);
        }
    }
}
