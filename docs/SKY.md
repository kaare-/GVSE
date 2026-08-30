# Sim-linked sky / atmosphere

Design record for the background stack in `wk-voxel-app`. Goal: leave
the Powder Toy “empty canvas” look without inventing a second weather
physics. The far field is an **instrument** for signals the sim already
computes.

## Draw order

```text
sky gradient                     weather / temp / carbon; smooth day phases
sun / moon (soft, fine px)       behind far ridge; soft crest reveal
far / near ridge fills           XY parallax; sky-washed; soft crest feather
active parcel banks (N)          humidity echo CloudStore — no depth animation
terrain + standing water         night: deep cool darken + weak moon ambient
day canopy shade                 under-surface dim + air corridor + sun cast
humidity tile diagnostic          H overlay (diagnostics, not “clouds”)
wind streak overlay               V overlay (off by default)
debug overlays → organisms
night moon cast                  after organisms
HUD
```

Constants live in [`crates/wk-voxel-app/src/atmosphere.rs`](../crates/wk-voxel-app/src/atmosphere.rs).

## Controls (do not conflate)

| Key | What |
|-----|------|
| **N** | Soft **active** cloud banks (parcel echo). Far/mid/front depth layers and wall-clock scroll are off by default for FPS; Tab can still raise vapour_far/mid/front if wanted. |
| **H** | Humidity **tile raster** diagnostic |
| **V** | Wind streak overlay (placeholder; default off) |
| **F6** | Glossary — keys, water/sky words, HUD tags |

Humidity still **drives** the weather, now with temperature/wind:
evap(T, wind) → thermal rise → drizzle when vapor meets colder air /
ground. `N` draws a capped visual echo of the wettest sky tiles (not a
second water store, and not a rain engine). Depth-echo banks used to
triple the lobe-mask stamp cost every frame; they are optional now.

## Signal → visual map

| Signal | Visual |
|--------|--------|
| Humidity → parcels | Soft active cloud banks (`N`); depth echoes optional via Tab |
| Humidity tiles | Diagnostic haze overlay (`H`) — bilinear wash on 4×4 seats with **16** opacity steps; a drop opens that column from itself downward |
| Day/night | Sky lerp + sun/moon; night landscape darken |
| Wind | `V` streaks from local field; no front-cloud wall-clock scroll |
| Ridges | Dual parallax fills from **ground** height (not falling snow, not mid-air wet Air) |
| Cast / celestial key | See prior plant/terrain lighting notes |

`RidgeSilhouette` walks from `continental_surface_y`, skipping anything
that `falls_through_empty_air` (snow, ice, loose organic). The old
ceiling scan treated a falling flake as the crest — a 1–2 px needle —
and the 30-tick cache kept the spike up until the flake landed. Wet Air
only counts at/below sea or as standing water; humidity tiles never live
in these cells.

## `sky_transmit`

```
lit = column_sky × day_factor × cloud_sky_transmit(x)
         × (1 − 0.1 · humidity_norm) × sun_sky_transmit(x, y, sun_local)
```

## Non-goals

- Treating the humidity raster as decorative clouds
- Second weather sim for background layers
- Volumetric sun beams
