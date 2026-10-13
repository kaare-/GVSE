# Phase 8 — Host ship *(single-player)*

**Status:** landed (2026-10-13). Owner path:
**host ship → remote multiplayer/auth → operator client**.

## Intent

Treat `wk-voxel-host` as the ship sim process for one viewer. Not
multiplayer yet — that is the next wedge after this acceptance closes.

## Ship bar (this wedge)

| Item | Notes |
|------|--------|
| Listen before stamp | Keep; `--spawn-host` needs the announce line |
| Session reconnect | After viewer disconnect (or crash), accept the next client; world kept |
| `--once` | Exit after one session (CI / smoke) |
| `--lan` | Bind `0.0.0.0:17420` (explicit; still **no auth**) |
| `--bind HOST:PORT` | Override (wins over `--lan` if both set → bind wins) |
| `--seed N` | Compact worldgen seed |
| `--dev` | Tiny ring (tests / fast smoke) |
| Schema banner | Log `VIEW_SCHEMA` / `COMMAND_SCHEMA` at listen |
| Runbook | This file + CLI `--help` |

## Non-goals (next wedges)

- Auth / tokens / TLS
- Multiple concurrent viewers
- Operator / editor client
- Public internet exposure without auth (LAN flag is for trusted LAN only)

## Run (local)

```bash
# Terminal A — Compact host (loop; reconnect OK)
cargo run -p wk-voxel-client --release --bin wk-voxel-host

# Terminal B — viewer
cargo run -p wk-voxel-client --release -- --connect 127.0.0.1:17420
```

LAN (trusted network only — **no auth**):

```bash
cargo run -p wk-voxel-client --release --bin wk-voxel-host -- --lan
# peer:
cargo run -p wk-voxel-client --release -- --connect <host-lan-ip>:17420
```

## Acceptance

- [x] Host accepts a second viewer after the first quits (same world tick)
- [x] `--lan` binds all interfaces; default stays loopback
- [x] `--once` exits after one session (spawn_host / CI)
- [x] Schema banner logs `VIEW_SCHEMA` / `COMMAND_SCHEMA` at listen
- [x] This runbook linked from [`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md)

## After this

Scope **remote multiplayer + auth** (design note first), then
**operator client** with full tool/settings control.
