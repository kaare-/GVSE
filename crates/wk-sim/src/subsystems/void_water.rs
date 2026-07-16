//! Cave-river flow and surface capture into open voids.

use std::collections::BTreeMap;

use wk_material::CHUNK_W;
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

/// Fraction of head-equalizing transfer applied between overlapping voids.
const VOID_FLOW_RELAXATION: f32 = 0.5;

/// Fraction of surface water drained into an open void per tick.
const SURFACE_CAPTURE_FRAC: f32 = 0.35;

fn voids_overlap(a_top: f32, a_bot: f32, b_top: f32, b_bot: f32) -> bool {
    a_bot < b_top && b_bot < a_top
}

/// Drain surface / flowable water into voids that breach the surface.
pub fn run_surface_void_capture(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        for i in 0..CHUNK_W {
            let col = &mut chunk.columns[i];
            let available = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            if available <= 0 {
                continue;
            }
            let want = ((available as f32) * SURFACE_CAPTURE_FRAC) as i64;
            let moved = col.drain_surface_water_into_voids(want);
            if moved > 0 {
                col.activity = Activity::HydrologyActive;
            }
        }
    }
}

/// Lateral head-gradient flow between overlapping voids in neighbouring
/// columns. Sparse — cost scales with void count, not column count.
pub fn run_void_water_flow(world: &mut World) {
    #[derive(Clone, Copy)]
    struct Snap {
        coord: i32,
        local: usize,
        void_idx: usize,
        head: f32,
        mass: i64,
        top: f32,
        bot: f32,
        world_x: i32,
    }

    let mut snaps: Vec<Snap> = Vec::new();
    for (&coord, chunk) in &world.chunks {
        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            for (vi, v) in col.voids.iter().enumerate() {
                if v.water_mass <= 0 || v.height_m <= 1e-4 {
                    continue;
                }
                let fill_m = (v.water_mass as f32 / WATER_MASS_PER_METRE_DEPTH)
                    .min(v.height_m);
                snaps.push(Snap {
                    coord,
                    local: i,
                    void_idx: vi,
                    head: v.floor_y() + fill_m,
                    mass: v.water_mass,
                    top: v.top_y,
                    bot: v.floor_y(),
                    world_x: coord * CHUNK_W as i32 + i as i32,
                });
            }
        }
    }

    // Deltas keyed by (coord, local, void_idx).
    let mut delta: BTreeMap<(i32, usize, usize), i64> = BTreeMap::new();

    for a in &snaps {
        for b in &snaps {
            if b.world_x != a.world_x + 1 {
                continue;
            }
            if !voids_overlap(a.top, a.bot, b.top, b.bot) {
                continue;
            }
            let dh = a.head - b.head;
            if dh.abs() < 1e-3 {
                continue;
            }
            let equalizing = dh.abs() * WATER_MASS_PER_METRE_DEPTH * 0.5;
            let amt = ((equalizing * VOID_FLOW_RELAXATION) as i64).max(0);
            if dh > 0.0 {
                let amt = amt.min(a.mass);
                if amt > 0 {
                    *delta.entry((a.coord, a.local, a.void_idx)).or_default() -= amt;
                    *delta.entry((b.coord, b.local, b.void_idx)).or_default() += amt;
                }
            }
        }
    }

    for ((coord, local, vi), d) in delta {
        if d == 0 {
            continue;
        }
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        let col = &mut chunk.columns[local];
        if vi >= col.voids.len() {
            continue;
        }
        col.voids[vi].water_mass = (col.voids[vi].water_mass + d).max(0);
        col.activity = Activity::HydrologyActive;
    }
}
