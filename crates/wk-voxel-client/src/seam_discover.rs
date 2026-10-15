//! Dig F4 — Shadow seam discovery + neighbor allowlist.
//!
//! LAN UDP announce finds complementary faces on the same mesh; TCP seam
//! exchange stays on [`crate::seam_ipc`]. Allowlist gates which
//! `region_id`s may Hello. See [`docs/HALO_SHARD.md`].

use std::collections::HashSet;
use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use wk_voxel::SeamSide;

use crate::seam_ipc::{connect_and_exchange, SeamIdentity};
use crate::Host;

/// Wire schema for [`SeamAnnounce`] — bump when layout breaks.
pub const SEAM_DISCOVER_SCHEMA: u32 = 1;
/// Default UDP port for LAN seam announce (loopback / LAN only).
pub const DEFAULT_SEAM_DISCOVER_PORT: u16 = 17430;
/// How often to broadcast our announce.
pub const SEAM_DISCOVER_PERIOD: Duration = Duration::from_secs(1);

/// Allowed peer region ids. Empty = unrestricted (Lab default).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeamAllowlist {
    regions: HashSet<String>,
}

impl SeamAllowlist {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn is_open(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn len(&self) -> usize {
        self.regions.len()
    }

    pub fn is_empty(&self) -> bool {
        self.regions.is_empty()
    }

    pub fn allow(&mut self, region_id: impl Into<String>) {
        let id = region_id.into();
        if !id.is_empty() {
            self.regions.insert(id);
        }
    }

    pub fn allows(&self, region_id: &str) -> bool {
        self.is_open() || self.regions.contains(region_id)
    }

    pub fn regions(&self) -> impl Iterator<Item = &String> {
        self.regions.iter()
    }

    /// Merge another allowlist into this one.
    pub fn extend_from(&mut self, other: &SeamAllowlist) {
        for id in other.regions() {
            self.allow(id.clone());
        }
    }

    /// Parse `#`-commented lines; one region id per non-empty line.
    pub fn from_lines(text: &str) -> Self {
        let mut a = Self::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("").trim();
            if !line.is_empty() {
                a.allow(line);
            }
        }
        a
    }
}

/// UDP postcard blob: "I offer a seam on this face at seam_addr".
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SeamAnnounce {
    pub schema: u32,
    pub mesh_id: String,
    pub region_id: String,
    pub side: SeamSide,
    /// TCP `host:port` peers should dial for [`crate::seam_ipc`] exchange.
    pub seam_addr: String,
}

impl SeamAnnounce {
    pub fn new(
        mesh_id: impl Into<String>,
        region_id: impl Into<String>,
        side: SeamSide,
        seam_addr: impl Into<String>,
    ) -> Self {
        Self {
            schema: SEAM_DISCOVER_SCHEMA,
            mesh_id: mesh_id.into(),
            region_id: region_id.into(),
            side,
            seam_addr: seam_addr.into(),
        }
    }
}

pub fn encode_announce(a: &SeamAnnounce) -> Vec<u8> {
    postcard::to_allocvec(a).expect("seam announce encode")
}

pub fn decode_announce(bytes: &[u8]) -> Option<SeamAnnounce> {
    postcard::from_bytes(bytes).ok()
}

pub fn complementary_side(side: SeamSide) -> SeamSide {
    match side {
        SeamSide::West => SeamSide::East,
        SeamSide::East => SeamSide::West,
    }
}

/// East dials West (Lab convention). Mesh + complementary face + allowlist.
pub fn should_dial_announce(
    ours: &SeamAnnounce,
    peer: &SeamAnnounce,
    allow: &SeamAllowlist,
) -> bool {
    if peer.schema != SEAM_DISCOVER_SCHEMA {
        return false;
    }
    if ours.mesh_id.is_empty() || peer.mesh_id != ours.mesh_id {
        return false;
    }
    if peer.region_id.is_empty() || peer.region_id == ours.region_id {
        return false;
    }
    if !allow.allows(&peer.region_id) {
        return false;
    }
    if peer.side != complementary_side(ours.side) {
        return false;
    }
    matches!(ours.side, SeamSide::East)
}

/// Bind UDP for announce/recv. `bind` e.g. `0.0.0.0:17430` or `127.0.0.1:0`.
pub fn bind_discover_socket(bind: impl ToSocketAddrs) -> std::io::Result<UdpSocket> {
    let sock = UdpSocket::bind(bind)?;
    sock.set_broadcast(true)?;
    sock.set_read_timeout(Some(Duration::from_millis(200)))?;
    sock.set_write_timeout(Some(Duration::from_millis(200)))?;
    Ok(sock)
}

fn broadcast_targets(port: u16) -> Vec<SocketAddr> {
    vec![
        SocketAddr::from(([255, 255, 255, 255], port)),
        SocketAddr::from(([127, 0, 0, 1], port)),
    ]
}

/// Send one announce to LAN broadcast + loopback.
pub fn send_announce(sock: &UdpSocket, announce: &SeamAnnounce, port: u16) {
    let bytes = encode_announce(announce);
    for dest in broadcast_targets(port) {
        let _ = sock.send_to(&bytes, dest);
    }
}

/// Try one non-blocking-ish recv; `None` on timeout / garbage.
pub fn recv_announce(sock: &UdpSocket) -> Option<(SeamAnnounce, SocketAddr)> {
    let mut buf = [0u8; 512];
    match sock.recv_from(&mut buf) {
        Ok((n, from)) => decode_announce(&buf[..n]).map(|a| (a, from)),
        Err(_) => None,
    }
}

/// Background: announce ourselves; East side dials allowlisted complementary peers.
pub fn seam_discover_loop(
    mut announce: SeamAnnounce,
    allow: SeamAllowlist,
    discover_port: u16,
    host: Arc<Mutex<Host>>,
    identity: SeamIdentity,
    token: Option<String>,
    period: Duration,
) {
    let bind = SocketAddr::from(([0, 0, 0, 0], discover_port));
    let sock = match bind_discover_socket(bind) {
        Ok(s) => s,
        Err(e) => {
            log::error!("seam discover bind {bind}: {e}");
            return;
        }
    };
    log::info!(
        "seam discover on UDP {discover_port} mesh={} region={} side={:?} tcp={}",
        announce.mesh_id,
        announce.region_id,
        announce.side,
        announce.seam_addr
    );

    let mut last_send = Instant::now()
        .checked_sub(period)
        .unwrap_or_else(Instant::now);
    let mut last_dial: Option<(String, Instant)> = None;

    loop {
        if last_send.elapsed() >= period {
            send_announce(&sock, &announce, discover_port);
            last_send = Instant::now();
        }
        if let Some((peer, _from)) = recv_announce(&sock) {
            if !should_dial_announce(&announce, &peer, &allow) {
                continue;
            }
            // Coalesce dials to the same peer (~product step).
            let now = Instant::now();
            if let Some((ref addr, t)) = last_dial {
                if addr == &peer.seam_addr && now.duration_since(t) < Duration::from_millis(45) {
                    continue;
                }
            }
            last_dial = Some((peer.seam_addr.clone(), now));
            let Ok(mut h) = host.lock() else {
                break;
            };
            match connect_and_exchange(
                &peer.seam_addr,
                &mut h,
                &token,
                announce.side,
                &identity,
                &allow,
            ) {
                Ok(()) => log::debug!("seam discover dial {} ok", peer.seam_addr),
                Err(e) => log::debug!("seam discover dial {}: {e}", peer.seam_addr),
            }
            // Keep announce.seam_addr as our listen (West); East may leave it empty.
            let _ = &mut announce;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowlist_open_allows_anyone() {
        let a = SeamAllowlist::new();
        assert!(a.is_open());
        assert!(a.allows("peer-a"));
        assert!(a.allows(""));
    }

    #[test]
    fn allowlist_from_lines_filters() {
        let a = SeamAllowlist::from_lines(
            "# friends\n\
             west-host\n\
             \n\
             east-host  # comment\n",
        );
        assert!(!a.is_open());
        assert!(a.allows("west-host"));
        assert!(a.allows("east-host"));
        assert!(!a.allows("stranger"));
    }

    #[test]
    fn postcard_announce_roundtrip() {
        let a = SeamAnnounce::new("mesh", "r1", SeamSide::East, "127.0.0.1:17421");
        let b = decode_announce(&encode_announce(&a)).unwrap();
        assert_eq!(a, b);
    }

    #[test]
    fn east_dials_allowlisted_west() {
        let ours = SeamAnnounce::new("m", "east", SeamSide::East, "");
        let peer = SeamAnnounce::new("m", "west", SeamSide::West, "127.0.0.1:9");
        let mut allow = SeamAllowlist::new();
        allow.allow("west");
        assert!(should_dial_announce(&ours, &peer, &allow));
    }

    #[test]
    fn rejects_wrong_mesh_or_side_or_allow() {
        let ours = SeamAnnounce::new("m", "east", SeamSide::East, "");
        let peer = SeamAnnounce::new("m", "west", SeamSide::West, "127.0.0.1:9");
        let mut allow = SeamAllowlist::new();
        allow.allow("other");
        assert!(!should_dial_announce(&ours, &peer, &allow));

        let allow = SeamAllowlist::new();
        let wrong_mesh = SeamAnnounce::new("other", "west", SeamSide::West, "127.0.0.1:9");
        assert!(!should_dial_announce(&ours, &wrong_mesh, &allow));

        let same_side = SeamAnnounce::new("m", "west", SeamSide::East, "127.0.0.1:9");
        assert!(!should_dial_announce(&ours, &same_side, &allow));

        // West never dials.
        let west = SeamAnnounce::new("m", "west", SeamSide::West, "127.0.0.1:8");
        let east_peer = SeamAnnounce::new("m", "east", SeamSide::East, "127.0.0.1:9");
        assert!(!should_dial_announce(&west, &east_peer, &allow));
    }

    #[test]
    fn udp_loopback_announce_recv() {
        let sender = bind_discover_socket("127.0.0.1:0").unwrap();
        let recv = bind_discover_socket("127.0.0.1:0").unwrap();
        let recv_addr = recv.local_addr().unwrap();
        let announce = SeamAnnounce::new("mesh", "r1", SeamSide::West, "127.0.0.1:1");
        let bytes = encode_announce(&announce);
        sender.send_to(&bytes, recv_addr).unwrap();
        let (got, _) = recv_announce(&recv).expect("recv announce");
        assert_eq!(got, announce);
    }
}
