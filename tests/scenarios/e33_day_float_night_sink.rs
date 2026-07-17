//! E33 — Day float / night sink (Set C vertical niche, Set A interim).
//!
//! Circadian-modulated buoyancy: default Atoms ride the warm mixed layer
//! while their active window is open, then sink into cooler water at night.

use wk_material::MaterialId;
use wk_sim::{Blueprint, Genome};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn deep_ocean(seed: u64) -> World {
    let mut world = World::new(seed);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    // Short cycles so the soak covers several migrations quickly.
    world.climate.day_length_ticks = 240;
    world.climate.night_length_ticks = 240;
    world.climate.day_night_amplitude_c = 4.0;
    world.climate.base_temp_c = 20.0;
    for c in -1..=1 {
        world.insert_chunk(generate_flat_sand(c, -80.0, 8.0));
    }
    let sea = world.sea_level;
    for x in -64..128 {
        if let Some(col) = world.column_at_mut(x) {
            col.moisture = col.moisture_cap();
            let (eta, mass) = col.flowable_water().unwrap_or((col.surface_y, 0));
            let bed = eta - mass as f32 / 250.0;
            let target = ((sea - bed).max(40.0) * 250.0) as i64;
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
fn e33_day_float_night_sink() {
    let mut world = deep_ocean(9033);
    let genome = Genome {
        metabolic_rate: 0.15,
        reproduce_at: 0.99, // no fission — track one body
        circadian_phase: 0.25,
        active_window: 0.55,
        buoyancy_bias: 0.0,
        temp_width: 20.0,
        ..Genome::default()
    };
    let mut sim = wk_sim::Simulation::new(&world);
    let e = sim
        .agents
        .spawn_from_blueprint(&world, 32, Blueprint::atom(genome), 50.0)
        .expect("spawn");

    let mut day_sum = 0.0f32;
    let mut day_n = 0u32;
    let mut night_sum = 0.0f32;
    let mut night_n = 0u32;
    let mut surface_t = 0.0f32;
    let mut deep_t = 0.0f32;
    let mut temp_n = 0u32;

    let start = std::time::Instant::now();
    for _ in 0..2_400 {
        let tick = sim.clock.tick;
        sim.step(&mut world);
        let Some(info) = sim.agents.inspect_organism(e) else {
            panic!("organism died unexpectedly");
        };
        if world.climate.is_daytime(tick) {
            day_sum += info.y;
            day_n += 1;
        } else {
            night_sum += info.y;
            night_n += 1;
        }
        if tick % 120 == 0 {
            let sea = world.sea_level;
            surface_t += world.temperature_at_point(32, sea - 1.0, tick);
            deep_t += world.temperature_at_point(32, sea - 40.0, tick);
            temp_n += 1;
        }
    }
    let elapsed = start.elapsed();
    assert!(day_n > 0 && night_n > 0);
    let day_y = day_sum / day_n as f32;
    let night_y = night_sum / night_n as f32;
    let surf = surface_t / temp_n.max(1) as f32;
    let deep = deep_t / temp_n.max(1) as f32;

    eprintln!(
        "E33: day_y={day_y:.2} night_y={night_y:.2} Δ={:.2}  skin={surf:.1}C deep={deep:.1}C in {:?}",
        day_y - night_y,
        elapsed
    );

    assert!(
        day_y > night_y + 2.0,
        "should float higher by day than night (day={day_y:.2} night={night_y:.2})"
    );
    assert!(
        surf > deep + 2.0,
        "mixed layer should stay warmer than deep water (skin={surf:.1} deep={deep:.1})"
    );
    assert!(elapsed.as_secs() < 60, "E33 perf: {:?}", elapsed);
}
