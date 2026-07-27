//! E25 — dissolved plume diffusion (stage 6.6).
//!
//! A concentrated dissolved-mineral injection into a wet sand bed must
//! spread under the dissolved-field diffusivity while conserving total
//! dissolved mass (no solid dissolution yet — Limestone lands in stage 7).

use crate::helpers::*;
use wk_world::CHUNK_W;
use wk_world::terrain::generate_flat_sand;
use wk_world::world::World;

fn peak_and_spread(world: &World) -> (f32, f32, usize) {
    let Some(d) = world.chunks.get(&0).and_then(|c| c.dissolved.as_ref()) else {
        return (0.0, 0.0, 0);
    };
    let w = d.0.width_cells as usize;
    let h = d.0.height_cells as usize;
    let mut peak = 0.0f32;
    let mut peak_cx = 0usize;
    let mut mass_moment = 0.0f32;
    let mut mass = 0.0f32;
    for cy in 0..h {
        for cx in 0..w {
            let c = d.0.cell_at(cx, cy);
            if c > peak {
                peak = c;
                peak_cx = cx;
            }
            if c > 0.0 {
                mass += c;
                mass_moment += c * cx as f32;
            }
        }
    }
    // Count cells above 5% of peak as the plume footprint.
    let thresh = peak * 0.05;
    let mut footprint = 0usize;
    for cy in 0..h {
        for cx in 0..w {
            if d.0.cell_at(cx, cy) > thresh {
                footprint += 1;
            }
        }
    }
    let _centroid = if mass > 0.0 { mass_moment / mass } else { 0.0 };
    let _ = (peak_cx, w);
    (peak, mass, footprint)
}

#[test]
fn e25_dissolved_plume_diffusion() {
    let mut world = World::new(2525);
    world.sea_level = 10.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;

    world.insert_chunk(generate_flat_sand(0, 0.0, 10.0));
    // Saturate so the plume can diffuse through wet pore space.
    for i in 0..CHUNK_W {
        if let Some(col) = world.column_at_mut(i as i32) {
            let cap = col.moisture_cap().max(1);
            col.moisture = (cap as f32 * 0.8) as i64;
        }
    }
    world.wake_all();
    world.enable_dissolved_fields();

    // Inject 500 kg of dissolved mineral mid-chunk, mid-aquifer.
    let wx = (CHUNK_W / 2) as i32;
    let y = 5.0;
    world.inject_dissolved_mass(wx, y, 500.0);
    let mass0 = world.dissolved_mass_kg();
    assert!(mass0 >= 490 && mass0 <= 510, "inject mass={mass0}");

    let (peak0, _cell_sum0, foot0) = peak_and_spread(&world);
    assert!(peak0 > 0.0, "expected a concentration peak");
    assert!(foot0 >= 1, "expected a non-empty plume");

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 3_000);
    assert!(elapsed.as_secs() < 60, "E25 perf: {:?}", elapsed);

    let mass1 = world.dissolved_mass_kg();
    let (peak1, _cell_sum1, foot1) = peak_and_spread(&world);

    assert!(
        (mass1 - mass0).abs() <= 2,
        "dissolved mass should be conserved: {mass0} → {mass1}"
    );
    assert!(
        foot1 > foot0,
        "plume should spread: footprint {foot0} → {foot1}"
    );
    assert!(
        peak1 < peak0 * 0.95,
        "peak concentration should fall as the plume spreads: {peak0:.3} → {peak1:.3}"
    );
    assert_eq!(
        world.mass_audit.dissolved_total, mass1,
        "audit dissolved_total should match integrated field"
    );

    for chunk in world.chunks.values() {
        let Some(d) = &chunk.dissolved else {
            continue;
        };
        for &v in &d.0.cells {
            assert!(v.is_finite() && v >= 0.0, "bad concentration {v}");
        }
    }

    eprintln!(
        "E25: mass {mass0}→{mass1} peak {peak0:.2}→{peak1:.2} foot {foot0}→{foot1} in {:?}",
        elapsed
    );
}
