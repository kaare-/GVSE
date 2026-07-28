//! E17 — reproduction with genome mutation (stage 11).
//!
//! A well-fed founder on a lush wet band should fission; offspring
//! carry mutated genomes. No global fitness function — just energy
//! thresholds + deterministic mutation.

use crate::helpers::*;
use wk_world::CHUNK_W;
use wk_sim::Genome;
use wk_world::column::Ecology;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e17_reproduction_mutates() {
    let mut world = World::new(9017);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.base_temp_c = 18.0;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.lapse_rate_c_per_m = 0.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    world.wake_all();

    // Wide lush band so a small breeding population can forage.
    let band = 8..56;
    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        for i in 0..CHUNK_W {
            let col = &mut chunk.columns[i];
            if band.contains(&i) {
                col.ecology = Ecology {
                    root_density: 0.7,
                    leaf_area: 0.7,
                    dead_biomass: 0,
                    alive_biomass: 4_000,
                    nutrient: 0.9,
                    ..Ecology::default()
                };
                col.moisture = col.moisture_cap().max(1);
            } else {
                col.ecology = Ecology::default();
                col.moisture = 0;
            }
        }
    }
    world.recompute_mass_audit();

    let founder = Genome {
        move_speed: 0.3,
        graze_rate: 35.0,
        drink_rate: 40.0,
        dig_drive: 0.0,
        graze_efficiency: 0.14,
        metabolic_rate: 0.12,
        repro_drive: 1.0, // always attempt when due / energetic
        ..Genome::default()
    };
    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();

    let mut sim = wk_sim::Simulation::new(&world);
    // Start below the repro threshold so the founder must forage first.
    sim.agents
        .spawn_grazer_energy(&world, 32, founder, 30.0, 60.0)
        .expect("spawn founder");
    assert_eq!(sim.agents.grazer_count(), 1);

    let start = std::time::Instant::now();
    let mut peak_pop = 1usize;
    let mut saw_mutant = false;
    // Step until we have seen a birth + mutant, or hit the budget.
    for _ in 0..2_000 {
        sim.step(&mut world);
        peak_pop = peak_pop.max(sim.agents.grazer_count());
        if sim
            .agents
            .genomes()
            .iter()
            .any(|g| g.differs_from(founder))
        {
            saw_mutant = true;
        }
        if sim.agents.births_total > 0
            && peak_pop > 1
            && saw_mutant
            && world.mass_audit.biomass_eaten_total > 0
        {
            break;
        }
    }
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs() < 60, "E17 perf: {:?}", elapsed);

    world.recompute_mass_audit();

    assert!(
        sim.agents.births_total > 0,
        "expected at least one birth, got {}",
        sim.agents.births_total
    );
    assert!(
        peak_pop > 1,
        "population should have grown beyond the founder (peak={peak_pop})"
    );
    assert!(saw_mutant, "at least one offspring genome should differ from founder");
    assert!(
        world.mass_audit.biomass_eaten_total > 0,
        "breeders should still forage"
    );

    let drift = bookkeeping_check(&world, tracked0, audit0);
    assert!(
        drift.abs() <= 80,
        "bookkeeping drift {drift} (eaten={})",
        world.mass_audit.biomass_eaten_total
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E17: births={} peak_pop={} living={} eaten={} mean_meta={:.3} mutant={saw_mutant} drift={drift} in {:?}",
        sim.agents.births_total,
        peak_pop,
        sim.agents.grazer_count(),
        world.mass_audit.biomass_eaten_total,
        sim.agents.mean_metabolism(),
        elapsed
    );
}
