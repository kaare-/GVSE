//! One-shot Compact host smoke (not CI): stamp, park, step, extract.
//!
//! ```text
//! cargo run -p wk-voxel-client --example compact_host_smoke --release
//! ```

use std::time::Instant;

use wk_voxel_client::{ClientCommand, Host, COMPACT_VIEW_COLS};

fn main() {
    let t0 = Instant::now();
    println!("stamp Compact Host::default() …");
    let mut host = Host::default();
    let stamp_ms = t0.elapsed().as_millis();
    assert!(host.is_compact(), "default host must be Compact-shaped");
    println!(
        "stamp_ok  {} ms · compact={} · view_cols={} (want ≤{})",
        stamp_ms,
        host.is_compact(),
        host.view_cols,
        COMPACT_VIEW_COLS
    );

    host.apply(ClientCommand::SetPaused(true));
    let f0 = host.frame();
    assert!(f0.schema_ok());
    assert_eq!(f0.world_width_cols, 4096);
    assert!(f0.world_height >= 1000, "tropopause height, got {}", f0.world_height);
    assert_eq!(f0.view_cols, COMPACT_VIEW_COLS.min(f0.world_width_cols));
    assert_eq!(f0.terrain.cols, f0.view_cols as u32);
    assert!(
        f0.terrain.rows <= 384,
        "Compact strip band should be ≤384, got {}",
        f0.terrain.rows
    );
    assert!(f0.terrain.rgba.iter().any(|&b| b != 0));
    println!(
        "frame0    tick={} · world {}×{} · strip {}×{} · sky day={}",
        f0.sim_tick,
        f0.world_width_cols,
        f0.world_height,
        f0.terrain.cols,
        f0.terrain.rows,
        f0.sky.is_day
    );

    let t1 = Instant::now();
    for _ in 0..5 {
        host.apply(ClientCommand::StepOnce);
    }
    let step_ms = t1.elapsed().as_millis();
    let f1 = host.frame();
    assert!(f1.sim_tick > f0.sim_tick);
    assert!(f1.product_step);
    println!(
        "step5     +{} ticks in {} ms · last_step_ms={} · product={}",
        f1.sim_tick - f0.sim_tick,
        step_ms,
        f1.last_step_ms,
        f1.product_step
    );

    let before = f1.focus_col;
    host.apply(ClientCommand::SetCamera {
        focus_col: before + 64,
        view_cols: COMPACT_VIEW_COLS,
    });
    host.apply(ClientCommand::StepOnce);
    let f2 = host.frame();
    assert_eq!(f2.focus_col, before + 64);
    assert_eq!(f2.terrain.cols, COMPACT_VIEW_COLS as u32);
    println!(
        "camera    focus {}→{} · strip origin_col={}",
        before, f2.focus_col, f2.terrain.origin_col
    );
    println!("compact_host_smoke OK");
}
