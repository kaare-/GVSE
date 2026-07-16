//! E16 — scripted grazer eats biomass (stage 10).
//!
//! A grazer on a wet vegetated band should reduce alive biomass, book
//! `biomass_eaten_total`, and remain alive with positive energy.

use crate::helpers::*;
use wk_material::CHUNK_W;
use wk_sim::Genome;
use wk_world::column::Ecology;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e16_grazer_eats_biomass() {
    let mut world = World::new(9016);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.base_temp_c = 18.0;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.lapse_rate_c_per_m = 0.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    world.wake_all();

    // Lush wet band for forage.
    let band = 16..40;
    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        for i in 0..CHUNK_W {
            let col = &mut chunk.columns[i];
            if band.contains(&i) {
                col.ecology = Ecology {
                    root_density: 0.6,
                    leaf_area: 0.6,
                    dead_biomass: 0,
                    alive_biomass: 2_000,
                    nutrient: 0.8,
                };
                col.moisture = col.moisture_cap().max(1);
            } else {
                col.ecology = Ecology::default();
                col.moisture = 0;
            }
        }
    }
    world.recompute_mass_audit();

    let wx = 28i32;
    let alive0: i64 = band
        .clone()
        .map(|i| world.chunks[&0].columns[i].ecology.alive_biomass)
        .sum();
    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();

    let mut sim = wk_sim::Simulation::new(&world);
    let genome = Genome {
        move_speed: 0.4,
        graze_rate: 60.0,
        drink_rate: 40.0,
        dig_drive: 0.0, // keep the test on foraging
        graze_efficiency: 0.1,
        metabolism: 0.2,
    };
    sim.agents
        .spawn_grazer(&world, wx, genome, 50.0)
        .expect("spawn");
    assert_eq!(sim.agents.grazer_count(), 1);

    let elapsed = run_ticks(&mut world, &mut sim, 2_000);
    assert!(elapsed.as_secs() < 60, "E16 perf: {:?}", elapsed);

    world.recompute_mass_audit();
    let alive1: i64 = band
        .map(|i| world.chunks[&0].columns[i].ecology.alive_biomass)
        .sum();

    assert!(
        sim.agents.grazer_count() >= 1,
        "grazer should still be alive"
    );
    assert!(
        sim.agents.total_energy() > 1.0,
        "grazer should have energy left"
    );
    assert!(
        alive1 < alive0,
        "forage should reduce band biomass: {alive0} → {alive1}"
    );
    assert!(
        world.mass_audit.biomass_eaten_total > 0,
        "biomass_eaten_total should increase"
    );
    // Host chunk must stay in the keep-awake set while the agent lives.
    assert!(
        !world.agent_keep_awake.is_empty(),
        "agent_keep_awake should list host columns"
    );

    let drift = bookkeeping_check(&world, tracked0, audit0);
    assert!(
        drift.abs() <= 80,
        "bookkeeping drift {drift} (eaten={})",
        world.mass_audit.biomass_eaten_total
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E16: alive {alive0}→{alive1} eaten={} energy={:.1} grazers={} drift={drift} in {:?}",
        world.mass_audit.biomass_eaten_total,
        sim.agents.total_energy(),
        sim.agents.grazer_count(),
        elapsed
    );
}
