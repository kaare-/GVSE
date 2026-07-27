//! E50 — Ring world topology + facies belts (worldgen).

use crate::helpers::*;
use wk_material::{MaterialId};
use wk_world::{CHUNK_W};
use wk_world::terrain::{generate_chunk, BEDROCK_FLOOR_M};
use wk_world::world::World;
use wk_world::{FaciesBelt, WorldGenParams, WorldGenProfile, WorldTopology};

fn ring_world(seed: u64, chunks: u32) -> World {
    let mut world = World::new(seed);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.gen = WorldGenParams {
        topology: WorldTopology::Ring { chunks },
        profile: WorldGenProfile::RingFacies,
    };
    for c in 0..chunks as i32 {
        let chunk = generate_chunk(
            c,
            world.seed,
            BEDROCK_FLOOR_M,
            world.sea_level,
            world.gen,
        );
        world.insert_chunk(chunk);
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

#[test]
fn e50a_ring_wrap_exchanges_mass_not_boundary() {
    let mut world = ring_world(5050, 8);
    let width = world.topology().width_columns().unwrap();
    // Flood a column near the right edge; surface flow should wrap left.
    let edge = width - 2;
    if let Some(col) = world.column_at_mut(edge) {
        col.deposit_to_top(MaterialId::Water, 3_000, 0);
    }
    world.wake_all();
    world.recompute_mass_audit();
    let audit0 = world.mass_audit.boundary_out_total;
    let tracked0 = world.mass_audit.total_tracked();
    let a0 = world.mass_audit.clone();

    let mut sim = wk_sim::Simulation::new(&world);
    for _ in 0..200 {
        sim.step(&mut world);
    }
    world.recompute_mass_audit();
    let boundary = world.mass_audit.boundary_out_total - audit0;
    let left_water = world
        .column_at(1)
        .map(|c| c.top_water_mass() + c.flowable_water().map(|(_, m)| m).unwrap_or(0))
        .unwrap_or(0);
    let drift = bookkeeping_check(&world, tracked0, a0);

    eprintln!(
        "E50a: boundary_delta={boundary} left_water≈{left_water} drift={drift}"
    );
    assert_eq!(
        boundary, 0,
        "ring must not book boundary_out across the seam"
    );
    assert!(
        left_water > 0 || world.column_at(0).unwrap().flowable_water().is_some(),
        "water should be able to exist near the wrapped left edge"
    );
    assert!(drift.abs() <= 80, "drift {drift}");
    assert_no_negative_masses(&world);
}

#[test]
fn e50b_facies_belts_vary_and_shelf_has_limestone() {
    let world = ring_world(5051, 32);
    let width = world.topology().width_columns().unwrap();
    let mut belts = std::collections::HashSet::new();
    let mut limestone_cols = 0usize;
    let mut stone_near_surface = 0usize;
    let mut overfull = 0usize;

    for x in 0..width {
        let belt = wk_world::facies_at(world.seed, world.gen.topology, x);
        belts.insert(std::mem::discriminant(&belt));
        let col = world.column_at(x).unwrap();
        if col.layer_count as usize > 7 {
            // water may be 8th — allow exactly 8, flag absurd
            if col.layer_count as usize > 8 {
                overfull += 1;
            }
        }
        let has_lime = (0..col.layer_count as usize)
            .any(|i| col.layers[i].material == MaterialId::Limestone);
        if has_lime {
            limestone_cols += 1;
        }
        if matches!(belt, FaciesBelt::HighRange) {
            // First solid below fluids should often be Stone/LooseRock/Bedrock.
            for i in 0..col.layer_count as usize {
                let m = col.layers[i].material;
                if m.is_fluid() {
                    continue;
                }
                if matches!(
                    m,
                    MaterialId::Stone | MaterialId::LooseRock | MaterialId::Bedrock
                ) {
                    stone_near_surface += 1;
                }
                break;
            }
        }
    }

    eprintln!(
        "E50b: distinct_belts≈{} limestone_cols={limestone_cols} highrange_hard={stone_near_surface} overfull={overfull} width={width}",
        belts.len()
    );
    assert!(
        belts.len() >= 5,
        "expected several facies belts around the ring, got {}",
        belts.len()
    );
    assert!(
        limestone_cols >= CHUNK_W,
        "shelf limestone should span many columns (got {limestone_cols})"
    );
    assert!(
        stone_near_surface > 10,
        "high range should expose hard rock near surface"
    );
    assert_eq!(overfull, 0, "stacks must fit MAX_LAYERS");
}

#[test]
fn e50c_seam_materials_are_continuous() {
    let world = ring_world(5052, 16);
    let width = world.topology().width_columns().unwrap();
    let a = world.column_at(0).unwrap();
    let b = world.column_at(width - 1).unwrap();
    // Climate elevations should not cliff by tens of metres at the seam
    // for neighbouring abyss/basin blend — allow a generous but finite jump.
    let da = (a.climate_elevation() - b.climate_elevation()).abs();
    eprintln!(
        "E50c: seam elev {} vs {} (|Δ|={da:.2})",
        a.climate_elevation(),
        b.climate_elevation()
    );
    assert!(
        da < 25.0,
        "seam elevation jump too large ({da:.1} m) — belts not periodic"
    );
}

#[test]
fn e50d_relief_has_deep_ocean_and_tall_peaks() {
    let world = ring_world(5053, 64);
    let width = world.topology().width_columns().unwrap();
    let sea = world.sea_level;
    let mut min_y = f32::MAX;
    let mut max_y = f32::MIN;
    for x in 0..width {
        let y = world.column_at(x).unwrap().climate_elevation();
        min_y = min_y.min(y);
        max_y = max_y.max(y);
    }
    let ocean_depth = sea - min_y;
    let peak_asl = max_y - sea;
    let span = max_y - min_y;
    eprintln!(
        "E50d: min_y={min_y:.1} max_y={max_y:.1} ocean_depth={ocean_depth:.1} peak_asl={peak_asl:.1} span={span:.1}"
    );
    assert!(
        ocean_depth > 200.0,
        "abyssal floor should sit hundreds of metres below sea (depth={ocean_depth:.1})"
    );
    assert!(
        peak_asl > 400.0,
        "high range should reach hundreds of metres a.s.l. (peak={peak_asl:.1})"
    );
    assert!(
        span > 600.0,
        "ring relief should span hundreds of metres vertically (span={span:.1})"
    );
}
