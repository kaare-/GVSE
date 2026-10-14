//! Length-prefixed postcard IPC for out-of-process host ↔ viewer.
//!
//! Framing: `u32` LE payload length + postcard([`ClientMsg`] / [`HostMsg`]).
//! Auth + concurrent peers: [`docs/PHASE8_MULTIPLAYER.md`].

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{ClientCommand, PresentHints, ViewFrame, COMMAND_SCHEMA, VIEW_SCHEMA};

/// Default bind / connect address (loopback).
pub const DEFAULT_HOST_ADDR: &str = "127.0.0.1:17420";
/// Explicit LAN bind (`--lan`). Requires `--token` unless `--insecure`.
pub const LAN_HOST_ADDR: &str = "0.0.0.0:17420";
/// Default concurrent peer cap (hard max [`MAX_PEERS_CAP`]).
pub const DEFAULT_MAX_PEERS: u8 = 4;
/// Hard cap this wedge.
pub const MAX_PEERS_CAP: u8 = 8;

/// Peer privilege on the wire / host.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum PeerRole {
    #[default]
    Viewer,
    Operator,
}

/// Product world tier ([`docs/PRODUCT_ROADMAP.md`](../../../docs/PRODUCT_ROADMAP.md)).
/// Host-side gate for Viewer clock cmds; Lab keeps local sandbox powers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum WorldTier {
    /// Offline / loopback laboratory — Viewer may still pause/step.
    #[default]
    Lab,
    /// Player-hosted shard — guests (Viewer) cannot pause the shared clock.
    Shadow,
    /// Official public region — players are Presence viewers only.
    Wild,
}

impl WorldTier {
    pub fn parse_cli(s: &str) -> Result<Self, String> {
        match s.to_ascii_lowercase().as_str() {
            "lab" => Ok(Self::Lab),
            "shadow" => Ok(Self::Shadow),
            "wild" => Ok(Self::Wild),
            other => Err(format!(
                "unknown --tier '{other}' (expected lab|shadow|wild)"
            )),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lab => "lab",
            Self::Shadow => "shadow",
            Self::Wild => "wild",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientMsg {
    Hello {
        view_schema: u32,
        command_schema: u32,
        token: Option<String>,
        role_hint: PeerRole,
        client_name: Option<String>,
    },
    Cmd(ClientCommand),
    /// Request one [`ViewFrame`] after draining commands.
    Pull,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum HostMsg {
    HelloAck {
        view_schema: u32,
        command_schema: u32,
        ok: bool,
        detail: String,
        session_id: u32,
        role: PeerRole,
        peer_limit: u8,
    },
    Frame(ViewFrame),
    Error { detail: String },
}

/// Host-side auth / peer limits ([`PHASE8_MULTIPLAYER.md`](../../../docs/PHASE8_MULTIPLAYER.md)).
#[derive(Debug, Clone)]
pub struct ServeConfig {
    /// Join token; when set, Hello.token must match.
    pub token: Option<String>,
    /// Ops token; Hello with this token + Operator hint → Operator.
    pub ops_token: Option<String>,
    pub max_peers: u8,
    /// Bind is loopback — token optional unless `token` is set.
    pub loopback: bool,
    /// Lab / Shadow / Wild — gates Viewer pause/step (product allowlist).
    pub world_tier: WorldTier,
}

impl Default for ServeConfig {
    fn default() -> Self {
        Self {
            token: None,
            ops_token: None,
            max_peers: DEFAULT_MAX_PEERS,
            loopback: true,
            world_tier: WorldTier::Lab,
        }
    }
}

impl ServeConfig {
    pub fn clamp_peers(n: u8) -> u8 {
        n.clamp(1, MAX_PEERS_CAP)
    }

    /// Fail closed for non-loopback without token (unless caller already allowed insecure).
    pub fn require_token_for_bind(loopback: bool, token: &Option<String>, insecure: bool) -> Result<(), String> {
        if !loopback && token.is_none() && !insecure {
            return Err(
                "non-loopback bind requires --token (or --insecure for trusted-dev only)".into(),
            );
        }
        Ok(())
    }

    fn authorize(&self, token: &Option<String>, role_hint: PeerRole) -> Result<PeerRole, String> {
        let join_ok = self.token.as_ref().is_some_and(|t| token.as_ref() == Some(t));
        let ops_ok = self
            .ops_token
            .as_ref()
            .is_some_and(|t| token.as_ref() == Some(t));
        let open = self.token.is_none() && self.ops_token.is_none();

        if self.token.is_some() || self.ops_token.is_some() {
            if !join_ok && !ops_ok {
                return Err(match token {
                    None => "auth failed: token required".into(),
                    Some(_) => "auth failed: bad token".into(),
                });
            }
        } else if !self.loopback && !open {
            // unreachable: open covers both-none
        }

        // Loopback with no tokens: Operator (Save/Load/Reset for local/tests).
        if open && self.loopback {
            return Ok(PeerRole::Operator);
        }

        let mut role = PeerRole::Viewer;
        if matches!(role_hint, PeerRole::Operator) && ops_ok {
            role = PeerRole::Operator;
        }
        Ok(role)
    }
}

#[derive(Debug, Clone)]
pub struct PeerView {
    pub session_id: u32,
    pub role: PeerRole,
    pub focus_col: i32,
    pub view_cols: i32,
    pub focus_y: i32,
    pub present: PresentHints,
    pub client_name: Option<String>,
    pub pending_acks: Vec<crate::CommandAck>,
}

#[derive(Debug, Default)]
pub struct PeerRegistry {
    peers: HashMap<u32, PeerView>,
    next_id: u32,
    max_peers: u8,
}

impl PeerRegistry {
    pub fn new(max_peers: u8) -> Self {
        Self {
            peers: HashMap::new(),
            next_id: 1,
            max_peers: ServeConfig::clamp_peers(max_peers),
        }
    }

    pub fn len(&self) -> usize {
        self.peers.len()
    }

    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    pub fn try_reserve(&mut self) -> Result<u32, String> {
        if self.peers.len() as u8 >= self.max_peers {
            return Err(format!("peer limit {}", self.max_peers));
        }
        let id = self.next_id;
        self.next_id = self.next_id.saturating_add(1);
        Ok(id)
    }

    pub fn insert(&mut self, peer: PeerView) {
        self.peers.insert(peer.session_id, peer);
    }

    pub fn remove(&mut self, id: u32) {
        self.peers.remove(&id);
    }

    pub fn get_mut(&mut self, id: u32) -> Option<&mut PeerView> {
        self.peers.get_mut(&id)
    }

    pub fn camera_list(&self) -> Vec<(i32, i32)> {
        self.peers
            .values()
            .map(|p| (p.focus_col, p.view_cols))
            .collect()
    }
}

#[derive(Debug)]
pub enum IpcError {
    Io(std::io::Error),
    Codec(String),
    Protocol(String),
}

impl std::fmt::Display for IpcError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "ipc io: {e}"),
            Self::Codec(e) => write!(f, "ipc codec: {e}"),
            Self::Protocol(e) => write!(f, "ipc protocol: {e}"),
        }
    }
}

impl std::error::Error for IpcError {}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        Self::Io(e)
    }
}

pub fn write_msg<T: Serialize, W: Write>(w: &mut W, msg: &T) -> Result<(), IpcError> {
    let bytes = postcard::to_allocvec(msg).map_err(|e| IpcError::Codec(e.to_string()))?;
    if bytes.len() > u32::MAX as usize {
        return Err(IpcError::Codec("message too large".into()));
    }
    let len = (bytes.len() as u32).to_le_bytes();
    w.write_all(&len)?;
    w.write_all(&bytes)?;
    w.flush()?;
    Ok(())
}

pub fn read_msg<T: for<'de> Deserialize<'de>, R: Read>(r: &mut R) -> Result<T, IpcError> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    // Compact strip ~720×384×4 ≈ 1.1 MiB (up to MAX_VIEW_COLS); 8 MiB headroom.
    if len > 8 * 1024 * 1024 {
        return Err(IpcError::Codec(format!("frame len {len} exceeds 8 MiB")));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    postcard::from_bytes(&buf).map_err(|e| IpcError::Codec(e.to_string()))
}

/// Viewer connect options.
#[derive(Debug, Clone, Default)]
pub struct ConnectOpts {
    pub token: Option<String>,
    pub role_hint: PeerRole,
    pub client_name: Option<String>,
}

/// Viewer-side TCP session to a remote host.
pub struct RemoteHost {
    stream: TcpStream,
    last: Option<ViewFrame>,
    quit: bool,
    pub session_id: u32,
    pub role: PeerRole,
}

impl RemoteHost {
    pub fn connect(addr: impl ToSocketAddrs) -> Result<Self, IpcError> {
        Self::connect_with(addr, &ConnectOpts::default())
    }

    pub fn connect_with(addr: impl ToSocketAddrs, opts: &ConnectOpts) -> Result<Self, IpcError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut remote = Self {
            stream,
            last: None,
            quit: false,
            session_id: 0,
            role: PeerRole::Viewer,
        };
        write_msg(
            &mut remote.stream,
            &ClientMsg::Hello {
                view_schema: VIEW_SCHEMA,
                command_schema: COMMAND_SCHEMA,
                token: opts.token.clone(),
                role_hint: opts.role_hint,
                client_name: opts.client_name.clone(),
            },
        )?;
        match read_msg::<HostMsg, _>(&mut remote.stream)? {
            HostMsg::HelloAck {
                ok,
                detail,
                view_schema,
                command_schema,
                session_id,
                role,
                peer_limit: _,
            } => {
                if !ok {
                    return Err(IpcError::Protocol(detail));
                }
                if view_schema != VIEW_SCHEMA || command_schema != COMMAND_SCHEMA {
                    return Err(IpcError::Protocol(format!(
                        "schema mismatch host view={view_schema} cmd={command_schema} \
                         want view={VIEW_SCHEMA} cmd={COMMAND_SCHEMA}"
                    )));
                }
                remote.session_id = session_id;
                remote.role = role;
            }
            other => {
                return Err(IpcError::Protocol(format!(
                    "expected HelloAck, got {other:?}"
                )));
            }
        }
        Ok(remote)
    }

    pub fn apply_all(&mut self, cmds: impl IntoIterator<Item = ClientCommand>) -> Result<(), IpcError> {
        for c in cmds {
            if matches!(c, ClientCommand::Quit) {
                self.quit = true;
            }
            write_msg(&mut self.stream, &ClientMsg::Cmd(c))?;
        }
        Ok(())
    }

    pub fn pull(&mut self) -> Result<ViewFrame, IpcError> {
        write_msg(&mut self.stream, &ClientMsg::Pull)?;
        match read_msg::<HostMsg, _>(&mut self.stream)? {
            HostMsg::Frame(frame) => {
                self.last = Some(frame.clone());
                Ok(frame)
            }
            HostMsg::Error { detail } => Err(IpcError::Protocol(detail)),
            other => Err(IpcError::Protocol(format!(
                "expected Frame, got {other:?}"
            ))),
        }
    }

    pub fn last(&self) -> Option<&ViewFrame> {
        self.last.as_ref()
    }

    pub fn quit_requested(&self) -> bool {
        self.quit
    }
}

/// Child `wk-voxel-host` + connected [`RemoteHost`].
///
/// Drop sends best-effort Quit then kills the child.
pub struct SpawnedHost {
    pub remote: RemoteHost,
    child: Child,
}

impl SpawnedHost {
    /// Launch `wk-voxel-host` on an ephemeral port and connect.
    ///
    /// `dev`: pass `--dev` (tiny ring). Otherwise Compact stamp.
    pub fn spawn(dev: bool) -> Result<Self, IpcError> {
        Self::spawn_with_bin(resolve_host_bin()?, dev)
    }

    pub fn spawn_with_bin(host_bin: impl AsRef<Path>, dev: bool) -> Result<Self, IpcError> {
        let host_bin = host_bin.as_ref();
        if !host_bin.is_file() {
            return Err(IpcError::Protocol(format!(
                "host binary not found: {}",
                host_bin.display()
            )));
        }
        let mut cmd = Command::new(host_bin);
        cmd.arg("--bind").arg("127.0.0.1:0");
        // Parent owns lifetime; host must not linger waiting for a 2nd client.
        cmd.arg("--once");
        if dev {
            cmd.arg("--dev");
        }
        // Inherit stderr so the pipe buffer cannot stall the host on logs.
        cmd.stdout(Stdio::piped()).stderr(Stdio::inherit());
        let mut child = cmd.spawn().map_err(IpcError::Io)?;
        let stdout = match child.stdout.take() {
            Some(s) => s,
            None => {
                let _ = child.kill();
                return Err(IpcError::Protocol("host stdout missing".into()));
            }
        };
        let addr = match wait_listening_addr(stdout, Duration::from_secs(120)) {
            Ok(a) => a,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        // Stamp may still be running; connect waits in accept backlog / HelloAck.
        let remote = match connect_with_retry(&addr, Duration::from_secs(120)) {
            Ok(r) => r,
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(e);
            }
        };
        Ok(Self { remote, child })
    }

    pub fn child_id(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for SpawnedHost {
    fn drop(&mut self) {
        let _ = self.remote.apply_all([ClientCommand::Quit]);
        let _ = self.remote.pull();
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// Resolve `wk-voxel-host` next to the running binary, else `PATH`.
pub fn resolve_host_bin() -> Result<PathBuf, IpcError> {
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sibling = dir.join("wk-voxel-host");
            if sibling.is_file() {
                return Ok(sibling);
            }
            #[cfg(windows)]
            {
                let sibling = dir.join("wk-voxel-host.exe");
                if sibling.is_file() {
                    return Ok(sibling);
                }
            }
        }
    }
    Ok(PathBuf::from("wk-voxel-host"))
}

fn wait_listening_addr(
    stdout: impl Read,
    timeout: Duration,
) -> Result<String, IpcError> {
    let mut reader = BufReader::new(stdout);
    let deadline = Instant::now() + timeout;
    let mut line = String::new();
    loop {
        if Instant::now() > deadline {
            return Err(IpcError::Protocol(
                "timed out waiting for 'wk-voxel-host listening on …'".into(),
            ));
        }
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => {
                return Err(IpcError::Protocol(
                    "host exited before publishing listen address".into(),
                ));
            }
            Ok(_) => {
                if let Some(rest) = line.strip_prefix("wk-voxel-host listening on ") {
                    return Ok(rest.trim().to_string());
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(IpcError::Io(e)),
        }
    }
}

fn connect_with_retry(addr: &str, timeout: Duration) -> Result<RemoteHost, IpcError> {
    let deadline = Instant::now() + timeout;
    let mut last = None;
    while Instant::now() < deadline {
        match RemoteHost::connect(addr) {
            Ok(r) => return Ok(r),
            Err(e) => {
                last = Some(e);
                std::thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(last.unwrap_or_else(|| {
        IpcError::Protocol(format!("connect {addr}: retries exhausted"))
    }))
}

/// Exclusive `&mut Host` path (unit tests / serial): one peer, loopback Operator.
pub fn serve_client(stream: TcpStream, host: &mut crate::Host) -> Result<(), IpcError> {
    let cfg = ServeConfig::default();
    let mut registry = PeerRegistry::new(1);
    serve_peer_on(stream, host, &cfg, &mut registry)
}

/// Concurrent peer session against `Arc<Mutex<Host>>`.
pub fn serve_peer_shared(
    stream: TcpStream,
    host: Arc<Mutex<crate::Host>>,
    registry: Arc<Mutex<PeerRegistry>>,
    cfg: Arc<ServeConfig>,
) -> Result<(), IpcError> {
    stream.set_nodelay(true)?;
    let mut stream = stream;

    let hello = match read_msg::<ClientMsg, _>(&mut stream)? {
        ClientMsg::Hello {
            view_schema,
            command_schema,
            token,
            role_hint,
            client_name,
        } => (
            view_schema,
            command_schema,
            token,
            role_hint,
            client_name,
        ),
        other => {
            return Err(IpcError::Protocol(format!(
                "expected Hello, got {other:?}"
            )));
        }
    };
    let (view_schema, command_schema, token, role_hint, client_name) = hello;

    let schema_ok = view_schema == VIEW_SCHEMA && command_schema == COMMAND_SCHEMA;
    let auth = if schema_ok {
        cfg.authorize(&token, role_hint)
    } else {
        Err(format!(
            "schema mismatch client view={view_schema} cmd={command_schema} \
             host view={VIEW_SCHEMA} cmd={COMMAND_SCHEMA}"
        ))
    };

    let (ok, detail, role, session_id) = match auth {
        Ok(role) => {
            let mut reg = registry.lock().map_err(|_| {
                IpcError::Protocol("peer registry lock poisoned".into())
            })?;
            match reg.try_reserve() {
                Ok(id) => (true, "ok".to_string(), role, id),
                Err(e) => (false, e, PeerRole::Viewer, 0),
            }
        }
        Err(e) => (false, e, PeerRole::Viewer, 0),
    };

    write_msg(
        &mut stream,
        &HostMsg::HelloAck {
            view_schema: VIEW_SCHEMA,
            command_schema: COMMAND_SCHEMA,
            ok,
            detail: detail.clone(),
            session_id,
            role,
            peer_limit: cfg.max_peers,
        },
    )?;
    if !ok {
        return Err(IpcError::Protocol(detail));
    }

    let (focus_col, view_cols, focus_y) = {
        let h = host
            .lock()
            .map_err(|_| IpcError::Protocol("host lock poisoned".into()))?;
        (h.focus_col, h.view_cols, h.focus_y)
    };

    {
        let mut reg = registry
            .lock()
            .map_err(|_| IpcError::Protocol("peer registry lock poisoned".into()))?;
        reg.insert(PeerView {
            session_id,
            role,
            focus_col,
            view_cols,
            focus_y,
            present: PresentHints::default(),
            client_name,
            pending_acks: Vec::new(),
        });
    }

    let mut peer_quit = false;
    loop {
        match read_msg::<ClientMsg, _>(&mut stream)? {
            ClientMsg::Hello { .. } => {
                write_msg(
                    &mut stream,
                    &HostMsg::Error {
                        detail: "duplicate Hello".into(),
                    },
                )?;
            }
            ClientMsg::Cmd(cmd) => {
                if matches!(cmd, ClientCommand::Quit) {
                    peer_quit = true;
                    // Do not tear down the host process / shared world.
                    continue;
                }
                // Lock order: registry (brief) → host → registry (ack). Never hold both.
                match &cmd {
                    ClientCommand::SetCamera {
                        focus_col,
                        view_cols,
                        focus_y,
                    } => {
                        {
                            let mut reg = registry.lock().map_err(|_| {
                                IpcError::Protocol("peer registry lock poisoned".into())
                            })?;
                            if let Some(p) = reg.get_mut(session_id) {
                                if *view_cols > 0 {
                                    p.focus_col = *focus_col;
                                    p.view_cols = (*view_cols).clamp(
                                        crate::MIN_VIEW_COLS,
                                        crate::MAX_VIEW_COLS,
                                    );
                                    p.focus_y = *focus_y;
                                }
                            }
                        }
                        let ack = {
                            let mut h = host.lock().map_err(|_| {
                                IpcError::Protocol("host lock poisoned".into())
                            })?;
                            let ack = h.apply_as(role, cfg.world_tier, cmd);
                            let _ = h.take_last_ack();
                            ack
                        };
                        let mut reg = registry.lock().map_err(|_| {
                            IpcError::Protocol("peer registry lock poisoned".into())
                        })?;
                        if let Some(p) = reg.get_mut(session_id) {
                            p.pending_acks.push(ack);
                        }
                    }
                    ClientCommand::SetPresentHints(hints) => {
                        {
                            let mut reg = registry.lock().map_err(|_| {
                                IpcError::Protocol("peer registry lock poisoned".into())
                            })?;
                            if let Some(p) = reg.get_mut(session_id) {
                                p.present = hints.clone();
                            }
                        }
                        let ack = {
                            let mut h = host.lock().map_err(|_| {
                                IpcError::Protocol("host lock poisoned".into())
                            })?;
                            let ack = h.apply_as(role, cfg.world_tier, cmd);
                            let _ = h.take_last_ack();
                            ack
                        };
                        let mut reg = registry.lock().map_err(|_| {
                            IpcError::Protocol("peer registry lock poisoned".into())
                        })?;
                        if let Some(p) = reg.get_mut(session_id) {
                            p.pending_acks.push(ack);
                        }
                    }
                    _ => {
                        let ack = {
                            let mut h = host.lock().map_err(|_| {
                                IpcError::Protocol("host lock poisoned".into())
                            })?;
                            let ack = h.apply_as(role, cfg.world_tier, cmd);
                            let _ = h.take_last_ack();
                            ack
                        };
                        let mut reg = registry.lock().map_err(|_| {
                            IpcError::Protocol("peer registry lock poisoned".into())
                        })?;
                        if let Some(p) = reg.get_mut(session_id) {
                            p.pending_acks.push(ack);
                        }
                    }
                }
            }
            ClientMsg::Pull => {
                let cams = {
                    let reg = registry
                        .lock()
                        .map_err(|_| IpcError::Protocol("peer registry lock poisoned".into()))?;
                    reg.camera_list()
                };
                let (fc, vc, fy, present, acks) = {
                    let mut reg = registry
                        .lock()
                        .map_err(|_| IpcError::Protocol("peer registry lock poisoned".into()))?;
                    let p = reg
                        .get_mut(session_id)
                        .ok_or_else(|| IpcError::Protocol("peer gone".into()))?;
                    (
                        p.focus_col,
                        p.view_cols,
                        p.focus_y,
                        p.present.clone(),
                        std::mem::take(&mut p.pending_acks),
                    )
                };
                let frame = {
                    let mut h = host
                        .lock()
                        .map_err(|_| IpcError::Protocol("host lock poisoned".into()))?;
                    h.push_acks(acks);
                    h.frame_as(fc, vc, fy, present, &cams)
                };
                write_msg(&mut stream, &HostMsg::Frame(frame))?;
                if peer_quit {
                    break;
                }
            }
        }
    }

    if let Ok(mut reg) = registry.lock() {
        reg.remove(session_id);
    }
    Ok(())
}

/// Exclusive `&mut Host` path (unit tests): one peer, same wire protocol.
fn serve_peer_on(
    stream: TcpStream,
    host: &mut crate::Host,
    cfg: &ServeConfig,
    registry: &mut PeerRegistry,
) -> Result<(), IpcError> {
    stream.set_nodelay(true)?;
    let mut stream = stream;

    let (view_schema, command_schema, token, role_hint, client_name) =
        match read_msg::<ClientMsg, _>(&mut stream)? {
            ClientMsg::Hello {
                view_schema,
                command_schema,
                token,
                role_hint,
                client_name,
            } => (view_schema, command_schema, token, role_hint, client_name),
            other => {
                return Err(IpcError::Protocol(format!(
                    "expected Hello, got {other:?}"
                )));
            }
        };

    let schema_ok = view_schema == VIEW_SCHEMA && command_schema == COMMAND_SCHEMA;
    let auth = if schema_ok {
        cfg.authorize(&token, role_hint)
    } else {
        Err(format!(
            "schema mismatch client view={view_schema} cmd={command_schema} \
             host view={VIEW_SCHEMA} cmd={COMMAND_SCHEMA}"
        ))
    };

    let (ok, detail, role, session_id) = match auth {
        Ok(role) => match registry.try_reserve() {
            Ok(id) => (true, "ok".to_string(), role, id),
            Err(e) => (false, e, PeerRole::Viewer, 0),
        },
        Err(e) => (false, e, PeerRole::Viewer, 0),
    };

    write_msg(
        &mut stream,
        &HostMsg::HelloAck {
            view_schema: VIEW_SCHEMA,
            command_schema: COMMAND_SCHEMA,
            ok,
            detail: detail.clone(),
            session_id,
            role,
            peer_limit: cfg.max_peers,
        },
    )?;
    if !ok {
        return Err(IpcError::Protocol(detail));
    }

    registry.insert(PeerView {
        session_id,
        role,
        focus_col: host.focus_col,
        view_cols: host.view_cols,
        focus_y: host.focus_y,
        present: PresentHints::default(),
        client_name,
        pending_acks: Vec::new(),
    });

    let mut peer_quit = false;
    loop {
        match read_msg::<ClientMsg, _>(&mut stream)? {
            ClientMsg::Hello { .. } => {
                write_msg(
                    &mut stream,
                    &HostMsg::Error {
                        detail: "duplicate Hello".into(),
                    },
                )?;
            }
            ClientMsg::Cmd(cmd) => {
                if matches!(cmd, ClientCommand::Quit) {
                    peer_quit = true;
                    continue;
                }
                match &cmd {
                    ClientCommand::SetCamera {
                        focus_col,
                        view_cols,
                        focus_y,
                    } => {
                        if let Some(p) = registry.get_mut(session_id) {
                            if *view_cols > 0 {
                                p.focus_col = *focus_col;
                                p.view_cols = (*view_cols).clamp(
                                    crate::MIN_VIEW_COLS,
                                    crate::MAX_VIEW_COLS,
                                );
                                p.focus_y = *focus_y;
                            }
                        }
                        let ack = host.apply_as(role, cfg.world_tier, cmd);
                        let _ = host.take_last_ack();
                        if let Some(p) = registry.get_mut(session_id) {
                            p.pending_acks.push(ack);
                        }
                    }
                    ClientCommand::SetPresentHints(hints) => {
                        if let Some(p) = registry.get_mut(session_id) {
                            p.present = hints.clone();
                        }
                        let ack = host.apply_as(role, cfg.world_tier, cmd);
                        let _ = host.take_last_ack();
                        if let Some(p) = registry.get_mut(session_id) {
                            p.pending_acks.push(ack);
                        }
                    }
                    _ => {
                        let ack = host.apply_as(role, cfg.world_tier, cmd);
                        let _ = host.take_last_ack();
                        if let Some(p) = registry.get_mut(session_id) {
                            p.pending_acks.push(ack);
                        }
                    }
                }
            }
            ClientMsg::Pull => {
                let cams = registry.camera_list();
                let (fc, vc, fy, present, acks) = {
                    let p = registry
                        .get_mut(session_id)
                        .ok_or_else(|| IpcError::Protocol("peer gone".into()))?;
                    (
                        p.focus_col,
                        p.view_cols,
                        p.focus_y,
                        p.present.clone(),
                        std::mem::take(&mut p.pending_acks),
                    )
                };
                host.push_acks(acks);
                let frame = host.frame_as(fc, vc, fy, present, &cams);
                write_msg(&mut stream, &HostMsg::Frame(frame))?;
                if peer_quit {
                    break;
                }
            }
        }
    }
    registry.remove(session_id);
    // Clear host quit flag so reconnect tests / serial sessions stay clean.
    host.quit_requested = false;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{ClientCommand, Host};
    use std::net::TcpListener;
    use std::thread;

    #[test]
    fn postcard_roundtrip_cmd_and_frame() {
        let mut host = Host::for_tests();
        host.apply(ClientCommand::SetPaused(true));
        let frame = host.frame();
        let msg = HostMsg::Frame(frame.clone());
        let bytes = postcard::to_allocvec(&msg).unwrap();
        let back: HostMsg = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, HostMsg::Frame(frame));

        let cmd = ClientMsg::Cmd(ClientCommand::SetCamera {
            focus_col: 12,
            view_cols: 64,
            focus_y: 40,
        });
        let bytes = postcard::to_allocvec(&cmd).unwrap();
        let back: ClientMsg = postcard::from_bytes(&bytes).unwrap();
        assert_eq!(back, cmd);
    }

    #[test]
    fn auth_rejects_bad_token() {
        let cfg = ServeConfig {
            token: Some("secret".into()),
            ops_token: None,
            max_peers: 4,
            loopback: true,
            world_tier: Default::default(),
        };
        assert!(cfg.authorize(&None, PeerRole::Viewer).is_err());
        assert!(cfg
            .authorize(&Some("nope".into()), PeerRole::Viewer)
            .is_err());
        assert_eq!(
            cfg.authorize(&Some("secret".into()), PeerRole::Viewer)
                .unwrap(),
            PeerRole::Viewer
        );
    }

    #[test]
    fn ops_token_grants_operator() {
        let cfg = ServeConfig {
            token: Some("join".into()),
            ops_token: Some("ops".into()),
            max_peers: 4,
            loopback: false,
            world_tier: Default::default(),
        };
        // Join token alone → Viewer even with Operator hint.
        assert_eq!(
            cfg.authorize(&Some("join".into()), PeerRole::Operator)
                .unwrap(),
            PeerRole::Viewer
        );
        // Ops token accepted as credential + Operator hint → Operator.
        assert_eq!(
            cfg.authorize(&Some("ops".into()), PeerRole::Operator)
                .unwrap(),
            PeerRole::Operator
        );
        // Ops token with Viewer hint stays Viewer.
        assert_eq!(
            cfg.authorize(&Some("ops".into()), PeerRole::Viewer)
                .unwrap(),
            PeerRole::Viewer
        );
    }

    #[test]
    fn tcp_pull_advances_dev_host() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut host = Host::for_tests();
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_client(stream, &mut host);
        });

        let mut remote = RemoteHost::connect(addr).unwrap();
        let f0 = remote.pull().unwrap();
        assert!(f0.schema_ok());
        assert!(!f0.paused);
        remote
            .apply_all([ClientCommand::SetPaused(true)])
            .unwrap();
        let f1 = remote.pull().unwrap();
        assert!(f1.paused);
        let t1 = f1.sim_tick;
        remote.apply_all([ClientCommand::StepOnce]).unwrap();
        let f2 = remote.pull().unwrap();
        assert!(f2.sim_tick > t1);
        remote.apply_all([ClientCommand::Quit]).unwrap();
        let _ = remote.pull();
        server.join().unwrap();
    }

    #[test]
    fn tcp_save_load_reset_over_ipc() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let slot = format!("ipc_test_{}", std::process::id());
        let slot_for_server = slot.clone();
        let server = thread::spawn(move || {
            let mut host = Host::for_tests();
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_client(stream, &mut host);
            let _ = std::fs::remove_file(wk_voxel::SimSnapshot::save_path(&slot_for_server));
        });

        let mut remote = RemoteHost::connect(addr).unwrap();
        remote
            .apply_all([
                ClientCommand::SetPaused(true),
                ClientCommand::StepOnce,
                ClientCommand::StepOnce,
            ])
            .unwrap();
        let t = remote.pull().unwrap().sim_tick;
        remote
            .apply_all([ClientCommand::Save { slot: slot.clone() }])
            .unwrap();
        let ack_frame = remote.pull().unwrap();
        assert!(
            ack_frame.command_acks.iter().any(|a| a.ok && a.detail.contains("saved")),
            "{:?}",
            ack_frame.command_acks
        );
        remote
            .apply_all([ClientCommand::ResetWorld { seed: Some(1) }])
            .unwrap();
        assert_eq!(remote.pull().unwrap().sim_tick, 0);
        remote
            .apply_all([ClientCommand::Load { slot: slot.clone() }])
            .unwrap();
        let loaded = remote.pull().unwrap();
        assert_eq!(loaded.sim_tick, t);
        remote.apply_all([ClientCommand::Quit]).unwrap();
        let _ = remote.pull();
        server.join().unwrap();
        let _ = std::fs::remove_file(wk_voxel::SimSnapshot::save_path(&slot));
    }

    #[test]
    fn wild_viewer_denied_pause_step() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut host = Host::for_tests();
            let cfg = ServeConfig {
                token: Some("t".into()),
                ops_token: Some("ops".into()),
                max_peers: 2,
                loopback: true,
                world_tier: WorldTier::Wild,
            };
            let mut registry = PeerRegistry::new(2);
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_peer_on(stream, &mut host, &cfg, &mut registry);
        });

        let mut remote = RemoteHost::connect_with(
            addr,
            &ConnectOpts {
                token: Some("t".into()),
                role_hint: PeerRole::Viewer,
                client_name: Some("v".into()),
            },
        )
        .unwrap();
        assert_eq!(remote.role, PeerRole::Viewer);
        remote
            .apply_all([
                ClientCommand::SetPaused(true),
                ClientCommand::StepOnce,
            ])
            .unwrap();
        let f = remote.pull().unwrap();
        assert!(
            f.command_acks.iter().filter(|a| !a.ok).count() >= 2,
            "wild viewer pause/step must deny: {:?}",
            f.command_acks
        );
        assert!(!f.paused);
        remote.apply_all([ClientCommand::Quit]).unwrap();
        let _ = remote.pull();
        server.join().unwrap();
    }

    #[test]
    fn viewer_role_denied_reset() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut host = Host::for_tests();
            let cfg = ServeConfig {
                token: Some("t".into()),
                ops_token: Some("ops".into()),
                max_peers: 2,
                loopback: true,
                world_tier: Default::default(),
            };
            let mut registry = PeerRegistry::new(2);
            let (stream, _) = listener.accept().unwrap();
            let _ = serve_peer_on(stream, &mut host, &cfg, &mut registry);
        });

        let mut remote = RemoteHost::connect_with(
            addr,
            &ConnectOpts {
                token: Some("t".into()),
                role_hint: PeerRole::Viewer,
                client_name: Some("v".into()),
            },
        )
        .unwrap();
        assert_eq!(remote.role, PeerRole::Viewer);
        remote
            .apply_all([ClientCommand::ResetWorld { seed: None }])
            .unwrap();
        let f = remote.pull().unwrap();
        assert!(
            f.command_acks.iter().any(|a| !a.ok && a.detail.contains("cannot")),
            "{:?}",
            f.command_acks
        );
        remote.apply_all([ClientCommand::Quit]).unwrap();
        let _ = remote.pull();
        server.join().unwrap();
    }
}
