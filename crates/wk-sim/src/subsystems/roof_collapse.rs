//! Roof collapse over voids wider than `roof_span_max_m`.
//!
//! Shares the "unsupported mass" idea with slumping: if a horizontal span
//! of void exceeds what the roof material can bridge, drop roof mass into
//! the void as LooseRock and shrink the cavity.

use wk_material::{CHUNK_W, MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::column::{Activity, VoidOrigin};
use wk_world::world::World;

/// Metres of void height collapsed per tick once a span is over-limit.
const COLLAPSE_HEIGHT_M: f32 = 0.25;

fn roof_span_m(material: MaterialId) -> f32 {
    MaterialRegistry::props(material).roof_span_max_m
}

/// Find runs of adjacent columns whose voids overlap in elevation, and
/// collapse when the run width exceeds the roof material's span limit.
pub fn run_roof_collapse(world: &mut World, tick: u64) {
    // Collect (world_x, coord, local, void_idx, mid_y, roof, top, bot)
    let mut entries: Vec<(i32, i32, usize, usize, f32, MaterialId, f32, f32)> = Vec::new();
    for (&coord, chunk) in &world.chunks {
        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            for (vi, v) in col.voids.iter().enumerate() {
                if v.height_m <= 1e-4 {
                    continue;
                }
                // Surface-open sinkholes have already collapsed — don't
                // keep eating them (and wiping void water storage).
                if v.open_to_surface(col.surface_y) || v.light > 200 {
                    continue;
                }
                entries.push((
                    coord * CHUNK_W as i32 + i as i32,
                    coord,
                    i,
                    vi,
                    v.mid_y(),
                    v.roof_material,
                    v.top_y,
                    v.floor_y(),
                ));
            }
        }
    }
    entries.sort_by_key(|e| e.0);

    // Group into contiguous spans at similar mid elevation (±1 m).
    let mut i = 0usize;
    let mut collapse_sites: Vec<(i32, usize, usize)> = Vec::new();
    while i < entries.len() {
        let mid0 = entries[i].4;
        let roof0 = entries[i].5;
        let mut j = i + 1;
        while j < entries.len() {
            let (wx_j, _, _, _, mid_j, roof_j, _, _) = entries[j];
            let wx_prev = entries[j - 1].0;
            if wx_j != wx_prev + 1 {
                break;
            }
            if (mid_j - mid0).abs() > 1.0 {
                break;
            }
            if roof_j != roof0 {
                break;
            }
            j += 1;
        }
        let span_cols = j - i;
        let span_m = span_cols as f32 * SAMPLE_WIDTH_M;
        let limit = roof_span_m(roof0);
        if span_m > limit + 1e-3 {
            // Collapse the middle of the span (most unsupported).
            for k in i..j {
                collapse_sites.push((entries[k].1, entries[k].2, entries[k].3));
            }
        }
        i = j;
    }

    for (coord, local, vi) in collapse_sites {
        let bedrock = world.chunks.get(&coord).map(|c| c.bedrock_y).unwrap_or(0.0);
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let col = &mut chunk.columns[local];
        if vi >= col.voids.len() {
            continue;
        }
        let v = col.voids[vi];
        let dh = COLLAPSE_HEIGHT_M.min(v.height_m);
        if dh <= 1e-4 {
            continue;
        }
        let roof_mat = v.roof_material;
        let want = {
            let density = MaterialRegistry::props(roof_mat).density.max(1) as f32;
            (dh * SAMPLE_WIDTH_M * density) as i64
        };
        // Take mass from a roof-matching layer (fallback: any solid) so
        // collapse doesn't mint mass out of thin air.
        let mut taken = 0i64;
        for li in 0..col.layer_count as usize {
            if col.layers[li].material == roof_mat && col.layers[li].thickness > 0 {
                let t = want.min(col.layers[li].thickness);
                col.layers[li].thickness -= t;
                taken = t;
                if col.layers[li].thickness <= 0 {
                    for j in li..(col.layer_count as usize).saturating_sub(1) {
                        col.layers[j] = col.layers[j + 1];
                    }
                    if col.layer_count > 0 {
                        col.layer_count -= 1;
                    }
                }
                break;
            }
        }
        if taken <= 0 {
            for li in 0..col.layer_count as usize {
                let m = col.layers[li].material;
                if m.is_solid()
                    && !matches!(m, MaterialId::Water | MaterialId::Ice | MaterialId::Snow)
                    && col.layers[li].thickness > 0
                {
                    let t = want.min(col.layers[li].thickness);
                    col.layers[li].thickness -= t;
                    taken = t;
                    if col.layers[li].thickness <= 0 {
                        for j in li..(col.layer_count as usize).saturating_sub(1) {
                            col.layers[j] = col.layers[j + 1];
                        }
                        if col.layer_count > 0 {
                            col.layer_count -= 1;
                        }
                    }
                    break;
                }
            }
        }
        // Shrink void from the top (roof drops).
        col.voids[vi].top_y -= dh;
        col.voids[vi].height_m -= dh;
        col.voids[vi].origin = VoidOrigin::Collapse;
        if col.voids[vi].height_m < 0.02 {
            let water = col.voids[vi].water_mass;
            col.voids.remove(vi);
            if water > 0 {
                col.deposit_to_top(MaterialId::Water, water, tick);
            }
        }
        if taken > 0 {
            // Debris falls as LooseRock into the solid stack.
            col.deposit_to_top(MaterialId::LooseRock, taken, tick);
        }
        col.activity = Activity::HydrologyActive;
        col.recompute_surface_y(bedrock);
    }
}
