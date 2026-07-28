//! E14 — ecology grows on wet warm plains (stage 8).
//!
//! A saturated plains band with nutrient should accumulate alive
//! biomass under `run_ecology`, while a dry band should not.

use crate::helpers::*;
use wk_world::column::Ecology;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e14_ecology_grows_when_wet() {
    let mut world = World::new(8014);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.base_temp_c = 18.0;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.lapse_rate_c_per_m = 0.0;
    world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
    world.wake_all();

    // Contiguous wet band so groundwater can't drain the moisture away
    // into dry neighbours before plants grow.
    let wet_lo = 8usize;
    let wet_hi = 24usize;
    let dry_lo = 40usize;
    let dry_hi = 56usize;
    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        for i in 0..wk_world::CHUNK_W {
            let col = &mut chunk.columns[i];
            col.ecology = Ecology {
                root_density: 0.2,
                leaf_area: 0.2,
                dead_biomass: 0,
                alive_biomass: 100,
                nutrient: 0.9,
                ..Ecology::default()
            };
            let cap = col.moisture_cap().max(1);
            if (wet_lo..wet_hi).contains(&i) {
                col.moisture = cap;
            } else {
                col.moisture = 0;
            }
        }
    }
    world.recompute_mass_audit();
    let alive0_wet: i64 = (wet_lo..wet_hi)
        .map(|i| world.chunks[&0].columns[i].ecology.alive_biomass)
        .sum();
    let alive0_dry: i64 = (dry_lo..dry_hi)
        .map(|i| world.chunks[&0].columns[i].ecology.alive_biomass)
        .sum();
    let tracked0 = world.mass_audit.total_tracked();
    let audit0 = world.mass_audit.clone();

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 4_000);
    assert!(elapsed.as_secs() < 60, "E14 perf: {:?}", elapsed);

    world.recompute_mass_audit();
    let alive1_wet: i64 = (wet_lo..wet_hi)
        .map(|i| world.chunks[&0].columns[i].ecology.alive_biomass)
        .sum();
    let alive1_dry: i64 = (dry_lo..dry_hi)
        .map(|i| world.chunks[&0].columns[i].ecology.alive_biomass)
        .sum();

    assert!(
        alive1_wet > alive0_wet + 200,
        "wet band should grow: {alive0_wet} → {alive1_wet}"
    );
    assert!(
        alive1_dry <= alive0_dry,
        "dry band should not grow: {alive0_dry} → {alive1_dry}"
    );
    assert!(
        alive1_wet - alive0_wet > alive1_dry - alive0_dry + 200,
        "wet growth should outpace dry: wetΔ={} dryΔ={}",
        alive1_wet - alive0_wet,
        alive1_dry - alive0_dry
    );
    assert!(world.mass_audit.biomass_grow_total > 0);

    let drift = bookkeeping_check(&world, tracked0, audit0);
    assert!(
        drift.abs() <= 50,
        "bookkeeping drift {drift} (tracked0={tracked0} now={})",
        world.mass_audit.total_tracked()
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E14: wet {alive0_wet}→{alive1_wet} dry {alive0_dry}→{alive1_dry} \
         grow={} decay={} drift={drift} in {:?}",
        world.mass_audit.biomass_grow_total, world.mass_audit.biomass_decay_total, elapsed
    );
}
