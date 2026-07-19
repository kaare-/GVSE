//! E54 — Rain must produce *visible* puddles on land.
//!
//! Bug context: recharge and karst void-capture used to drain any
//! top-water on emergent land into pores or voids on the same tick it
//! landed, so rain streaks fell on visibly dry ground. Those passes are
//! gone after the water prune; this scenario is the regression guard.

use crate::helpers::*;
use wk_material::CHUNK_W;
use wk_sim::subsystems::run_infiltration;
use wk_sim::WorldTransferScratch;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

/// End-to-end: continuous rain on flat sand with initially dry pores
/// must eventually leave visible standing water.
#[test]
fn e54_sustained_rain_accumulates_visible_puddle() {
    let mut world = World::new(5402);
    world.sea_level = -10.0;
    world.rain_enabled = true;
    world.rain_rate = 100.0;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 6.0, 8.0));
    world.wake_all();

    let mut sim = wk_sim::Simulation::new(&world);
    let _ = run_ticks(&mut world, &mut sim, 2_000);

    let mut visible = 0;
    let mut max_puddle = 0i64;
    for i in 0..CHUNK_W {
        let col = &world.chunks.get(&0).unwrap().columns[i];
        let m = col.top_water_mass();
        if m > 40 {
            visible += 1;
        }
        max_puddle = max_puddle.max(m);
    }
    assert!(
        visible >= 4,
        "no visible puddles after sustained rain (max={max_puddle} kg)"
    );
}

/// Normal infiltration still soaks a light rain film into pores over
/// many ticks — the water prune must not turn every sand plateau into
/// an impervious sheet.
#[test]
fn e54_light_rain_still_soaks_over_time() {
    use wk_material::MaterialId;

    let mut world = World::new(5404);
    world.sea_level = -20.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 12.0, 8.0));
    world.wake_all();
    for i in 0..CHUNK_W {
        let col = &mut world.chunks.get_mut(&0).unwrap().columns[i];
        col.moisture = 0;
        col.deposit_to_top(MaterialId::Water, 30, 0);
    }

    let mut scratch = WorldTransferScratch::default();
    for _ in 0..30 {
        run_infiltration(&mut world, &mut scratch);
    }
    let booked: i64 = scratch
        .buffers
        .get(&0)
        .map(|b| b.infil_delta[..CHUNK_W].iter().sum())
        .unwrap_or(0);
    assert!(
        booked > 30,
        "shallow rain film should still book infiltration, got {booked}"
    );
}
