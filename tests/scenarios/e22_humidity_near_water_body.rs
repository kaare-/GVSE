//! E22 — humidity near a water body (stage 6.3).
//!
//! With the humidity field enabled, open water emits vapour so air RH
//! above a pond must exceed RH over a distant dry patch. Chunks are
//! placed with a gap so humidity halos fall back to ambient rather than
//! equalising across the whole domain.

use crate::helpers::*;
use wk_material::{MaterialId};
use wk_world::{CHUNK_W};
use wk_world::chunk::Chunk;
use wk_world::terrain::fill_column_strata;
use wk_world::world::World;

#[test]
fn e22_humidity_near_water_body() {
    let mut world = World::new(4242);
    world.sea_level = 10.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.ambient_humidity = 0.35;

    // Chunk 0: impermeable floor + pond. Chunk 3: dry stone (gap → ambient halo).
    for &coord in &[0i32, 3] {
        let mut chunk = Chunk::new(coord, 0.0);
        for i in 0..CHUNK_W {
            fill_column_strata(&mut chunk.columns[i], 10.0, 0.0, 0, 20_000, 0);
        }
        if coord == 0 {
            for i in 16..48 {
                chunk.columns[i].deposit_to_top(MaterialId::Water, 10_000, 0);
                chunk.columns[i].recompute_surface_y(chunk.bedrock_y);
            }
        }
        world.insert_chunk(chunk);
    }
    world.wake_all();
    world.recompute_mass_audit();
    world.enable_humidity_fields();

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 3_000);
    assert!(elapsed.as_secs() < 60, "E22 perf: {:?}", elapsed);

    let wet_x = 32; // mid-pond on chunk 0
    let dry_x = 3 * CHUNK_W as i32 + 32; // mid dry chunk
    let wet_mass = world.column_at(wet_x).unwrap().top_water_mass();
    let dry_mass = world.column_at(dry_x).unwrap().top_water_mass();
    assert!(wet_mass >= 50, "pond should hold open water, got {wet_mass} kg");
    assert_eq!(dry_mass, 0, "dry chunk should have no surface water");

    let wet_y = world.column_at(wet_x).unwrap().surface_y;
    let dry_y = world.column_at(dry_x).unwrap().surface_y;
    let rh_wet = world.humidity_at_point(wet_x, wet_y);
    let rh_dry = world.humidity_at_point(dry_x, dry_y);

    assert!(
        rh_wet.is_finite() && rh_dry.is_finite(),
        "NaN/Inf RH: wet={rh_wet} dry={rh_dry}"
    );
    assert!(
        rh_wet > rh_dry + 0.08,
        "expected higher RH over water: wet={rh_wet:.3} dry={rh_dry:.3}"
    );
    assert!(
        rh_wet > world.ambient_humidity + 0.05,
        "wet RH {rh_wet:.3} should exceed ambient {}",
        world.ambient_humidity
    );
    // Dry patch should stay near the regional ambient (no local source).
    assert!(
        (rh_dry - world.ambient_humidity).abs() < 0.08,
        "dry RH {rh_dry:.3} drifted far from ambient {}",
        world.ambient_humidity
    );

    for chunk in world.chunks.values() {
        let Some(humidity) = &chunk.humidity else {
            continue;
        };
        for &v in &humidity.0.cells {
            assert!(
                v.is_finite() && (0.0..=1.0).contains(&v),
                "RH cell out of range on chunk {}: {v}",
                chunk.coord
            );
        }
    }

    eprintln!(
        "E22: RH_wet={rh_wet:.3} RH_dry={rh_dry:.3} wet_kg={wet_mass} in {:?}",
        elapsed
    );
}
