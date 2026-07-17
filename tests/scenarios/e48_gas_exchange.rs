//! E48 — Air ↔ dissolved CO₂ / O₂ exchange without organisms.

use wk_material::MaterialId;
use wk_world::column::{
    AMBIENT_AIR_CO2, AMBIENT_AIR_O2, EQUIL_WATER_CO2, EQUIL_WATER_O2,
};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

use crate::helpers::assert_no_negative_masses;

fn wet_world(seed: u64) -> World {
    let mut world = World::new(seed);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    for x in 0..64 {
        if let Some(col) = world.column_at_mut(x) {
            col.deposit_to_top(MaterialId::Water, 2_000, 0);
            col.ecology.air_co2 = AMBIENT_AIR_CO2;
            col.ecology.air_o2 = AMBIENT_AIR_O2;
            col.ecology.water_co2 = EQUIL_WATER_CO2;
            col.ecology.water_o2 = EQUIL_WATER_O2;
        }
    }
    world.wake_all();
    world.recompute_mass_audit();
    world
}

fn mean_water_co2(world: &World) -> f32 {
    (0..64)
        .filter_map(|x| world.column_at(x).map(|c| c.ecology.water_co2))
        .sum::<f32>()
        / 64.0
}

fn mean_water_o2(world: &World) -> f32 {
    (0..64)
        .filter_map(|x| world.column_at(x).map(|c| c.ecology.water_o2))
        .sum::<f32>()
        / 64.0
}

#[test]
fn e48a_depleted_dissolved_co2_recharges_from_air() {
    let mut world = wet_world(4801);
    for x in 0..64 {
        if let Some(col) = world.column_at_mut(x) {
            col.ecology.water_co2 = 0.05;
            col.ecology.water_o2 = 0.05;
        }
    }
    let co2_0 = mean_water_co2(&world);
    let o2_0 = mean_water_o2(&world);

    let mut sim = wk_sim::Simulation::new(&world);
    for _ in 0..400 {
        sim.step(&mut world);
    }

    let co2_1 = mean_water_co2(&world);
    let o2_1 = mean_water_o2(&world);
    assert!(
        co2_1 > co2_0 + 0.25,
        "air↔water exchange must recharge CO₂ (start={co2_0:.3} end={co2_1:.3})"
    );
    assert!(
        o2_1 > o2_0 + 0.25,
        "air↔water exchange must recharge O₂ (start={o2_0:.3} end={o2_1:.3})"
    );
    assert!(
        co2_1 > EQUIL_WATER_CO2 * 0.5,
        "CO₂ should approach Henry equil (end={co2_1:.3} equil={EQUIL_WATER_CO2})"
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E48a: co2 {co2_0:.3}→{co2_1:.3}  o2 {o2_0:.3}→{o2_1:.3}"
    );
}

#[test]
fn e48b_supersaturated_water_outgasses_to_air() {
    let mut world = wet_world(4802);
    for x in 0..64 {
        if let Some(col) = world.column_at_mut(x) {
            col.ecology.water_co2 = 2.5;
            col.ecology.water_o2 = 2.5;
            col.ecology.air_co2 = AMBIENT_AIR_CO2;
            col.ecology.air_o2 = AMBIENT_AIR_O2;
        }
    }
    let co2_0 = mean_water_co2(&world);

    let mut sim = wk_sim::Simulation::new(&world);
    for _ in 0..400 {
        sim.step(&mut world);
    }
    let co2_1 = mean_water_co2(&world);
    assert!(
        co2_1 < co2_0 - 0.4,
        "supersaturated water must outgas (start={co2_0:.3} end={co2_1:.3})"
    );
    assert!(
        co2_1 < 1.6,
        "should relax toward equil, not stay spiked (end={co2_1:.3})"
    );

    eprintln!("E48b: co2 {co2_0:.3}→{co2_1:.3}");
}
