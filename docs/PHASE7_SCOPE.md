# Phase 7 — Surface spice (atmosphere & light) *(wedge A)*

Phase 6 agent slices parked (2026-10-12). **Wedge A accepted by
default (2026-10-12):** celestial polish — richer sunrise/sunset, moon
phase/cycle, night stars that drift. App/render + climate helpers;
no sim-mass changes.

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

## Wedge A slice plan

1. **Sunrise/sunset palette** — stronger dusk/dawn stops; dawn vs dusk
   split via climate phase; warm horizon lift.
2. **Moon phase** — multi-night lunar cycle drives crescent bite
   (new → full → new).
3. **Night stars** — sparse fine-px stars with slow drift while
   `day_night < 0`.
4. **Xvfb gate** — stills at dusk / midnight / dawn under
   `/opt/cursor/artifacts/`.

## Acceptance (proposed)

- [x] Named Phase 7 scope frozen (this file).
- [x] Wedge A locked (agent default).
- [x] Slices 1–3 landed with unit tests where cheap.
- [x] Xvfb dusk / midnight / dawn stills under `/opt/cursor/artifacts/`.
- [ ] No sim-mass regressions (short soak park=0 — in flight).

## Landed (wedge A)

| Slice | Where |
|-------|--------|
| Sunrise/sunset palette | `climate::sky_rgb_phased` / `sky_rgb_at_height_phased` + weather_phased |
| Moon phase crescent | `moon_phase_cfg` → app `draw_celestials` bite |
| Night stars | `atmosphere::draw_night_stars` from `draw_sky` |

### Xvfb evidence (2026-10-12)

| Still | Tick (approx) | Reads |
|-------|---------------|-------|
| `phase7-celestial-11-dusk.png` | ~315 | Amber/orange sky + sun |
| `phase7-celestial-13-midnight.png` | ~626 | Dark sky + stars + crescent moon |
| `phase7-celestial-16-morning.png` | ~887 | Rose/salmon dawn + sun/moon handoff |

## Non-goals

New Set B/C modules, Phase 8 client split, cast shadows (wedge B),
season dial (C), water glitter (D) until A parks — see
[`ROADMAP.md`](ROADMAP.md).

## Decision list (2026-10-12)

1. **Wedge A nearly closed** — short soak park=0, then park A / pick B–D.
2. Optional: E2 tipped grey-log owner visual (Phase 5 leftover).
3. Optional: wedges B/C/D after A parks or owner redirects.
