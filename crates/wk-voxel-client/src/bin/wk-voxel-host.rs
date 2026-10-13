//! Phase 8 out-of-process sim host (TCP).
//!
//! ```text
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --dev
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --lan --token SECRET
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --seed 42 --once
//! cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --bind 127.0.0.1:0
//! ```
//!
//! Concurrent viewers (default max 4). Shared-secret auth for non-loopback.
//! See [`docs/PHASE8_HOST_SHIP.md`], [`docs/PHASE8_MULTIPLAYER.md`].

use std::env;
use std::io::Write;
use std::net::TcpListener;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;

use wk_voxel_client::ipc::{
    serve_peer_shared, PeerRegistry, ServeConfig, DEFAULT_HOST_ADDR, DEFAULT_MAX_PEERS,
    LAN_HOST_ADDR, MAX_PEERS_CAP,
};
use wk_voxel_client::{Host, COMMAND_SCHEMA, VIEW_SCHEMA};

struct HostCli {
    bind: String,
    dev: bool,
    once: bool,
    lan: bool,
    insecure: bool,
    seed: Option<u64>,
    token: Option<String>,
    ops_token: Option<String>,
    max_peers: u8,
}

fn env_token(name: &str) -> Option<String> {
    env::var(name).ok().filter(|s| !s.is_empty())
}

fn parse_cli() -> HostCli {
    let mut bind: Option<String> = None;
    let mut dev = false;
    let mut once = false;
    let mut lan = false;
    let mut insecure = false;
    let mut seed = None;
    let mut token = env_token("WK_VOXEL_HOST_TOKEN");
    let mut ops_token = env_token("WK_VOXEL_HOST_OPS_TOKEN");
    let mut max_peers = DEFAULT_MAX_PEERS;
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dev" => dev = true,
            "--once" => once = true,
            "--lan" => lan = true,
            "--insecure" => insecure = true,
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
            "--token" => {
                token = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--token requires SECRET");
                    std::process::exit(2);
                }));
            }
            "--ops-token" => {
                ops_token = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--ops-token requires SECRET");
                    std::process::exit(2);
                }));
            }
            "--max-peers" => {
                let raw = args.next().unwrap_or_else(|| {
                    eprintln!("--max-peers requires integer");
                    std::process::exit(2);
                });
                let n: u8 = raw.parse().unwrap_or_else(|_| {
                    eprintln!("--max-peers: invalid u8 '{raw}'");
                    std::process::exit(2);
                });
                max_peers = ServeConfig::clamp_peers(n);
            }
            "-h" | "--help" => {
                eprintln!(
                    "wk-voxel-host [options]\n\
                     \n\
                       --dev              tiny ring (fast smoke)\n\
                       --once             exit after first peer session ends\n\
                       --lan              bind {LAN_HOST_ADDR} (requires --token unless --insecure)\n\
                       --bind HOST:PORT   listen address (default {DEFAULT_HOST_ADDR})\n\
                       --seed N           Compact worldgen seed\n\
                       --token SECRET     join token (env WK_VOXEL_HOST_TOKEN)\n\
                       --ops-token SECRET operator token (env WK_VOXEL_HOST_OPS_TOKEN)\n\
                       --max-peers N      concurrent viewers (default {DEFAULT_MAX_PEERS}, max {MAX_PEERS_CAP})\n\
                       --insecure         allow non-loopback with no token (dev only)\n\
                     \n\
                     Schemas: VIEW_SCHEMA={VIEW_SCHEMA} COMMAND_SCHEMA={COMMAND_SCHEMA}\n\
                     See docs/PHASE8_HOST_SHIP.md and docs/PHASE8_MULTIPLAYER.md"
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
    if once {
        max_peers = 1;
    }
    HostCli {
        bind,
        dev,
        once,
        lan,
        insecure,
        seed,
        token,
        ops_token,
        max_peers,
    }
}

fn is_loopback_bind(bind: &str) -> bool {
    bind.starts_with("127.0.0.1") || bind.starts_with("localhost") || bind.starts_with("[::1]")
}

fn is_reject(err: &wk_voxel_client::ipc::IpcError) -> bool {
    let s = err.to_string();
    s.contains("auth failed") || s.contains("schema mismatch") || s.contains("peer limit")
}

fn main() {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let cli = parse_cli();
    let loopback = is_loopback_bind(&cli.bind);
    if let Err(e) = ServeConfig::require_token_for_bind(loopback, &cli.token, cli.insecure) {
        eprintln!("wk-voxel-host: {e}");
        std::process::exit(2);
    }
    if cli.lan && cli.insecure && cli.token.is_none() {
        log::warn!("--lan --insecure: binding all interfaces with NO AUTH — trusted-dev only");
    } else if cli.lan {
        log::info!("--lan: binding all interfaces; token auth enabled");
    }

    let listener = TcpListener::bind(&cli.bind).unwrap_or_else(|e| {
        eprintln!("bind {}: {e}", cli.bind);
        std::process::exit(1);
    });
    let local = listener.local_addr().expect("local_addr");
    println!("wk-voxel-host listening on {local}");
    let _ = std::io::stdout().flush();
    log::info!(
        "schemas VIEW_SCHEMA={VIEW_SCHEMA} COMMAND_SCHEMA={COMMAND_SCHEMA} once={} lan={} max_peers={} auth={}",
        cli.once,
        cli.lan,
        cli.max_peers,
        cli.token.is_some() || cli.ops_token.is_some()
    );

    let host = if cli.dev {
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

    let host = Arc::new(Mutex::new(host));
    let registry = Arc::new(Mutex::new(PeerRegistry::new(cli.max_peers)));
    let cfg = Arc::new(ServeConfig {
        token: cli.token.clone(),
        ops_token: cli.ops_token.clone(),
        max_peers: cli.max_peers,
        loopback,
    });

    if cli.once {
        // One successful session then exit; auth rejects may retry.
        loop {
            let (stream, peer) = listener.accept().unwrap_or_else(|e| {
                eprintln!("accept: {e}");
                std::process::exit(1);
            });
            log::info!("client connected from {peer}");
            let h = host.clone();
            let r = registry.clone();
            let c = cfg.clone();
            let (tx, rx) = mpsc::channel();
            thread::spawn(move || {
                let _ = tx.send(serve_peer_shared(stream, h, r, c));
            });
            match rx.recv() {
                Ok(Ok(())) => {
                    log::info!("--once: session ended — exiting");
                    break;
                }
                Ok(Err(e)) if is_reject(&e) => {
                    log::warn!("peer rejected: {e} — waiting for next");
                }
                Ok(Err(e)) => {
                    log::warn!("peer session error: {e} — exiting (--once)");
                    break;
                }
                Err(_) => {
                    log::warn!("peer thread dropped — exiting (--once)");
                    break;
                }
            }
        }
        return;
    }

    // Concurrent accept loop (runs until process kill).
    loop {
        let (stream, peer) = listener.accept().unwrap_or_else(|e| {
            eprintln!("accept: {e}");
            std::process::exit(1);
        });
        log::info!("client connected from {peer}");
        let h = host.clone();
        let r = registry.clone();
        let c = cfg.clone();
        thread::spawn(move || match serve_peer_shared(stream, h, r, c) {
            Ok(()) => log::info!("peer session ended (quit)"),
            Err(e) => log::warn!("peer session error: {e}"),
        });
    }
}
