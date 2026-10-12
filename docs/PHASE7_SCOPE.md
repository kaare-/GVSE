# Phase 7 — Surface spice (atmosphere & light) *(wedge D)*

Wedges A–C closed (2026-10-12). **Wedge D accepted by default
(2026-10-12):** moonlight / sun glitter on free water — soft sheen +
sparse sparkle on lit water tops. App/render only; no sim-mass changes.

## Intent

Presentation that makes the ring readable over a day / season without
reopening TRACKED / park or sim-mass budgets. Prefer app/render layers
over sim rewrites. Kernel stays MS Paint 1×.

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| Celestial polish (A) | Live | [`SKY.md`](SKY.md) |
| Sundown cast / terrain lee (B) | Live | [`SKY.md`](SKY.md) |
| Season dial sky-only (C) | Live | [`SKY.md`](SKY.md) |
| Terrain celestial key on water | Live | `terrain_atlas` + `terrain_celestial_key_strength` |

## Wedge D slice plan

1. **Water glitter helper** — sheen + sparse flecks from celestial key /
   tick hash (warm day / cool night).
2. **Atlas wire** — apply on exposed free-water tops in
   `fill_terrain_atlas`.
3. **Tab dial** — glitter strength under Sky look.
4. **Xvfb gate** — day water + night water stills.

## Acceptance (proposed)

- [x] Wedges A–C closed.
- [x] Wedge D locked (agent default).
- [x] Slices 1–3 landed with unit tests where cheap.
- [ ] At least one Xvfb water glitter still.
- [ ] No sim-mass regressions (short soak park=0 still green).

## Landed (wedge D code)

| Slice | Where |
|-------|--------|
| Glitter helper | `apply_water_glitter_rgb` |
| Atlas wire | `fill_terrain_atlas` on exposed free water |
| Tab dial | Sky look → Water glitter |

## Non-goals

Phase 8 client, seasonal leaf drop, sim temperature season amp,
volumetric caustics — see [`ROADMAP.md`](ROADMAP.md).

## Decision list (2026-10-12)

1. **Wedge D in progress** — water glitter.
2. Optional: E2 tipped grey-log owner visual (Phase 5 leftover).
3. Phase 7 park after D closes (or owner redirects).
