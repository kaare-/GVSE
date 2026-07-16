//! E24 — Darcy pressure equilibration (stage 6.5).
//!
//! An uneven water table on a permeable sand bed must flatten under
//! groundwater head diffusion + moisture transfer. Head variance falls
//! and left/right moisture contrast shrinks.

use crate::helpers::*;
use wk_material::CHUNK_W;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn head_variance(world: &World) -> f32 {
    let mut heads = Vec::new();
    for chunk in world.chunks.values() {
        let base = chunk.world_x_base();
        for i in 0..CHUNK_W {
            let wx = base + i as i32;
            let y = 0.5 * (chunk.bedrock_y + chunk.columns[i].surface_y);
            heads.push(world.groundwater_head_at_point(wx, y));
        }
    }
    let n = heads.len() as f32;
    let mean = heads.iter().sum::<f32>() / n;
    heads.iter().map(|h| (h - mean).powi(2)).sum::<f32>() / n
}

fn half_mean_moisture(world: &World, left: bool) -> f64 {
    let (lo, hi) = if left {
        (0, CHUNK_W as i32)
    } else {
        (CHUNK_W as i32, 2 * CHUNK_W as i32)
    };
    let mut sum = 0i64;
    let mut n = 0i64;
    for wx in lo..hi {
        if let Some(col) = world.column_at(wx) {
            sum += col.moisture;
            n += 1;
        }
    }
    sum as f64 / n.max(1) as f64
}

#[test]
fn e24_darcy_pressure_equilibration() {
    let mut world = World::new(2424);
    world.sea_level = 10.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;

    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    world.insert_chunk(generate_flat_sand(1, 0.0, 10.0));

    for wx in 0..(CHUNK_W as i32) {
        if let Some(col) = world.column_at_mut(wx) {
            let cap = col.moisture_cap().max(1);
            col.moisture = (cap as f32 * 0.85) as i64;
        }
    }
    for wx in (CHUNK_W as i32)..(2 * CHUNK_W as i32) {
        if let Some(col) = world.column_at_mut(wx) {
            let cap = col.moisture_cap().max(1);
            col.moisture = (cap as f32 * 0.15) as i64;
        }
    }
    world.wake_all();
    world.recompute_mass_audit();
    world.enable_groundwater_head_fields();

    let var0 = head_variance(&world);
    let left0 = half_mean_moisture(&world, true);
    let right0 = half_mean_moisture(&world, false);
    let contrast0 = left0 - right0;
    assert!(var0 > 0.05, "expected initial head contrast, var={var0}");
    assert!(contrast0 > 100.0, "expected initial moisture step, Δ={contrast0}");

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 4_000);
    assert!(elapsed.as_secs() < 60, "E24 perf: {:?}", elapsed);

    let var1 = head_variance(&world);
    let left1 = half_mean_moisture(&world, true);
    let right1 = half_mean_moisture(&world, false);
    let contrast1 = left1 - right1;

    assert!(
        var1 < var0 * 0.6,
        "head variance should drop: var0={var0:.4} var1={var1:.4}"
    );
    assert!(
        contrast1 < contrast0 * 0.85,
        "moisture contrast should shrink: {contrast0:.1} → {contrast1:.1}"
    );

    let h_left = world.groundwater_head_at_point(16, 5.0);
    let h_right = world.groundwater_head_at_point(CHUNK_W as i32 + 16, 5.0);
    assert!(
        (h_left - h_right).abs() < (var0.sqrt() * 2.0).max(1.0),
        "heads should approach each other: left={h_left:.2} right={h_right:.2}"
    );

    for chunk in world.chunks.values() {
        let Some(gw) = &chunk.gw_head else {
            continue;
        };
        for &v in &gw.0.cells {
            assert!(v.is_finite(), "head NaN on chunk {}", chunk.coord);
        }
    }

    eprintln!(
        "E24: var {var0:.4}→{var1:.4} moistΔ {contrast0:.1}→{contrast1:.1} \
         hL={h_left:.2} hR={h_right:.2} in {:?}",
        elapsed
    );
}
