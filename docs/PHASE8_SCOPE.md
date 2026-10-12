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

1. **Ship target first?** Steam desktop, Apple (Mac App Store / notarized
   .app), or both? (Stack A supports either; packaging differs.)
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

**Next after contract:** A spike (empty window + host tick pump). No
gameplay / atlas port until spike loop is stable.

## Wedge plan

1. [x] **Scope freeze** — this file + ROADMAP pointer.
2. [x] **Stack comparison** — [`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md).
3. [x] **Owner pick** — ship client **A**; macroquad = soak/dev only.
4. [x] **Contract sketch** — [`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md).
5. [x] **Spike** — `crates/wk-voxel-client` empty window + host tick HUD.
6. [x] **Terrain strip** — `ViewFrame` v2 RGBA window + stamped world `tick`.
7. [x] **Sky** — `ViewFrame` v3 sky fields + air fill + sun/moon marker.
8. [x] **Palette / glitter** — app-aligned cell colours + water glitter.
9. [ ] **Parity gate** — Compact soak park=0 still green on soak host
      (macroquad / headless; client host is a small ring for now).

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
4. Contract + A spike + strip + sky + **palette/glitter** landed.
5. Next: grow client host / `step_world` when owner wants; soak stays macroquad.
6. Optional leftover: E2 tipped grey-log owner visual (Phase 5).
