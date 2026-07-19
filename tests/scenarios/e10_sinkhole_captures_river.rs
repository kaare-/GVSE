//! E10 — a river runs *over* a surface-breaching sinkhole.
//!
//! Void-water capture was removed in the water prune (voids are dry
//! air pockets in this build). What we still want to guarantee: an
//! open sinkhole doesn't create a bookkeeping hole — surface water
//! stays on top and flows sideways under normal `SurfaceWater`/
//! `LakeLevel` rules, and no mass appears/disappears at the mouth.

use crate::helpers::*;
use wk_material::{CHUNK_W, MaterialId};
use wk_world::column::{Void, VoidOrigin};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e10_river_runs_past_dry_sinkhole() {
    let mut world = World::new(9010);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.wake_all();

    let mid = (CHUNK_W / 2) as i32;
    // Open a surface-breaching void (sinkhole) in the middle columns.
    if let Some(chunk) = world.chunks.get_mut(&0) {
        let bedrock = chunk.bedrock_y;
        for i in (mid - 2) as usize..=(mid + 2) as usize {
            let col = &mut chunk.columns[i];
            col.voids.push(Void {
                top_y: 0.0,
                height_m: 2.0,
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
    let audit0 = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();
    let surface0: i64 = (mid - 2..=mid + 2)
        .filter_map(|wx| world.column_at(wx).map(|c| c.top_water_mass()))
        .sum();
    assert!(surface0 > 10_000, "expected a surface pond, got {surface0}");

    let mut sim = wk_sim::Simulation::new(&world);
    let _ = run_ticks(&mut world, &mut sim, 200);

    // Mass conservation: the sinkhole must not eat water into nowhere.
    let drift = bookkeeping_check(&world, initial_total, audit0);
    assert!(
        drift.abs() < 200,
        "mass audit drift with dry sinkhole: {drift}"
    );
    assert_no_negative_masses(&world);
}
