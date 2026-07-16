//! E13 — burrow dig produces surface tailings (stage 9).
//!
//! `world.dig` must remove substrate, leave a burrow/trench void, dump
//! an equal tailings mass on the surface, and keep layer mass closed.

use crate::helpers::*;
use wk_material::{CHUNK_W, MaterialId};
use wk_world::column::VoidOrigin;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn layer_mass(world: &World, wx: i32) -> i64 {
    let col = world.column_at(wx).unwrap();
    (0..col.layer_count as usize)
        .map(|i| col.layers[i].thickness)
        .sum()
}

#[test]
fn e13_burrow_produces_surface_tailings() {
    let mut world = World::new(9013);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.wake_all();
    world.recompute_mass_audit();

    let wx = (CHUNK_W / 2) as i32;
    let surface0 = world.column_at(wx).unwrap().surface_y;
    let mass0 = layer_mass(&world, wx);
    let voids0 = world.column_at(wx).unwrap().voids.len();
    let tracked0 = world.mass_audit.total_tracked();

    // Dig below the climate surface into the sand body.
    let target_y = world.column_at(wx).unwrap().climate_elevation() - 2.0;
    let dig_kg = 800i64;
    let res = world.dig(wx, target_y, dig_kg);

    assert!(!res.refused, "dig should succeed in sand");
    assert_eq!(res.removed_kg, dig_kg);
    assert_eq!(res.material, MaterialId::Sand);
    // Sand roof_span is 0 → digs collapse to open trenches.
    assert!(
        res.collapsed_to_trench || res.void_delta_m > 0.0,
        "expected a void or trench, got {res:?}"
    );

    let col = world.column_at(wx).unwrap();
    assert!(
        col.voids.len() > voids0,
        "expected a new void annotation"
    );
    assert!(
        col.voids.iter().any(|v| {
            matches!(v.origin, VoidOrigin::Burrow | VoidOrigin::Collapse) && v.height_m > 0.0
        }),
        "void should be burrow/trench origin"
    );
    // Tailings raise the surface (mound).
    assert!(
        col.surface_y > surface0 + 0.01,
        "tailings mound should raise surface: {surface0} → {}",
        col.surface_y
    );
    // Removed then redeposited — layer mass unchanged.
    assert_eq!(layer_mass(&world, wx), mass0);

    world.recompute_mass_audit();
    assert_eq!(
        world.mass_audit.total_tracked(),
        tracked0,
        "dig must not create or destroy tracked mass"
    );

    // Chain dig in neighbouring columns → connected passage span.
    let res2 = world.dig(wx + 1, target_y, dig_kg);
    let res3 = world.dig(wx + 2, target_y, dig_kg);
    assert!(!res2.refused && !res3.refused);
    let with_void = (wx..=wx + 2)
        .filter(|&x| {
            world
                .column_at(x)
                .map(|c| !c.voids.is_empty())
                .unwrap_or(false)
        })
        .count();
    assert!(
        with_void >= 3,
        "adjacent digs should leave voids in a row, got {with_void}"
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E13: dug {dig_kg} kg sand, trench={} surface {surface0:.2}→{:.2} voids={}",
        res.collapsed_to_trench,
        world.column_at(wx).unwrap().surface_y,
        world.column_at(wx).unwrap().voids.len()
    );
}
