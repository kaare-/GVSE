//! Phase 8 out-of-process sim host (localhost TCP).
//!
//! ```text
//! wk-voxel-host                # Compact stamp, bind 127.0.0.1:17420
//! wk-voxel-host --dev          # tiny ring (fast smoke)
//! wk-voxel-host --bind 127.0.0.1:0
//! ```
//!
//! One client at a time. Same [`ViewFrame`] / [`ClientCommand`] contract as
//! the in-process path.
//!
//! Bind + announce listen address **before** stamping so `--spawn-host`
//! can discover an ephemeral port while Compact worldgen runs.

use std::env;
use std::io::Write;
use std::net::TcpListener;

use wk_voxel_client::ipc::{serve_client, DEFAULT_HOST_ADDR};
use wk_voxel_client::Host;

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut bind = DEFAULT_HOST_ADDR.to_string();
    let mut dev = false;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dev" => dev = true,
            "--bind" => {
                bind = args
                    .next()
                    .expect("--bind requires host:port");
            }
            "-h" | "--help" => {
                eprintln!(
                    "wk-voxel-host [--dev] [--bind HOST:PORT]\n\
                     default bind {DEFAULT_HOST_ADDR}; --dev uses Host::for_tests()"
                );
                return;
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }

    let listener = TcpListener::bind(&bind).unwrap_or_else(|e| {
        eprintln!("bind {bind}: {e}");
        std::process::exit(1);
    });
    let local = listener.local_addr().expect("local_addr");
    println!("wk-voxel-host listening on {local}");
    let _ = std::io::stdout().flush();

    let mut host = if dev {
        log::info!("stamping dev (tiny) host");
        Host::for_tests()
    } else {
        log::info!("stamping Compact host (stream + park)");
        Host::default()
    };
    log::info!(
        "host ready compact={} focus={} view={}",
        host.is_compact(),
        host.focus_col,
        host.view_cols
    );
    log::info!("waiting for viewer…");

    let (stream, peer) = listener.accept().unwrap_or_else(|e| {
        eprintln!("accept: {e}");
        std::process::exit(1);
    });
    log::info!("client connected from {peer}");
    match serve_client(stream, &mut host) {
        Ok(()) => log::info!("session ended (quit)"),
        Err(e) => {
            log::warn!("session error: {e}");
        }
    }
}
