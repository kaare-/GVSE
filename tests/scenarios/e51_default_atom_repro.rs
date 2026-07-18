//! E51 — Default editor Atom must fission under app-like thermal ocean.
//!
//! Regression: geothermal-seeded thermal fields left near-surface water
//! ~35–40 °C on shallow bedrock floors (sky end of the seed gradient was
//! the top of the air domain). That hard-gated fission while photosynthesis
//! could still top up energy — peak energy, zero clones.

use wk_material::MaterialId;
use wk_sim::{temp_comfort_factor, Blueprint, Energy, Genome};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn flooded_ocean(seed: u64, bedrock_y: f32) -> World {
    let mut world = World::new(seed);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    for c in -1..=1 {
        world.insert_chunk(generate_flat_sand(c, bedrock_y, 8.0));
    }
    let sea = world.sea_level;
    for x in -64..128 {
        if let Some(col) = world.column_at_mut(x) {
            col.moisture = col.moisture_cap();
            let (eta, mass) = col.flowable_water().unwrap_or((col.surface_y, 0));
            let bed = eta - mass as f32 / 250.0;
            let target = ((sea - bed).max(2.0) * 250.0) as i64;
            let need = target - mass;
            if need > 0 {
                col.deposit_to_top(MaterialId::Water, need, 0);
            }
        }
    }
    world.wake_all();
    world.enable_thermal_fields();
    world.recompute_mass_audit();
    world
}

#[test]
fn e51a_default_atom_fissions_in_app_ocean() {
    // Shallow bedrock is the hostile case that used to sterilise Atoms.
    let mut world = flooded_ocean(9051, -20.0);
    let mut sim = wk_sim::Simulation::new(&world);
    let e = sim
        .agents
        .spawn_from_blueprint(&world, 32, Blueprint::atom(Genome::default()), 50.0)
        .expect("spawn");

    let mut min_comfort = f32::MAX;
    let mut max_temp = f32::MIN;
    let start = std::time::Instant::now();
    for _ in 0..8_000 {
        let tick = sim.clock.tick;
        if let Some(info) = sim.agents.inspect_organism(e) {
            let t = world.temperature_at_point(info.x.floor() as i32, info.y, tick);
            let c = temp_comfort_factor(t, &info.genome);
            min_comfort = min_comfort.min(c);
            max_temp = max_temp.max(t);
        }
        // Keep energy peaked so only environment gates can block fission.
        for (_ent, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
        sim.step(&mut world);
    }
    let elapsed = start.elapsed();
    eprintln!(
        "E51a: births={} clones={} max_temp={max_temp:.2} min_comfort={min_comfort:.3} in {:?}",
        sim.agents.births_total,
        sim.agents
            .inspect_organism(e)
            .map(|i| i.clones_produced)
            .unwrap_or(0),
        elapsed
    );
    assert!(
        sim.agents.births_total > 0,
        "default Atom should fission with peak energy (max_temp={max_temp:.1} min_comfort={min_comfort:.3})"
    );
    assert!(
        max_temp < 36.0,
        "surface water should stay near climate+solar, not geothermal (~40C+); max_temp={max_temp:.1}"
    );
    assert!(elapsed.as_secs() < 60, "E51a perf: {:?}", elapsed);
}

#[test]
fn e51b_deep_ocean_ambient_not_geothermal() {
    // Abyssal sand bed under a full ocean column: solid bed sits below the
    // thermal field floor (sea − 100 m). Sampling climate_elevation there
    // clamps to the geothermal Dirichlet (55°C) — the "boiling next to
    // ice" HUD bug. Ambient must use the free surface instead.
    let mut world = World::new(9052);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.geothermal_bottom_c = 55.0;
    world.climate.base_temp_c = 0.0;
    world.climate.day_night_amplitude_c = 0.0;
    let bed_y = -200.0;
    for c in -1..=1 {
        world.insert_chunk(generate_flat_sand(c, -900.0, bed_y));
    }
    let sea = world.sea_level;
    for x in -64..128 {
        if let Some(col) = world.column_at_mut(x) {
            col.moisture = col.moisture_cap();
            let need = ((sea - bed_y).max(1.0) * 250.0) as i64;
            col.deposit_to_top(MaterialId::Water, need, 0);
        }
    }
    world.wake_all();
    world.enable_thermal_fields();

    let wx = 32;
    let col = world.column_at(wx).expect("column");
    let bed = col.climate_elevation();
    let skin = col.ambient_elevation(world.sea_level);
    assert!(
        bed < world.sea_level - 100.0,
        "expected abyssal bed below field floor, bed={bed:.1}"
    );
    assert!(
        (skin - world.sea_level).abs() <= 8.0,
        "ocean ambient should sit near sea level, skin={skin:.1}"
    );

    let bed_t = world.temperature_at_point(wx, bed, 0);
    let skin_t = world.temperature_at_point(wx, skin, 0);

    assert!(
        (bed_t - world.geothermal_bottom_c).abs() < 5.0,
        "bed sample should hit geothermal clamp, got {bed_t:.1}"
    );
    assert!(
        skin_t < 30.0,
        "free-surface ocean must not read geothermal, got {skin_t:.1}"
    );
    eprintln!("E51b: bed={bed:.1}m → {bed_t:.1}C; skin={skin:.1}m → {skin_t:.1}C");
}

#[test]
fn e51c_ice_tower_is_capped() {
    // Regression: snow was capped but melt→refreeze stacked unbounded ice
    // (megametre "ice columns" on cold mountains). Phase-change must cull
    // excess and refuse further water→ice once the frozen budget is full.
    use wk_sim::subsystems::run_phase_change;

    let mut world = World::new(9053);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.base_temp_c = -10.0;
    world.climate.day_night_amplitude_c = 0.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 40.0));
    {
        let col = world.column_at_mut(8).unwrap();
        col.deposit_to_top(MaterialId::Ice, 50_000_000, 0);
        col.recompute_surface_y(0.0);
    }
    world.wake_all();
    assert!(
        world.column_at(8).unwrap().frozen_surface_mass() > 10_000,
        "precondition: oversized ice tower"
    );

    run_phase_change(&mut world, 0);
    let col = world.column_at(8).unwrap();
    let frozen = col.frozen_surface_mass();
    assert!(
        frozen <= 10_000,
        "ice tower must cull down to frozen cap, got {frozen}"
    );
    assert!(
        col.surface_y < 100.0,
        "surface must leave megametre range, surface_y={:.1}",
        col.surface_y
    );
    assert!(
        col.top_water_mass() < 50_000,
        "cull must not replace ice with an equal water tower"
    );
    eprintln!(
        "E51c: frozen={frozen} surface_y={:.1} water={}",
        col.surface_y,
        col.top_water_mass()
    );
}
