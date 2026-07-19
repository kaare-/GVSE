//! Cave-river flow, surface capture into open voids, and pore seepage
//! into buried cavities.

use std::collections::{BTreeMap, HashSet};

use wk_material::CHUNK_W;
use wk_world::column::Activity;
use wk_world::world::World;

use super::shared::WATER_MASS_PER_METRE_DEPTH;

/// Fraction of head-equalizing transfer applied between overlapping voids.
const VOID_FLOW_RELAXATION: f32 = 0.5;

/// Fraction of surface water drained into an open void per tick.
const SURFACE_CAPTURE_FRAC: f32 = 0.35;

/// Max pore kg that may seep into cavities per column per tick.
/// Slow enough that a cave fills over seconds of rain, not instantly.
const VOID_SEEP_MAX_KG: i64 = 400;
/// Fraction of available (above-reserve) moisture offered to voids / tick.
const VOID_SEEP_FRAC: f32 = 0.12;

fn voids_overlap(a_top: f32, a_bot: f32, b_top: f32, b_bot: f32) -> bool {
    a_bot < b_top && b_bot < a_top
}

/// Drain surface / flowable water into voids that breach the surface.
///
/// Coastal / oceanic columns are skipped. Lit sea-cliff and karst mouths
/// used to swallow ~35% of standing water every tick (via a `light > 200`
/// latch), then lake-level / tide refilled — a persistent pump that cut a
/// vertical notch in the free surface and launched algae along the
/// oscillating refill.
pub fn run_surface_void_capture(world: &mut World) {
    let sea = world.sea_level;
    // High-tide + buffer: mouths on the splash zone still saw capture under
    // the old "solid_bed < sea" gate and kept pumping with each swell.
    let coastal_limit = sea + world.tide_amplitude_m.abs() + 2.0;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        for i in 0..CHUNK_W {
            let col = &mut chunk.columns[i];
            if col.solid_bed_y() < coastal_limit {
                continue;
            }
            let available = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
            if available <= 0 {
                continue;
            }
            // Already inundated near sea level — this is coastal flooding,
            // not a land sinkhole drinking a rain pond.
            if let Some((wtop, _)) = col.flowable_water() {
                if available >= 25 && wtop >= sea - 0.5 {
                    continue;
                }
            }
            let want = ((available as f32) * SURFACE_CAPTURE_FRAC) as i64;
            let moved = col.drain_surface_water_into_voids(want);
            if moved > 0 {
                col.activity = Activity::HydrologyActive;
            }
        }
    }
}

/// Seep pore moisture into buried cavities when the water table
/// intersects them. Without this, karst/burrow voids under a solid roof
/// stay bone-dry forever while rain only fills surface-open sinkholes.
pub fn run_void_moisture_seep(world: &mut World) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get_mut(&coord) else {
            continue;
        };
        if !chunk.columns.iter().any(|c| !c.voids.is_empty()) {
            continue;
        }
        for i in 0..CHUNK_W {
            let col = &mut chunk.columns[i];
            if col.voids.is_empty() || col.moisture <= 0 {
                continue;
            }
            let available = col.moisture.saturating_sub(
                ((col.moisture_cap() as f32) * 0.20).round() as i64,
            );
            if available <= 0 {
                continue;
            }
            let want = ((available as f32) * VOID_SEEP_FRAC)
                .round()
                .max(1.0) as i64;
            let want = want.min(VOID_SEEP_MAX_KG).min(available);
            let _ = col.seep_moisture_into_voids(want);
        }
    }
}

/// Lateral head-gradient flow between overlapping voids in neighbouring
/// columns. Sparse — cost scales with void count × avg voids/column, not
/// void-count² (cliff worldgen can seed thousands of dry cavities).
///
/// Dry voids are included as receivers so a wet cave can fill an empty
/// neighbour over time (previously only wet↔wet pairs were considered).
pub fn run_void_water_flow(world: &mut World) {
    #[derive(Clone, Copy)]
    struct Snap {
        coord: i32,
        local: usize,
        void_idx: usize,
        head: f32,
        mass: i64,
        free: i64,
        top: f32,
        bot: f32,
        world_x: i32,
    }

    // Only wet voids and dry neighbours of wet voids participate. Bare cliff
    // cavities (thousands after worldgen) stay out of the matcher until
    // pore seepage or capture puts water in them.
    let mut wet_x: HashSet<i32> = HashSet::new();
    for (&coord, chunk) in &world.chunks {
        for i in 0..CHUNK_W {
            let col = &chunk.columns[i];
            if col.voids.iter().any(|v| v.water_mass > 0 && v.height_m > 1e-4) {
                wet_x.insert(coord * CHUNK_W as i32 + i as i32);
            }
        }
    }

    let mut snaps: Vec<Snap> = Vec::new();
    if !wet_x.is_empty() {
        for (&coord, chunk) in &world.chunks {
            for i in 0..CHUNK_W {
                let col = &chunk.columns[i];
                if col.voids.is_empty() {
                    continue;
                }
                let wx = coord * CHUNK_W as i32 + i as i32;
                let near_wet =
                    wet_x.contains(&wx) || wet_x.contains(&(wx - 1)) || wet_x.contains(&(wx + 1));
                if !near_wet {
                    continue;
                }
                for (vi, v) in col.voids.iter().enumerate() {
                    if v.height_m <= 1e-4 {
                        continue;
                    }
                    let fill_m = (v.water_mass.max(0) as f32 / WATER_MASS_PER_METRE_DEPTH)
                        .min(v.height_m);
                    snaps.push(Snap {
                        coord,
                        local: i,
                        void_idx: vi,
                        head: v.floor_y() + fill_m,
                        mass: v.water_mass.max(0),
                        free: v.free_capacity_kg(),
                        top: v.top_y,
                        bot: v.floor_y(),
                        world_x: wx,
                    });
                }
            }
        }
    }

    // Index by column so wet→right-neighbour pairs are O(active voids).
    let mut by_x: BTreeMap<i32, Vec<usize>> = BTreeMap::new();
    for (idx, s) in snaps.iter().enumerate() {
        by_x.entry(s.world_x).or_default().push(idx);
    }

    // Deltas keyed by (coord, local, void_idx).
    let mut delta: BTreeMap<(i32, usize, usize), i64> = BTreeMap::new();

    for a in &snaps {
        if a.mass <= 0 {
            continue;
        }
        let Some(right) = by_x.get(&(a.world_x + 1)) else {
            continue;
        };
        for &bi in right {
            let b = &snaps[bi];
            if !voids_overlap(a.top, a.bot, b.top, b.bot) {
                continue;
            }
            let dh = a.head - b.head;
            if dh <= 1e-3 {
                continue;
            }
            let equalizing = dh * WATER_MASS_PER_METRE_DEPTH * 0.5;
            let amt = ((equalizing * VOID_FLOW_RELAXATION) as i64)
                .max(0)
                .min(a.mass)
                .min(b.free);
            if amt > 0 {
                *delta.entry((a.coord, a.local, a.void_idx)).or_default() -= amt;
                *delta.entry((b.coord, b.local, b.void_idx)).or_default() += amt;
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
        let cap = col.voids[vi].capacity_kg();
        col.voids[vi].water_mass = (col.voids[vi].water_mass + d).clamp(0, cap);
        col.activity = Activity::HydrologyActive;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_material::MaterialId;
    use wk_world::column::VoidOrigin;
    use wk_world::terrain::generate_flat_sand;

    #[test]
    fn buried_cavity_fills_from_pore_moisture() {
        let mut world = World::new(1);
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        world.wake_all();
        let surface = world.column_at(4).unwrap().surface_y;
        let mid = surface - 2.5;
        if let Some(col) = world.column_at_mut(4) {
            col.moisture = col.moisture_cap();
            col.grow_void_at(mid, 1.5, MaterialId::Sand, VoidOrigin::Karst);
            assert_eq!(col.voids[0].water_mass, 0);
        }
        let moist_before = world.column_at(4).unwrap().moisture;
        for _ in 0..40 {
            run_void_moisture_seep(&mut world);
        }
        let col = world.column_at(4).unwrap();
        assert!(
            col.voids[0].water_mass > 0,
            "buried cavity should fill from pore water"
        );
        assert!(
            col.moisture < moist_before,
            "seepage must take moisture (before={moist_before} after={})",
            col.moisture
        );
        assert!(
            col.voids[0].water_mass + col.moisture
                == moist_before
                || col.voids[0].water_mass + col.moisture <= moist_before,
            "mass conserved into void"
        );
        // Exact conservation: moisture drop == void gain (no other sinks).
        assert_eq!(
            moist_before - col.moisture,
            col.voids[0].water_mass,
            "pore→void must conserve water mass"
        );
    }
}
