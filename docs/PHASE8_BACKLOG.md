# Phase 8 — Ordered backlog *(agent default path)*

**Status:** 2026-10-13 — **owner path locked**. Agent follows this list
top-down until blocked. Do not ask clarifying questions when a default
applies.

## Done

| # | Wedge | Notes |
|---|--------|--------|
| 1 | Stack A + contract | egui+wgpu ship; macroquad soak/dev |
| 2 | ViewFrame present | strip, sky, palette/glitter (`VIEW_SCHEMA` 4→5) |
| 3 | Host `step_world` | product step + climate fields |
| 4 | Compact host | 4096×tropopause + stream/park |
| 5 | OOP host IPC | `wk-voxel-host` postcard TCP |
| 6 | Apple pack scripts | `packaging/macos/*` — **notarize pinned → 2026-11-15** |
| 7 | Save/Load/Reset | `COMMAND_SCHEMA` 2→3 path |
| 8 | `--spawn-host` | child host + connect |
| 9 | SetSimSettings | `ViewFrame.sim`; viewer checkboxes |
| 10 | HostInfo crumbs | resident/parked chunks + org counts |
| 11 | PresentHints | humidity overlay wash |
| 12 | Steam/cave present | steam + caveH overlays (`VIEW_SCHEMA` 8) |
| 13 | Compact stream re-profile | confirms ~115 sim-FPS; hotspots body/temp/steam |
| 14 | Broader view + V-pan | Compact view 720; `focus_y` ↑/↓; `[`/`]` width (`VIEW_SCHEMA` 9) |
| 15 | Viewer UX polish | held-key diagonal pan; 20 Hz present; HUD + load screen |
| 16 | Viewer fill | cover-scale strip (no letterbox bars) |
| 17 | Host ship | reconnect loop; `--lan` / `--once` / `--seed`; runbook |
| 18 | Multiplayer + auth | token Hello; N peers; Viewer/Operator roles |
| 19 | Operator client w1 | role-aware UI; sim + save/load/reset gated |
| 20 | Operator paint | `PaintTerrain` + F3 brush click (`COMMAND_SCHEMA` 6) |
| 21 | Operator spawn | `SpawnOrganism` presets + F2 click (`COMMAND_SCHEMA` 7) |

## Soak / park gate (parity)

- **Canonical park=0 / TRACKED soaks** stay on **macroquad / headless**
  (`wk-voxel-app` + integration tests). Do not treat A-client Compact
  stream as a substitute mass gate.

## Owner path (2026-10-13) — locked order

1. **Host ship** — ✅ landed. See [`PHASE8_HOST_SHIP.md`](PHASE8_HOST_SHIP.md).
2. **Remote multiplayer + auth** — ✅ landed.
   See [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md).
3. **Operator client** — ✅ role UI + paint + spawn presets landed.
   See [`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md).

Apple notarize remains pinned → **2026-11-15** (parallel, not blocking).

## Agent default next

Dig **F1** Lab P0 mass seam on park-bank ([`HALO_SHARD.md`](HALO_SHARD.md)):
`wk_voxel::seam` + host `--seam-*` IPC. Next: owner two-host smoke, then
**F2** edge fields — or Dig **E** / Apple notarize (**2026-11-15**) if
redirected. Cost: [`HOSTING_COST.md`](HOSTING_COST.md). Dig **D**
agent-closed ([`BIOLOGY_DEPTH.md`](BIOLOGY_DEPTH.md); D4 owner visual).

## Pinned / later

| Item | When | Notes |
|------|------|--------|
| Apple notarize | **2026-11-15** | Dev membership; scripts ready |
| Steam depots | after Apple smoke | same binaries |
| MAS | later | explicit unlock |
| E2 tipped grey-log visual | owner play/Xvfb | not a code blocker |
| Dig F0 halo design | ✅ | [`HALO_SHARD.md`](HALO_SHARD.md) |
| Dig F1 Lab mass seam | ✅ code | seam + `--seam-*`; owner play smoke open |
| Dig F2–F5 | after F1 smoke | fields → life → Shadow → Wild |
| Dig D (biology) | agent-closed | [`BIOLOGY_DEPTH.md`](BIOLOGY_DEPTH.md) |
| Optional FPS headroom | owner ask | Compact/5 km leftovers |

## Stop and ask owner only if

- Changing ship stack away from A
- Spending money / renewing Apple membership early
- Skipping paint/spawn for an unrelated large redirect
- Dropping macroquad soak harness
- Large redirect outside this ordered path
