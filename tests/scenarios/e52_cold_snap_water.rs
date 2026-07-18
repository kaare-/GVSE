//! Cold-snap hydrology: dropping base temperature from warm to hard
//! freeze must grow a bounded ice skin without flash-flood oscillation.

use wk_material::MaterialId;
use wk_sim::Simulation;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn deep_pool_world(with_thermal: bool) -> World {
    let mut world = World::new(31);
    world.sea_level = 5.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.base_temp_c = 20.0;
    world.climate.day_night_amplitude_c = 0.0;
    for c in 0..2 {
        world.insert_chunk(generate_flat_sand(c, 0.0, 8.0));
    }
    for chunk in world.chunks.values_mut() {
        let bed = chunk.bedrock_y;
        for col in &mut chunk.columns {
            col.deposit_to_top(MaterialId::Water, 5000, 0);
            col.clamp_state();
            col.recompute_surface_y(bed);
        }
    }
    if with_thermal {
        world.enable_thermal_fields();
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

fn totals(world: &World) -> (i64, i64) {
    let mut ice = 0i64;
    let mut water = 0i64;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            for j in 0..col.layer_count as usize {
                match col.layers[j].material {
                    MaterialId::Ice => ice += col.layers[j].thickness,
                    MaterialId::Water => water += col.layers[j].thickness,
                    MaterialId::Snow => {}
                    _ => break,
                }
            }
        }
    }
    (ice, water)
}

fn run_cold_snap(with_thermal: bool) {
    let mut world = deep_pool_world(with_thermal);
    let mut sim = Simulation::new(&world);
    sim.run_ticks(&mut world, 40);

    let (ice_warm, water_warm) = totals(&world);
    assert_eq!(ice_warm, 0, "no ice while warm");
    assert!(water_warm > 100_000, "expected a deep pool, got {water_warm}");

    world.climate.base_temp_c = -20.0;

    let mut water_series = Vec::new();
    let mut ice_series = Vec::new();
    let mut max_flux = 0i64;
    for _ in 0..180 {
        sim.step(&mut world);
        let (ice, water) = totals(&world);
        ice_series.push(ice);
        water_series.push(water);
        for f in &sim.overlay().per_column_flux {
            max_flux = max_flux.max(f.abs());
        }
    }

    let (ice_end, water_end) = totals(&world);
    let tail = &water_series[water_series.len() - 60..];
    let w_min = *tail.iter().min().unwrap();
    let w_max = *tail.iter().max().unwrap();
    let osc = w_max - w_min;
    let ice_max = *ice_series.iter().max().unwrap();
    let n_cols = world.chunks.values().map(|c| c.columns.len()).sum::<usize>() as i64;

    assert!(
        ice_end > 0,
        "expected ice after cold snap (thermal={with_thermal})"
    );
    assert!(
        ice_max <= n_cols * 10_000 + 500,
        "ice tower exceeded cap: ice_max={ice_max} cols={n_cols} thermal={with_thermal}"
    );
    assert!(
        osc < 2_000,
        "water oscillation too large: osc={osc} (min={w_min} max={w_max}) \
         ice_end={ice_end} water_end={water_end} max_flux={max_flux} thermal={with_thermal}"
    );
    assert!(
        max_flux < 4_000,
        "peak water flux too high after cold snap: {max_flux} thermal={with_thermal}"
    );
    // Liquid water should largely remain under the ice skin.
    assert!(
        water_end > water_warm / 2,
        "cold snap locked away too much water as ice: water_end={water_end} \
         water_warm={water_warm} ice_end={ice_end} thermal={with_thermal}"
    );
}

#[test]
fn e52_cold_snap_climate_stable() {
    run_cold_snap(false);
}

#[test]
fn e52_cold_snap_thermal_stable() {
    run_cold_snap(true);
}
