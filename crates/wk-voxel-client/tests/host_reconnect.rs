//! Host session reconnect: second viewer attaches to the same world.

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use wk_voxel_client::ipc::RemoteHost;
use wk_voxel_client::ClientCommand;

fn wait_listen(stdout: impl std::io::Read, timeout: Duration) -> String {
    let deadline = Instant::now() + timeout;
    let mut lines = BufReader::new(stdout).lines();
    while Instant::now() < deadline {
        let line = lines
            .next()
            .expect("host stdout closed")
            .expect("host stdout");
        if let Some(rest) = line.strip_prefix("wk-voxel-host listening on ") {
            return rest.trim().to_string();
        }
    }
    panic!("timed out waiting for listen line");
}

#[test]
fn second_client_keeps_world_tick() {
    let bin = env!("CARGO_BIN_EXE_wk-voxel-host");
    let mut child = Command::new(bin)
        .args(["--dev", "--bind", "127.0.0.1:0"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn host");
    let stdout = child.stdout.take().expect("stdout");
    let addr = wait_listen(stdout, Duration::from_secs(30));

    let mut a = RemoteHost::connect(&addr).expect("connect a");
    a.apply_all([ClientCommand::SetPaused(true)]).expect("pause");
    for _ in 0..5 {
        a.apply_all([ClientCommand::StepOnce]).expect("step");
        let _ = a.pull().expect("pull");
    }
    let tick = a.pull().expect("tick frame").sim_tick;
    assert!(tick >= 5, "expected advanced tick, got {tick}");
    a.apply_all([ClientCommand::Quit]).expect("quit a");
    // Drain until session ends (host clears quit for next client).
    let _ = a.pull();
    drop(a);

    let mut b = RemoteHost::connect(&addr).expect("connect b");
    let f = b.pull().expect("pull b");
    assert!(
        f.sim_tick >= tick,
        "reconnect should keep world tick ≥{tick}, got {}",
        f.sim_tick
    );
    b.apply_all([ClientCommand::Quit]).expect("quit b");
    let _ = b.pull();
    drop(b);

    let _ = child.kill();
    let _ = child.wait();
}
