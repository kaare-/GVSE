# Phase 8 — Server / client *(scope draft)*

Phase 7 closed for agent (2026-10-12): wedges A–D (celestial, sundown
cast, season dial, water glitter) with Xvfb + 3k soak park=0.
**Phase 8 is architecture-first** — no production client coding until
the owner accepts a stack shortlist and ownership split.

## Intent

Split **sim ownership** from **presentation** so a shippable client can
pass **Apple and/or Steam** inspection. Prefer a production-compatible
path over another research prototype. Kernel stays MS Paint 1× on the
sim side; the client may grow richer presentation without rewriting mass
rules.

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| Headless sim + soak harness | Live | [`VOXEL_BUDGET_SOAK.md`](VOXEL_BUDGET_SOAK.md) |
| `wk-voxel-app` (macroquad demo) | Live | [`SKY.md`](SKY.md), app crate |
| Stream window / chunk residency | Live | Phase 4 docs |
| Save / SIM schema | Live | `save.rs` |
| Phase 7 surface spice | Live | [`PHASE7_SCOPE.md`](PHASE7_SCOPE.md) |

## Open questions (owner)

1. ~~**Ship target first?**~~ **Apple notarized `.app` first** (outside
   MAS); Steam later. Scripts: [`PHASE8_APPLE_PACK.md`](PHASE8_APPLE_PACK.md).
2. **Multiplayer now or later?** Default: single-player in-process first;
   netcode is a separate program of work ([`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md)).

Resolved: macroquad stays **dev / soak / Xvfb**; ship client is A.

## Proposed ownership split

```text
┌─────────────────────┐   ViewFrame + commands   ┌──────────────────┐
│  Sim host (Rust)    │◄────────────────────────►│  Client (A)      │
│  wk-voxel + step    │  (no mass rules in client)│  egui+wgpu/winit │
│  audit / park / save│                          │  present + input │
└─────────────────────┘                          └──────────────────┘
```

- **Host owns:** world step, mass audit, organisms, climate clock, save.
- **Client owns:** camera UX, draw, HUD, settings UX, storefront packaging.
- **Contract:** [`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md) —
  `ViewFrame` (not full `SimSnapshot`) + `ClientCommand` queue.

## Client stack *(owner pick 2026-10-12)*

Full comparison: [`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md).

| Role | Stack |
|------|--------|
| **Ship / explicit client** | **A — egui + wgpu / winit** (new binary; present + input only) |
| **Dev / soak / Xvfb viewer** | Keep **`wk-voxel-app` (macroquad 0.4.14 + `macroquad::ui`)** |

B (Bevy) and C (Godot) parked unless owner reopens. D (ship the
macroquad binary) is **not** the production path — macroquad stays
research/dev.

## Architecture (owner confirm 2026-10-12)

**Server / client:** host owns the sim; client is a **viewer** — camera,
HUD, and limited interaction via `ClientCommand` (pause, pan/focus,
save/load, editors later). Client never mutates sat/mass. In-process
first; same messages can move to a separate host process later.

## Landed (2026-10-12)

`wk-voxel-client` is the **viewer**; in-process host defaults to
**Compact** (4096×tropopause) with `StreamWindow` + `sync_residency`,
runs product `step_world`, and emits `ViewFrame` v4 (strip band ~384
around sea level + sky + palette/glitter). Unit tests use
`Host::for_tests()` (tiny ring). Macroquad remains soak/dev/Xvfb.

## Wedge plan

1. [x] **Scope freeze** — this file + ROADMAP pointer.
2. [x] **Stack comparison** — [`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md).
3. [x] **Owner pick** — ship client **A**; macroquad = soak/dev only.
4. [x] **Contract sketch** — [`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md).
5. [x] **Spike** — `crates/wk-voxel-client` empty window + host tick HUD.
6. [x] **Terrain strip** — `ViewFrame` v2 RGBA window + stamped world `tick`.
7. [x] **Sky** — `ViewFrame` v3 sky fields + air fill + sun/moon marker.
8. [x] **Palette / glitter** — app-aligned cell colours + water glitter.
9. [x] **Grow host** — `step_world` + climate fields; present throttled.
10. [x] **Compact host** — default `Host` stamps Compact; stream
    `COMPACT_VIEW_COLS` (320) + park far chunks; extract ~view×384 band.
11. [x] **Out-of-process host** — `wk-voxel-host` TCP + postcard IPC;
    viewer `--connect`; in-process remains default.
12. [x] **Apple pack scripts** — `packaging/macos/*` +
    [`PHASE8_APPLE_PACK.md`](PHASE8_APPLE_PACK.md); owner Mac runs
    codesign/notarize.
13. [x] **Host durable cmds** — `Save` / `Load` / `ResetWorld`
    (`COMMAND_SCHEMA` 2); F5 / F9 / R on viewer.
14. [x] **`--spawn-host`** — viewer launches `wk-voxel-host` child
    (ephemeral port); HUD `spawn-host`.
15. [x] **SetSimSettings** — `ViewFrame.sim` + viewer checkboxes
    (`COMMAND_SCHEMA` 3).
16. [x] **HostInfo** — resident/parked chunk + organism crumbs.
17. [x] **PresentHints** — humidity overlay on strip (`VIEW_SCHEMA` 7).
18. [x] **Parity note** — park=0 / TRACKED soaks stay macroquad/headless;
      A-client Compact stream is not the mass gate
      ([`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md)).

## Backlog (ordered)

See [`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md) — agent default path.
Pinned: Apple notarize → **2026-11-15**. Owner-only: E2 visual, Steam,
MAS/multiplayer.

## Acceptance (proposed)

- [x] Named Phase 8 scope draft frozen (this file).
- [x] Stack A–D comparison landed ([`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md)).
- [x] Owner accepts stack: **A ship + macroquad soak/dev**.
- [x] Contract sketch landed ([`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md)).
- [ ] No sim-mass regressions when a host/client split lands.

## Non-goals (until owner unlocks)

Full multiplayer, console ports, web WASM client, rewriting CA/mass into
the client, abandoning the macroquad soak viewer.

## Decision list (2026-10-12)

1. Phase 8 scope draft frozen; stack A–D comparison written.
2. **Owner pick:** ship client = **A (egui + wgpu / winit)**; keep
   **macroquad** as soak/dev/Xvfb viewer (not the production ship path).
3. **Contract sketch:** `ViewFrame` ≠ `SimSnapshot`; `ClientCommand`
   queue; in-process single-player first ([`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md)).
4. Contract + A spike + strip + sky + palette/glitter landed (#425–#430).
5. Owner: client = **viewer** (camera + limited commands); host = sim.
6. Grow-host (`step_world`) landed (#432). Soak stays macroquad.
7. Compact host: default stamp 4096×tropopause + stream/park; tests use
   `for_tests()`.
8. OOP host: `wk-voxel-host` + viewer `--connect` (localhost postcard).
9. Apple pack: notarized `.app` scripts first (not MAS); owner credentials
   on Mac for real notarize.
10. **2026-10-13:** Apple notarize **pinned until 2026-11-15** (membership
    expired / cost). Resume then; do not block other wedges.
11. Host Save/Load/ResetWorld + viewer keys (`COMMAND_SCHEMA` 2).
12. Viewer `--spawn-host` (child host + IPC).
13. Ordered backlog doc ([`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md)).
14. SetSimSettings + `ViewFrame.sim` (`VIEW_SCHEMA` 5 / `COMMAND_SCHEMA` 3).
15. Optional leftover: E2 tipped grey-log (Phase 5).
