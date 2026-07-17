//! E31 — Messy boom (Organism Kernel Set A).
//!
//! Two Atom lineages share a flooded sand bed. The "messy" lineage
//! (low ReproduceAt + low CloneFidelity) should overshoot then crash;
//! the "thrifty" lineage (high ReproduceAt + high CloneFidelity) should
//! hold a higher steady-state population after crossover.

use crate::helpers::*;
use wk_material::MaterialId;
use wk_sim::{Blueprint, Genome};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

const MESSY: u8 = 1;
const THRIFTY: u8 = 2;

#[test]
fn e31_messy_boom() {
    let mut world = World::new(9031);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    // Short day/night so several cycles fit in the soak.
    world.climate.day_length_ticks = 120;
    world.climate.night_length_ticks = 120;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.lapse_rate_c_per_m = 0.0;
    world.climate.base_temp_c = 18.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    for x in 0..64 {
        if let Some(col) = world.column_at_mut(x) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
        }
    }
    world.wake_all();
    world.recompute_mass_audit();

    let shared = Genome {
        metabolic_rate: 0.18,
        circadian_phase: 0.25,
        active_window: 0.7,
        repro_drive: 0.0,
        temp_optimum: 18.0,
        temp_width: 20.0,
        ..Genome::default()
    };
    let messy_genome = Genome {
        reproduce_at: 0.38,
        clone_fidelity: 0.12,
        ..shared
    };
    let thrifty_genome = Genome {
        reproduce_at: 0.88,
        clone_fidelity: 0.95,
        ..shared
    };

    let mut messy_bp = Blueprint::atom(messy_genome);
    messy_bp.name = "messy".into();
    let mut thrifty_bp = Blueprint::atom(thrifty_genome);
    thrifty_bp.name = "thrifty".into();

    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();

    let mut sim = wk_sim::Simulation::new(&world);
    let messy_e = sim
        .agents
        .spawn_from_blueprint(&world, 20, messy_bp, 50.0)
        .expect("spawn messy");
    let thrifty_e = sim
        .agents
        .spawn_from_blueprint(&world, 44, thrifty_bp, 50.0)
        .expect("spawn thrifty");
    sim.agents.tag_founder(messy_e, MESSY);
    sim.agents.tag_founder(thrifty_e, THRIFTY);
    assert_eq!(sim.agents.organism_count(), 2);

    let soak = 5_000u64;
    let mut messy_peak = 1usize;
    let mut thrifty_peak = 1usize;
    let mut crossover: Option<u64> = None;
    let mut messy_led = false;
    let mut end_messy = 1usize;
    let mut end_thrifty = 1usize;

    let start = std::time::Instant::now();
    for _ in 0..soak {
        let tick = sim.clock.tick;
        sim.step(&mut world);
        let m = sim.agents.count_living_by_founder(MESSY);
        let t = sim.agents.count_living_by_founder(THRIFTY);
        messy_peak = messy_peak.max(m);
        thrifty_peak = thrifty_peak.max(t);
        if m > t {
            messy_led = true;
        }
        if crossover.is_none() && messy_led && t > m {
            crossover = Some(tick);
        }
        end_messy = m;
        end_thrifty = t;
    }
    let elapsed = start.elapsed();
    assert!(elapsed.as_secs() < 90, "E31 perf: {:?}", elapsed);

    world.recompute_mass_audit();
    let drift = bookkeeping_check(&world, tracked0, audit0);
    assert_no_negative_masses(&world);

    eprintln!(
        "E31: messy peak={messy_peak}, thrifty peak={thrifty_peak}, crossover tick={}, end messy={end_messy} thrifty={end_thrifty}, births={} drift={drift} in {:?}",
        crossover.map(|t| t.to_string()).unwrap_or_else(|| "none".into()),
        sim.agents.births_total,
        elapsed
    );

    assert!(
        messy_peak > thrifty_peak,
        "messy should overshoot thrifty (messy_peak={messy_peak} thrifty_peak={thrifty_peak})"
    );
    assert!(
        messy_peak >= 4,
        "messy boom too weak (peak={messy_peak})"
    );
    let crossover_tick = crossover.expect("expected messy→thrifty crossover after boom");
    assert!(
        crossover_tick < soak,
        "crossover should occur within soak (tick={crossover_tick})"
    );
    assert!(
        end_thrifty > end_messy,
        "thrifty steady-state should beat messy (end thrifty={end_thrifty} messy={end_messy})"
    );
    assert!(
        drift.abs() <= 120.0,
        "bookkeeping drift {drift}"
    );
}
