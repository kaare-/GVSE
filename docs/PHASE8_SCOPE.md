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
   .app), or both?
2. **Multiplayer now or later?** Single-player host + local client first
   is the safer wedge; netcode is a separate program of work.
3. **Keep macroquad demo?** Likely yes as a **dev / soak viewer**; ship
   client is a different binary.

## Proposed ownership split

```text
┌─────────────────────┐     snapshots / commands      ┌──────────────────┐
│  Sim host (Rust)    │◄────────────────────────────►│  Client (TBD)    │
│  wk-voxel + step    │   (no mass rules in client)   │  present + input │
│  audit / park / save│                               │  Apple/Steam UI  │
└─────────────────────┘                               └──────────────────┘
```

- **Host owns:** world step, mass audit, organisms, climate clock, save.
- **Client owns:** camera, draw, HUD, settings UX, storefront packaging.
- **Contract:** versioned snapshot or delta stream + input commands;
  client never mutates sat/mass directly.

## Client stack *(owner pick 2026-10-12)*

Full comparison: [`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md).

| Role | Stack |
|------|--------|
| **Ship / explicit client** | **A — egui + wgpu / winit** (new binary; present + input only) |
| **Dev / soak / Xvfb viewer** | Keep **`wk-voxel-app` (macroquad 0.4.14 + `macroquad::ui`)** |

B (Bevy) and C (Godot) parked unless owner reopens. D (ship the
macroquad binary) is **not** the production path — macroquad stays
research/dev.

**Next:** contract sketch (snapshot + commands, docs only), then A spike
(empty window + host tick pump). No gameplay port until contract lands.

## Wedge plan

1. [x] **Scope freeze** — this file + ROADMAP pointer.
2. [x] **Stack comparison** — [`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md).
3. [x] **Owner pick** — ship client **A**; macroquad = soak/dev only.
4. [ ] **Contract sketch** — snapshot schema / command list (docs only).
5. [ ] **Spike** — A: empty `winit`+`wgpu`+egui window + host tick pump.
6. [ ] **Parity gate** — Compact soak park=0 still green with host binary.

## Acceptance (proposed)

- [x] Named Phase 8 scope draft frozen (this file).
- [x] Stack A–D comparison landed ([`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md)).
- [x] Owner accepts stack: **A ship + macroquad soak/dev**.
- [ ] Contract sketch landed before any client rewrite.
- [ ] No sim-mass regressions when a host/client split lands.

## Non-goals (until owner unlocks)

Full multiplayer, console ports, web WASM client, rewriting CA/mass into
the client, abandoning the macroquad soak viewer.

## Decision list (2026-10-12)

1. Phase 8 scope draft frozen; stack A–D comparison written.
2. **Owner pick:** ship client = **A (egui + wgpu / winit)**; keep
   **macroquad** as soak/dev/Xvfb viewer (not the production ship path).
3. Next wedge: **contract sketch** (docs), then A spike — no gameplay port yet.
4. Optional leftover: E2 tipped grey-log owner visual (Phase 5).
