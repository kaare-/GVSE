# Phase 8 — Remote multiplayer + auth *(design note)*

**Status:** implemented (2026-10-13). Owner path:
**host ship → remote multiplayer/auth → operator client**.

Wire: `Hello` token / role + concurrent `serve_peer_shared` in
`crates/wk-voxel-client` (`--token`, `--ops-token`, `--max-peers`,
`--insecure`; viewer `--token` / `--name` / `--operator`).

## Intent

Let friends join a running `wk-voxel-host` over the network with a
**shared secret**, each with their own camera window. Host remains sole
sim authority. Operator / full tool surface stays the **next** wedge
after this one closes.

## Defaults (locked unless owner redirects)

| Dial | Choice | Why |
|------|--------|-----|
| Auth | Shared secret token | Smallest ship; no account server |
| Transport | TCP postcard + length prefix (today) | Keep IPC; add auth frame only |
| Encryption | Optional later (`--tls` / rustls) | Token first; TLS when exposing beyond LAN |
| Concurrent viewers | **N ≥ 1**, each own `SetCamera` | Real multiplayer, not just reconnect |
| Mutating cmds | Viewer: camera/present/quit; pause/step only on **Lab** tier. Shadow/Wild viewers are Presence-only for the shared clock. **Save/Load/Reset/Paint/Spawn/SetSimSettings** require Operator | See product player allowlist; `--tier lab\|shadow\|wild` |
| Discovery | Explicit `--connect host:port` | No mDNS / lobby in this wedge |
| Loopback | Token **optional** when bind is `127.0.0.1` | Local `--spawn-host` stays zero-friction |

## Threat model (this wedge)

- **In scope:** casual remote join; stop drive-by LAN/`--lan` attach; reject wrong schema / wrong token with a clear string.
- **Out of scope:** hostile internet hardening, DoS, cheat-proofing, account identity, Steam auth, relay servers.
- `--lan` without a token remains a **warn-and-refuse** (or warn-only under `--insecure`) so shipping cannot silently expose an open sim.

## Protocol sketch

Extend Hello only; keep `Cmd` / `Pull` / `Frame` shapes.

```text
Client → Host
  Hello {
    view_schema, command_schema,
    token: Option<String>,     // None ok on loopback if host has no token set
    role_hint: Viewer | Operator,  // host decides; Operator needs ops token
    client_name: Option<String>,   // HUD crumb only
  }

Host → Client
  HelloAck {
    ok, detail,
    view_schema, command_schema,
    session_id: u32,
    role: Viewer | Operator,
    peer_limit: u8,
  }
```

- Mismatch schemas → `ok=false`, close.
- Missing/wrong token when host requires one → `ok=false`, close.
- `role_hint=Operator` without ops auth → downgrade to Viewer (or reject if `--strict-roles`).

### Per-client view

Host keeps a small `PeerSession` map:

```text
PeerSession
├── session_id
├── role
├── focus_col / view_cols / focus_y
├── present_hints (optional per peer)
└── last_pull_tick
```

Each `Pull` builds a `ViewFrame` for **that** peer’s camera (residency
union of all peer windows, or “primary + spectators” — implement
**union residency** so every peer’s strip is valid). Sim still steps
once; N presents may lag independently.

### Command policy

| Command | Viewer (Lab) | Viewer (Shadow/Wild) | Operator |
|---------|--------------|----------------------|----------|
| `SetCamera` / `SetPresentHints` | ✓ | ✓ | ✓ |
| `SetPaused` / `StepOnce` | ✓ | ✗ (shared clock) | ✓ |
| `SetSimSettings` | ✗ | ✗ | ✓ |
| `Save` / `Load` / `ResetWorld` | ✗ | ✗ | ✓ |
| `PaintTerrain` / `SpawnOrganism` / `ReleaseBlueprint` | ✗ | ✗ | ✓ |
| `Quit` | session only | session only | same (does **not** tear down host) |

`--tier` on `wk-voxel-host` (default Lab on loopback, Shadow on `--lan`).

Host process exit stays CLI / SIGINT / future operator “shutdown” cmd
(not in this wedge).

## Host CLI additions (implement PR)

```text
--token SECRET          require for non-loopback (or always if set)
--ops-token SECRET      optional; grants Operator role
--max-peers N           default 4; hard cap ≤ 8 this wedge
--tier lab|shadow|wild  Viewer pause/step policy (default lab@loopback, shadow@lan)
--insecure              allow --lan with no token (dev only; loud warn)
```

Env overrides: `WK_VOXEL_HOST_TOKEN`, `WK_VOXEL_HOST_OPS_TOKEN`.

Viewer:

```text
--token SECRET          send in Hello
--name LABEL            optional HUD name
```

## Topology

```text
wk-voxel-host (one sim)
   ├── peer 1 Viewer  ←── TCP + token ──→ camera A
   ├── peer 2 Viewer  ←── TCP + token ──→ camera B
   └── peer 3 Operator←── TCP + ops-token → tools (subset now; full UI later)
```

Single-player in-process and `--spawn-host` loopback remain unchanged
(no token required unless configured).

## Acceptance (implement PR must meet)

- [x] Design note merged (this file) before protocol code lands
- [x] Host with `--token` rejects Hello without / with wrong token
- [x] Loopback `--spawn-host` still works with no token configured
- [x] `--lan` without token fails closed unless `--insecure`
- [x] Two concurrent viewers, distinct cameras, same `sim_tick`
- [x] Viewer cannot `ResetWorld` / `SetSimSettings`; ops-token can
- [x] `Quit` from one peer does not drop others; world continues
- [x] Schema mismatch still clear-fails (existing behavior)
- [x] Tests: auth reject, two-peer camera smoke, role deny (`tests/mp_auth.rs`)
- [x] Backlog / contract pointers updated; Hello extended (no
      `COMMAND_SCHEMA` bump — commands unchanged)

## Non-goals (explicit)

- Full operator egui surface (next wedge)
- Delta / interest management beyond camera strips
- Prediction, rollback, lockstep
- TLS / QUIC / WebRTC (note-only until owner asks)
- Steam / platform identity
- Public matchmaking

## After this

**Operator client** — [`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md) (wedge 1
landed: role-aware UI). Paint/spawn follow.

## Open only if implement hits a wall

- Residency cost of N wide Compact windows (may need spectator-follows-
  host mode as a fallback)
- Whether pause should be Operator-only (default: shared Viewer pause OK)
