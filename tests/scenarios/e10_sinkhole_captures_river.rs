//! E10 — sinkhole captures surface water (stage 7).
//!
//! An open void at the surface must swallow standing water from the
//! column into void storage instead of leaving it entirely on top.

use crate::helpers::*;
use wk_material::{CHUNK_W, MaterialId};
use wk_world::column::{Void, VoidOrigin};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e10_sinkhole_captures_river() {
    let mut world = World::new(9010);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.wake_all();

    let mid = (CHUNK_W / 2) as i32;
    // Open a surface-breaching void (sinkhole) in the middle columns.
    // After recompute, pin the void ceiling to the new surface so it
    // reads as open (`open_to_surface`).
    if let Some(chunk) = world.chunks.get_mut(&0) {
        let bedrock = chunk.bedrock_y;
        for i in (mid - 2) as usize..=(mid + 2) as usize {
            let col = &mut chunk.columns[i];
            col.voids.push(Void {
                top_y: 0.0, // set after recompute
                height_m: 2.0,
                water_mass: 0,
                // Stone roof so residual collapse logic won't treat a
                // single-column mouth as an over-span sand roof.
                roof_material: MaterialId::Stone,
                origin: VoidOrigin::Collapse,
                light: 255,
            });
            col.recompute_surface_y(bedrock);
            if let Some(v) = col.voids.last_mut() {
                v.top_y = col.surface_y;
            }
        }
    }

    // Pond a lot of surface water on the sinkhole columns.
    for wx in mid - 2..=mid + 2 {
        if let Some(col) = world.column_at_mut(wx) {
            col.deposit_to_top(MaterialId::Water, 5_000, 0);
        }
    }
    world.recompute_mass_audit();
    let surface0: i64 = (mid - 2..=mid + 2)
        .filter_map(|wx| world.column_at(wx).map(|c| c.top_water_mass()))
        .sum();
    let void0: i64 = (mid - 2..=mid + 2)
        .filter_map(|wx| world.column_at(wx).map(|c| c.void_water_total()))
        .sum();
    assert!(surface0 > 10_000, "expected a surface pond, got {surface0}");
    assert_eq!(void0, 0);

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 200);
    assert!(elapsed.as_secs() < 30, "E10 perf: {:?}", elapsed);

    let surface1: i64 = (mid - 2..=mid + 2)
        .filter_map(|wx| world.column_at(wx).map(|c| c.top_water_mass()))
        .sum();
    let void1: i64 = (mid - 2..=mid + 2)
        .filter_map(|wx| world.column_at(wx).map(|c| c.void_water_total()))
        .sum();

    assert!(
        void1 > 1_000,
        "sinkhole should capture water into voids: void={void1}"
    );
    assert!(
        surface1 < surface0,
        "surface water should drop as it drains in: {surface0} → {surface1}"
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E10: surface {surface0}→{surface1} void {void0}→{void1} in {:?}",
        elapsed
    );
}
