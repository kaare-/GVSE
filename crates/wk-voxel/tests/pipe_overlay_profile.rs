//! Times the `P` overlay frame path with the cell pipe active.
//!
//! Playtest: "overlay on P still tanks the fps" (20 fps -> 12). Splits the
//! per-frame cost into the leftover field rebuild — which the pipe then
//! discards for every cell — and the per-visible-cell scan, so the fix
//! targets the cost that is actually there.
//!
//! ```text
//! cargo test -p wk-voxel --test pipe_overlay_profile --release -- --ignored --nocapture
//! ```

use std::time::{Duration, Instant};

use wk_material::MaterialId;
use wk_voxel::{
    apply_pipe_motor, cell_pressure_norm_with_boil, ensure_leftover_hill_view, last_pipe_beat_timings,
    leftover_soak_stats, pipe_network_stats, pipe_overlay_cells, pipe_painting, stamp_world,
    water_capacity_cell, CellPressureKind, PipeBeatTimings, Sat, SteamConfig, Temperature, World,
    WorldgenParams, CHUNK_CELLS_H, CHUNK_CELLS_W, PHASE_EXPANSION_DRIVE_MAX, STEAM_EVERY,
};

fn ms(d: Duration) -> f32 {
    d.as_secs_f32() * 1000.0
}

fn params() -> WorldgenParams {
    WorldgenParams {
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 5,
        width_cols: (CHUNK_CELLS_W as i32) * 8,
        ..WorldgenParams::default()
    }
}

/// A soaked, superheated hill: a broad wet body above boil, so the pipe
/// claims a large reservoir and the leftover field has plenty to flood.
fn hot_hill() -> (World, Temperature, WorldgenParams) {
    let p = params();
    let mut world = World::new(p.seed);
    stamp_world(&mut world, &p);
    let mut temp = Temperature::with_world_bounds(
        4,
        0,
        p.bedrock_floor_y,
        p.width_cols,
        p.sky_ceiling_y,
        world.seed.0,
        p.width_cols,
        20,
        false,
    );
    // Fill pores so the claimed body is wide, and superheat the lower rock.
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        let x0 = coord.cx * CHUNK_CELLS_W as i32;
        let y0 = coord.cy * CHUNK_CELLS_H as i32;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                let gx = world.wrap_x(x0 + lx as i32);
                let gy = y0 + ly as i32;
                let Some(cell) = world.get_cell(gx, gy) else {
                    continue;
                };
                if cell.material == MaterialId::Air || cell.material == MaterialId::Bedrock {
                    continue;
                }
                let cap = water_capacity_cell(cell, &world.hydro);
                if cap > 0 {
                    let mut c = cell;
                    c.sat = Sat(cap);
                    world.set_cell(gx, gy, c);
                }
                let (hx, hy) = temp.tile_of(gx, gy);
                temp.set_tile_c(hx, hy, if gy < 90 { 180.0 } else { 30.0 });
            }
        }
    }
    (world, temp, p)
}

fn run_motor(world: &mut World, temp: &mut Temperature, beats: u64) {
    let mut cfg = SteamConfig::default();
    cfg.enable_pipe = true;
    cfg.phase_expansion_drive = PHASE_EXPANSION_DRIVE_MAX;
    for _ in 0..beats {
        world.tick += STEAM_EVERY;
        apply_pipe_motor(world, temp, &cfg, None);
    }
}

/// The per-visible-cell overlay scan exactly as `main.rs` runs it.
fn scan_frame(world: &World, temp: &Temperature, cols: i32, rows: i32) -> (Duration, usize) {
    let start = Instant::now();
    let mut painted = 0usize;
    for x in 0..cols {
        for y in 1..rows {
            if world.get_cell(x, y).is_none() {
                continue;
            }
            let t = temp.at_cell(x, y);
            let (p, kind) =
                cell_pressure_norm_with_boil(world, x, y, t, 100.0, PHASE_EXPANSION_DRIVE_MAX);
            if kind != CellPressureKind::None && p > 0.0 {
                painted += 1;
            }
        }
    }
    (start.elapsed(), painted)
}

/// The overlay must not build the leftover field while the pipe owns `P`:
/// the memo keys on `world.tick` so a running sim rebuilds it every frame,
/// and `cell_pressure_norm` discards it for every cell.
#[test]
fn overlay_skips_the_leftover_flood_while_the_pipe_owns_p() {
    let (mut world, mut temp, _) = hot_hill();
    run_motor(&mut world, &mut temp, 8);
    assert!(pipe_painting(&world), "setup: pipe should own P");
    for _ in 0..4 {
        world.tick += 1;
        ensure_leftover_hill_view(&world, &temp, 100.0, PHASE_EXPANSION_DRIVE_MAX);
    }
    let stats = leftover_soak_stats(&world);
    assert_eq!(
        stats.map, 0,
        "leftover field was flooded anyway ({} cells)",
        stats.map
    );
    assert_eq!(stats.zone, 0, "leftover zone was flooded anyway");
}

/// Cost of one motor beat at soak scale. The soak reported `[sim skip]` at
/// 3 fps with a 21k-cell network, so the beat itself matters, not just the
/// overlay.
#[test]
#[ignore]
fn pipe_motor_beat_cost() {
    let (mut world, mut temp, _) = hot_hill();
    run_motor(&mut world, &mut temp, 40);
    let stats = pipe_network_stats(&world);
    eprintln!(
        "network: mains={} feeders={} cells={}",
        stats.mains, stats.feeders, stats.cells
    );
    let mut cfg = SteamConfig::default();
    cfg.enable_pipe = true;
    cfg.phase_expansion_drive = PHASE_EXPANSION_DRIVE_MAX;
    let mut total = Duration::ZERO;
    let mut phases = PipeBeatTimings::default();
    const BEATS: u32 = 20;
    for _ in 0..BEATS {
        world.tick += STEAM_EVERY;
        let start = Instant::now();
        apply_pipe_motor(&mut world, &mut temp, &cfg, None);
        total += start.elapsed();
        let t = last_pipe_beat_timings();
        phases.retire_us += t.retire_us;
        phases.rewalk_us += t.rewalk_us;
        phases.rebuild_us += t.rebuild_us;
        phases.attach_us += t.attach_us;
        phases.reclaim_us += t.reclaim_us;
        phases.wick_us += t.wick_us;
        phases.reflash_us += t.reflash_us;
        phases.pulse_us += t.pulse_us;
        phases.deposit_us += t.deposit_us;
        phases.total_us += t.total_us;
    }
    let avg = total / BEATS;
    let n = BEATS as f32;
    eprintln!(
        "apply_pipe_motor: {:.2}ms per beat (every {STEAM_EVERY} ticks)\n  \
         retire={:.2} rewalk={:.2} rebuild={:.2} attach={:.2} reclaim={:.2}\n  \
         wick={:.2} reflash={:.2} pulse={:.2} deposit={:.2} (inner {:.2})",
        ms(avg),
        phases.retire_us as f32 / n / 1000.0,
        phases.rewalk_us as f32 / n / 1000.0,
        phases.rebuild_us as f32 / n / 1000.0,
        phases.attach_us as f32 / n / 1000.0,
        phases.reclaim_us as f32 / n / 1000.0,
        phases.wick_us as f32 / n / 1000.0,
        phases.reflash_us as f32 / n / 1000.0,
        phases.pulse_us as f32 / n / 1000.0,
        phases.deposit_us as f32 / n / 1000.0,
        phases.total_us as f32 / n / 1000.0,
    );
}

#[test]
#[ignore]
fn pipe_overlay_frame_cost() {
    let (mut world, mut temp, p) = hot_hill();
    run_motor(&mut world, &mut temp, 40);
    let stats = pipe_network_stats(&world);
    eprintln!(
        "pipe painting={} mains={} feeders={} cells={}",
        pipe_painting(&world),
        stats.mains,
        stats.feeders,
        stats.cells
    );

    let (cols, rows) = (p.width_cols, 140);
    let visible = (cols * (rows - 1)) as usize;

    // What main.rs paid per frame: the leftover memo keys on world.tick, so
    // a running sim invalidates it every frame.
    let mut flood_total = Duration::ZERO;
    const FRAMES: u32 = 10;
    for f in 0..FRAMES {
        world.tick += 1;
        let start = Instant::now();
        ensure_leftover_hill_view(&world, &temp, 100.0, PHASE_EXPANSION_DRIVE_MAX);
        let d = start.elapsed();
        flood_total += d;
        if f == 0 {
            eprintln!("leftover flood, first frame: {:.2}ms", ms(d));
        }
    }
    let flood_avg = flood_total / FRAMES;

    let mut scan_total = Duration::ZERO;
    let mut painted = 0;
    for _ in 0..FRAMES {
        let (d, n) = scan_frame(&world, &temp, cols, rows);
        scan_total += d;
        painted = n;
    }
    let scan_avg = scan_total / FRAMES;

    eprintln!(
        "per frame: leftover_flood={:.2}ms cell_scan={:.2}ms ({visible} visible, {painted} painted)",
        ms(flood_avg),
        ms(scan_avg),
    );
    eprintln!(
        "at a 50ms/20fps budget the overlay eats {:.0}%",
        (ms(flood_avg) + ms(scan_avg)) / 50.0 * 100.0
    );

    // Quads pushed per frame: one per painted cell, versus one per merged
    // vertical run of the same band (what the app now draws).
    let mut runs = 0usize;
    let q = |v: f32| (v.clamp(0.0, 1.0) * 256.0) as u16;
    for x in 0..cols {
        let mut open: Option<u16> = None;
        for y in 1..rows {
            let band = world.get_cell(x, y).and_then(|_| {
                let t = temp.at_cell(x, y);
                let (p, kind) =
                    cell_pressure_norm_with_boil(&world, x, y, t, 100.0, PHASE_EXPANSION_DRIVE_MAX);
                (kind != CellPressureKind::None && p > 0.0).then(|| q(p))
            });
            match (band, open) {
                (Some(b), Some(o)) if b == o => {}
                (Some(b), _) => {
                    runs += 1;
                    open = Some(b);
                }
                (None, _) => open = None,
            }
        }
    }
    eprintln!(
        "quads per frame: {painted} per-cell -> {runs} merged runs ({:.0}x fewer)",
        painted as f32 / runs.max(1) as f32
    );

    // What the renderer does now: ask the pipe for its cells instead of
    // asking every visible cell whether it paints.
    let mut list_total = Duration::ZERO;
    let mut listed = 0usize;
    for _ in 0..FRAMES {
        let start = Instant::now();
        let cells = pipe_overlay_cells(&world);
        list_total += start.elapsed();
        listed = cells.len();
    }
    let list_avg = list_total / FRAMES;
    eprintln!(
        "cell-driven overlay: {:.2}ms for {listed} cells (vs {:.2}ms scanning {visible})",
        ms(list_avg),
        ms(scan_avg),
    );
    // The point is the scaling, not this world: cost now follows the network
    // instead of the window. This slab is pathological — nearly every cell is
    // claimed. A soak network of ~4.6k cells under a ~112k-cell view is the
    // realistic shape.
    eprintln!(
        "  per cell {:.0}ns, so a 4.6k-cell network costs ~{:.2}ms",
        list_avg.as_secs_f64() * 1e9 / listed.max(1) as f64,
        list_avg.as_secs_f64() * 1000.0 * 4600.0 / listed.max(1) as f64,
    );

    // A real view is mostly sky, and the soak ran with `steam=0c` — an empty
    // `world.steam`. Every one of those air cells still went through the
    // cavity branch's vessel probe.
    let sky_rows = 220;
    let mut sky_total = Duration::ZERO;
    for _ in 0..FRAMES {
        let (d, _) = scan_frame(&world, &temp, cols, sky_rows);
        sky_total += d;
    }
    eprintln!(
        "sky-heavy view ({} visible, {} sky): cell_scan={:.2}ms",
        cols * (sky_rows - 1),
        cols * (sky_rows - rows),
        ms(sky_total / FRAMES),
    );
}
