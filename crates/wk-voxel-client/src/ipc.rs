//! Length-prefixed postcard IPC for out-of-process host ↔ viewer.
//!
//! Framing: `u32` LE payload length + postcard([`ClientMsg`] / [`HostMsg`]).
//! Localhost TCP only for this spike (no auth, no multiplayer).

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::{ClientCommand, ViewFrame, COMMAND_SCHEMA, VIEW_SCHEMA};

/// Default bind / connect address for the OOP host spike.
pub const DEFAULT_HOST_ADDR: &str = "127.0.0.1:17420";

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum ClientMsg {
    Hello {
        view_schema: u32,
        command_schema: u32,
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
    },
    Frame(ViewFrame),
    Error { detail: String },
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

/// Viewer-side TCP session to a remote host.
pub struct RemoteHost {
    stream: TcpStream,
    last: Option<ViewFrame>,
    quit: bool,
}

impl RemoteHost {
    pub fn connect(addr: impl ToSocketAddrs) -> Result<Self, IpcError> {
        let stream = TcpStream::connect(addr)?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(Duration::from_secs(30)))?;
        stream.set_write_timeout(Some(Duration::from_secs(30)))?;
        let mut remote = Self {
            stream,
            last: None,
            quit: false,
        };
        write_msg(
            &mut remote.stream,
            &ClientMsg::Hello {
                view_schema: VIEW_SCHEMA,
                command_schema: COMMAND_SCHEMA,
            },
        )?;
        match read_msg::<HostMsg, _>(&mut remote.stream)? {
            HostMsg::HelloAck { ok, detail, view_schema, command_schema } => {
                if !ok {
                    return Err(IpcError::Protocol(detail));
                }
                if view_schema != VIEW_SCHEMA || command_schema != COMMAND_SCHEMA {
                    return Err(IpcError::Protocol(format!(
                        "schema mismatch host view={view_schema} cmd={command_schema} \
                         want view={VIEW_SCHEMA} cmd={COMMAND_SCHEMA}"
                    )));
                }
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
        // Blocking read; host prints promptly after bind (before Compact stamp… actually
        // host stamps BEFORE bind. So for Compact, listen line comes after stamp.
        // Stdout is line-buffered when piped? Rust println! flushes on newline usually
        // when connected to terminal; for pipes may block until buffer fills.
        // Host uses println! which should flush on newline in practice for small lines.
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

/// Serve one client connection: drain cmds, answer Pull with [`crate::Host::frame`].
pub fn serve_client(stream: TcpStream, host: &mut crate::Host) -> Result<(), IpcError> {
    stream.set_nodelay(true)?;
    let mut stream = stream;
    // Handshake.
    match read_msg::<ClientMsg, _>(&mut stream)? {
        ClientMsg::Hello {
            view_schema,
            command_schema,
        } => {
            let ok = view_schema == VIEW_SCHEMA && command_schema == COMMAND_SCHEMA;
            let detail = if ok {
                "ok".into()
            } else {
                format!(
                    "schema mismatch client view={view_schema} cmd={command_schema} \
                     host view={VIEW_SCHEMA} cmd={COMMAND_SCHEMA}"
                )
            };
            write_msg(
                &mut stream,
                &HostMsg::HelloAck {
                    view_schema: VIEW_SCHEMA,
                    command_schema: COMMAND_SCHEMA,
                    ok,
                    detail: detail.clone(),
                },
            )?;
            if !ok {
                return Err(IpcError::Protocol(detail));
            }
        }
        other => {
            return Err(IpcError::Protocol(format!(
                "expected Hello, got {other:?}"
            )));
        }
    }

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
                let _ = host.apply(cmd);
                if host.quit_requested {
                    // Still answer a pending Pull if any; otherwise exit after next Pull.
                }
            }
            ClientMsg::Pull => {
                let frame = host.frame();
                write_msg(&mut stream, &HostMsg::Frame(frame))?;
                if host.quit_requested {
                    break;
                }
            }
        }
    }
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
    fn tcp_pull_advances_dev_host() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = thread::spawn(move || {
            let mut host = Host::for_tests();
            let (stream, _) = listener.accept().unwrap();
            // Quit ends the session cleanly after the Pull that follows it.
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
}
