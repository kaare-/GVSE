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
use std::fs;
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
use wk_voxel_client::seam_discover::{
    seam_discover_loop, SeamAllowlist, SeamAnnounce, DEFAULT_SEAM_DISCOVER_PORT,
    SEAM_DISCOVER_PERIOD,
};
use wk_voxel_client::seam_graph::RegionGraph;
use wk_voxel_client::seam_ipc::{seam_loop_dial, seam_loop_listen, SeamIdentity};
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
    /// Dig F4 — this host's region id (allowlist key).
    seam_region_id: Option<String>,
    /// Dig F4 — shared mesh name for discovery / Hello match.
    seam_mesh_id: Option<String>,
    /// Dig F4 — allowed peer region ids.
    seam_allow: SeamAllowlist,
    /// Dig F4 — UDP discover port; None = off.
    seam_discover: Option<u16>,
    /// Dig F5 — official region graph file path.
    seam_graph: Option<String>,
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
    let mut seam_region_id = env_token("WK_VOXEL_SEAM_REGION");
    let mut seam_mesh_id = env_token("WK_VOXEL_SEAM_MESH");
    let mut seam_allow = SeamAllowlist::new();
    let mut seam_discover: Option<u16> = None;
    let mut seam_graph = None;
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
            "--seam-region-id" => {
                seam_region_id = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--seam-region-id requires NAME");
                    std::process::exit(2);
                }));
            }
            "--seam-mesh-id" => {
                seam_mesh_id = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--seam-mesh-id requires NAME");
                    std::process::exit(2);
                }));
            }
            "--seam-allow" => {
                let id = args.next().unwrap_or_else(|| {
                    eprintln!("--seam-allow requires REGION_ID");
                    std::process::exit(2);
                });
                seam_allow.allow(id);
            }
            "--seam-allow-file" => {
                let path = args.next().unwrap_or_else(|| {
                    eprintln!("--seam-allow-file requires PATH");
                    std::process::exit(2);
                });
                let text = fs::read_to_string(&path).unwrap_or_else(|e| {
                    eprintln!("--seam-allow-file {path}: {e}");
                    std::process::exit(2);
                });
                seam_allow.extend_from(&SeamAllowlist::from_lines(&text));
            }
            "--seam-discover" => {
                seam_discover = Some(DEFAULT_SEAM_DISCOVER_PORT);
            }
            "--seam-discover-port" => {
                let raw = args.next().unwrap_or_else(|| {
                    eprintln!("--seam-discover-port requires PORT");
                    std::process::exit(2);
                });
                let port: u16 = raw.parse().unwrap_or_else(|_| {
                    eprintln!("--seam-discover-port: invalid port '{raw}'");
                    std::process::exit(2);
                });
                seam_discover = Some(port);
            }
            "--seam-graph" => {
                seam_graph = Some(args.next().unwrap_or_else(|| {
                    eprintln!("--seam-graph requires PATH");
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
                       --seam-region-id NAME    Dig F4 region id (env WK_VOXEL_SEAM_REGION)\n\
                       --seam-mesh-id NAME      Dig F4 mesh name (env WK_VOXEL_SEAM_MESH)\n\
                       --seam-allow REGION_ID   Dig F4 allowlisted peer (repeatable)\n\
                       --seam-allow-file PATH   Dig F4 allowlist file (one id / line)\n\
                       --seam-discover          Dig F4 LAN UDP announce (port {DEFAULT_SEAM_DISCOVER_PORT})\n\
                       --seam-discover-port N   Dig F4 UDP port (implies discover)\n\
                       --seam-graph PATH        Dig F5 official region graph (Wild)\n\
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
    if seam_graph.is_some() {
        if seam_side.is_some() || seam_listen.is_some() || seam_peer.is_some() {
            eprintln!(
                "wk-voxel-host: --seam-graph sets faces/listen/dial; omit --seam-side/--seam-listen/--seam-peer"
            );
            std::process::exit(2);
        }
        if seam_discover.is_some() {
            eprintln!("wk-voxel-host: --seam-graph cannot combine with --seam-discover");
            std::process::exit(2);
        }
        if seam_region_id.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
            eprintln!("wk-voxel-host: --seam-graph requires --seam-region-id");
            std::process::exit(2);
        }
    } else {
        let seam_any = seam_listen.is_some() || seam_peer.is_some() || seam_discover.is_some();
        if seam_any && seam_side.is_none() {
            eprintln!("wk-voxel-host: seam flags require --seam-side");
            std::process::exit(2);
        }
        if seam_listen.is_some() && seam_peer.is_some() {
            eprintln!("wk-voxel-host: use only one of --seam-listen / --seam-peer");
            std::process::exit(2);
        }
        if seam_discover.is_some() {
            if seam_mesh_id.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                eprintln!("wk-voxel-host: --seam-discover requires --seam-mesh-id");
                std::process::exit(2);
            }
            if seam_region_id.as_ref().map(|s| s.is_empty()).unwrap_or(true) {
                eprintln!("wk-voxel-host: --seam-discover requires --seam-region-id");
                std::process::exit(2);
            }
            if seam_listen.is_none() && seam_peer.is_none() {
                // West must listen; East may discover-dial without a fixed peer.
                if matches!(seam_side, Some(SeamSide::West)) {
                    eprintln!("wk-voxel-host: west --seam-discover requires --seam-listen");
                    std::process::exit(2);
                }
            }
        }
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
        seam_region_id,
        seam_mesh_id,
        seam_allow,
        seam_discover,
        seam_graph,
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

    let world_tier = cli.tier.unwrap_or(if loopback {
        WorldTier::Lab
    } else {
        WorldTier::Shadow
    });
    log::info!("world_tier={}", world_tier.as_str());

    if matches!(world_tier, WorldTier::Wild) && cli.seam_discover.is_some() {
        eprintln!("wk-voxel-host: Wild forbids --seam-discover (use --seam-graph)");
        std::process::exit(2);
    }
    if matches!(world_tier, WorldTier::Shadow)
        && cli.seam_discover.is_some()
        && cli.seam_allow.is_empty()
        && cli.seam_graph.is_none()
    {
        eprintln!(
            "wk-voxel-host: Shadow --seam-discover requires --seam-allow / --seam-allow-file"
        );
        std::process::exit(2);
    }

    // Dig F5: resolve official graph into listen/dials/faces/allowlist.
    let mut seam_listen = cli.seam_listen.clone();
    let mut seam_peer = cli.seam_peer.clone();
    let mut seam_side = cli.seam_side;
    let mut seam_dials: Vec<(SeamSide, String)> = Vec::new();
    let mut open_west = matches!(seam_side, Some(SeamSide::West));
    let mut open_east = matches!(seam_side, Some(SeamSide::East));
    let mut identity = SeamIdentity {
        region_id: cli.seam_region_id.clone().unwrap_or_default(),
        mesh_id: cli.seam_mesh_id.clone().unwrap_or_default(),
    };
    let mut seam_allow = cli.seam_allow.clone();

    if let Some(path) = cli.seam_graph.as_ref() {
        let text = fs::read_to_string(path).unwrap_or_else(|e| {
            eprintln!("--seam-graph {path}: {e}");
            std::process::exit(2);
        });
        let graph = RegionGraph::parse(&text).unwrap_or_else(|e| {
            eprintln!("--seam-graph {path}: {e}");
            std::process::exit(2);
        });
        let plan = graph.plan(&identity.region_id).unwrap_or_else(|e| {
            eprintln!("--seam-graph: {e}");
            std::process::exit(2);
        });
        if !cli.seam_mesh_id.as_ref().map(|s| s.is_empty()).unwrap_or(true)
            && cli.seam_mesh_id.as_deref() != Some(plan.mesh_id.as_str())
        {
            eprintln!(
                "wk-voxel-host: --seam-mesh-id {:?} disagrees with graph mesh {:?}",
                cli.seam_mesh_id, plan.mesh_id
            );
            std::process::exit(2);
        }
        identity.mesh_id = plan.mesh_id;
        open_west = plan.open_west;
        open_east = plan.open_east;
        seam_listen = plan.listen;
        seam_peer = None;
        seam_dials = plan.dials;
        seam_allow.extend_from(&plan.allowlist);
        // Single-face Lab flags for discover path compatibility.
        seam_side = match (open_west, open_east) {
            (true, false) => Some(SeamSide::West),
            (false, true) => Some(SeamSide::East),
            (true, true) => None,
            (false, false) => None,
        };
        log::info!(
            "Dig F5 seam graph region={} mesh={} west={} east={} dials={}",
            identity.region_id,
            identity.mesh_id,
            open_west,
            open_east,
            seam_dials.len()
        );
    } else if let Some(peer) = seam_peer.clone() {
        seam_dials.push((seam_side.expect("seam-side"), peer));
    }

    if matches!(world_tier, WorldTier::Wild)
        && (open_west || open_east)
        && cli.seam_graph.is_none()
    {
        log::warn!("Wild seam without --seam-graph — prefer official adjacency file");
    }

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
    if open_west || open_east {
        if open_west && open_east {
            host.enable_seam_faces(true, true);
            log::info!("Dig F5 seam open both faces");
        } else if let Some(side) = seam_side {
            host.enable_lab_seam(side);
            log::info!("Dig F1 Lab seam open side={side:?}");
        } else {
            host.enable_seam_faces(open_west, open_east);
            log::info!("seam open west={open_west} east={open_east}");
        }
    }
    log::info!(
        "host ready compact={} seed={} focus={} view={}",
        host.is_compact(),
        host.world_seed(),
        host.focus_col,
        host.view_cols
    );

    if !identity.region_id.is_empty() || !identity.mesh_id.is_empty() {
        log::info!(
            "seam identity region={} mesh={}",
            identity.region_id,
            identity.mesh_id
        );
    }
    if !seam_allow.is_empty() {
        log::info!("seam allowlist {} peer(s)", seam_allow.len());
    }

    let host = Arc::new(Mutex::new(host));
    // Bind seam listen before discover announce so `:0` resolves to a real port.
    let mut seam_advertise: Option<String> = None;
    if let Some(addr) = seam_listen.clone() {
        // Graph (F5): geographic east listens on West face. Lab CLI: use --seam-side.
        let side = if cli.seam_graph.is_some() {
            SeamSide::West
        } else {
            cli.seam_side.expect("seam-side")
        };
        let listener = TcpListener::bind(&addr).unwrap_or_else(|e| {
            eprintln!("seam-listen {addr}: {e}");
            std::process::exit(1);
        });
        let local = listener.local_addr().expect("seam local");
        seam_advertise = Some(local.to_string());
        log::info!("seam listening on {local} side={side:?}");
        let h = host.clone();
        let tok = cli.seam_token.clone();
        let id = identity.clone();
        let allow = seam_allow.clone();
        thread::spawn(move || {
            seam_loop_listen(listener, h, tok, side, id, allow);
        });
    }

    for (side, peer) in seam_dials {
        let h = host.clone();
        let tok = cli.seam_token.clone();
        let id = identity.clone();
        let allow = seam_allow.clone();
        thread::spawn(move || {
            log::info!("seam dialing {peer} side={side:?}");
            seam_loop_dial(
                peer,
                h,
                tok,
                side,
                id,
                allow,
                Duration::from_millis(50),
            );
        });
    }

    if let Some(discover_port) = cli.seam_discover {
        let side = cli.seam_side.expect("seam-side");
        let seam_addr = seam_advertise.unwrap_or_default();
        let announce = SeamAnnounce::new(
            identity.mesh_id.clone(),
            identity.region_id.clone(),
            side,
            seam_addr,
        );
        let h = host.clone();
        let tok = cli.seam_token.clone();
        let id = identity.clone();
        let allow = seam_allow.clone();
        thread::spawn(move || {
            seam_discover_loop(
                announce,
                allow,
                discover_port,
                h,
                id,
                tok,
                SEAM_DISCOVER_PERIOD,
            );
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
