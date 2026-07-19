//! E53 — lit shoreline / karst mouths must not drain the ocean.
//!
//! Sea-cliff and limestone-shelf cavities often carry `light > 200`.
//! Surface void capture used to swallow ~35% of each ocean column per
//! tick into those voids, fighting lake-level refill — a fixed notch in
//! the free surface (vertical "water edge") with algae riding the
//! oscillation as a sine curve.

use crate::helpers::*;
use wk_material::{CHUNK_W, MaterialId};
use wk_sim::subsystems::{run_lake_level, run_surface_void_capture, run_void_water_flow};
use wk_world::column::{Void, VoidOrigin};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn flood_shelf_with_mouth() -> (World, i32) {
    let mut world = World::new(9053);
    world.sea_level = 12.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    // Flat bed at 8 m — ~4 m below sea (shelf).
    world.insert_chunk(generate_flat_sand(0, -900.0, 8.0));
    world.wake_all();

    let glitch = 20i32;
    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        let bedrock = chunk.bedrock_y;
        for col in &mut chunk.columns {
            // Fill pores so infiltration doesn't drain the "ocean".
            col.moisture = col.moisture_cap();
            let bed = col.climate_elevation();
            let need = ((world.sea_level - bed).max(0.5) * 250.0) as i64;
            col.deposit_to_top(MaterialId::Water, need, 0);
            col.clamp_state();
            col.recompute_surface_y(bedrock);
        }
        let col = &mut chunk.columns[glitch as usize];
        let bed = col.solid_bed_y();
        col.voids.push(Void {
            top_y: bed + 0.5,
            height_m: 2.0,
            water_mass: 0,
            roof_material: MaterialId::Limestone,
            origin: VoidOrigin::Karst,
            light: 235,
        });
        col.recompute_surface_y(bedrock);
    }
    world.recompute_mass_audit();
    (world, glitch)
}

#[test]
fn e53_shore_void_capture_skips_submerged_bed() {
    let (mut world, glitch) = flood_shelf_with_mouth();
    assert!(
        world.column_at(glitch).unwrap().solid_bed_y() < world.sea_level - 0.25,
        "precondition: submerged shelf bed"
    );

    let mass0 = world
        .column_at(glitch)
        .unwrap()
        .flowable_water()
        .map(|(_, m)| m)
        .unwrap_or(0);
    assert!(mass0 > 500, "precondition: flooded, got {mass0}");

    // The old bug: each capture tick ate 35% into the lit void.
    for _ in 0..40 {
        run_surface_void_capture(&mut world);
        run_void_water_flow(&mut world);
        run_lake_level(&mut world);
    }

    let col = world.column_at(glitch).unwrap();
    let mass1 = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
    let void1 = col.void_water_total();
    assert!(
        void1 < 50,
        "ocean must not drain into lit shore void, void_water={void1}"
    );
    let ratio = mass1 as f32 / mass0.max(1) as f32;
    assert!(
        ratio > 0.85,
        "shore void drained the sea column: before={mass0} after={mass1} ratio={ratio:.2}"
    );
}

#[test]
fn e53_shore_void_full_sim_keeps_flat_sea() {
    let (mut world, glitch) = flood_shelf_with_mouth();
    let mut sim = wk_sim::Simulation::new(&world);
    let _ = run_ticks(&mut world, &mut sim, 400);

    let glitch_mass = world
        .column_at(glitch)
        .unwrap()
        .flowable_water()
        .map(|(_, m)| m)
        .unwrap_or(0);
    let neighbour_mass = world
        .column_at(glitch + 3)
        .unwrap()
        .flowable_water()
        .map(|(_, m)| m)
        .unwrap_or(0);

    assert!(
        neighbour_mass > 400,
        "ocean neighbour should stay flooded, got {neighbour_mass}"
    );
    let ratio = glitch_mass as f32 / neighbour_mass.max(1) as f32;
    assert!(
        ratio > 0.75,
        "shore void drained the sea column: glitch={glitch_mass} neighbour={neighbour_mass} ratio={ratio:.2}"
    );

    let eta = |wx: i32| {
        world
            .column_at(wx)
            .and_then(|c| c.flowable_water().map(|(t, _)| t))
            .unwrap_or(0.0)
    };
    let step = (eta(glitch) - eta(glitch + 1)).abs();
    assert!(
        step < 0.75,
        "vertical water edge at shore void: |Δη|={step:.2} m"
    );
}

#[test]
fn e53_land_sinkhole_still_captures() {
    // Regression guard: ocean gate must not disable terrestrial E10 capture.
    let mut world = World::new(9054);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.wake_all();

    let mid = (CHUNK_W / 2) as i32;
    if let Some(chunk) = world.chunks.get_mut(&0) {
        let bedrock = chunk.bedrock_y;
        for i in (mid - 1) as usize..=(mid + 1) as usize {
            let col = &mut chunk.columns[i];
            col.voids.push(Void {
                top_y: 0.0,
                height_m: 2.0,
                water_mass: 0,
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
    for wx in mid - 1..=mid + 1 {
        if let Some(col) = world.column_at_mut(wx) {
            col.deposit_to_top(MaterialId::Water, 4_000, 0);
        }
    }
    let surface0: i64 = (mid - 1..=mid + 1)
        .filter_map(|wx| world.column_at(wx).map(|c| c.top_water_mass()))
        .sum();

    let mut sim = wk_sim::Simulation::new(&world);
    let _ = run_ticks(&mut world, &mut sim, 200);

    let void1: i64 = (mid - 1..=mid + 1)
        .filter_map(|wx| world.column_at(wx).map(|c| c.void_water_total()))
        .sum();
    let surface1: i64 = (mid - 1..=mid + 1)
        .filter_map(|wx| world.column_at(wx).map(|c| c.top_water_mass()))
        .sum();
    assert!(void1 > 500, "land sinkhole must still capture, void={void1}");
    assert!(surface1 < surface0, "surface should drop: {surface0} → {surface1}");
}
