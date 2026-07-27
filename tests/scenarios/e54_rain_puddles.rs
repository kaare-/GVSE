//! E54 — Rain must produce *visible* puddles on land.
//!
//! Bug context: `recharge_deep_water_tables` pass 1 used to drain **any**
//! top-water on any column whose pores weren't at full cap. A 2 kg/tick
//! rain film would land, get siphoned into moisture on the same tick,
//! and never accumulate — the app rendered rain streaks over bone-dry
//! ground even after a full storm. Meanwhile pore-moisture evaporation
//! kept `cap - moisture > 0` between showers so the siphon never
//! rested.
//!
//! What we want: light rain still soaks in through normal infiltration,
//! but a sustained shower on saturated pores accumulates as a real
//! standing puddle on top.

use crate::helpers::*;
use wk_world::CHUNK_W;
use wk_sim::subsystems::{recharge_deep_water_tables, run_infiltration};
use wk_sim::WorldTransferScratch;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

/// Direct unit-style: a tiny (< 80 kg) rain film on a *not-fully-saturated*
/// column must not be sucked into pores by recharge. Only the standard
/// infiltration pass (throttled) is allowed to touch it.
#[test]
fn e54_recharge_leaves_rain_film_alone() {
    use wk_material::MaterialId;

    let mut world = World::new(5401);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 5.0, 6.0));
    world.wake_all();
    // Emergent land: bed sits well above sea level.
    for i in 0..CHUNK_W {
        let col = &mut world.chunks.get_mut(&0).unwrap().columns[i];
        // Half-full pores (recharge would otherwise want to top them up).
        col.moisture = col.moisture_cap() / 2;
        // Light rain film — well below HYDRAULIC_CONTACT_MIN_KG (80 kg).
        col.deposit_to_top(MaterialId::Water, 25, 0);
        col.clamp_state();
        let bed = world.chunks.get(&0).unwrap().bedrock_y;
        world.chunks.get_mut(&0).unwrap().columns[i].recompute_surface_y(bed);
    }

    let before: i64 = (0..CHUNK_W)
        .map(|i| world.chunks.get(&0).unwrap().columns[i].top_water_mass())
        .sum();

    recharge_deep_water_tables(&mut world);

    let after: i64 = (0..CHUNK_W)
        .map(|i| world.chunks.get(&0).unwrap().columns[i].top_water_mass())
        .sum();
    assert!(
        after as f32 > before as f32 * 0.85,
        "recharge stole the rain film: {before} -> {after}"
    );
}

/// End-to-end: continuous rain on flat sand with initially dry pores
/// must eventually leave visible standing water — the app screenshot
/// otherwise shows falling rain streaks with no ground water at all.
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
    // Long enough to blow past infiltration and pore fill.
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

/// Full-sim guard for image 1: rain on a limestone hilltop must not
/// vanish entirely into the karst void network within a few seconds.
/// The old `SURFACE_CAPTURE_FRAC = 0.35` + `VOID_FLOW_RELAXATION = 0.5`
/// pumped surface water through overlapping voids toward the sea so
/// fast the hill was a permanent internal river.
#[test]
fn e54_karst_hill_keeps_rain_on_top() {
    use wk_material::MaterialId;
    use wk_world::column::{Void, VoidOrigin};

    let mut world = World::new(5403);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_flat_sand(0, 30.0, 20.0));
    world.wake_all();

    // Give every column a shallow karst void just under the surface
    // (worldgen limestone hills are riddled with these mouths).
    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        let bedrock = chunk.bedrock_y;
        for col in &mut chunk.columns {
            col.moisture = col.moisture_cap();
            let mid = col.solid_bed_y() - 3.0;
            col.voids.push(Void {
                top_y: mid + 1.5,
                height_m: 3.0,
                water_mass: 0,
                roof_material: MaterialId::Limestone,
                origin: VoidOrigin::Karst,
                light: 40,
            });
            col.recompute_surface_y(bedrock);
        }
    }
    // Simulate a fat rain shower on top of the hill.
    for i in 0..CHUNK_W {
        let col = &mut world.chunks.get_mut(&0).unwrap().columns[i];
        col.deposit_to_top(MaterialId::Water, 400, 0);
        col.clamp_state();
        let bed = world.chunks.get(&0).unwrap().bedrock_y;
        world.chunks.get_mut(&0).unwrap().columns[i].recompute_surface_y(bed);
    }
    let surface0: i64 = (0..CHUNK_W)
        .map(|i| world.chunks.get(&0).unwrap().columns[i].top_water_mass())
        .sum();

    let mut sim = wk_sim::Simulation::new(&world);
    let _ = run_ticks(&mut world, &mut sim, 200);

    let surface1: i64 = (0..CHUNK_W)
        .map(|i| world.chunks.get(&0).unwrap().columns[i].top_water_mass())
        .sum();
    let void1: i64 = (0..CHUNK_W)
        .map(|i| world.chunks.get(&0).unwrap().columns[i].void_water_total())
        .sum();

    // After 200 ticks the hilltop puddle should still be mostly on top.
    // Void capture is allowed to nibble but not eat > half the puddle.
    let void_frac = void1 as f32 / surface0.max(1) as f32;
    let surface_frac = surface1 as f32 / surface0.max(1) as f32;
    assert!(
        surface_frac > 0.5,
        "karst pumped the hilltop puddle: before={surface0} surface_after={surface1} void_after={void1} (surface_frac={surface_frac:.2})"
    );
    assert!(
        void_frac < 0.4,
        "karst network absorbed too much rain: void_frac={void_frac:.2} (surface={surface1} void={void1})"
    );
}

/// Ensures normal infiltration still soaks a light rain film into pores
/// over many ticks — the fix must not turn every sand plateau into an
/// impervious sheet.
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
    // Several infiltration ticks accumulate residual → real kg.
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
