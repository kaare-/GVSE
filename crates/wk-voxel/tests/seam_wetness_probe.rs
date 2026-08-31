//! Is there a wetness discontinuity at horizontal chunk seams?
//!
//! Playtest shows a full-width band of different wetness at a chunk boundary,
//! made visible by terrain now darkening with pore saturation. Chunks are 64
//! cells tall, so seams sit at y = 64, 128, 192, ...
//!
//! Reports mean pore wetness per row and flags rows whose wetness jumps
//! relative to their neighbours, so the offending row is identified rather
//! than guessed at.
//!
//! ```text
//! cargo test -p wk-voxel --release --test seam_wetness_probe -- --ignored --nocapture
//! ```

use wk_material::MaterialId;
use wk_voxel::{
    stamp_world, tick_with_perf, water_capacity_cell, PerfConfig, World, WorldgenParams,
    CHUNK_CELLS_H, CHUNK_CELLS_W,
};

/// Mean `sat / capacity` over porous solids in a row, and how many there were.
fn row_wetness(world: &World, params: &WorldgenParams, y: i32) -> (f32, usize) {
    let mut sum = 0.0f32;
    let mut n = 0usize;
    for x in 0..params.width_cols {
        let Some(cell) = world.get_cell(x, y) else {
            continue;
        };
        if cell.material == MaterialId::Air {
            continue;
        }
        let cap = water_capacity_cell(cell, &world.hydro);
        if cap == 0 {
            continue;
        }
        sum += cell.sat.0 as f32 / cap as f32;
        n += 1;
    }
    if n == 0 {
        (0.0, 0)
    } else {
        (sum / n as f32, n)
    }
}

#[test]
#[ignore = "diagnostic probe; run explicitly"]
fn probe_seam_wetness_band() {
    // The world the demo actually runs.
    let params = WorldgenParams::default();
    let mut world = World::new(params.seed);
    stamp_world(&mut world, &params);
    let perf = PerfConfig::default();
    for _ in 0..3000 {
        tick_with_perf(&mut world, &perf);
    }
    println!(
        "\n  demo world {}x{}  sea_level_y={}  bedrock_floor_y={}",
        params.width_cols,
        params.sky_ceiling_y - params.bedrock_floor_y,
        params.sea_level_y,
        params.bedrock_floor_y
    );

    let ch = CHUNK_CELLS_H as i32;
    println!("\n=== per-row pore wetness (chunk height {ch}) ===");
    let mut rows: Vec<(i32, f32, usize)> = Vec::new();
    for y in params.bedrock_floor_y..params.sky_ceiling_y {
        let (w, n) = row_wetness(&world, &params, y);
        if n > params.width_cols as usize / 4 {
            rows.push((y, w, n));
        }
    }

    // Flag rows that differ sharply from the average of their neighbours.
    println!("  rows with a wetness step vs neighbours (|delta| > 0.06):");
    let mut worst: Vec<(f32, i32, bool)> = Vec::new();
    for i in 1..rows.len().saturating_sub(1) {
        let (y, w, _) = rows[i];
        let neighbour = (rows[i - 1].1 + rows[i + 1].1) * 0.5;
        let delta = w - neighbour;
        if delta.abs() > 0.06 {
            let on_seam = y % ch == 0 || y % ch == ch - 1;
            println!(
                "    y={y:<4} wet={w:.3}  neighbours={neighbour:.3}  delta={delta:+.3}{}",
                if on_seam { "   <-- CHUNK SEAM" } else { "" }
            );
            worst.push((delta.abs(), y, on_seam));
        }
    }
    worst.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

    let seam_steps = worst.iter().filter(|(_, _, s)| *s).count();
    let other_steps = worst.len() - seam_steps;
    println!("\n  steps on a chunk seam row: {seam_steps}");
    println!("  steps elsewhere:           {other_steps}");
    println!("  top 10 steps (largest first):");
    for (d, y, s) in worst.iter().take(10) {
        println!(
            "    y={y:<4} |delta|={d:.3}{}",
            if *s { "   <-- CHUNK SEAM" } else { "" }
        );
    }

    // Vertical profile through the wetted zone: a water table shows as a sharp
    // edge, a seam artifact as a one-or-two-row spike.
    println!("\n  --- vertical wetness profile ---");
    for y in params.bedrock_floor_y..(params.bedrock_floor_y + 90) {
        let (w, n) = row_wetness(&world, &params, y);
        if n == 0 {
            continue;
        }
        let bar = "#".repeat((w * 40.0).round() as usize);
        let seam = if y % ch == 0 { " <-- seam" } else { "" };
        println!("    y={y:<4} {w:.3} {bar}{seam}");
    }

    // Also print the immediate neighbourhood of every seam for context.
    for seam in 1..(params.sky_ceiling_y / ch) {
        let y0 = seam * ch;
        println!("\n  --- seam at y={y0} ---");
        for y in (y0 - 3)..=(y0 + 2) {
            let (w, n) = row_wetness(&world, &params, y);
            if n > 0 {
                println!("    y={y:<4} wet={w:.3}  cells={n}");
            }
        }
    }
}
