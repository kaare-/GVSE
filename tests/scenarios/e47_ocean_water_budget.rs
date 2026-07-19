//! E47 — Ocean / water-table hydrology budget.
//!
//! Headless falsifiers for the open-water mass leaks (depth-proportional
//! evaporation + deep infiltration into dry beds + fake bathymetric
//! groundwater head) and for seeding a solid underground water table at
//! continental generation.

use wk_material::{CHUNK_W, MaterialId};
use wk_world::terrain::{
    generate_chunk_continental, generate_flat_sand, BEDROCK_FLOOR_M,
};
use wk_world::world::World;

use crate::helpers::assert_no_negative_masses;

fn total_surface_water(world: &World) -> i64 {
    world
        .chunks
        .values()
        .flat_map(|c| c.columns.iter())
        .map(|c| c.top_water_mass())
        .sum()
}

fn total_moisture(world: &World) -> i64 {
    world
        .chunks
        .values()
        .flat_map(|c| c.columns.iter())
        .map(|c| c.moisture)
        .sum()
}

fn mean_sat_under_deep_water(world: &World) -> f32 {
    let mut s = 0.0f32;
    let mut n = 0.0f32;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            if col.top_water_mass() > 500 {
                let cap = col.moisture_cap().max(1) as f32;
                s += col.moisture as f32 / cap;
                n += 1.0;
            }
        }
    }
    s / n.max(1.0)
}

fn mean_land_moisture_sat(world: &World) -> f32 {
    let mut s = 0.0f32;
    let mut n = 0.0f32;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            if col.climate_elevation() > world.sea_level + 1.0 && col.moisture_cap() > 0 {
                let cap = col.moisture_cap() as f32;
                s += col.moisture as f32 / cap;
                n += 1.0;
            }
        }
    }
    s / n.max(1.0)
}

/// Ocean → shelf → coast → plains. Needs enough world-x for emergent land
/// (continental coast begins ~260 m ≈ chunk 16+).
fn setup_continental_strip(seed: u64, chunk_lo: i32, chunk_hi: i32) -> World {
    let mut world = World::new(seed);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    for c in chunk_lo..chunk_hi {
        world.insert_chunk(generate_chunk_continental(
            c,
            world.seed,
            BEDROCK_FLOOR_M,
            world.sea_level,
        ));
    }
    world.wake_all();
    world.recompute_mass_audit();
    world.enable_humidity_fields();
    world.enable_pressure_wind_fields();
    world
}

#[test]
fn e47a_water_table_seeded_at_generation() {
    // Chunks covering abyss through plains (~0–500 m).
    let world = setup_continental_strip(4701, 0, 28);
    let ocean_sat = mean_sat_under_deep_water(&world);
    let land_sat = mean_land_moisture_sat(&world);
    let moist = total_moisture(&world);
    let land_cols = world
        .chunks
        .values()
        .flat_map(|c| c.columns.iter())
        .filter(|c| c.climate_elevation() > world.sea_level + 1.0)
        .count();

    assert!(land_cols > 64, "fixture must include emergent land ({land_cols} cols)");
    assert!(
        moist > 50_000,
        "continental gen must seed a real aquifer (moisture={moist})"
    );
    assert!(
        ocean_sat > 0.95,
        "ocean beds must start saturated (sat={ocean_sat:.3})"
    );
    assert!(
        land_sat > 0.08,
        "emergent land must hold base pore water (sat={land_sat:.3})"
    );

    eprintln!(
        "E47a: moisture={} ocean_sat={:.3} land_sat={:.3} land_cols={}",
        moist, ocean_sat, land_sat, land_cols
    );
}

#[test]
fn e47b_deep_water_evap_matches_shallow_skin() {
    // Same surface area, very different depths → similar evaporative loss.
    let mut deep = World::new(4702);
    deep.sea_level = 0.0;
    deep.rain_enabled = false;
    deep.weather.weather_enabled = false;
    deep.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    for x in 0..CHUNK_W as i32 {
        if let Some(col) = deep.column_at_mut(x) {
            col.deposit_to_top(MaterialId::Water, 20_000, 0);
            col.moisture = col.moisture_cap();
        }
    }
    deep.wake_all();
    deep.recompute_mass_audit();

    let mut shallow = World::new(4703);
    shallow.sea_level = 0.0;
    shallow.rain_enabled = false;
    shallow.weather.weather_enabled = false;
    shallow.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    for x in 0..CHUNK_W as i32 {
        if let Some(col) = shallow.column_at_mut(x) {
            col.deposit_to_top(MaterialId::Water, 80, 0);
            col.moisture = col.moisture_cap();
        }
    }
    shallow.wake_all();
    shallow.recompute_mass_audit();

    let m0 = total_moisture(&deep);
    let _s0 = total_surface_water(&shallow);
    let ev_d0 = deep.mass_audit.evap_out_total;
    let ev_s0 = shallow.mass_audit.evap_out_total;
    let mut sim_d = wk_sim::Simulation::new(&deep);
    let mut sim_s = wk_sim::Simulation::new(&shallow);
    // Evaporation period is 60 ticks — run long enough for a clear signal.
    for _ in 0..3_600 {
        sim_d.step(&mut deep);
        sim_s.step(&mut shallow);
    }
    let d_evap = deep.mass_audit.evap_out_total - ev_d0;
    let s_evap = shallow.mass_audit.evap_out_total - ev_s0;

    assert!(d_evap > 50, "deep pond should evaporate (evap={d_evap})");
    assert!(s_evap > 50, "shallow puddle should evaporate (evap={s_evap})");
    // Depth-proportional bug made deep lose ~250× shallow; skin form keeps
    // them within a small factor (humidity / residual jitter).
    let ratio = d_evap as f64 / (s_evap as f64).max(1.0);
    assert!(
        ratio < 3.0,
        "deep evap should track shallow skin, not depth (deep={d_evap} shallow={s_evap} ratio={ratio:.2})"
    );
    let m1 = total_moisture(&deep);
    assert!(
        (m1 as f64) > (m0 as f64) * 0.5,
        "deep pond aquifer should mostly hold (m0={m0} m1={m1})"
    );

    eprintln!(
        "E47b: deep_evap={} shallow_evap={} ratio={:.2} moist {}→{}",
        d_evap, s_evap, ratio, m0, m1
    );
}

#[test]
fn e47c_continental_ocean_holds_over_hour() {
    let mut world = setup_continental_strip(4704, 0, 28);
    let w0 = total_surface_water(&world);
    let m0 = total_moisture(&world);
    let sat0 = mean_sat_under_deep_water(&world);
    let rain0 = world.mass_audit.rain_inject_total;
    let evap0 = world.mass_audit.evap_out_total;
    assert!(w0 > 1_000_000, "expected a real ocean mass, got {w0}");
    assert!(sat0 > 0.95, "precondition: saturated beds, sat={sat0:.3}");

    let mut sim = wk_sim::Simulation::new(&world);
    let ticks = 3_600u64;
    let start = std::time::Instant::now();
    for _ in 0..ticks {
        sim.step(&mut world);
    }
    let elapsed = start.elapsed();

    let w1 = total_surface_water(&world);
    let m1 = total_moisture(&world);
    let sat1 = mean_sat_under_deep_water(&world);
    let rain = world.mass_audit.rain_inject_total - rain0;
    let evap = world.mass_audit.evap_out_total - evap0;
    let frac_loss = (w0 - w1) as f64 / w0 as f64;

    assert_no_negative_masses(&world);
    assert!(
        frac_loss < 0.05,
        "ocean must not drain >5%/hour (loss={:.1}%, Δsurf={}, rain={}, evap={}, clouds={})",
        frac_loss * 100.0,
        w1 - w0,
        rain,
        evap,
        world.clouds.len()
    );
    assert!(
        sat1 > 0.50,
        "ocean bed water table must stay wet (sat={sat1:.3})"
    );
    // Aquifer must not dump into the sea (the bathymetric-head bug).
    assert!(
        (m1 as f64) > (m0 as f64) * 0.50,
        "seeded aquifer must hold (m0={m0} m1={m1})"
    );
    assert!(
        rain > 2_000,
        "weather rain too sparse (rain={rain} evap={evap} clouds={})",
        world.clouds.len()
    );
    assert!(elapsed.as_secs() < 120, "E47c perf: {:?}", elapsed);

    eprintln!(
        "E47c: Δsurf={} ({:.2}%) rain={} evap={} moist {}→{} sat {:.3}→{:.3} clouds={} in {:?}",
        w1 - w0,
        frac_loss * 100.0,
        rain,
        evap,
        m0,
        m1,
        sat0,
        sat1,
        world.clouds.len(),
        elapsed
    );
}
