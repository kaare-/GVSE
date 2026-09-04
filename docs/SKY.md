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
terrain + standing water         night: deep cool darken + weak moon ambient
day canopy shade                 humidity column dim + sun cast
humidity vapour wash             H overlay (default on; the sky water look)
wind lattice arrows               V overlay (off by default; coarse, short)
debug overlays → organisms
night moon cast                  after organisms
HUD
```

Constants live in [`crates/wk-voxel-app/src/atmosphere.rs`](../crates/wk-voxel-app/src/atmosphere.rs).

## Controls (do not conflate)

| Key | What |
|-----|------|
| **H** | Humidity **tile raster** — the vapour look. Tab → Climate → Wind + humidity: resample button (bilinear vs 4×4 tiles) and min-mass slider. |
| **V** | Wind lattice — coarse local-field arrows (default off) |
| **F6** | Glossary — keys, water/sky words, HUD tags |

Humidity **is** the weather store, now with temperature/wind:
evap(T, wind) → thermal rise → drizzle when vapor meets colder air /
ground. There is no `N` bank layer and no derived cloud deck. Shade
(`cloud_sky_transmit`) reads the same tiles. CPU stays on 4×4 tiles.
`H` / `V` / `T` skip paint for tiles that cannot touch the viewport
(probe the camera tile box when it is smaller than the vapour / T
map). `U` / `M` / `G` scan only visible world-x. The sim field still
runs at full rate off-screen (ring + pan). Not a quadtree.

## Signal → visual map

| Signal | Visual |
|--------|--------|
| Humidity tiles | Vapour wash (`H`) — bilinear on 4×4 seats; a drop opens that column from itself downward |
| Wet columns | Canopy / column shade via `cloud_sky_transmit` (humidity, not parcels) |
| Day/night | Sky lerp + sun/moon; night landscape darken |
| Wind | `V` overlay — coarse local-field arrows (default off) |
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

- A second cartoon-bank / parcel / derived-deck layer on top of humidity
- Second weather sim for background layers
- Volumetric sun beams
- Bringing the `N` overlay back
