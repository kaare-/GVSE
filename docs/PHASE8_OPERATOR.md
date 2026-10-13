# Phase 8 — Operator client *(wedge 1)*

**Status:** wedge 1 + paint landed (2026-10-13). Owner path:
**host ship → remote multiplayer/auth → operator client**.

## Intent

Same egui viewer binary (`wk-voxel-client`) with role-aware UI. Operator
gets full sim dials + save/load/reset + terrain paint against a live
host; Viewer keeps camera / present / clock only.

## Defaults

| Dial | Choice |
|------|--------|
| Binary | `wk-voxel-client` (not a separate operator crate) |
| Role | Hello `role_hint` via `--operator` + host `--ops-token` |
| In-process / `--spawn-host` | always Operator (trusted local) |

## Wedge 1 (this PR)

- HUD shows `Viewer` / `Operator`
- **sim** panel + **operator** Save/Load/Reset (slot field) only when Operator
- F5 / F9 / R gated to Operator
- Viewer overlay + camera + pause/step unchanged
- Host still enforces [`Host::command_allowed`](../crates/wk-voxel-client/src/lib.rs)

```bash
# host
cargo run -p wk-voxel-client --release --bin wk-voxel-host -- \
  --lan --token join --ops-token ops

# operator peer
cargo run -p wk-voxel-client --release -- \
  --connect HOST:17420 --token ops --operator --name ops

# viewer peer
cargo run -p wk-voxel-client --release -- \
  --connect HOST:17420 --token join --name cam
```

## Acceptance

- [x] Role shown in HUD
- [x] Viewer UI does not send SetSimSettings / Save / Load / Reset
- [x] Operator can Save/Load/Reset + sim dials (keys + buttons)
- [x] Host role gate still rejects Viewer mutating cmds (`mp_auth`)

## Paint (wedge 2)

- `ClientCommand::PaintTerrain` (`COMMAND_SCHEMA` 6) — Operator only
- F3 / checkbox toggles paint; brush + radius; click cover-scale strip
- Palette mirrors macroquad F3 (Erase/Water/Ice/…/Bedrock)

## Spawn (wedge 3)

- `ClientCommand::SpawnOrganism` (`COMMAND_SCHEMA` 7) — Operator only
- F2 / checkbox; presets Atom / Plant / Seaweed / Fungus / Epiphyte
- Click cover-scale strip (one-shot press); host `spawn_blueprint_free`

## Later wedges

- Full blueprint paint editor (macroquad F2 canvas)
- Broader sim dials (steam, stream disk, perf) aligned with macroquad Tab
