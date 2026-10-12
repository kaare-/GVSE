# Phase 7 — Surface spice (atmosphere & light) *(scope draft)*

Phase 6 agent slices parked (2026-10-12): organisms-on baselines,
stream+plants, hotspot #1, overlay GC, 5k soak green. Hotspot #2
(land/metab) stays deferred. See
[`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md).

This file is a **decision draft** — no presentation code until the
wedge is accepted.

## Intent

Presentation that makes the ring readable over a day / season without
reopening TRACKED / park or sim-mass budgets. Prefer app/render layers
over sim rewrites. Kernel stays MS Paint 1×.

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| Day/night sky lerp + sun/moon screen | Live | [`SKY.md`](SKY.md) |
| Celestial local / sun sky transmit | Live | `climate` + `atmosphere_metrics` |
| Night landscape cool darken + moon ambient | Live | [`SKY.md`](SKY.md) |
| Clouds / precip / humidity | Live | [`VOXEL_WEATHER.md`](VOXEL_WEATHER.md) |
| Canopy shade (organisms) | Live | [`organism/LIGHT.md`](organism/LIGHT.md) |
| Phase 6 org cost / overlay GC | Live | [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md) |

## ROADMAP wishlist (from [`ROADMAP.md`](ROADMAP.md))

- Colors; sunrise / sunset polish
- Moon + moon cycle
- Seasons
- Stars that move over time and seasons
- Shadows / shading from sundown
- Moonlight glitter on water

## Proposed wedges (pick one)

1. **Default: celestial polish (A)** — richer sunrise/sunset palette,
   moon phase / cycle, night stars that drift. Mostly `wk-voxel-app`
   draw + existing celestial helpers; Xvfb screenshots as gate.
2. **Terrain cast shadows (B)** — sundown column / ridge cast into the
   pixel landscape (heavier; couple carefully with FPS budget).
3. **Season dial (C)** — calendar length / leaf tint / snow bias without
   new organism genes (docs + climate knobs first).
4. **Water glitter (D)** — moonlight / low-sun sparkle on standing water
   (app-only; after A if night read is weak).

## Acceptance (proposed)

- [x] Named Phase 7 scope frozen (this file).
- [ ] Owner: accept a wedge (default A) or park Phase 7 / reopen E2 visual.
- [ ] At least one Xvfb / playtest acceptance for the chosen wedge.
- [ ] No sim-mass regressions (short soak park=0 still green).

## Non-goals

New Set B/C modules, Phase 8 client split, reopening Phase 6 land/metab
micro-cuts that fail river growth — see [`ROADMAP.md`](ROADMAP.md).

## Decision list (2026-10-12)

1. **Default next: wedge A (celestial polish)** — sunrise/sunset + moon
   cycle + stars; prove on Xvfb.
2. Optional: E2 tipped grey-log owner visual (Phase 5 leftover).
3. Optional: wedges B/C/D only after A lands or owner redirects.
