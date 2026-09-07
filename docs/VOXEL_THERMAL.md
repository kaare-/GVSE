# Thermal loop — heat transport before pressurized vapour

**Status:** plan (blocks richer geyser / “humidity under pressure” work).
**Crate:** `wk-voxel`. App: `wk-voxel-app`.
**Goal:** coarse **thermal loops** that move heat with water (and later
pore water), so gradients can drive currents — not a detailed CFD heat
model.

Read first: [`VOXEL_WATER.md`](VOXEL_WATER.md), [`VOXEL_WEATHER.md`](VOXEL_WEATHER.md),
[`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) (§2 diffusivity), [`VOXEL_GEYSER.md`](VOXEL_GEYSER.md).

Companion restart tip: underground vapour stays sparse and **off** the sky
Humidity store ([`VOXEL_GEYSER.md`](VOXEL_GEYSER.md) sky-path lock).

---

## Why this exists

Geysers / hot springs need a believable heat story:

```text
hot rock → warms water → warm water rises → heat reaches surface /
vents / open air → cold water (and cold air) sink / contact → rock cools
```

Today we have tile `Temperature` (solar, geothermal, uniform diffuse α),
sky humidity weather, and sparse underground vapour — but **almost no
rock↔water heat exchange** and **no water convection**. Cold lakes do not
quench hot rock; warm water does not rise. Pressurized vapour on top of
that would look fake and fight FPS for no physical gain.

This doc locks a **coarse** path: get heat *moving with water*, then
extend the same couples into pores. Detail is optional later.

---

## Locked decisions

| Topic | Decision |
|-------|----------|
| Fidelity | **Coarse couples only.** Tile T + existing water CA. No cell-T grid, no Navier–Stokes, no continuum pore-pressure PDE. |
| Purpose | Produce **gradients that move water** (currents / biased flow), and close a **thermal loop** with the surface and air. |
| Diffusivity | Wire material `thermal_diffusivity` into Temperature diffusion / buried lag (today: uniform `diffuse_alpha`). Keep `heat_capacity` / `albedo` as now. |
| Rock ↔ free water | Coarse heat exchange where wet Air (standing water) contacts solids / sits in a watery column: water takes heat from hot rock; cold water cools rock. |
| Rock ↔ pore water | Same idea, later wave: wet porous cells exchange with their host / neighbour rock using the same knobs, smaller rates. |
| Water convection | Bias existing vertical / confined / flow exchange by ΔT (warm up, cold down). Not a second fluid solver. |
| Air ↔ water surface | Cold air cools the free-water skin (and reverse); reuse near-surface air↔ground couple patterns already in `temperature.rs`. |
| Sky Humidity | Weather store only. **Open caves** should share moist air with the sky (continuity fix — separate PR). **Sealed** cavities keep pressurized vapour on the sparse store — never dump into rain lottery. |
| Boil / evap | Overground “boil” is **fast evaporation** into Humidity when water contacts hot material. Sealed flash stays on the sparse underground store until a vent opens. |
| Geyser gating | No new sealed-pressure / hot-humidity assault work until T0–T2 below are landed and demo FPS still holds. |

### Hard no’s

- No world-wide heat PDE finer than the existing tile field.
- No second temperature store per cell.
- No merging underground boil into sky Humidity “to make it one vapour.”
- No Darcy pore-pressure continuum as a prerequisite (local rate bias later is enough).

---

## The loop (what “done” feels like)

```text
                 solar / geothermal
                        │
                        ▼
              ┌──── hot rock / buried T ────┐
              │                             │
     cold water contact              warm water rises
     cools rock (couple)             (ΔT flow bias)
              │                             │
              ▼                             ▼
         cooler rock                  heat to surface /
         draws less flash             vents / open water
                                              │
                         cold air cools water skin
                                              │
                                              ▼
                                    denser/cooler water sinks
                                    (and re-contacts rock)
```

Acceptance sketches (tests / HUD, not photoreal):

1. Hot buried column + cold pond above → pond warms; rock under it cools over thermal steps.
2. Stratified lake / shaft → warm free water tends upward, cold downward (bias visible vs control).
3. Night / cold air over a warm lake → surface tile cools faster than an insulated control.
4. Later: wet pore column shows the same rock↔water couple at lower rate.

---

## Phased plan

### T0 — Thermal honesty (tiles)

- Weight Temperature diffusion / buried relax by scanned
  `thermal_diffusivity` (see [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) §2).
- Keep cadence (`TEMP_STEP_PERIOD`) and determinism.
- Prove day/night and geothermal still feel sane.

### T1 — Coarse rock ↔ free-water couple

- On thermal step (or a matching cadence): for watery columns / wet Air
  seats, exchange heat between water-bearing tiles and neighbouring /
  underlying solid surface tiles using `heat_capacity` (and diffusivity
  as a rate scale).
- Mass stays in cells; only °C moves.
- Goal: **cold water cools hot rock**; hot rock warms water.

### T2 — Water currents from ΔT

- Bias vertical free-water exchange and/or confined rise / fall by local
  ΔT (warm prefers up). Small dimensionless knobs; Tab-tunable.
- Optional: slight horizontal mixing where strong lateral ∇T exists —
  only if free with existing flow neighbourhoods.
- Goal: **heat rides water toward the surface**; return flow of cooler
  water.

### T3 — Air ↔ water skin

- Strengthen near-surface couple so cold air cools open water (and warm
  water can warm a thin air band). Same coarse tile math.
- Closes the surface half of the loop with weather already in
  [`VOXEL_WEATHER.md`](VOXEL_WEATHER.md).

### T4 — Extend couples to pore water

- Wet porous solids exchange with host/neighbour rock tiles at a reduced
  rate (pore fraction × diffusivity).
- Seepage can optionally carry a tiny heat bias with mass (still tile T,
  not per-cell enthalpy).
- Goal: geothermal heat reaches seepage paths; cold recharge cools
  aquifers.

### T5 — Open-cave humidity continuity (weather, not pressure)

- Fix: sky Humidity should occupy **open** caves / shafts connected to
  free air (stop crest-hoisting those seats).
- Sealed / roofed cavities remain outside the rain lottery (sparse
  pressurized vapour when we return to geysers).

### T6 — Return to pressurized vapour / geysers

Only after T0–T2 (ideally T3–T4) and FPS gate:

- Evap continuum: contact with hot material → fast evaporization into
  **Humidity** when open to sky.
- Sealed flash → sparse underground vapour; pressure → reverse pore
  seepage + escape (existing geyser motor), cool → condense.
- Rename/docs: “humidity under pressure” = sealed sparse store, not sky H.

See [`VOXEL_GEYSER.md`](VOXEL_GEYSER.md).

---

## Performance

- Prefer work on the existing thermal cadence (period ~20) and dirty /
  watery columns — not every cell every tick.
- ΔT flow bias must be cheap additives on paths we already run.
- Same interactive gate as geyser: if wet FPS is already bad, stop and
  cut water/confined cost first ([`VOXEL_GEYSER.md`](VOXEL_GEYSER.md)
  performance section).

```bash
cargo test -p wk-voxel --test perf_profile --release -- --ignored --nocapture
```

---

## Relation to other docs

| Doc | Relation |
|-----|----------|
| [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) | Diffusivity wiring ranked there; this doc owns the **loop + convection** story. |
| [`VOXEL_WATER.md`](VOXEL_WATER.md) | Free water + pore `sat`; currents bias those CA paths. |
| [`VOXEL_WEATHER.md`](VOXEL_WEATHER.md) | Sky H / rain; open-cave continuity (T5); air↔water skin (T3). |
| [`VOXEL_GEYSER.md`](VOXEL_GEYSER.md) | Geysers wait on this loop; sparse vapour stays off sky H. |

---

## Non-goals (this track)

- Per-cell temperature or enthalpy CA.
- Full equation-of-state density / salinity convection.
- Pore-pressure PDE or world-wide vapour grid.
- Replacing geothermal or solar with a new climate model.
