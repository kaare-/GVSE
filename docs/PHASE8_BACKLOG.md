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
| 10 | HostInfo crumbs | resident/parked chunks + org counts (`VIEW_SCHEMA` 6) |

## Agent default next (no owner pick needed)

1. **Present parity gaps** — humidity/steam overlay toggles as
   present-hints or client-local; keep mass on host.
2. **Compact soak parity note** — keep park=0 gate on macroquad/
   headless; optionally document A-client stream soak separately.

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
