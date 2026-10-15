//! Dig F1 Lab seam postcard channel (separate from viewer IPC).
//!
//! Framing matches [`crate::ipc`]: `u32` LE length + postcard payload.
//! Dig F4 adds `region_id` / `mesh_id` on Hello + neighbor allowlist.
//! See [`docs/HALO_SHARD.md`].

use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use wk_voxel::{SeamFieldMirror, SeamLifeParcel, SeamSide, SeamTransfer};

use crate::ipc::{read_msg, write_msg, IpcError};
use crate::seam_discover::{complementary_side, SeamAllowlist};
use crate::Host;

/// Schema for [`SeamMsg`] — bump when wire layout breaks.
/// Dig F4: Hello carries region_id + mesh_id (was 3 = mass + fields + life).
pub const SEAM_SCHEMA: u32 = 4;

/// Local identity presented on Hello (Dig F4). Empty strings = Lab legacy.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeamIdentity {
    pub region_id: String,
    pub mesh_id: String,
}

impl SeamIdentity {
    pub fn lab() -> Self {
        Self::default()
    }

    pub fn shadow(region_id: impl Into<String>, mesh_id: impl Into<String>) -> Self {
        Self {
            region_id: region_id.into(),
            mesh_id: mesh_id.into(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum SeamMsg {
    Hello {
        schema: u32,
        token: Option<String>,
        /// Side **this** peer opens toward the neighbor.
        side: SeamSide,
        tick: u64,
        /// Dig F4 — peer region id (allowlist key). Empty on Lab-only pairs.
        region_id: String,
        /// Dig F4 — shared mesh name. Empty = do not require mesh match.
        mesh_id: String,
    },
    HelloAck {
        schema: u32,
        ok: bool,
        detail: String,
        side: SeamSide,
        tick: u64,
        region_id: String,
        mesh_id: String,
    },
    /// P0 mass + Dig F2 fields + Dig F3 life.
    Exchange {
        tick: u64,
        transfer: SeamTransfer,
        fields: SeamFieldMirror,
        life: Vec<SeamLifeParcel>,
        /// Ids from peer life we successfully applied (clear their in-flight).
        ack_life_ids: Vec<u64>,
    },
    ExchangeAck {
        tick: u64,
        /// Peer's outbound for us (may be empty).
        transfer: SeamTransfer,
        fields: SeamFieldMirror,
        life: Vec<SeamLifeParcel>,
        ack_life_ids: Vec<u64>,
    },
    Bye,
}

fn token_ok(expect: &Option<String>, got: Option<String>) -> bool {
    match (expect, got) {
        (None, _) => true,
        (Some(e), Some(t)) => e == &t,
        (Some(_), None) => false,
    }
}

/// Hello auth: schema + token + complementary side + mesh/allowlist (Dig F4).
pub fn hello_authorized(
    schema: u32,
    token: Option<String>,
    peer_side: SeamSide,
    peer_region: &str,
    peer_mesh: &str,
    expect_token: &Option<String>,
    our_side: SeamSide,
    our_mesh: &str,
    allow: &SeamAllowlist,
) -> Result<(), String> {
    if schema != SEAM_SCHEMA {
        return Err(format!("schema mismatch {schema}!={SEAM_SCHEMA}"));
    }
    if !token_ok(expect_token, token) {
        return Err("auth failed".into());
    }
    if peer_side == our_side {
        return Err(format!("seam side clash: both {peer_side:?}"));
    }
    if peer_side != complementary_side(our_side) {
        return Err(format!(
            "seam side mismatch: peer {peer_side:?} vs our {our_side:?}"
        ));
    }
    if !our_mesh.is_empty() && peer_mesh != our_mesh {
        return Err(format!("mesh mismatch {peer_mesh:?}!={our_mesh:?}"));
    }
    if !allow.allows(peer_region) {
        return Err(format!("region {peer_region:?} not on seam allowlist"));
    }
    Ok(())
}

/// Run one seam Hello + Exchange round on an established stream.
pub fn seam_round_trip(
    stream: &mut TcpStream,
    host: &mut Host,
    token: &Option<String>,
    our_side: SeamSide,
    identity: &SeamIdentity,
    allow: &SeamAllowlist,
) -> Result<(), IpcError> {
    let tick = host.world_tick();
    write_msg(
        stream,
        &SeamMsg::Hello {
            schema: SEAM_SCHEMA,
            token: token.clone(),
            side: our_side,
            tick,
            region_id: identity.region_id.clone(),
            mesh_id: identity.mesh_id.clone(),
        },
    )?;
    match read_msg::<SeamMsg, _>(stream)? {
        SeamMsg::HelloAck {
            ok: true,
            region_id,
            mesh_id,
            ..
        } => {
            if !allow.allows(&region_id) {
                return Err(IpcError::Protocol(format!(
                    "peer region {region_id:?} not on seam allowlist"
                )));
            }
            if !identity.mesh_id.is_empty() && mesh_id != identity.mesh_id {
                return Err(IpcError::Protocol(format!(
                    "peer mesh mismatch {mesh_id:?}!={:?}",
                    identity.mesh_id
                )));
            }
        }
        SeamMsg::HelloAck { detail, .. } => {
            return Err(IpcError::Protocol(format!("seam HelloAck: {detail}")));
        }
        other => {
            return Err(IpcError::Protocol(format!(
                "expected seam HelloAck, got {other:?}"
            )));
        }
    }
    let outbound = host.drain_seam_outbound(our_side);
    let fields = host.sample_seam_fields(our_side);
    let life = host.drain_seam_life(our_side);
    let ack_life_ids = host.take_seam_life_acks(our_side);
    write_msg(
        stream,
        &SeamMsg::Exchange {
            tick,
            transfer: outbound,
            fields,
            life,
            ack_life_ids,
        },
    )?;
    match read_msg::<SeamMsg, _>(stream)? {
        SeamMsg::ExchangeAck {
            transfer,
            fields,
            life,
            ack_life_ids,
            ..
        } => {
            host.apply_seam_inbound(our_side, transfer);
            host.apply_seam_fields(our_side, fields);
            host.ack_seam_life(our_side, &ack_life_ids);
            let acked = host.apply_seam_life(our_side, life);
            host.stash_seam_life_acks(our_side, acked);
            Ok(())
        }
        other => Err(IpcError::Protocol(format!(
            "expected seam ExchangeAck, got {other:?}"
        ))),
    }
}

/// Accept side: answer Hello, apply peer transfer, return our outbound.
pub fn serve_seam_round(
    stream: &mut TcpStream,
    host: &mut Host,
    expect_token: &Option<String>,
    our_side: SeamSide,
    identity: &SeamIdentity,
    allow: &SeamAllowlist,
) -> Result<(), IpcError> {
    let hello = read_msg::<SeamMsg, _>(stream)?;
    let tick = match hello {
        SeamMsg::Hello {
            schema,
            token,
            side,
            tick,
            region_id,
            mesh_id,
        } => {
            let auth = hello_authorized(
                schema,
                token,
                side,
                &region_id,
                &mesh_id,
                expect_token,
                our_side,
                &identity.mesh_id,
                allow,
            );
            let (ok, detail) = match &auth {
                Ok(()) => (true, "ok".into()),
                Err(d) => (false, d.clone()),
            };
            write_msg(
                stream,
                &SeamMsg::HelloAck {
                    schema: SEAM_SCHEMA,
                    ok,
                    detail: detail.clone(),
                    side: our_side,
                    tick: host.world_tick(),
                    region_id: identity.region_id.clone(),
                    mesh_id: identity.mesh_id.clone(),
                },
            )?;
            if !ok {
                return Err(IpcError::Protocol(detail));
            }
            tick
        }
        other => {
            return Err(IpcError::Protocol(format!(
                "expected seam Hello, got {other:?}"
            )));
        }
    };
    let _ = tick;
    match read_msg::<SeamMsg, _>(stream)? {
        SeamMsg::Exchange {
            transfer,
            fields,
            life,
            ack_life_ids,
            tick,
        } => {
            // Drain first so face export cannot re-absorb water we are about
            // to paint from the peer (same-round ping-pong).
            let outbound = host.drain_seam_outbound(our_side);
            let our_fields = host.sample_seam_fields(our_side);
            let our_life = host.drain_seam_life(our_side);
            host.ack_seam_life(our_side, &ack_life_ids);
            host.apply_seam_inbound(our_side, transfer);
            host.apply_seam_fields(our_side, fields);
            let applied = host.apply_seam_life(our_side, life);
            write_msg(
                stream,
                &SeamMsg::ExchangeAck {
                    tick,
                    transfer: outbound,
                    fields: our_fields,
                    life: our_life,
                    ack_life_ids: applied,
                },
            )?;
            Ok(())
        }
        other => Err(IpcError::Protocol(format!(
            "expected seam Exchange, got {other:?}"
        ))),
    }
}

/// Dial peer and run one exchange (Lab / Shadow client side).
pub fn connect_and_exchange(
    addr: impl ToSocketAddrs,
    host: &mut Host,
    token: &Option<String>,
    our_side: SeamSide,
    identity: &SeamIdentity,
    allow: &SeamAllowlist,
) -> Result<(), IpcError> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    seam_round_trip(&mut stream, host, token, our_side, identity, allow)
}

/// Listen once, exchange, return (Lab accept side).
pub fn accept_and_exchange(
    listener: &TcpListener,
    host: &mut Host,
    token: &Option<String>,
    our_side: SeamSide,
    identity: &SeamIdentity,
    allow: &SeamAllowlist,
) -> Result<(), IpcError> {
    listener.set_nonblocking(false)?;
    let (mut stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    serve_seam_round(&mut stream, host, token, our_side, identity, allow)
}

/// Background Lab/Shadow seam loop against `Arc<Mutex<Host>>`.
pub fn seam_loop_dial(
    peer: String,
    host: Arc<Mutex<Host>>,
    token: Option<String>,
    our_side: SeamSide,
    identity: SeamIdentity,
    allow: SeamAllowlist,
    period: Duration,
) {
    loop {
        std::thread::sleep(period);
        let Ok(mut h) = host.lock() else {
            break;
        };
        match connect_and_exchange(&peer, &mut h, &token, our_side, &identity, &allow) {
            Ok(()) => {}
            Err(e) => log::debug!("seam dial {peer}: {e}"),
        }
    }
}

/// Accept loop: one exchange per accepted connection (peer dials each tick).
pub fn seam_loop_listen(
    listener: TcpListener,
    host: Arc<Mutex<Host>>,
    token: Option<String>,
    our_side: SeamSide,
    identity: SeamIdentity,
    allow: SeamAllowlist,
) {
    let _ = listener.set_nonblocking(false);
    loop {
        let Ok((mut stream, _)) = listener.accept() else {
            continue;
        };
        let _ = stream.set_nodelay(true);
        let Ok(mut h) = host.lock() else {
            break;
        };
        match serve_seam_round(&mut stream, &mut h, &token, our_side, &identity, &allow) {
            Ok(()) => {}
            Err(e) => log::debug!("seam accept: {e}"),
        }
    }
}

/// Postcard round-trip unit helper (no TCP).
pub fn encode_decode_transfer(t: SeamTransfer) -> SeamTransfer {
    let msg = SeamMsg::Exchange {
        tick: 1,
        transfer: t,
        fields: SeamFieldMirror::default(),
        life: Vec::new(),
        ack_life_ids: Vec::new(),
    };
    let bytes = postcard::to_allocvec(&msg).expect("encode");
    match postcard::from_bytes::<SeamMsg>(&bytes).expect("decode") {
        SeamMsg::Exchange { transfer, .. } => transfer,
        _ => panic!("wrong variant"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wk_voxel::{attach_open_seam, sat_totals, Cell, SeamSide, CHUNK_CELLS_W};

    #[test]
    fn postcard_transfer_roundtrip() {
        let t = SeamTransfer {
            water: 12,
            sediment: 3,
            dissolved: 1,
        };
        assert_eq!(encode_decode_transfer(t), t);
    }

    #[test]
    fn hello_allowlist_rejects_stranger() {
        let mut allow = SeamAllowlist::new();
        allow.allow("west");
        let err = hello_authorized(
            SEAM_SCHEMA,
            None,
            SeamSide::West,
            "stranger",
            "mesh",
            &None,
            SeamSide::East,
            "mesh",
            &allow,
        )
        .unwrap_err();
        assert!(err.contains("allowlist"), "{err}");
    }

    #[test]
    fn hello_mesh_mismatch() {
        let allow = SeamAllowlist::new();
        let err = hello_authorized(
            SEAM_SCHEMA,
            None,
            SeamSide::West,
            "w",
            "other",
            &None,
            SeamSide::East,
            "mesh",
            &allow,
        )
        .unwrap_err();
        assert!(err.contains("mesh"), "{err}");
    }

    #[test]
    fn hello_lab_open_allowlist_ok() {
        let allow = SeamAllowlist::new();
        hello_authorized(
            SEAM_SCHEMA,
            Some("tok".into()),
            SeamSide::West,
            "",
            "",
            &Some("tok".into()),
            SeamSide::East,
            "",
            &allow,
        )
        .unwrap();
    }

    #[test]
    fn tcp_seam_exchange_moves_water() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let w = CHUNK_CELLS_W as i32;

        let mut east_host = Host::for_tests();
        // Rebuild as open west seam slab — for_tests wraps; re-stamp seam on world.
        {
            let world = east_host.world_mut();
            world.chunks.clear();
            for x in 0..w {
                world.set_cell(x, 0, Cell::solid(wk_material::MaterialId::Bedrock));
                world.set_cell(x, 1, Cell::air());
            }
            attach_open_seam(world, w, SeamSide::West);
        }

        let mut west_host = Host::for_tests();
        {
            let world = west_host.world_mut();
            world.chunks.clear();
            for x in 0..w {
                world.set_cell(x, 0, Cell::solid(wk_material::MaterialId::Bedrock));
                world.set_cell(x, 1, Cell::air());
            }
            attach_open_seam(world, w, SeamSide::East);
            world.set_cell(w - 1, 1, Cell::water());
        }

        let pair0 = sat_totals(west_host.world()).cell_total
            + sat_totals(east_host.world()).cell_total;

        let id = SeamIdentity::lab();
        let allow = SeamAllowlist::new();
        let east = std::thread::spawn(move || {
            accept_and_exchange(
                &listener,
                &mut east_host,
                &None,
                SeamSide::West,
                &SeamIdentity::lab(),
                &SeamAllowlist::new(),
            )
            .expect("accept");
            east_host
        });

        std::thread::sleep(Duration::from_millis(20));
        connect_and_exchange(addr, &mut west_host, &None, SeamSide::East, &id, &allow)
            .expect("dial");
        let east_host = east.join().unwrap();

        let pair1 = sat_totals(west_host.world()).cell_total
            + sat_totals(east_host.world()).cell_total;
        assert_eq!(pair1, pair0);
        let got: i64 = (0..4)
            .filter_map(|y| east_host.world().get_cell(0, y).map(|c| c.sat.0 as i64))
            .sum();
        assert!(got > 0, "east edge should receive water, got {got}");
    }

    #[test]
    fn tcp_seam_allowlist_blocks_stranger() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();

        let mut east_host = Host::for_tests();
        east_host.enable_lab_seam(SeamSide::West);
        let mut west_host = Host::for_tests();
        west_host.enable_lab_seam(SeamSide::East);

        let mut allow = SeamAllowlist::new();
        allow.allow("west-ok");
        let allow_srv = allow.clone();

        let east = std::thread::spawn(move || {
            let id = SeamIdentity::shadow("east", "mesh");
            let r = accept_and_exchange(
                &listener,
                &mut east_host,
                &None,
                SeamSide::West,
                &id,
                &allow_srv,
            );
            r
        });

        std::thread::sleep(Duration::from_millis(20));
        let id = SeamIdentity::shadow("stranger", "mesh");
        let err = connect_and_exchange(
            addr,
            &mut west_host,
            &None,
            SeamSide::East,
            &id,
            &SeamAllowlist::new(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("allowlist") || err.to_string().contains("HelloAck"),
            "{err}"
        );
        let _ = east.join();
    }
}
