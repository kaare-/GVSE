//! Hydrostatic lake-level equalization across connected wet segments.

use wk_material::CHUNK_W;
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

struct LakeCell {
    coord: i32,
    local: usize,
    /// Absolute column index — lake segments must be contiguous in
    /// world-x. Loaded chunks can have gaps (e.g. humidity tests); those
    /// must not look adjacent just because they sit next to each other
    /// in the BTreeMap iteration order.
    world_x: i32,
    ground: f32,
    water: i64,
}

fn spatially_adjacent(a: &LakeCell, b: &LakeCell) -> bool {
    b.world_x == a.world_x + 1
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
/// Wave-mode blend for oceans. Tide-only waves don't need a tiny blend;
/// a stronger pull keeps shelf-edge columns from oscillating as the tide
/// injects and lake-level slowly corrects.
const LAKE_LEVEL_BLEND_WITH_WAVES: f32 = 0.18;
/// Minimum standing water (kg, ~10cm depth on one column) to count as a
/// *seed* for a lake body. Trace sheen below this is still governed by
/// ordinary diffusion; it just doesn't start a leveling pass on its own.
/// Dry columns below the solved free surface are still flooded into the
/// segment (see expand below) so a vertical wall of water can spill onto
/// a lower empty bed.
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
    for c in cells.iter_mut() {
        let depth = (level - c.ground).max(0.0);
        let target_mass = (depth * WATER_MASS_PER_METRE_DEPTH) as i64;
        let blended = (c.water as f32 + (target_mass - c.water) as f32 * blend) as i64;
        let blended = blended.max(0);
        c.water = blended;
        assigned += blended;
    }
    // Rounding can leave a tiny drift. Spread it 1 kg at a time across the
    // deepest cells so a full ring of ocean doesn't accumulate every tick's
    // drift onto one particular column (the old "dump on deepest" version
    // created a persistent spike where the drift pile grew every tick).
    let mut drift = total_mass - assigned;
    if drift == 0 {
        return;
    }
    // Sort a copy of indices by water depth so we can walk the deepest cells.
    // Small: only touch |drift| cells (drift is typically ≤ few kg on a ring).
    let mut order: Vec<usize> = (0..cells.len()).collect();
    order.sort_by(|&a, &b| cells[b].water.cmp(&cells[a].water));
    let step: i64 = if drift > 0 { 1 } else { -1 };
    let mut i = 0usize;
    while drift != 0 && !order.is_empty() {
        let idx = order[i % order.len()];
        let next = cells[idx].water + step;
        if next >= 0 {
            cells[idx].water = next;
            drift -= step;
        }
        i += 1;
        if i > order.len() * 4 {
            // Safety valve — should never trigger for realistic drifts.
            break;
        }
    }
}

/// Expand a wet seed segment into neighbouring dry (or trace-wet) columns
/// whose bed sits below the hydrostatic level of the current mass. Without
/// this, lake-level never floods an empty lower neighbour and the free
/// surface freezes into a vertical wall at the wet/dry contact.
fn expand_flood_segment(cells: &[LakeCell], start: usize, end: usize) -> (usize, usize) {
    let mut start = start;
    let mut end = end;
    for _ in 0..12 {
        let total_mass: i64 = cells[start..=end].iter().map(|c| c.water).sum();
        if total_mass <= 0 {
            break;
        }
        let level = solve_level(&cells[start..=end], total_mass);
        let mut expanded = false;
        while start > 0
            && spatially_adjacent(&cells[start - 1], &cells[start])
            && cells[start - 1].ground < level - 0.01
        {
            start -= 1;
            expanded = true;
        }
        while end + 1 < cells.len()
            && spatially_adjacent(&cells[end], &cells[end + 1])
            && cells[end + 1].ground < level - 0.01
        {
            end += 1;
            expanded = true;
        }
        if !expanded {
            break;
        }
    }
    (start, end)
}

/// Gradually flattens every connected body of standing water across the
/// loaded world toward a single hydrostatic level, including dry columns
/// that the solved free surface would flood.
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

    let mut cells: Vec<LakeCell> = Vec::with_capacity(coords.len() * CHUNK_W);
    for &coord in &coords {
        let chunk = &world.chunks[&coord];
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            // Solid bed under weather fluids *and* cavities. Deriving bed
            // from flowable water_top - mass used to inherit void inflation
            // in surface_y, which notched the ocean at karst mouths.
            let water = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            let bed_y = col.solid_bed_y();
            let world_x = chunk.world_x_base() + local as i32;
            cells.push(LakeCell {
                coord,
                local,
                world_x,
                ground: bed_y,
                water,
            });
        }
    }

    let n = cells.len();
    let mut i = 0usize;
    // Track which cells were touched so we don't double-level overlapping
    // expansions from adjacent seeds.
    let mut claimed = vec![false; n];
    while i < n {
        if cells[i].water < MIN_LAKE_WATER_KG || claimed[i] {
            i += 1;
            continue;
        }
        let mut start = i;
        let mut end = i;
        while end + 1 < n
            && cells[end + 1].water >= MIN_LAKE_WATER_KG
            && !claimed[end + 1]
            && spatially_adjacent(&cells[end], &cells[end + 1])
        {
            end += 1;
        }
        let (s, e) = expand_flood_segment(&cells, start, end);
        start = s;
        end = e;
        if end > start {
            let segment = &mut cells[start..=end];
            level_segment(segment, blend);
            for c in &mut claimed[start..=end] {
                *c = true;
            }
        }
        i = end + 1;
    }

    // Rewrite each cell's total-flowable-water mass to the newly
    // computed value. We read `flowable_water` (not `top_water_mass`)
    // so a snow-covered pool sees its actual water total, not the
    // zero it would show if only the top layer counted — that
    // mismatch would double the water on write-back.
    //
    // Only touch columns that actually moved — the old loop settled
    // every loaded column every tick (including dry / deep-skipped),
    // which dominated the frame on large rings.
    for cell in cells {
        if let Some(chunk) = world.chunks.get_mut(&cell.coord) {
            let col = &mut chunk.columns[cell.local];
            let current = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            let delta = cell.water - current;
            if delta == 0 {
                continue;
            }
            col.adjust_top_water(delta, 0);
            col.recompute_surface_y(chunk.bedrock_y);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;
    use wk_world::terrain::generate_flat_sand;
    use wk_world::world::World;

    #[test]
    fn lake_level_floods_dry_lower_neighbour() {
        let mut world = World::new(42);
        world.sea_level = 0.0;
        world.rain_enabled = false;
        world.weather.weather_enabled = false;
        world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
        // Column 0: deep pool. Column 1: dry, same bed (flat sand).
        {
            let chunk = world.chunks.get_mut(&0).unwrap();
            let bed = chunk.bedrock_y;
            chunk.columns[0].deposit_to_top(MaterialId::Water, 2000, 0);
            chunk.columns[0].clamp_state();
            chunk.columns[0].recompute_surface_y(bed);
            chunk.columns[1].clamp_state();
            chunk.columns[1].recompute_surface_y(bed);
        }
        world.wake_all();

        for _ in 0..120 {
            run_lake_level(&mut world);
        }

        let right = world.chunks.get(&0).unwrap().columns[1]
            .flowable_water()
            .map(|(_, m)| m)
            .unwrap_or(0);
        assert!(
            right >= 15,
            "dry neighbour should flood, got {right} kg"
        );
    }

    #[test]
    fn lake_level_does_not_jump_chunk_gaps() {
        let mut world = World::new(44);
        world.sea_level = 0.0;
        world.rain_enabled = false;
        world.weather.weather_enabled = false;
        world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
        world.insert_chunk(generate_flat_sand(2, 0.0, 10.0)); // gap at chunk 1
        {
            let chunk = world.chunks.get_mut(&0).unwrap();
            let bed = chunk.bedrock_y;
            for col in &mut chunk.columns {
                col.deposit_to_top(MaterialId::Water, 5000, 0);
                col.clamp_state();
                col.recompute_surface_y(bed);
            }
        }
        world.wake_all();
        for _ in 0..80 {
            run_lake_level(&mut world);
        }
        let far = world.chunks.get(&2).unwrap().columns[0]
            .flowable_water()
            .map(|(_, m)| m)
            .unwrap_or(0);
        assert_eq!(far, 0, "water must not teleport across unloaded chunk gaps");
    }

    #[test]
    fn lake_level_spills_past_snow_bank() {
        let mut world = World::new(43);
        world.sea_level = 0.0;
        world.rain_enabled = false;
        world.weather.weather_enabled = false;
        world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
        {
            let chunk = world.chunks.get_mut(&0).unwrap();
            let bed = chunk.bedrock_y;
            chunk.columns[0].deposit_to_top(MaterialId::Water, 3000, 0);
            // Snow bank on the dry neighbour — surface_y is high, but the
            // solid bed is still flat. Water must still spill.
            chunk.columns[1].deposit_to_top(MaterialId::Snow, 4000, 0);
            for col in &mut chunk.columns {
                col.clamp_state();
                col.recompute_surface_y(bed);
            }
        }
        world.wake_all();

        for _ in 0..60 {
            run_lake_level(&mut world);
        }

        let right = world.chunks.get(&0).unwrap().columns[1]
            .flowable_water()
            .map(|(_, m)| m)
            .unwrap_or(0);
        assert!(
            right >= MIN_LAKE_WATER_KG,
            "snow bank must not dam the lake, got {right} kg under snow"
        );
    }
}
