//! E15 — roots reduce erosion / slumping (stage 8).
//!
//! A modest sand mound with dense roots should retain more of its sand
//! than a bare twin under the same neighbour grade.

use crate::helpers::*;
use wk_material::MaterialId;
use wk_world::column::Ecology;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn sand_mass(world: &World, local: usize) -> i64 {
    let col = &world.chunks[&0].columns[local];
    (0..col.layer_count as usize)
        .filter(|&i| col.layers[i].material == MaterialId::Sand)
        .map(|i| col.layers[i].thickness)
        .sum()
}

#[test]
fn e15_roots_reduce_erosion() {
    let mut world = World::new(8015);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.wake_all();

    let bare = 20usize;
    let rooted = 40usize;
    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        let bedrock = chunk.bedrock_y;
        for &i in &[bare, rooted] {
            let col = &mut chunk.columns[i];
            // Modest mound (~2.5 m of sand) — enough to exceed bare repose
            // but holdable by a full root mat.
            col.deposit_to_top(MaterialId::Sand, 1_000, 0);
            col.recompute_surface_y(bedrock);
            col.activity = wk_world::column::Activity::HydrologyActive;
        }
        chunk.columns[bare].ecology = Ecology::default();
        chunk.columns[rooted].ecology = Ecology {
            root_density: 1.0,
            leaf_area: 0.8,
            dead_biomass: 0,
            alive_biomass: 2_000,
            nutrient: 0.5,
        };
    }
    world.recompute_mass_audit();
    let sand0_bare = sand_mass(&world, bare);
    let sand0_rooted = sand_mass(&world, rooted);

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 500);
    assert!(elapsed.as_secs() < 30, "E15 perf: {:?}", elapsed);

    let sand1_bare = sand_mass(&world, bare);
    let sand1_rooted = sand_mass(&world, rooted);
    let lost_bare = sand0_bare - sand1_bare;
    let lost_rooted = sand0_rooted - sand1_rooted;

    assert!(
        lost_bare > 20,
        "bare mound should slump/erode: lost={lost_bare} ({sand0_bare}→{sand1_bare})"
    );
    assert!(
        lost_rooted < lost_bare,
        "rooted mound should retain more sand: bare_lost={lost_bare} rooted_lost={lost_rooted}"
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E15: bare lost {lost_bare} rooted lost {lost_rooted} in {:?}",
        elapsed
    );
}
