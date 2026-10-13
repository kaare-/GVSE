//! Phase 8 out-of-process sim host (TCP).
//!
//! ```text
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --dev
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --lan
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --seed 42 --once
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --bind 127.0.0.1:0
//! ```
//!
//! One client at a time. After a session ends, accept the next viewer unless
//! `--once`. Same [`ViewFrame`] / [`ClientCommand`] contract as in-process.
//!
//! Bind + announce listen address **before** stamping so `--spawn-host`
//! can discover an ephemeral port while Compact worldgen runs.
//!
//! See [`docs/PHASE8_HOST_SHIP.md`].

use std::env;
use std::io::Write;
use std::net::TcpListener;

use wk_voxel_client::ipc::{serve_client, DEFAULT_HOST_ADDR, LAN_HOST_ADDR};
use wk_voxel_client::{Host, COMMAND_SCHEMA, VIEW_SCHEMA};

struct HostCli {
    bind: String,
    dev: bool,
    once: bool,
    lan: bool,
    seed: Option<u64>,
}

fn parse_cli() -> HostCli {
    let mut bind: Option<String> = None;
    let mut dev = false;
    let mut once = false;
    let mut lan = false;
    let mut seed = None;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dev" => dev = true,
            "--once" => once = true,
            "--lan" => lan = true,
            "--bind" => {
                bind = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--bind requires HOST:PORT");
                    std::process::exit(2);
                }));
            }
            "--seed" => {
                let raw = args.next().unwrap_or_else(|| {
                    eprintln!("--seed requires integer");
                    std::process::exit(2);
                });
                seed = Some(raw.parse().unwrap_or_else(|_| {
                    eprintln!("--seed: invalid u64 '{raw}'");
                    std::process::exit(2);
                }));
            }
            "-h" | "--help" => {
                eprintln!(
                    "wk-voxel-host [options]\n\
                     \n\
                       --dev              tiny ring (fast smoke)\n\
                       --once             exit after one client session\n\
                       --lan              bind {LAN_HOST_ADDR} (no auth; trusted LAN only)\n\
                       --bind HOST:PORT   listen address (default {DEFAULT_HOST_ADDR})\n\
                       --seed N           Compact worldgen seed\n\
                     \n\
                     Schemas: VIEW_SCHEMA={VIEW_SCHEMA} COMMAND_SCHEMA={COMMAND_SCHEMA}\n\
                     See docs/PHASE8_HOST_SHIP.md"
                );
                std::process::exit(0);
            }
            other => {
                eprintln!("unknown arg: {other}");
                std::process::exit(2);
            }
        }
    }
    let bind = bind.unwrap_or_else(|| {
        if lan {
            LAN_HOST_ADDR.to_string()
        } else {
            DEFAULT_HOST_ADDR.to_string()
        }
    });
    HostCli {
        bind,
        dev,
        once,
        lan,
        seed,
    }
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = parse_cli();
    if cli.lan && cli.bind.starts_with("0.0.0.0") {
        log::warn!("--lan: binding all interfaces with NO AUTH — trusted network only");
    }

    let listener = TcpListener::bind(&cli.bind).unwrap_or_else(|e| {
        eprintln!("bind {}: {e}", cli.bind);
        std::process::exit(1);
    });
    let local = listener.local_addr().expect("local_addr");
    println!("wk-voxel-host listening on {local}");
    let _ = std::io::stdout().flush();
    log::info!(
        "schemas VIEW_SCHEMA={VIEW_SCHEMA} COMMAND_SCHEMA={COMMAND_SCHEMA} once={} lan={}",
        cli.once,
        cli.lan
    );

    let mut host = if cli.dev {
        log::info!("stamping dev (tiny) host");
        Host::for_tests()
    } else if let Some(seed) = cli.seed {
        log::info!("stamping Compact host seed={seed:#x}");
        Host::compact_with_seed(seed)
    } else {
        log::info!("stamping Compact host (stream + park)");
        Host::default()
    };
    log::info!(
        "host ready compact={} seed={} focus={} view={}",
        host.is_compact(),
        host.world_seed(),
        host.focus_col,
        host.view_cols
    );

    let mut session = 0u32;
    loop {
        session += 1;
        log::info!("waiting for viewer (session {session})…");
        let (stream, peer) = listener.accept().unwrap_or_else(|e| {
            eprintln!("accept: {e}");
            std::process::exit(1);
        });
        log::info!("client connected from {peer} (session {session})");
        // Clear quit so a reconnecting viewer can drive the same world.
        host.quit_requested = false;
        match serve_client(stream, &mut host) {
            Ok(()) => log::info!("session {session} ended (quit)"),
            Err(e) => log::warn!("session {session} error: {e}"),
        }
        if cli.once {
            log::info!("--once: exiting after session {session}");
            break;
        }
        log::info!("session {session} done — ready for reconnect (same world)");
    }
}
