# Phase 8 — Ordered backlog *(agent default path)*

**Status:** 2026-10-13. Agent follows this list top-down until blocked
on an **owner decision**. Do not ask clarifying questions when a default
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

## Soak / park gate (parity)

- **Canonical park=0 / TRACKED soaks** stay on **macroquad / headless**
  (`wk-voxel-app` + integration tests). Do not treat A-client Compact
  stream as a substitute mass gate.
- A-client Compact host is for ship UX + stream residency; optional
  future “stream soak” is a separate measurement, not a Phase 0 replace.

## Agent default next (no owner pick needed)

Phase 8 host/viewer infra is **sufficient** for ship UX. Do not open
new present wedges unless a soak/play gap demands it. Ordered path:

1. **Re-profile Compact stream+evict** on park-bank tip
   (`perf_profile_stretch_stream_ab` / equivalent) — refresh hotspot
   table after Phases 4–8 stack; log under `/opt/cursor/artifacts/`.
2. **One ≥1 ms surgical win** from that table — prefer seepage /
   bodies / field shell that does **not** coarsen weather, skip
   condensation lottery, or reopen Phase 6 hotspot #2 (river tip
   regression risk). Gate: short soak park=0 + TRACKED flat.
3. Repeat 1→2 while clear ≥1 ms wins remain; otherwise park and wait
   for owner (notarize date, E2 visual, or a named phase).

Refs: [`VOXEL_PERF.md`](VOXEL_PERF.md), [`ROADMAP.md`](ROADMAP.md).

## Pinned / owner-only

| Item | When | Notes |
|------|------|--------|
| Apple notarize | **2026-11-15** | Dev membership; scripts ready |
| E2 tipped grey-log visual | owner play/Xvfb | not a code blocker |
| Steam depots | after Apple smoke | same binaries |
| MAS / multiplayer / auth | later | explicit unlock |
| Phase 6 hotspot #2 (land/metab) | owner / harvest-neutral design | deferred — river tip starve |

## Stop and ask owner only if

- Changing ship stack away from A
- Spending money / renewing Apple membership early
- Starting multiplayer or MAS sandbox
- Dropping macroquad soak harness
- Reopening Phase 6 hotspot #2 without a harvest-neutral plan
- Large redirect outside this ordered path

## Default if measurement finds no ≥1 ms win

Park sim/perf; do not invent wedges. Wait for Apple notarize date,
owner E2 visual, or an explicit new phase name.
