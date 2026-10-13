//! Connect to a running `wk-voxel-host` and pull a few frames.
//!
//! ```text
//! wk-voxel-host --dev --bind 127.0.0.1:17420 &
//! cargo run -p wk-voxel-client --example oop_ipc_smoke --release -- 127.0.0.1:17420
//! ```

use std::env;

use wk_voxel_client::ipc::{RemoteHost, DEFAULT_HOST_ADDR};
use wk_voxel_client::ClientCommand;

fn main() {
    let addr = env::args()
        .nth(1)
        .unwrap_or_else(|| DEFAULT_HOST_ADDR.to_string());
    println!("connect {addr}");
    let mut remote = RemoteHost::connect(&*addr).expect("connect");
    let f0 = remote.pull().expect("pull0");
    assert!(f0.schema_ok());
    println!(
        "frame0 tick={} world {}×{} strip {}×{} paused={}",
        f0.sim_tick,
        f0.world_width_cols,
        f0.world_height,
        f0.terrain.cols,
        f0.terrain.rows,
        f0.paused
    );
    remote
        .apply_all([ClientCommand::SetPaused(true)])
        .expect("pause");
    let f1 = remote.pull().expect("pull1");
    assert!(f1.paused);
    let t1 = f1.sim_tick;
    remote.apply_all([ClientCommand::StepOnce]).expect("step");
    let f2 = remote.pull().expect("pull2");
    assert!(f2.sim_tick > t1);
    println!(
        "stepped {} → {} · last_step_ms={}",
        t1, f2.sim_tick, f2.last_step_ms
    );
    remote.apply_all([ClientCommand::Quit]).expect("quit");
    let _ = remote.pull();
    println!("oop_ipc_smoke OK");
}
