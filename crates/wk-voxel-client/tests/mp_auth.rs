//! Multiplayer / auth acceptance ([`docs/PHASE8_MULTIPLAYER.md`]).

use std::io::{BufRead, BufReader};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use wk_voxel_client::ipc::{
    serve_peer_shared, ConnectOpts, PeerRegistry, PeerRole, RemoteHost, ServeConfig,
};
use wk_voxel_client::{ClientCommand, Host};

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
fn host_token_rejects_bad_hello() {
    let bin = env!("CARGO_BIN_EXE_wk-voxel-host");
    let mut child = Command::new(bin)
        .args(["--dev", "--bind", "127.0.0.1:0", "--token", "sekrit"])
        .stdout(Stdio::piped())
        .stderr(Stdio::inherit())
        .spawn()
        .expect("spawn host");
    let stdout = child.stdout.take().expect("stdout");
    let addr = wait_listen(stdout, Duration::from_secs(30));

    let err = match RemoteHost::connect_with(
        &addr,
        &ConnectOpts {
            token: Some("wrong".into()),
            role_hint: PeerRole::Viewer,
            client_name: None,
        },
    ) {
        Ok(_) => panic!("bad token must fail"),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("auth failed") || err.to_string().contains("bad token"),
        "{err}"
    );

    // Correct token works after a reject.
    let mut ok = RemoteHost::connect_with(
        &addr,
        &ConnectOpts {
            token: Some("sekrit".into()),
            role_hint: PeerRole::Viewer,
            client_name: None,
        },
    )
    .expect("good token");
    ok.apply_all([ClientCommand::Quit]).unwrap();
    let _ = ok.pull();
    let _ = child.kill();
    let _ = child.wait();
}

#[test]
fn lan_without_token_fails_closed() {
    let bin = env!("CARGO_BIN_EXE_wk-voxel-host");
    let out = Command::new(bin)
        .args(["--dev", "--lan"])
        .output()
        .expect("run host");
    assert!(!out.status.success(), "expected non-zero exit");
    let err = String::from_utf8_lossy(&out.stderr);
    assert!(
        err.contains("--token") || err.contains("non-loopback"),
        "stderr={err}"
    );
}

#[test]
fn two_peers_distinct_cameras_same_tick() {
    let host = Arc::new(Mutex::new(Host::for_tests()));
    let registry = Arc::new(Mutex::new(PeerRegistry::new(4)));
    let cfg = Arc::new(ServeConfig {
        token: None,
        ops_token: None,
        max_peers: 4,
        loopback: true,
    });

    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let h1 = host.clone();
    let r1 = registry.clone();
    let c1 = cfg.clone();
    let h2 = host.clone();
    let r2 = registry.clone();
    let c2 = cfg.clone();
    let server = thread::spawn(move || {
        for (h, r, c) in [(h1, r1, c1), (h2, r2, c2)] {
            let (stream, _) = listener.accept().unwrap();
            let hh = h.clone();
            let rr = r.clone();
            let cc = c.clone();
            thread::spawn(move || {
                let _ = serve_peer_shared(stream, hh, rr, cc);
            });
        }
        // Keep accept thread alive briefly for peer work.
        thread::sleep(Duration::from_secs(5));
    });

    let mut a = RemoteHost::connect(addr).unwrap();
    let mut b = RemoteHost::connect(addr).unwrap();
    a.apply_all([
        ClientCommand::SetPaused(true),
        ClientCommand::SetCamera {
            focus_col: 10,
            view_cols: 96,
            focus_y: 40,
        },
    ])
    .unwrap();
    b.apply_all([ClientCommand::SetCamera {
        focus_col: 80,
        view_cols: 96,
        focus_y: 40,
    }])
    .unwrap();
    let fa = a.pull().unwrap();
    let fb = b.pull().unwrap();
    assert_eq!(fa.sim_tick, fb.sim_tick);
    assert_eq!(fa.focus_col, 10);
    assert_eq!(fb.focus_col, 80);
    a.apply_all([ClientCommand::Quit]).unwrap();
    let _ = a.pull();
    // B still alive after A quit.
    let fb2 = b.pull().unwrap();
    assert_eq!(fb2.focus_col, 80);
    b.apply_all([ClientCommand::Quit]).unwrap();
    let _ = b.pull();
    let _ = server.join();
}

#[test]
fn ops_can_reset_viewer_cannot() {
    let host = Arc::new(Mutex::new(Host::for_tests()));
    let registry = Arc::new(Mutex::new(PeerRegistry::new(4)));
    let cfg = Arc::new(ServeConfig {
        token: Some("join".into()),
        ops_token: Some("ops".into()),
        max_peers: 4,
        loopback: true,
    });
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let h = host.clone();
    let r = registry.clone();
    let c = cfg.clone();
    let server = thread::spawn(move || {
        for _ in 0..2 {
            let (stream, _) = listener.accept().unwrap();
            let hh = h.clone();
            let rr = r.clone();
            let cc = c.clone();
            thread::spawn(move || {
                let _ = serve_peer_shared(stream, hh, rr, cc);
            });
        }
        thread::sleep(Duration::from_secs(5));
    });

    let mut viewer = RemoteHost::connect_with(
        addr,
        &ConnectOpts {
            token: Some("join".into()),
            role_hint: PeerRole::Viewer,
            client_name: None,
        },
    )
    .unwrap();
    assert_eq!(viewer.role, PeerRole::Viewer);
    viewer
        .apply_all([ClientCommand::ResetWorld { seed: Some(1) }])
        .unwrap();
    let fv = viewer.pull().unwrap();
    assert!(
        fv.command_acks.iter().any(|a| !a.ok),
        "viewer reset should deny: {:?}",
        fv.command_acks
    );

    let mut ops = RemoteHost::connect_with(
        addr,
        &ConnectOpts {
            token: Some("ops".into()),
            role_hint: PeerRole::Operator,
            client_name: None,
        },
    )
    .unwrap();
    assert_eq!(ops.role, PeerRole::Operator);
    ops.apply_all([ClientCommand::SetPaused(true)]).unwrap();
    ops.apply_all([ClientCommand::ResetWorld { seed: Some(2) }])
        .unwrap();
    let fo = ops.pull().unwrap();
    assert!(
        fo.command_acks.iter().any(|a| a.ok && a.detail.contains("reset")),
        "{:?}",
        fo.command_acks
    );
    assert_eq!(fo.sim_tick, 0);

    viewer.apply_all([ClientCommand::Quit]).unwrap();
    let _ = viewer.pull();
    ops.apply_all([ClientCommand::Quit]).unwrap();
    let _ = ops.pull();
    let _ = server.join();
}
