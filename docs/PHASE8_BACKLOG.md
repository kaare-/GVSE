# Phase 8 — Ordered backlog *(agent default path)*

**Status:** 2026-10-13 — **agent parked; owner pick required**.
Agent follows this list top-down until blocked on an **owner decision**.
Do not ask clarifying questions when a default applies.

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
| 14 | Broader view + V-pan | Compact view 720; `focus_y` ↑/↓; `[`/`]` width (`VIEW_SCHEMA` 9) |
| 13 | Compact stream re-profile | confirms ~115 sim-FPS; hotspots body/temp/steam |

## Soak / park gate (parity)

- **Canonical park=0 / TRACKED soaks** stay on **macroquad / headless**
  (`wk-voxel-app` + integration tests). Do not treat A-client Compact
  stream as a substitute mass gate.
- A-client Compact host is for ship UX + stream residency; optional
  future “stream soak” is a separate measurement, not a Phase 0 replace.

## Agent default next — PARKED (needs owner pick)

Phase 8 host/viewer infra is **sufficient**. Compact stream+evict
re-profile on park-bank tip:

| Mode | Wall ms/tick | ~sim-FPS | Residency |
|------|-------------:|---------:|-----------|
| full-resident | 52.6 | ~19 | chunks=1088 |
| stream view=320 + evict | **8.7** | **~115** | 1088→459 (store=629) |

Stream hotspots: rock bodies **2.15**, temperature **1.59**, steam
**1.22**, seepage **0.79**. Log:
`/opt/cursor/artifacts/phase8-compact-stream-reprofile.log`.

≥30 / Compact ~115 goals are **already met**. Remaining body/temp/steam
cuts are optional headroom (same class as Phase 4 “owner wants more
FPS”), not required wins. Do **not** invent wedges.

### Owner pick (choose one)

| Pick | What happens |
|------|----------------|
| **A. Idle until 2026-11-15** | Agent idle / daily lists only; Apple notarize then |
| **B. Optional FPS headroom** | One ≥1 ms cut on Compact or 5 km (bodies / temp / steam / wind) with soak gate |
| **C. A-client editors** | `PaintTerrain` / `SpawnOrganism` (or subset) via `ClientCommand` |
| **D. E2 visual** | Owner play/Xvfb on tipped grey-log (Phase 5 leftover) |
| **E. Named new phase** | You name Phase 9+ scope; agent scopes then executes |

Default if silent: **A** (do not spend cycles inventing sim wedges).

## Pinned / owner-only

| Item | When | Notes |
|------|------|--------|
| Apple notarize | **2026-11-15** | Dev membership; scripts ready |
| E2 tipped grey-log visual | owner play/Xvfb | not a code blocker |
| Steam depots | after Apple smoke | same binaries |
| MAS / multiplayer / auth | later | explicit unlock |
| Phase 6 hotspot #2 (land/metab) | owner / harvest-neutral design | deferred — river tip starve |
| Optional FPS headroom | owner pick **B** | Compact/5 km leftovers |

## Stop and ask owner only if

- Changing ship stack away from A
- Spending money / renewing Apple membership early
- Starting multiplayer or MAS sandbox
- Dropping macroquad soak harness
- Reopening Phase 6 hotspot #2 without a harvest-neutral plan
- Taking optional FPS headroom without pick **B**
- Large redirect outside this ordered path
