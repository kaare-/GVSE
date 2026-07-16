use std::time::Instant;

use wk_sim::Simulation;
use wk_world::terrain::{
    generate_chunk_hill, generate_chunk_stratified_tilt, generate_flat_sand,
};
use wk_world::world::World;

pub fn setup_hill_world(columns: usize) -> World {
    let mut world = World::new(12345);
    world.sea_level = 0.0;
    world.rain_enabled = true;
    // Keep these hand-crafted scenarios purely rain-toggle-driven — the
    // automatic cloud-based weather system defaults on for the live app but
    // would add uncontrolled extra precipitation to these deterministic
    // experiments otherwise.
    world.weather.weather_enabled = false;
    world.rain_rate = 80.0;
    world.wake_all();

    let chunks_needed = (columns + wk_material::CHUNK_W - 1) / wk_material::CHUNK_W;
    let center = (columns / 2) as i32;
    for c in 0..chunks_needed as i32 {
        let chunk = generate_chunk_hill(c, world.seed, center, 0.0);
        world.insert_chunk(chunk);
    }
    world.recompute_mass_audit();
    world
}

pub fn setup_basin_world() -> World {
    let mut world = World::new(54321);
    world.sea_level = 2.0;
    world.rain_enabled = true;
    // Keep these hand-crafted scenarios purely rain-toggle-driven — the
    // automatic cloud-based weather system defaults on for the live app but
    // would add uncontrolled extra precipitation to these deterministic
    // experiments otherwise.
    world.weather.weather_enabled = false;
    world.rain_rate = 100.0;
    let chunk = wk_world::terrain::generate_chunk_basin(0, world.seed, 32, 0.0);
    world.insert_chunk(chunk);
    world.wake_all();
    world.recompute_mass_audit();
    world
}

pub fn setup_river_world() -> World {
    let mut world = World::new(99999);
    world.sea_level = 5.0;
    world.rain_enabled = true;
    // Keep these hand-crafted scenarios purely rain-toggle-driven — the
    // automatic cloud-based weather system defaults on for the live app but
    // would add uncontrolled extra precipitation to these deterministic
    // experiments otherwise.
    world.weather.weather_enabled = false;
    world.rain_rate = 120.0;
    for c in 0..4 {
        let chunk = generate_chunk_stratified_tilt(c, world.seed, 0.0, 0.02);
        world.insert_chunk(chunk);
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

pub fn setup_delta_world() -> World {
    let mut world = World::new(77777);
    world.sea_level = 15.0;
    world.rain_enabled = true;
    // Keep these hand-crafted scenarios purely rain-toggle-driven — the
    // automatic cloud-based weather system defaults on for the live app but
    // would add uncontrolled extra precipitation to these deterministic
    // experiments otherwise.
    world.weather.weather_enabled = false;
    world.rain_rate = 150.0;
    for c in -1..3 {
        let chunk = generate_chunk_stratified_tilt(c, world.seed, 0.0, 0.015);
        world.insert_chunk(chunk);
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

pub fn setup_flat_sand() -> World {
    let mut world = World::new(11111);
    world.rain_enabled = true;
    // Keep these hand-crafted scenarios purely rain-toggle-driven — the
    // automatic cloud-based weather system defaults on for the live app but
    // would add uncontrolled extra precipitation to these deterministic
    // experiments otherwise.
    world.weather.weather_enabled = false;
    world.rain_rate = 60.0;
    for c in 0..2 {
        world.insert_chunk(generate_flat_sand(c, 0.0, 10.0));
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

pub fn setup_two_chunks() -> World {
    use wk_material::CHUNK_W;
    use wk_world::chunk::Chunk;
    use wk_world::terrain::{fill_column_strata, gaussian_hill_surface};

    let mut world = World::new(22222);
    world.sea_level = 0.0;
    world.rain_enabled = true;
    // Keep these hand-crafted scenarios purely rain-toggle-driven — the
    // automatic cloud-based weather system defaults on for the live app but
    // would add uncontrolled extra precipitation to these deterministic
    // experiments otherwise.
    world.weather.weather_enabled = false;
    world.rain_rate = 100.0;

    let center = 64i32;
    for c in 0..2 {
        let mut chunk = Chunk::new(c, 0.0);
        let base = chunk.world_x_base();
        for i in 0..CHUNK_W {
            let wx = base + i as i32;
            let surface = gaussian_hill_surface(world.seed, wx, center, 30.0, 5.0, 25.0);
            fill_column_strata(&mut chunk.columns[i], surface, 0.0, 5000, 8000, 0);
        }
        world.insert_chunk(chunk);
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

pub fn run_ticks(world: &mut World, sim: &mut Simulation, n: u64) -> std::time::Duration {
    let start = Instant::now();
    sim.run_ticks(world, n);
    start.elapsed()
}

pub fn assert_no_negative_masses(world: &World) {
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            assert!(col.top_water_mass() >= 0, "negative surface water");
            assert!(col.moisture >= 0, "negative moisture");
            assert!(col.sediment.total >= 0, "negative sediment");
            for i in 0..col.layer_count as usize {
                assert!(col.layers[i].thickness >= 0, "negative layer");
            }
            assert!(!col.surface_y.is_nan(), "NaN surface_y");
        }
    }
}

pub fn total_water_mass(world: &World) -> i64 {
    let mut t = 0i64;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            t += col.top_water_mass() + col.moisture;
        }
    }
    t
}

pub fn manual_total(world: &World) -> i64 {
    let mut t = 0i64;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            for i in 0..col.layer_count as usize {
                t += col.layers[i].thickness;
            }
            t += col.moisture + col.sediment.total + col.void_water_total();
            t += col.ecology.biomass_total();
        }
    }
    t + world.mass_audit.dissolved_total
}

pub fn debug_bookkeeping(world: &World, initial_total: i64, initial_audit: &wk_world::world::MassAudit) {
    let rain = world.mass_audit.rain_inject_total - initial_audit.rain_inject_total;
    let sea = world.mass_audit.sea_inject_total - initial_audit.sea_inject_total;
    let evap = world.mass_audit.evap_out_total - initial_audit.evap_out_total;
    let boundary = world.mass_audit.boundary_out_total - initial_audit.boundary_out_total;
    eprintln!(
        "initial={} current={} manual={} audit={} rain={} sea={} evap={} boundary={} drift={}",
        initial_total,
        world.mass_audit.total_tracked(),
        manual_total(world),
        world.mass_audit.total_tracked(),
        rain,
        sea,
        evap,
        boundary,
        bookkeeping_check(world, initial_total, initial_audit.clone())
    );
}

pub fn bookkeeping_check(world: &World, initial_total: i64, initial_audit: wk_world::world::MassAudit) -> i64 {
    let current = world.mass_audit.total_tracked();
    let rain = world.mass_audit.rain_inject_total - initial_audit.rain_inject_total;
    let sea = world.mass_audit.sea_inject_total - initial_audit.sea_inject_total;
    let grow = world.mass_audit.biomass_grow_total - initial_audit.biomass_grow_total;
    let evap = world.mass_audit.evap_out_total - initial_audit.evap_out_total;
    let boundary = world.mass_audit.boundary_out_total - initial_audit.boundary_out_total;
    let decay = world.mass_audit.biomass_decay_total - initial_audit.biomass_decay_total;
    let expected_delta = rain + sea + grow - evap - boundary - decay;
    current - initial_total - expected_delta
}
