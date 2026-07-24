//! E20 — geothermal steady state (stage 6.2).
//!
//! With thermal fields enabled and day/night swing zeroed, deeper cells
//! must stay warmer than near-surface cells after the field has run for
//! a while. Dirichlet boundaries (sky top, geothermal bottom) drive a
//! stable vertical gradient; the solver must not blow up or produce NaN.

use crate::helpers::*;

#[test]
fn e20_geothermal_steady_state() {
    let mut world = setup_flat_sand();
    world.rain_enabled = false;
    world.climate.day_night_amplitude_c = 0.0;
    world.geothermal_bottom_c = 55.0;
    world.enable_thermal_fields();

    // Every loaded chunk must carry a thermal field after enable.
    for chunk in world.chunks.values() {
        assert!(chunk.thermal.is_some(), "thermal field missing on chunk {}", chunk.coord);
    }

    let mut sim = wk_sim::Simulation::new(&world);
    // 5_000 ticks → 500 thermal steps at period 10.
    let elapsed = run_ticks(&mut world, &mut sim, 5_000);
    assert!(elapsed.as_secs() < 60, "E20 perf: {:?}", elapsed);

    let tick = sim.clock.tick;
    let wx = 32; // mid-chunk column on chunk 0
    let thermal = world
        .chunks
        .get(&0)
        .and_then(|c| c.thermal.as_ref())
        .expect("chunk 0 thermal");
    let origin_y = thermal.0.origin_y_m;
    let cell = thermal.0.cell_size_m;
    let h = thermal.0.height_cells as f32;
    // One cell above the geothermal Dirichlet row, and one below the sky row.
    let deep_y = origin_y + 1.5 * cell;
    let shallow_y = origin_y + (h - 1.5) * cell;
    let deep = world.temperature_at_point(wx, deep_y, tick);
    let shallow = world.temperature_at_point(wx, shallow_y, tick);

    assert!(
        deep.is_finite() && shallow.is_finite(),
        "NaN/Inf temperatures: deep={deep} shallow={shallow}"
    );
    assert!(
        deep > shallow + 5.0,
        "expected deeper warmer: deep={deep:.2}C shallow={shallow:.2}C \
         (deep_y={deep_y:.1} shallow_y={shallow_y:.1})"
    );
    // Near-bottom should sit close to the geothermal Dirichlet BC.
    assert!(
        (deep - world.geothermal_bottom_c).abs() < 15.0,
        "deep temp far from geothermal BC: {deep:.2}C"
    );
    // Near-top should sit closer to the sky temperature than to geothermal.
    let sky = world.temperature_at(world.sea_level, tick);
    assert!(
        (shallow - sky).abs() < (shallow - world.geothermal_bottom_c).abs(),
        "shallow={shallow:.2}C closer to geo than sky={sky:.2}C"
    );

    // No cell in any thermal field may blow up.
    for chunk in world.chunks.values() {
        let Some(thermal) = &chunk.thermal else {
            continue;
        };
        let w = thermal.0.width_cells as usize;
        let h = thermal.0.height_cells as usize;
        for cy in 0..h {
            for cx in 0..w {
                let t = thermal.0.cell_at(cx, cy);
                assert!(
                    t.is_finite() && t > -50.0 && t < 120.0,
                    "cell ({cx},{cy}) chunk {} out of range: {t}",
                    chunk.coord
                );
            }
        }
    }

    eprintln!(
        "E20: deep={deep:.2}C shallow={shallow:.2}C in {:?}",
        elapsed
    );
}
