//! E12 — speleothem / stalactite forms (stage 7).
//!
//! Dissolved mineral mass inside a damp void must reprecipitate as
//! Limestone, shrinking the void and incrementing `dissolved_return_total`.

use crate::helpers::*;
use wk_material::{MaterialId};
use wk_world::{CHUNK_W};
use wk_world::column::{Void, VoidOrigin};
use wk_world::terrain::generate_limestone_bed;
use wk_world::world::World;

#[test]
fn e12_stalactite_forms() {
    let mut world = World::new(9012);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_limestone_bed(0, 0.0, 2.0, 6.0, 1.0));
    world.enable_dissolved_fields();
    world.wake_all();

    let mid = (CHUNK_W / 2) as i32;
    let void_mid_y = 4.0f32;
    let h0 = 2.0f32;
    if let Some(col) = world.column_at_mut(mid) {
        col.voids.push(Void {
            top_y: void_mid_y + h0 * 0.5,
            height_m: h0,
            water_mass: 800,
            roof_material: MaterialId::Limestone,
            origin: VoidOrigin::Karst,
            light: 80,
        });
    }
    if let Some(chunk) = world.chunks.get_mut(&0) {
        let bedrock = chunk.bedrock_y;
        chunk.columns[CHUNK_W / 2].recompute_surface_y(bedrock);
    }

    // Seed a concentrated dissolved plume at the void.
    world.inject_dissolved_mass(mid, void_mid_y, 400.0);
    world.recompute_mass_audit();
    let diss0 = world.dissolved_mass_kg();
    let lime0 = world.mass_audit.by_material[MaterialId::Limestone.index()];
    let void_h0 = world
        .column_at(mid)
        .map(|c| c.void_height_total())
        .unwrap_or(0.0);
    assert!(diss0 >= 350, "expected dissolved seed, got {diss0}");

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 3_000);
    assert!(elapsed.as_secs() < 60, "E12 perf: {:?}", elapsed);

    world.recompute_mass_audit();
    let diss1 = world.dissolved_mass_kg();
    let lime1 = world.mass_audit.by_material[MaterialId::Limestone.index()];
    let void_h1 = world
        .column_at(mid)
        .map(|c| c.void_height_total())
        .unwrap_or(0.0);
    let returned = world.mass_audit.dissolved_return_total;

    assert!(
        returned > 0,
        "speleogenesis should return mass, got {returned}"
    );
    assert!(
        diss1 < diss0,
        "dissolved mass should fall as it precipitates: {diss0} → {diss1}"
    );
    assert!(
        lime1 > lime0 || void_h1 < void_h0,
        "limestone should grow or void shrink: lime {lime0}→{lime1} void_h {void_h0:.3}→{void_h1:.3}"
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E12: diss {diss0}→{diss1} lime {lime0}→{lime1} void_h {void_h0:.3}→{void_h1:.3} \
         return={returned} in {:?}",
        elapsed
    );
}
