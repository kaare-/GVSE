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
use std::time::Duration;

use wk_voxel::SeamSide;
use wk_voxel_client::ipc::{
    serve_peer_shared, PeerRegistry, ServeConfig, WorldTier, DEFAULT_HOST_ADDR,
    DEFAULT_MAX_PEERS, LAN_HOST_ADDR, MAX_PEERS_CAP,
};
use wk_voxel_client::seam_ipc::{seam_loop_dial, seam_loop_listen};
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
    /// Explicit `--tier`; else Lab on loopback, Shadow on LAN / public bind.
    tier: Option<WorldTier>,
    /// Dig F1 Lab seam: open west|east face.
    seam_side: Option<SeamSide>,
    /// Dig F1: listen for seam dials on HOST:PORT.
    seam_listen: Option<String>,
    /// Dig F1: dial peer seam each product step.
    seam_peer: Option<String>,
    seam_token: Option<String>,
}

fn parse_seam_side(s: &str) -> Result<SeamSide, String> {
    match s.to_ascii_lowercase().as_str() {
        "west" | "w" => Ok(SeamSide::West),
        "east" | "e" => Ok(SeamSide::East),
        other => Err(format!("unknown --seam-side '{other}' (west|east)")),
    }
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
    let mut tier = None;
    let mut seam_side = None;
    let mut seam_listen = None;
    let mut seam_peer = None;
    let mut seam_token = env_token("WK_VOXEL_SEAM_TOKEN");
    let mut args = env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--dev" => dev = true,
            "--once" => once = true,
            "--lan" => lan = true,
            "--insecure" => insecure = true,
            "--tier" => {
                let raw = args.next().unwrap_or_else(|| {
                    eprintln!("--tier requires lab|shadow|wild");
                    std::process::exit(2);
                });
                tier = Some(WorldTier::parse_cli(&raw).unwrap_or_else(|e| {
                    eprintln!("wk-voxel-host: {e}");
                    std::process::exit(2);
                }));
            }
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
            "--seam-side" => {
                let raw = args.next().unwrap_or_else(|| {
                    eprintln!("--seam-side requires west|east");
                    std::process::exit(2);
                });
                seam_side = Some(parse_seam_side(&raw).unwrap_or_else(|e| {
                    eprintln!("wk-voxel-host: {e}");
                    std::process::exit(2);
                }));
            }
            "--seam-listen" => {
                seam_listen = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--seam-listen requires HOST:PORT");
                    std::process::exit(2);
                }));
            }
            "--seam-peer" => {
                seam_peer = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--seam-peer requires HOST:PORT");
                    std::process::exit(2);
                }));
            }
            "--seam-token" => {
                seam_token = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--seam-token requires SECRET");
                    std::process::exit(2);
                }));
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
                       --tier lab|shadow|wild  world tier (default: lab on loopback, shadow on --lan)\n\
                       --insecure         allow non-loopback with no token (dev only)\n\
                       --seam-side west|east  Dig F1 open Lab seam face\n\
                       --seam-listen HOST:PORT  accept seam dials\n\
                       --seam-peer HOST:PORT    dial peer seam (~20 Hz)\n\
                       --seam-token SECRET      seam auth (env WK_VOXEL_SEAM_TOKEN)\n\
                     \n\
                     Schemas: VIEW_SCHEMA={VIEW_SCHEMA} COMMAND_SCHEMA={COMMAND_SCHEMA}\n\
                     See docs/PHASE8_HOST_SHIP.md, docs/HALO_SHARD.md"
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
    if (seam_listen.is_some() || seam_peer.is_some()) && seam_side.is_none() {
        eprintln!("wk-voxel-host: --seam-listen/--seam-peer require --seam-side");
        std::process::exit(2);
    }
    if seam_listen.is_some() && seam_peer.is_some() {
        eprintln!("wk-voxel-host: use only one of --seam-listen / --seam-peer");
        std::process::exit(2);
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
        tier,
        seam_side,
        seam_listen,
        seam_peer,
        seam_token,
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
    if let Some(side) = cli.seam_side {
        host.enable_lab_seam(side);
        log::info!("Dig F1 Lab seam open side={side:?}");
    }
    log::info!(
        "host ready compact={} seed={} focus={} view={}",
        host.is_compact(),
        host.world_seed(),
        host.focus_col,
        host.view_cols
    );

    let world_tier = cli.tier.unwrap_or(if loopback {
        WorldTier::Lab
    } else {
        WorldTier::Shadow
    });
    log::info!("world_tier={}", world_tier.as_str());

    let host = Arc::new(Mutex::new(host));
    if let Some(addr) = cli.seam_listen.clone() {
        let side = cli.seam_side.expect("seam-side");
        let h = host.clone();
        let tok = cli.seam_token.clone();
        thread::spawn(move || {
            let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
                eprintln!("seam-listen {addr}: {e}");
                std::process::exit(1);
            });
            log::info!("seam listening on {addr} side={side:?}");
            seam_loop_listen(listener, h, tok, side);
        });
    } else if let Some(peer) = cli.seam_peer.clone() {
        let side = cli.seam_side.expect("seam-side");
        let h = host.clone();
        let tok = cli.seam_token.clone();
        thread::spawn(move || {
            log::info!("seam dialing {peer} side={side:?}");
            seam_loop_dial(peer, h, tok, side, Duration::from_millis(50));
        });
    }
    let registry = Arc::new(Mutex::new(PeerRegistry::new(cli.max_peers)));
    let cfg = Arc::new(ServeConfig {
        token: cli.token.clone(),
        ops_token: cli.ops_token.clone(),
        max_peers: cli.max_peers,
        loopback,
        world_tier,
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
