//! Angle-of-repose slumping of granular top layers.

use wk_material::{CHUNK_W, MaterialId, MaterialRegistry};
use wk_world::column::Activity;
use wk_world::world::World;

/// Fraction of the "just enough to bring the slope back to the angle of
/// repose" transfer applied each tick when a granular top layer sits
/// steeper than its material allows. Small enough that a large slump
/// takes several ticks to fully unfold (avoiding the classic explicit-
/// diffusion oscillation), large enough to visibly settle within a
/// second or two at 1× play speed.
const SLUMP_RELAXATION: f32 = 0.35;

/// Angle-of-repose slumping: loose granular material on a slope steeper
/// than its material's stable angle slides toward the lower neighbour.
///
/// Post-barrier direct mutation, same shape as `run_phase_change` /
/// `run_lake_level`, so it operates on already-committed layer state
/// (no interference with the buffered water/sediment deltas). For each
/// column pair, if the *solid* top surfaces differ by more than the
/// top solid material's `repose_rise_m`, transfer part of the excess
/// mass from the higher column's top solid layer to the lower one.
///
/// This is what turns a jagged terrain generation output — or a fresh
/// sediment deposit dropped straight onto one column — into a smoothly
/// graded slope over a handful of ticks, without any renderer-side
/// smoothing hack.
pub fn run_slumping(world: &mut World, tick: u64) {
    // Two passes per invocation to accelerate convergence for wide
    // repose fronts (excess piles up until it hits repose, then flows).
    for _ in 0..2 {
        slumping_pass(world, tick);
    }
}

fn slumping_pass(world: &mut World, tick: u64) {
    // Snapshot every column's top-solid state, INCLUDING columns whose
    // top solid can't slump itself (Stone / Bedrock outcrops). Those
    // still have to appear in the snapshot as valid *destinations* — a
    // sand column right next to an exposed bedrock outcrop should be
    // able to shed material onto the outcrop's shoulder, otherwise its
    // sand just piles up against an invisible wall and the visible
    // cliff never levels out.
    #[derive(Clone, Copy)]
    struct TopSolid {
        coord: i32,
        local: usize,
        material: MaterialId,
        thickness: i64,
        // Elevation of the top of this solid layer (excludes fluid cap above).
        top_y: f32,
        repose_rise: f32,
        // Kg per metre of layer height for this material — for
        // converting height differences back into transferable mass.
        // Zero for non-slumpable materials (they aren't sources).
        mass_per_m: f32,
        /// The top layer of the *entire* column, before skipping the
        /// fluid cap — used at deposit-time to decide whether to melt
        /// snow on contact with a water body.
        column_top_material: MaterialId,
    }

    // Roots raise the effective angle of repose (stage 8).

    let mut snapshot: Vec<TopSolid> = Vec::new();
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = &world.chunks[&coord];
        for local in 0..CHUNK_W {
            let col = &chunk.columns[local];
            let Some(idx) = (0..col.layer_count as usize).find(|&j| {
                // Skip fluids (Water flows via surface flow, Ice is rigid).
                // Include Snow — it's granular, has an angle of repose.
                !matches!(col.layers[j].material, MaterialId::Water | MaterialId::Ice)
            }) else {
                continue;
            };
            let layer = col.layers[idx];
            let props = MaterialRegistry::props(layer.material);
            let density = props.density.max(1) as f32;
            // Elevation of the top of this solid layer.
            let mut y = col.surface_y;
            for j in 0..idx {
                y -= col.mass_to_height_delta(col.layers[j].material, col.layers[j].thickness);
            }
            // Roots bind slopes — denser mats tolerate a steeper rise.
            let root = col.ecology.root_density.clamp(0.0, 1.0);
            let repose = if props.repose_rise_m.is_finite() {
                props.repose_rise_m * (1.0 + 3.0 * root)
            } else {
                props.repose_rise_m
            };
            snapshot.push(TopSolid {
                coord,
                local,
                material: layer.material,
                thickness: layer.thickness,
                top_y: y,
                repose_rise: repose,
                mass_per_m: density * wk_material::SAMPLE_WIDTH_M,
                column_top_material: col.top_material(),
            });
        }
    }

    // Sort by (coord, local) so we can index by world_x quickly.
    // World-x = coord * CHUNK_W + local. Use a BTreeMap keyed on that.
    use std::collections::BTreeMap;
    let mut by_wx: BTreeMap<i32, usize> = BTreeMap::new();
    for (i, s) in snapshot.iter().enumerate() {
        let wx = s.coord * CHUNK_W as i32 + s.local as i32;
        by_wx.insert(wx, i);
    }

    // For each column, compute a signed transfer against left and right
    // neighbours. Positive = mass leaving this column. Sources must
    // themselves be slumpable material (finite repose_rise); destinations
    // can be anything, including bedrock/stone outcrops that just
    // happen to sit lower — sand piling against a rock wall should still
    // shed material onto the shoulder of that wall.
    let mut transfers: Vec<(i32, i32, MaterialId, i64, MaterialId)> = Vec::new(); // (from_wx, to_wx, source_material, mass, dest_column_top)
    let wx_list: Vec<i32> = by_wx.keys().copied().collect();
    for wx in wx_list {
        let idx = by_wx[&wx];
        let s = snapshot[idx];
        if !s.repose_rise.is_finite() {
            continue; // Immovable source (Stone / Bedrock / Ice).
        }
        for &neighbour_wx in &[wx - 1, wx + 1] {
            let Some(&nidx) = by_wx.get(&neighbour_wx) else {
                continue;
            };
            let n = snapshot[nidx];
            let dy = s.top_y - n.top_y;
            if dy <= s.repose_rise {
                continue;
            }
            let excess = dy - s.repose_rise;
            let height_to_move = 0.5 * excess * SLUMP_RELAXATION;
            let mass_to_move = (height_to_move * s.mass_per_m) as i64;
            let mass_to_move = mass_to_move.min(s.thickness / 2).max(0);
            if mass_to_move > 0 {
                transfers.push((wx, neighbour_wx, s.material, mass_to_move, n.column_top_material));
            }
        }
    }

    // Apply transfers. Mass conservation is critical here: whatever
    // we actually manage to remove from the source is exactly what
    // gets deposited on the destination.
    //
    // Snow sliding into a water body melts on contact — an avalanche
    // reaching a lake doesn't leave a persistent floating slush ring,
    // it just adds equivalent water volume. Deposit as Water instead
    // of Snow whenever the destination column's *top layer* is Water.
    for (from_wx, to_wx, source_material, mass, dest_top) in transfers {
        let from_coord = World::chunk_coord_for_world_x(from_wx);
        let from_local = World::local_x(from_wx);
        let mut actual_take = 0i64;
        if let Some(chunk) = world.chunks.get_mut(&from_coord) {
            let col = &mut chunk.columns[from_local];
            if let Some(j) = (0..col.layer_count as usize)
                .find(|&j| col.layers[j].material == source_material)
            {
                let take = mass.min(col.layers[j].thickness);
                if take > 0 {
                    let dh = col.mass_to_height_delta(source_material, take);
                    col.layers[j].thickness -= take;
                    col.surface_y -= dh;
                    col.activity = Activity::HydrologyActive;
                    actual_take = take;
                }
            }
        }
        if actual_take <= 0 {
            continue;
        }
        let deposit_material = if source_material == MaterialId::Snow
            && dest_top == MaterialId::Water
        {
            // Snow sliding into open water melts on contact.
            MaterialId::Water
        } else {
            source_material
        };
        if let Some(chunk) = world.chunks.get_mut(&World::chunk_coord_for_world_x(to_wx)) {
            let local = World::local_x(to_wx);
            let col = &mut chunk.columns[local];
            col.deposit_to_top(deposit_material, actual_take, tick);
        }
    }

    // Clean up: settle+clamp per touched column so any empty layer
    // slots collapse, densities restack correctly, and surface_y
    // stays consistent with total layer heights.
    let touched_coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in touched_coords {
        let bedrock_y = world.chunks[&coord].bedrock_y;
        if let Some(chunk) = world.chunks.get_mut(&coord) {
            for col in &mut chunk.columns {
                col.clamp_state();
                col.recompute_surface_y(bedrock_y);
            }
        }
    }
}
