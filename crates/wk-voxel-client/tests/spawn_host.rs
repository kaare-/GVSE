//! Integration: viewer-side [`SpawnedHost`] launches `wk-voxel-host --dev`.

use wk_voxel_client::ipc::SpawnedHost;
use wk_voxel_client::ClientCommand;

#[test]
fn spawn_dev_host_pull_and_quit() {
    let bin = env!("CARGO_BIN_EXE_wk-voxel-host");
    let mut spawned = SpawnedHost::spawn_with_bin(bin, true).expect("spawn");
    let f0 = spawned.remote.pull().expect("pull0");
    assert!(f0.schema_ok());
    assert!(f0.world_width_cols < 4096, "dev host should be tiny");
    spawned
        .remote
        .apply_all([ClientCommand::SetPaused(true), ClientCommand::StepOnce])
        .expect("cmds");
    let f1 = spawned.remote.pull().expect("pull1");
    assert!(f1.sim_tick > f0.sim_tick);
    // Drop sends Quit + kills child.
}
