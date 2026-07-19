//! E11 — cave roof collapses (stage 7).
//!
//! A wide void under a sand roof (`roof_span_max_m = 0`) must collapse:
//! void height shrinks and LooseRock debris appears.

use crate::helpers::*;
use wk_material::MaterialId;
use wk_world::column::{Void, VoidOrigin};
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

#[test]
fn e11_cave_roof_collapses() {
    let mut world = World::new(9011);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.wake_all();

    // Contiguous void under sand across many columns — span much greater than 0 m.
    let h0 = 1.5f32;
    if let Some(chunk) = world.chunks.get_mut(&0) {
        let bedrock = chunk.bedrock_y;
        for i in 10..30 {
            let col = &mut chunk.columns[i];
            let mid = bedrock + 4.0;
            col.voids.push(Void {
                top_y: mid + h0 * 0.5,
                height_m: h0,
                roof_material: MaterialId::Sand,
                origin: VoidOrigin::Karst,
                light: 0,
            });
            col.recompute_surface_y(bedrock);
        }
    }
    world.recompute_mass_audit();
    let loose0 = world.mass_audit.by_material[MaterialId::LooseRock.index()];
    let void_h0: f32 = world.chunks[&0].columns[10..30]
        .iter()
        .map(|c| c.void_height_total())
        .sum();

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 500);
    assert!(elapsed.as_secs() < 30, "E11 perf: {:?}", elapsed);

    world.recompute_mass_audit();
    let loose1 = world.mass_audit.by_material[MaterialId::LooseRock.index()];
    let void_h1: f32 = world.chunks[&0].columns[10..30]
        .iter()
        .map(|c| c.void_height_total())
        .sum();

    assert!(
        void_h1 < void_h0 - 0.5,
        "void should shrink under collapse: {void_h0:.2} -> {void_h1:.2}"
    );
    assert!(
        loose1 > loose0,
        "collapsed roof should yield LooseRock: {loose0} -> {loose1}"
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E11: void_h {void_h0:.2}->{void_h1:.2} loose {loose0}->{loose1} in {:?}",
        elapsed
    );
}
