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

## Soak / park gate (parity)

- **Canonical park=0 / TRACKED soaks** stay on **macroquad / headless**
  (`wk-voxel-app` + integration tests). Do not treat A-client Compact
  stream as a substitute mass gate.

## Owner path (2026-10-13) — locked order

1. **Host ship** — ✅ landed. See [`PHASE8_HOST_SHIP.md`](PHASE8_HOST_SHIP.md).
2. **Remote multiplayer + auth** — next; own design note before coding
   (tokens / allowlist / TLS-or-equivalent).
3. **Operator client** — full tools + settings control against a live
   host (circle back after multiplayer/auth).

Apple notarize remains pinned → **2026-11-15** (parallel, not blocking).

## Agent default next

Host ship landed. Next: short **multiplayer/auth design note**, then
implement (do not code multiplayer without that note).

## Pinned / later

| Item | When | Notes |
|------|------|--------|
| Apple notarize | **2026-11-15** | Dev membership; scripts ready |
| Steam depots | after Apple smoke | same binaries |
| MAS | later | explicit unlock |
| E2 tipped grey-log visual | owner play/Xvfb | not a code blocker |
| Phase 6 hotspot #2 | harvest-neutral design | deferred |
| Optional FPS headroom | owner ask | Compact/5 km leftovers |

## Stop and ask owner only if

- Changing ship stack away from A
- Spending money / renewing Apple membership early
- Skipping host ship to jump to multiplayer
- Dropping macroquad soak harness
- Large redirect outside this ordered path
