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
| 11 | PresentHints | humidity overlay wash (`VIEW_SCHEMA` 7 / `COMMAND_SCHEMA` 4) |

## Soak / park gate (parity)

- **Canonical park=0 / TRACKED soaks** stay on **macroquad / headless**
  (`wk-voxel-app` + integration tests). Do not treat A-client Compact
  stream as a substitute mass gate.
- A-client Compact host is for ship UX + stream residency; optional
  future “stream soak” is a separate measurement, not a Phase 0 replace.

## Agent default next (no owner pick needed)

1. **More present parity** — steam/cave haze overlays when useful.
2. Then: sim/perf OPEN items on park-bank ([`ROADMAP.md`](ROADMAP.md)).

## Pinned / owner-only

| Item | When | Notes |
|------|------|--------|
| Apple notarize | **2026-11-15** | Dev membership; scripts ready |
| E2 tipped grey-log visual | owner play/Xvfb | not a code blocker |
| Steam depots | after Apple smoke | same binaries |
| MAS / multiplayer / auth | later | explicit unlock |

## Stop and ask owner only if

- Changing ship stack away from A
- Spending money / renewing Apple membership early
- Starting multiplayer or MAS sandbox
- Dropping macroquad soak harness
- Large redirect outside Phase 8 / sim backlog

## Default if backlog empty

Resume sim/perf work on park-bank tip per [`ROADMAP.md`](ROADMAP.md)
Phase 6 leftovers / VOXEL_PERF OPEN items — not packaging.
