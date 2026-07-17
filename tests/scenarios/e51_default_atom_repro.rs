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
