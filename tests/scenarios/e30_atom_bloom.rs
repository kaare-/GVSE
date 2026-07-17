//! E30 — Atom bloom (Organism Kernel Set A).
//!
//! A Nucleus+Photosystem Atom in lit free water should grow population by day
//! and thin at night; mass audit stays closed. Plankton require water.

use crate::helpers::*;
use wk_material::MaterialId;
use wk_sim::{Blueprint, Genome};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e30_atom_bloom() {
    let mut world = World::new(9030);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    // Short day/night so the test covers several cycles quickly.
    world.climate.day_length_ticks = 120;
    world.climate.night_length_ticks = 120;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.lapse_rate_c_per_m = 0.0;
    world.climate.base_temp_c = 18.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    // Plankton need standing water — flood the test bed.
    for x in 0..64 {
        if let Some(col) = world.column_at_mut(x) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
    }
    world.wake_all();
    world.recompute_mass_audit();

    let genome = Genome {
        metabolic_rate: 0.18,
        reproduce_at: 0.65,
        clone_fidelity: 0.6,
        circadian_phase: 0.25, // peak around mid-day in phase_fraction
        active_window: 0.7,
        repro_drive: 0.0, // grazers off
        temp_optimum: 18.0,
        temp_width: 20.0, // wide so short-cycle test isn't temp-starved
        ..Genome::default()
    };
    let bp = Blueprint::atom(genome);

    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();

    let mut sim = wk_sim::Simulation::new(&world);
    sim.agents
        .spawn_from_blueprint(&world, 32, bp, 50.0)
        .expect("spawn atom");
    assert_eq!(sim.agents.organism_count(), 1);

    let mut peak_day = 1usize;
    let mut peak_night = 1usize;
    let mut min_night = usize::MAX;
    let start = std::time::Instant::now();
    for _ in 0..2_000 {
        let tick = sim.clock.tick;
        sim.step(&mut world);
        let n = sim.agents.organism_count();
        if world.climate.is_daytime(tick) {
            peak_day = peak_day.max(n);
        } else {
            peak_night = peak_night.max(n);
            min_night = min_night.min(n);
        }
    }
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs() < 60, "E30 perf: {:?}", elapsed);

    world.recompute_mass_audit();

    assert!(
        sim.agents.births_total > 0,
        "expected reproduction, births={}",
        sim.agents.births_total
    );
    assert!(
        peak_day > 1,
        "day population should grow beyond founder (peak_day={peak_day})"
    );
    assert!(
        min_night <= peak_day,
        "night min {min_night} should not exceed day peak {peak_day}"
    );
    assert!(
        sim.agents.organism_count() >= 1,
        "at least one atom should remain alive (births={})",
        sim.agents.births_total
    );

    let drift = bookkeeping_check(&world, tracked0, audit0);
    assert!(
        drift.abs() <= 80,
        "bookkeeping drift {drift} grow={} decay={}",
        world.mass_audit.biomass_grow_total,
        world.mass_audit.biomass_decay_total
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E30: pop peak_day={peak_day} peak_night={peak_night} min_night={} living={} births={} grow={} drift={drift} in {:?}",
        if min_night == usize::MAX { 0 } else { min_night },
        sim.agents.organism_count(),
        sim.agents.births_total,
        world.mass_audit.biomass_grow_total,
        elapsed
    );
}
