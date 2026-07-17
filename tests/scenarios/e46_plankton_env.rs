//! E46 — Plankton environment gates (water, ice, CO₂, temperature).
//!
//! Headless falsifiers for the Set A algae environmental physics:
//! dry land / ice death, dissolved-CO₂ bloom drawdown against `run_gas`,
//! and cold-blocked reproduction.

use wk_material::MaterialId;
use wk_sim::{Blueprint, Energy, Genome};
use crate::helpers::assert_no_negative_masses;
use wk_world::column::{EQUIL_WATER_CO2, EQUIL_WATER_O2};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn wet_flat_world(seed: u64, temp_c: f32) -> World {
    let mut world = World::new(seed);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.day_length_ticks = 2_000;
    world.climate.night_length_ticks = 1;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.lapse_rate_c_per_m = 0.0;
    world.climate.base_temp_c = temp_c;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    for x in 0..64 {
        if let Some(col) = world.column_at_mut(x) {
            col.deposit_to_top(MaterialId::Water, 2_500, 0);
            col.ecology.water_co2 = EQUIL_WATER_CO2;
            col.ecology.water_o2 = EQUIL_WATER_O2;
        }
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

fn mean_water_co2(world: &World) -> f32 {
    let mut s = 0.0f32;
    let mut n = 0.0f32;
    for x in 0..64 {
        if let Some(col) = world.column_at(x) {
            s += col.ecology.water_co2;
            n += 1.0;
        }
    }
    s / n.max(1.0)
}

fn mean_water_o2(world: &World) -> f32 {
    let mut s = 0.0f32;
    let mut n = 0.0f32;
    for x in 0..64 {
        if let Some(col) = world.column_at(x) {
            s += col.ecology.water_o2;
            n += 1.0;
        }
    }
    s / n.max(1.0)
}

#[test]
fn e46a_dry_land_kills_plankton() {
    let mut world = World::new(9046);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.base_temp_c = 18.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    world.wake_all();

    let mut sim = wk_sim::Simulation::new(&world);
    sim.agents
        .spawn_from_blueprint(&world, 16, Blueprint::atom(Genome::default()), 50.0)
        .expect("spawn");
    assert_eq!(sim.agents.organism_count(), 1);

    for _ in 0..5 {
        sim.step(&mut world);
    }
    assert_eq!(
        sim.agents.organism_count(),
        0,
        "plankton must die without free water"
    );
    assert!(
        sim.agents.corpse_count() >= 1 || world.column_at(16).unwrap().ecology.dead_biomass > 0,
        "death should leave a corpse or litter"
    );
    eprintln!("E46a: dry land → organism_count=0");
}

#[test]
fn e46b_ice_cap_kills_plankton() {
    // AgentStore path — full `Simulation::step` would melt ice at 18°C
    // via phase_change before the organism tick.
    let mut world = wet_flat_world(9047, 18.0);
    if let Some(col) = world.column_at_mut(20) {
        col.deposit_to_top(MaterialId::Ice, 500, 1);
        col.settle_by_density(2);
        assert!(col.top_ice_mass() > 0);
    }

    let mut store = wk_sim::AgentStore::new();
    store
        .spawn_from_blueprint(&world, 20, Blueprint::atom(Genome::default()), 50.0)
        .expect("spawn under ice");
    store.step_organisms(&mut world, 1);
    assert_eq!(
        store.organism_count(),
        0,
        "ice cap must kill plankton even with water below"
    );
    eprintln!("E46b: ice cap → organism_count=0");
}

#[test]
fn e46c_bloom_draws_down_dissolved_co2() {
    let mut world = wet_flat_world(9048, 18.0);
    let co2_0 = mean_water_co2(&world);
    let o2_0 = mean_water_o2(&world);

    let genome = Genome {
        metabolic_rate: 0.12,
        reproduce_at: 0.55,
        clone_fidelity: 0.7,
        circadian_phase: 0.0,
        active_window: 1.0,
        repro_drive: 0.0,
        temp_optimum: 18.0,
        temp_width: 20.0,
        ..Genome::default()
    };
    let bp = Blueprint::atom(genome);

    let mut sim = wk_sim::Simulation::new(&world);
    // Seed a dense band so drawdown beats air↔water recharge.
    for x in 2..62 {
        let _ = sim.agents.spawn_from_blueprint(&world, x, bp.clone(), 50.0);
    }
    let seeded = sim.agents.organism_count();
    assert!(seeded >= 24, "seeded={seeded}");

    let start = std::time::Instant::now();
    let mut min_co2 = co2_0;
    for _ in 0..1_200 {
        sim.step(&mut world);
        min_co2 = min_co2.min(mean_water_co2(&world));
    }
    let elapsed = start.elapsed();
    let co2_1 = mean_water_co2(&world);
    let o2_1 = mean_water_o2(&world);
    let living = sim.agents.organism_count();

    assert!(
        min_co2 < co2_0 - 0.10,
        "bloom must pull dissolved CO₂ below recharge (start={co2_0:.3} min={min_co2:.3} end={co2_1:.3} living={living})"
    );
    assert!(
        o2_1 > o2_0 + 0.04,
        "bloom should raise dissolved O₂ (start={o2_0:.3} end={o2_1:.3})"
    );
    assert!(elapsed.as_secs() < 90, "E46c perf: {:?}", elapsed);
    assert_no_negative_masses(&world);

    eprintln!(
        "E46c: co2 {co2_0:.3}→min {min_co2:.3}→{co2_1:.3}  o2 {o2_0:.3}→{o2_1:.3}  living={living} births={} in {:?}",
        sim.agents.births_total, elapsed
    );
}

#[test]
fn e46d_cold_blocks_reproduction() {
    // Stay above 0°C so phase_change doesn't ice the water (that would
    // kill plankton via the ice gate — a different test). 3°C + narrow
    // comfort band is enough to block fission (comfort < 0.20).
    let mut world = wet_flat_world(9049, 3.0);
    let genome = Genome {
        metabolic_rate: 0.1,
        reproduce_at: 0.45,
        circadian_phase: 0.0,
        active_window: 1.0,
        repro_drive: 0.0,
        temp_optimum: 22.0,
        temp_width: 6.0,
        ..Genome::default()
    };

    let mut sim = wk_sim::Simulation::new(&world);
    sim.agents
        .spawn_from_blueprint(&world, 32, Blueprint::atom(genome), 50.0)
        .expect("spawn");

    for _ in 0..600 {
        // Keep the founder topped up so only temperature can block fission.
        for (_e, energy) in sim.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
        sim.step(&mut world);
        assert!(
            world.column_at(32).unwrap().top_ice_mass() == 0,
            "test setup must stay ice-free"
        );
    }

    assert_eq!(
        sim.agents.births_total, 0,
        "cold (but unfrozen) water must block plankton fission"
    );
    assert_eq!(sim.agents.organism_count(), 1);

    // Control: same genome in warm water should reproduce.
    let mut warm = wet_flat_world(9050, 22.0);
    let mut sim_w = wk_sim::Simulation::new(&warm);
    sim_w
        .agents
        .spawn_from_blueprint(&warm, 32, Blueprint::atom(genome), 50.0)
        .expect("spawn warm");
    for _ in 0..600 {
        for (_e, energy) in sim_w.agents.ecs.query_mut::<&mut Energy>() {
            energy.current = energy.max;
        }
        sim_w.step(&mut warm);
    }
    assert!(
        sim_w.agents.births_total > 0,
        "warm control must reproduce (births={})",
        sim_w.agents.births_total
    );

    eprintln!(
        "E46d: cold births={} warm births={}",
        sim.agents.births_total, sim_w.agents.births_total
    );
}
