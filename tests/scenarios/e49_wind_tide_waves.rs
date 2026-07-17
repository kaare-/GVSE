//! E49 — Wind setup + tidal free-surface (hydrology).
//!
//! Standing water should respond to wind stress (pile-up downwind) and to
//! a sinusoidal tide around sea level, instead of the old LakeLevel/Rain
//! beat-frequency "fake waves".

use crate::helpers::*;
use wk_material::MaterialId;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn mean_eta(world: &World, x0: i32, x1: i32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0u32;
    for x in x0..=x1 {
        if let Some(col) = world.column_at(x) {
            if let Some((eta, _)) = col.flowable_water() {
                sum += eta;
                n += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

fn mean_depth_m(world: &World, x0: i32, x1: i32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0u32;
    for x in x0..=x1 {
        if let Some(col) = world.column_at(x) {
            if let Some((_, mass)) = col.flowable_water() {
                sum += mass as f32 / 250.0;
                n += 1;
            }
        }
    }
    if n == 0 {
        0.0
    } else {
        sum / n as f32
    }
}

/// Three flat chunks so interior columns don't drain through open boundaries.
fn flooded_basin(seed: u64) -> World {
    let mut world = World::new(seed);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.surface_waves_enabled = true;
    world.tide_enabled = false;
    for c in -1..=1 {
        world.insert_chunk(generate_flat_sand(c, 0.0, 8.0));
    }
    // Fill moisture so infiltration doesn't steal the free surface, then
    // flood every column. Measure on the interior chunk (0..63).
    for x in -64..128 {
        if let Some(col) = world.column_at_mut(x) {
            col.moisture = col.moisture_cap();
            col.deposit_to_top(MaterialId::Water, 2_500, 0);
        }
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

#[test]
fn e49a_wind_piles_water_downwind() {
    let mut world = flooded_basin(9049);
    // Strong positive wind → +x. Climate fallback is used (no pressure field).
    world.climate.wind_speed = 1.5;

    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();
    // Interior left vs right of centre chunk.
    let left0 = mean_depth_m(&world, 4, 20);
    let right0 = mean_depth_m(&world, 44, 60);

    let mut sim = wk_sim::Simulation::new(&world);
    let start = std::time::Instant::now();
    for _ in 0..900 {
        sim.step(&mut world);
    }
    let elapsed = start.elapsed();

    let left1 = mean_depth_m(&world, 4, 20);
    let right1 = mean_depth_m(&world, 44, 60);
    let setup = right1 - left1;
    let drift = bookkeeping_check(&world, tracked0, audit0);

    eprintln!(
        "E49a: wind setup left {left0:.2}→{left1:.2} m  right {right0:.2}→{right1:.2} m  Δ={setup:.2} m  drift={drift} in {:?}",
        elapsed
    );

    assert!(
        setup > 0.15,
        "wind should pile water downwind (right-left setup={setup:.3} m; left={left1:.3} right={right1:.3})"
    );
    assert!(
        right1 > left1,
        "downwind side should be deeper than upwind (left={left1:.3} right={right1:.3})"
    );
    assert!(
        left1 > 2.0 && right1 > 2.0,
        "basin should retain a free surface (left={left1:.3} right={right1:.3})"
    );
    assert!(drift.abs() <= 80, "bookkeeping drift {drift}");
    assert_no_negative_masses(&world);
    assert!(elapsed.as_secs() < 60, "E49a perf: {:?}", elapsed);
}

#[test]
fn e49b_tide_raises_and_lowers_ocean() {
    let mut world = World::new(9050);
    // `generate_flat_sand` builds a tall sand/stone stack (~20 m bed). Put
    // sea level above that bed so the flooded columns count as oceanic.
    world.sea_level = 30.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.surface_waves_enabled = true;
    world.tide_enabled = true;
    world.tide_amplitude_m = 0.8;
    world.tide_period_ticks = 400;
    world.climate.wind_speed = 0.0;
    for c in -1..=1 {
        world.insert_chunk(generate_flat_sand(c, 0.0, 8.0));
    }
    let sea = world.sea_level;
    for x in -64..128 {
        if let Some(col) = world.column_at_mut(x) {
            col.moisture = col.moisture_cap();
            // Start near sea level so the tide has room to rise/fall.
            let (eta, mass) = col
                .flowable_water()
                .unwrap_or((col.surface_y, 0));
            let bed = eta - mass as f32 / 250.0;
            let target = ((sea - bed).max(1.0) * 250.0) as i64;
            let need = target - mass;
            if need > 0 {
                col.deposit_to_top(MaterialId::Water, need, 0);
            }
        }
    }
    world.wake_all();
    world.recompute_mass_audit();

    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();
    let eta0 = mean_eta(&world, 0, 63);
    assert!(
        eta0 > world.sea_level - 1.0,
        "precondition: flooded near sea level (eta0={eta0:.2} sea={})",
        world.sea_level
    );

    let mut sim = wk_sim::Simulation::new(&world);
    let mut eta_high = eta0;
    let mut eta_low = eta0;
    let start = std::time::Instant::now();
    for _ in 0..800 {
        sim.step(&mut world);
        let e = mean_eta(&world, 0, 63);
        eta_high = eta_high.max(e);
        eta_low = eta_low.min(e);
    }
    let elapsed = start.elapsed();
    let range = eta_high - eta_low;
    let sea_exchanged =
        (world.mass_audit.sea_inject_total - audit0.sea_inject_total).abs();
    let drift = bookkeeping_check(&world, tracked0, audit0);

    eprintln!(
        "E49b: tide η range={range:.3} m (low={eta_low:.3} high={eta_high:.3} start={eta0:.3}) sea_exchange={sea_exchanged} drift={drift} in {:?}",
        elapsed
    );

    assert!(
        range > 0.35,
        "tide should move the free surface by a sizable fraction of amplitude (range={range:.3})"
    );
    assert!(
        sea_exchanged > 100,
        "tide should exchange mass with the shelf (sea_exchange={sea_exchanged})"
    );
    assert!(drift.abs() <= 80, "bookkeeping drift {drift}");
    assert_no_negative_masses(&world);
    assert!(elapsed.as_secs() < 60, "E49b perf: {:?}", elapsed);
}
