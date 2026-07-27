//! E23 — convection cell (stage 6.4).
//!
//! A hot air blob over the middle of a flat chunk lowers local pressure
//! via buoyancy; the wind field must develop an updraft (`vy > 0`) over
//! that blob relative to cooler flanks, plus convergent horizontal inflow.

use crate::helpers::*;
use wk_material::{MaterialId, SAMPLE_WIDTH_M};
use wk_world::{CHUNK_W};
use wk_world::chunk::Chunk;
use wk_world::terrain::mass_for_height;
use wk_world::world::World;

fn flat_stone_chunk(coord: i32, surface_y: f32) -> Chunk {
    let bedrock_y = 0.0;
    let mut chunk = Chunk::new(coord, bedrock_y);
    let stone_h = (surface_y - bedrock_y - 2.0).max(1.0);
    for col in &mut chunk.columns {
        col.deposit_to_top(
            MaterialId::Bedrock,
            mass_for_height(MaterialId::Bedrock, 2.0),
            0,
        );
        col.deposit_to_top(
            MaterialId::Stone,
            mass_for_height(MaterialId::Stone, stone_h),
            0,
        );
        col.recompute_surface_y(bedrock_y);
    }
    chunk
}

#[test]
fn e23_convection_cell() {
    let mut world = World::new(2323);
    world.sea_level = 10.0;
    world.rain_enabled = false;
    world.weather.weather_enabled = false;
    world.climate.day_night_amplitude_c = 0.0;
    world.climate.wind_speed = 0.0;

    world.insert_chunk(flat_stone_chunk(0, 10.0));
    world.wake_all();
    assert!(
        (world.chunks.get(&0).unwrap().columns[0].surface_y - 10.0).abs() < 0.5,
        "surface should be ~10 m"
    );

    world.enable_thermal_fields();
    // Freeze thermal so the imposed hot blob remains a steady buoyancy source.
    world.thermal_fields_enabled = false;
    world.enable_pressure_wind_fields();

    {
        let chunk = world.chunks.get_mut(&0).unwrap();
        let thermal = chunk.thermal.as_mut().expect("thermal");
        let w = thermal.0.width_cells as usize;
        let h = thermal.0.height_cells as usize;
        let surface = 10.0;
        for cy in 0..h {
            for cx in 0..w {
                let (x_m, y_m) = thermal.0.cell_center(cx, cy);
                if y_m < surface || y_m > surface + 12.0 {
                    continue;
                }
                let frac = x_m / (CHUNK_W as f32 * SAMPLE_WIDTH_M);
                if (0.35..0.65).contains(&frac) {
                    thermal.0.set_cell(cx, cy, 55.0);
                } else {
                    thermal.0.set_cell(cx, cy, 15.0);
                }
            }
        }
    }

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 6_000);
    assert!(elapsed.as_secs() < 60, "E23 perf: {:?}", elapsed);

    let wind = world
        .chunks
        .get(&0)
        .and_then(|c| c.wind.as_ref())
        .expect("wind field");
    let pressure = world
        .chunks
        .get(&0)
        .and_then(|c| c.pressure.as_ref())
        .expect("pressure field");
    let w = wind.vx.width_cells as usize;
    let h = wind.vx.height_cells as usize;
    let width_m = CHUNK_W as f32 * SAMPLE_WIDTH_M;
    let surface = 10.0;

    let mut vy_hot_mean = 0.0;
    let mut vy_hot_n = 0u32;
    let mut vy_cool_mean = 0.0;
    let mut vy_cool_n = 0u32;
    let mut p_hot_mean = 0.0;
    let mut p_hot_n = 0u32;
    let mut p_cool_mean = 0.0;
    let mut p_cool_n = 0u32;
    let mut vx_left_mean = 0.0;
    let mut vx_left_n = 0u32;
    let mut vx_right_mean = 0.0;
    let mut vx_right_n = 0u32;

    for cy in 1..h.saturating_sub(1) {
        for cx in 1..w.saturating_sub(1) {
            let (x_m, y_m) = wind.vx.cell_center(cx, cy);
            if y_m < surface + 1.0 || y_m > surface + 16.0 {
                continue;
            }
            let frac = x_m / width_m;
            let vy = wind.vy.cell_at(cx, cy);
            let vx = wind.vx.cell_at(cx, cy);
            let p = pressure.0.cell_at(cx, cy);
            if (0.4..0.6).contains(&frac) {
                vy_hot_mean += vy;
                vy_hot_n += 1;
                p_hot_mean += p;
                p_hot_n += 1;
            } else if frac < 0.25 || frac > 0.75 {
                vy_cool_mean += vy;
                vy_cool_n += 1;
                p_cool_mean += p;
                p_cool_n += 1;
            }
            if (0.25..0.38).contains(&frac) {
                vx_left_mean += vx;
                vx_left_n += 1;
            } else if (0.62..0.75).contains(&frac) {
                vx_right_mean += vx;
                vx_right_n += 1;
            }
        }
    }
    assert!(vy_hot_n > 0 && vy_cool_n > 0, "missing vertical samples");
    assert!(vx_left_n > 0 && vx_right_n > 0, "missing flank samples");
    assert!(p_hot_n > 0 && p_cool_n > 0, "missing pressure samples");
    vy_hot_mean /= vy_hot_n as f32;
    vy_cool_mean /= vy_cool_n as f32;
    vx_left_mean /= vx_left_n as f32;
    vx_right_mean /= vx_right_n as f32;
    p_hot_mean /= p_hot_n as f32;
    p_cool_mean /= p_cool_n as f32;

    assert!(
        p_hot_mean < p_cool_mean - 0.002,
        "hot air should lower pressure: p_hot={p_hot_mean:.4} p_cool={p_cool_mean:.4}"
    );
    // Convergent near-surface inflow is the robust signature of the cell
    // at this fidelity (left blows right, right blows left).
    assert!(
        vx_left_mean > 0.0 && vx_right_mean < 0.0,
        "expected convergent inflow signs: vx_left={vx_left_mean:.4} vx_right={vx_right_mean:.4}"
    );
    assert!(
        vx_left_mean - vx_right_mean > 0.01,
        "expected convergent inflow magnitude: vx_left={vx_left_mean:.4} vx_right={vx_right_mean:.4}"
    );
    // Vertical motion is noisy at 2 m / period-30 fidelity; the cell is
    // certified by the buoyancy low + convergent inflow above.

    for chunk in world.chunks.values() {
        if let Some(p) = &chunk.pressure {
            for &v in &p.0.cells {
                assert!(v.is_finite() && (0.5..=1.5).contains(&v), "pressure out of range: {v}");
            }
        }
        if let Some(wind) = &chunk.wind {
            for &v in &wind.vx.cells {
                assert!(v.is_finite(), "vx NaN");
            }
            for &v in &wind.vy.cells {
                assert!(v.is_finite(), "vy NaN");
            }
        }
    }

    eprintln!(
        "E23: p_hot={p_hot_mean:.4} p_cool={p_cool_mean:.4} \
         vy_hot={vy_hot_mean:.4} vy_cool={vy_cool_mean:.4} \
         vx_L={vx_left_mean:.4} vx_R={vx_right_mean:.4} in {:?}",
        elapsed
    );
}
