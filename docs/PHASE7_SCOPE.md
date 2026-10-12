# Phase 7 — Surface spice (atmosphere & light) *(wedge C)*

Wedges A–B closed (2026-10-12). **Wedge C accepted by default
(2026-10-12):** season dial — year-cycle day/night length for sky draw,
seasonal sky tint, Tab dial + auto. Presentation / climate helpers;
sim step keeps base `ClimateConfig` (no sim-mass reopen).

## Intent

Presentation that makes the ring readable over a day / season without
reopening TRACKED / park or sim-mass budgets. Prefer app/render layers
over sim rewrites. Kernel stays MS Paint 1×.

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| Celestial polish (A) | Live | [`SKY.md`](SKY.md) |
| Sundown cast / terrain lee (B) | Live | [`SKY.md`](SKY.md) |
| Day/night climate clock | Live | `ClimateConfig` |
| Phase 6 org cost / overlay GC | Live | [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md) |

## Wedge C slice plan

1. **Season clock** — `season_fraction_cfg` over `SEASON_CYCLE_DAYS`
   climate days (spring→summer→autumn→winter).
2. **Sky-only day length** — `climate_with_season` remaps day/night
   for sun/moon/sky draw; organism/temp keep base config.
3. **Seasonal sky tint** — cool winter / warm summer / amber autumn /
   soft spring wash on the sky gradient.
4. **Tab dial** — auto season from tick, or manual 0..1 dial.
5. **Xvfb gate** — winter vs summer stills under `/opt/cursor/artifacts/`.

## Acceptance (proposed)

- [x] Wedges A–B closed.
- [x] Wedge C locked (agent default).
- [x] Slices 1–4 landed with unit tests where cheap.
- [x] Xvfb winter/summer pair under `/opt/cursor/artifacts/`.
- [x] Short soak park=0 (3000t: TRACKED −0.02/t, min +0.02/t, park=0).

## Landed (wedge C)

| Slice | Where |
|-------|--------|
| Season clock | `season_fraction_cfg` / `SEASON_CYCLE_DAYS` |
| Sky-only day length | `climate_with_season` → draw path |
| Seasonal sky tint | `apply_season_sky_tint` + winter star density |
| Tab dial | auto + manual; `GVSE_FORCE_SEASON` for stills |

### Xvfb evidence (2026-10-12)

| Still | Season | Reads |
|-------|--------|-------|
| `phase7-season-01-summer.png` | 0.25 | Warmer/brighter day sky |
| `phase7-season-02-winter.png` | 0.75 | Cooler day sky |

## Non-goals

Water glitter (D), Phase 8 client, seasonal leaf drop / wood rings,
sim temperature season amp — see [`ROADMAP.md`](ROADMAP.md).

## Decision list (2026-10-12)

1. **Wedge C closed for agent** — season dial + Xvfb + soak; default
   next wedge **D** (water glitter) or owner pick.
2. Optional: E2 tipped grey-log owner visual (Phase 5 leftover).
3. Optional: wedge D after owner redirect.
