# Phase 7 — Surface spice (atmosphere & light) *(wedge B)*

Wedge A closed (2026-10-12): celestial polish + Xvfb + 3k soak park=0.
**Wedge B accepted by default (2026-10-12):** cast shadows / sundown
shading — longer plant lee + stronger terrain occlusion + warmer key
at low sun. App/render only; no sim-mass changes.

## Intent

Presentation that makes the ring readable over a day / season without
reopening TRACKED / park or sim-mass budgets. Prefer app/render layers
over sim rewrites. Kernel stays MS Paint 1×.

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| Day/night sky + moon phase + stars | Live (A) | [`SKY.md`](SKY.md) |
| Plant/creature cast + under-canopy dim | Live | `atmosphere::draw_canopy_air_dim` |
| Terrain celestial key / exposure | Live | `terrain_celestial_key_strength` |
| Night moon cast after organisms | Live | [`SKY.md`](SKY.md) |
| Phase 6 org cost / overlay GC | Live | [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md) |

## Wedge B slice plan

1. **Low-sun cast boost** — plant lee longer/stronger near horizon
   (dusk / dawn elevation).
2. **Terrain lee polish** — `celestial_exposure` reaches farther and
   occludes harder at low sun so ridge faces read as shaded.
3. **Warm sundown key** — day rim highlight leans amber when the sun
   is low (pairs with A palette).
4. **Xvfb gate** — stills near dusk with readable long lee under
   `/opt/cursor/artifacts/`.

## Acceptance (proposed)

- [x] Wedge A closed.
- [x] Wedge B locked (agent default).
- [x] Slices 1–3 landed with unit tests where cheap.
- [ ] At least one Xvfb dusk cast still.
- [ ] No sim-mass regressions (short soak park=0 still green).

## Landed (wedge B code)

| Slice | Where |
|-------|--------|
| Low-sun cast boost | `stamp_celestial_cast_shadows` via `low_sun_factor` |
| Terrain lee polish | `celestial_exposure` longer/harder at low sun |
| Warm sundown key | `apply_celestial_key_rgb` amber lean + elev_k boost |

## Non-goals

Season dial (C), water glitter (D), Phase 8 client, new Set B/C
modules, volumetric god-rays — see [`ROADMAP.md`](ROADMAP.md).

## Decision list (2026-10-12)

1. **Wedge B in progress** — sundown cast / terrain lee.
2. Optional: E2 tipped grey-log owner visual (Phase 5 leftover).
3. Optional: wedges C/D after B parks or owner redirects.
