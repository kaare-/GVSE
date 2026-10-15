//! Dig F1 Lab seam postcard channel (separate from viewer IPC).
//!
//! Framing matches [`crate::ipc`]: `u32` LE length + postcard payload.
//! See [`docs/HALO_SHARD.md`].

use std::net::{TcpListener, TcpStream, ToSocketAddrs};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use wk_voxel::{SeamFieldMirror, SeamSide, SeamTransfer};

use crate::ipc::{read_msg, write_msg, IpcError};
use crate::Host;

/// Schema for [`SeamMsg`] — bump when wire layout breaks.
/// Dig F2: Exchange carries [`SeamFieldMirror`] (was 1 = P0 mass only).
pub const SEAM_SCHEMA: u32 = 2;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum SeamMsg {
    Hello {
        schema: u32,
        token: Option<String>,
        /// Side **this** peer opens toward the neighbor.
        side: SeamSide,
        tick: u64,
    },
    HelloAck {
        schema: u32,
        ok: bool,
        detail: String,
        side: SeamSide,
        tick: u64,
    },
    /// P0 mass + Dig F2 edge field mirror.
    Exchange {
        tick: u64,
        transfer: SeamTransfer,
        fields: SeamFieldMirror,
    },
    ExchangeAck {
        tick: u64,
        /// Peer's outbound for us (may be empty).
        transfer: SeamTransfer,
        fields: SeamFieldMirror,
    },
    Bye,
}

/// Run one seam Hello + Exchange round on an established stream.
pub fn seam_round_trip(
    stream: &mut TcpStream,
    host: &mut Host,
    token: &Option<String>,
    our_side: SeamSide,
) -> Result<(), IpcError> {
    let tick = host.world_tick();
    write_msg(
        stream,
        &SeamMsg::Hello {
            schema: SEAM_SCHEMA,
            token: token.clone(),
            side: our_side,
            tick,
        },
    )?;
    match read_msg::<SeamMsg, _>(stream)? {
        SeamMsg::HelloAck { ok: true, .. } => {}
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
    write_msg(
        stream,
        &SeamMsg::Exchange {
            tick,
            transfer: outbound,
            fields,
        },
    )?;
    match read_msg::<SeamMsg, _>(stream)? {
        SeamMsg::ExchangeAck {
            transfer, fields, ..
        } => {
            host.apply_seam_inbound(our_side, transfer);
            host.apply_seam_fields(our_side, fields);
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
) -> Result<(), IpcError> {
    let hello = read_msg::<SeamMsg, _>(stream)?;
    let (peer_side, tick) = match hello {
        SeamMsg::Hello {
            schema,
            token,
            side,
            tick,
        } => {
            let ok = schema == SEAM_SCHEMA
                && match (expect_token, token) {
                    (None, _) => true,
                    (Some(e), Some(t)) => e == &t,
                    (Some(_), None) => false,
                };
            let detail = if ok {
                "ok".into()
            } else if schema != SEAM_SCHEMA {
                format!("schema mismatch {schema}!={SEAM_SCHEMA}")
            } else {
                "auth failed".into()
            };
            write_msg(
                stream,
                &SeamMsg::HelloAck {
                    schema: SEAM_SCHEMA,
                    ok,
                    detail: detail.clone(),
                    side: our_side,
                    tick: host.world_tick(),
                },
            )?;
            if !ok {
                return Err(IpcError::Protocol(detail));
            }
            // Peer side should be the opposite face.
            if side == our_side {
                return Err(IpcError::Protocol(format!(
                    "seam side clash: both {side:?}"
                )));
            }
            (side, tick)
        }
        other => {
            return Err(IpcError::Protocol(format!(
                "expected seam Hello, got {other:?}"
            )));
        }
    };
    let _ = (peer_side, tick);
    match read_msg::<SeamMsg, _>(stream)? {
        SeamMsg::Exchange {
            transfer,
            fields,
            tick,
        } => {
            // Drain first so face export cannot re-absorb water we are about
            // to paint from the peer (same-round ping-pong).
            let outbound = host.drain_seam_outbound(our_side);
            let our_fields = host.sample_seam_fields(our_side);
            host.apply_seam_inbound(our_side, transfer);
            host.apply_seam_fields(our_side, fields);
            write_msg(
                stream,
                &SeamMsg::ExchangeAck {
                    tick,
                    transfer: outbound,
                    fields: our_fields,
                },
            )?;
            Ok(())
        }
        other => Err(IpcError::Protocol(format!(
            "expected seam Exchange, got {other:?}"
        ))),
    }
}

/// Dial peer and run one exchange (Lab client side).
pub fn connect_and_exchange(
    addr: impl ToSocketAddrs,
    host: &mut Host,
    token: &Option<String>,
    our_side: SeamSide,
) -> Result<(), IpcError> {
    let mut stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    stream.set_write_timeout(Some(Duration::from_secs(5)))?;
    seam_round_trip(&mut stream, host, token, our_side)
}

/// Listen once, exchange, return (Lab accept side).
pub fn accept_and_exchange(
    listener: &TcpListener,
    host: &mut Host,
    token: &Option<String>,
    our_side: SeamSide,
) -> Result<(), IpcError> {
    listener.set_nonblocking(false)?;
    let (mut stream, _) = listener.accept()?;
    stream.set_nodelay(true)?;
    serve_seam_round(&mut stream, host, token, our_side)
}

/// Background Lab seam loop against `Arc<Mutex<Host>>`.
pub fn seam_loop_dial(
    peer: String,
    host: Arc<Mutex<Host>>,
    token: Option<String>,
    our_side: SeamSide,
    period: Duration,
) {
    loop {
        std::thread::sleep(period);
        let Ok(mut h) = host.lock() else {
            break;
        };
        match connect_and_exchange(&peer, &mut h, &token, our_side) {
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
        match serve_seam_round(&mut stream, &mut h, &token, our_side) {
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

        let east = std::thread::spawn(move || {
            accept_and_exchange(&listener, &mut east_host, &None, SeamSide::West)
                .expect("accept");
            east_host
        });

        std::thread::sleep(Duration::from_millis(20));
        connect_and_exchange(addr, &mut west_host, &None, SeamSide::East).expect("dial");
        let east_host = east.join().unwrap();

        let pair1 = sat_totals(west_host.world()).cell_total
            + sat_totals(east_host.world()).cell_total;
        assert_eq!(pair1, pair0);
        let got: i64 = (0..4)
            .filter_map(|y| east_host.world().get_cell(0, y).map(|c| c.sat.0 as i64))
            .sum();
        assert!(got > 0, "east edge should receive water, got {got}");
    }
}
