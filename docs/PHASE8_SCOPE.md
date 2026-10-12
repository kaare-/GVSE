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

## Client stack shortlist *(draft — not decided)*

| Stack | Pros | Cons / risks |
|-------|------|----------------|
| **A. egui + wgpu / winit** | Rust-native; good desktop packaging; egui already in demo settings | Rewrite draw from macroquad; Apple Metal via wgpu OK |
| **B. Bevy** | ECS + rendering ecosystem; Steam-friendly desktop | Heavy; learning curve; risk of pulling sim into Bevy world |
| **C. Godot 4 (GDExtension / Rust)** | Store pipelines mature; editor tooling | Two runtimes; FFI surface; Apple review quirks |
| **D. Keep macroquad + ship wrapper** | Least rewrite | Weakest store/inspection story; limited UI polish |

**Agent default for next architecture wedge:** shortlist review only —
expand stack A vs B with a one-page spike plan; **no** client rewrite
until owner picks.

## Wedge plan (proposed)

1. **Scope freeze** — this file + ROADMAP pointer (this PR).
2. **Owner pick** — ship target + stack A/B/C/D (or defer Phase 8).
3. **Contract sketch** — snapshot schema / command list (docs only).
4. **Spike** — empty client window + host tick pump (no gameplay).
5. **Parity gate** — Compact soak park=0 still green with host binary.

## Acceptance (proposed)

- [x] Named Phase 8 scope draft frozen (this file).
- [ ] Owner accepts ship target + stack shortlist (or parks Phase 8).
- [ ] Contract sketch landed before any client rewrite.
- [ ] No sim-mass regressions when a host/client split lands.

## Non-goals (until owner unlocks)

Full multiplayer, console ports, web WASM client, rewriting CA/mass into
the client, abandoning the macroquad soak viewer.

## Decision list (2026-10-12)

1. **Phase 8 scope draft in flight** — architecture note only.
2. Optional: E2 tipped grey-log owner visual (Phase 5 leftover).
3. Owner: pick stack / park Phase 8 / redirect.
