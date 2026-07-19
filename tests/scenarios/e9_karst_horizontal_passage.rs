//! E9 — karst forms a horizontal passage (geology only).
//!
//! A limestone bed under a sand cap with a lateral moisture/head gradient
//! must dissolve along the water table and open voids at a consistent
//! elevation across neighbouring columns. Dissolution mass leaves the
//! solid stack and is booked to `dissolved_out_total`; the audit
//! invariant is `dissolved_bank = out_total − return_total ≥ 0`.
//!
//! No water inside voids in this build — the assertion set has been
//! trimmed to the geology emergent from `run_karst` alone.

use crate::helpers::*;
use wk_material::{CHUNK_W, MaterialId};
use wk_world::terrain::generate_limestone_bed;
use wk_world::world::World;

fn void_stats(world: &World) -> (usize, f32, f32) {
    let mut n = 0usize;
    let mut sum_mid = 0.0f32;
    let mut sum_h = 0.0f32;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            for v in &col.voids {
                if v.height_m > 0.05 {
                    n += 1;
                    sum_mid += v.mid_y();
                    sum_h += v.height_m;
                }
            }
        }
    }
    let mean_mid = if n > 0 { sum_mid / n as f32 } else { 0.0 };
    (n, mean_mid, sum_h)
}

#[test]
fn e9_karst_forms_horizontal_passage() {
    let mut world = World::new(9009);
    world.sea_level = 0.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.insert_chunk(generate_limestone_bed(0, 0.0, 2.0, 6.0, 1.0));
    world.wake_all();

    // Lateral head gradient across a span narrower than limestone's
    // roof_span_max_m (10 m ≈ 40 cols) so roof collapse doesn't bury the
    // passage while it forms.
    for i in 0..CHUNK_W {
        if let Some(col) = world.column_at_mut(i as i32) {
            let cap = col.moisture_cap().max(1);
            if (16..40).contains(&i) {
                col.moisture = cap;
            } else if (40..48).contains(&i) {
                col.moisture = (cap as f32 * 0.15) as i64;
            } else {
                col.moisture = (cap as f32 * 0.02) as i64;
            }
        }
    }
    world.recompute_mass_audit();
    let solid0 = world.mass_audit.by_material[MaterialId::Limestone.index()];

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 8_000);
    assert!(elapsed.as_secs() < 90, "E9 perf: {:?}", elapsed);

    let (n_voids, mean_mid, sum_h) = void_stats(&world);
    assert!(
        n_voids >= 8,
        "expected a horizontal passage (≥8 void columns), got {n_voids}"
    );
    assert!(sum_h > 0.5, "expected meaningful cave volume, sum_h={sum_h}");

    // Voids should cluster near one elevation (horizontal passage).
    let mut spread = 0.0f32;
    let mut count = 0usize;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            for v in &col.voids {
                if v.height_m > 0.05 {
                    spread = spread.max((v.mid_y() - mean_mid).abs());
                    count += 1;
                }
            }
        }
    }
    assert!(count >= 8);
    assert!(
        spread < 3.5,
        "passage should be roughly level: mid spread {spread:.2} m around {mean_mid:.2}"
    );

    world.recompute_mass_audit();
    let solid1 = world.mass_audit.by_material[MaterialId::Limestone.index()];
    assert!(solid1 < solid0, "limestone should dissolve: {solid0} → {solid1}");
    assert!(
        world.mass_audit.dissolved_out_total > 0,
        "dissolved_out_total should increase"
    );
    assert!(
        world.mass_audit.dissolved_bank() >= 0,
        "dissolved bank stays non-negative: out={} return={}",
        world.mass_audit.dissolved_out_total,
        world.mass_audit.dissolved_return_total
    );
    assert_no_negative_masses(&world);

    eprintln!(
        "E9: voids={n_voids} sum_h={sum_h:.2} mid={mean_mid:.2}±{spread:.2} \
         lime {solid0}→{solid1} diss_out={} return={} in {:?}",
        world.mass_audit.dissolved_out_total,
        world.mass_audit.dissolved_return_total,
        elapsed
    );
}
