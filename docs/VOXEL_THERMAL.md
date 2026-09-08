# Thermal loop — heat transport before pressurized vapour

**Status:** T0–T5 + T2b landed. **Humidity model:** sky H (incl. open caves) /
sparse sealed `cave_humidity` / pressurized cavity humidity (`steam` wire). Open hot water is
accelerated evap into sky H. T6b/P4 geyser jet still FPS-gated.
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
sky humidity weather, and sparse underground vapour — and rock↔water couples exist; pressurized cavity flow now also **advects heat** with reverse seep so conduits warm over time. Cold lakes do not
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
| Sky Humidity | Weather store. **Open caves / overhangs** share it (T5 crest-hoist / flux skip + open-to-sky class). Unroofed hot water is ordinary **accelerated evaporation** into this store — not a special steam flash. |
| Cave humidity | Sparse `World.cave_humidity` — ambient moist Air in **sealed** voids under rock. Not pressurized, not the rain lottery. Same soft white `H` wash as sky humidity. |
| Cavity humidity (pressure) | Sparse `World.steam` wire — **closed / semi-closed cavity humidity under pressure** for geysers. Hot water/vapour **carry heat** into colder rock; density-driven push continues below 100 °C. Never dumps into sky H. |
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
4. Wet pore column shows the same rock↔water couple at lower rate.

---

## Phased plan

### T0 — Thermal honesty (tiles) ✅

- Weight Temperature diffusion / buried relax by scanned
  `thermal_diffusivity` (see [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) §2).
- Keep cadence (`TEMP_STEP_PERIOD`) and determinism.
- Prove day/night and geothermal still feel sane.

### T1 — Coarse rock ↔ free-water couple ✅

- On thermal step: watery surface tiles mix toward capacity-weighted
  equilibrium with the tile below (`TempConfig::water_rock_couple`).
- Mass stays in cells; only °C moves.
- Goal: **cold water cools hot rock**; hot rock warms water.

### T2 — Water currents from ΔT ✅

- Bias confined rise by ΔT (`water_convect_rise_scale`: warm donor under
  cooler destination → faster).
- Soft-throttle Air→Air gravity fall when warm sits over cold
  (`water_convect_fall_scale`).
- **Open lakes:** tile °C buoyancy mix when warm free water sits under
  colder free water (`couple_free_water_buoyancy`) — full columns do not
  move cells, so flow bias alone cannot overturn stratification.
- Deep free-water tiles use water capacity (not rock geothermal).
- Geothermal overburden is **rock surface only** — standing water does
  not count as crust cover (avoids painting lakes as a static hot bed).
- Diffuse is gated at free-water ↔ rock faces so cliff geo isotherms are
  not copied sideways into the lake (cut-hill banding ≠ lake T).
- Tab: `TempConfig::water_convect_bias` (default 0.35).
- Goal: **heat rides water toward the surface**; cooler return flow.

### T2b — Lake circulation loop (skin + wind + paired rise/sink) ✅

Closes the open-water half of the thermal loop for play readability:

- **Night quench:** warm lake under cold air couples harder
  (`couple_air_water_skin`) so the skin cools enough to overturn.
- **Buoyancy:** unstable free-water columns mix with a directed warm-up /
  cold-down push (`couple_free_water_buoyancy`, slightly lower ΔT gate).
- **Wind stress:** near-surface free-water heat drifts downwind
  (`couple_free_water_wind_drift`, Tab `water_wind_drift`).
- Day still heats watery surface tiles via solar × (1 − cloud shade);
  night is sun off + radiate + air skin — same knobs as T0/T3.
- **`V` overlay:** column-relative buoyancy (warm↑ cold↓) + skin wind
  stress, free-water neighbours only, wrapped camera box.

Not CFD: tile °C + existing CA bias. Return flow is the cold anomaly
sinking while warm rises / skin drifts.

### T3 — Air ↔ water skin ✅

- Watery surface tiles mix with the Air tile above
  (`TempConfig::air_water_skin_couple`, default 0.18).
- Cold air cools the water skin; warm water warms a thin air band.
- Complements the existing one-way air→ground near-surface couple.
- Closes the surface half of the loop with weather already in
  [`VOXEL_WEATHER.md`](VOXEL_WEATHER.md).

### T4 — Extend couples to pore water ✅

- Wet porous solids exchange with neighbour rock/surface tiles
  (`TempConfig::pore_water_couple`, default 0.06).
- Rate scales by mean pore wetness × diffusivity; free-water surfaces
  stay on the T1 path.
- Props scan stores `pore_wet` and a small capacity bump from wet pores.
- Goal: geothermal heat reaches seepage paths; cold recharge cools
  aquifers.

### T5 — Open-cave humidity continuity (weather, not pressure) ✅

- Crest-hoist / vertical flux snap skip seats whose tile-centre Air is
  sky-connected (`air_void_open_to_sky`: upward probe + short Air BFS).
- Open shafts / vented caves / cliff overhangs keep weather Humidity
  (rain lottery eligible) — same store as free sky.
- Sealed / roofed cavities still hoist off the under-crest seat (stay
  outside the lottery; ambient moisture → `cave_humidity`, pressure → steam).

**Walk cost:** sky-open class is **not** a second humidity grid. Cheap path
is an upward column probe; only roofed seats pay a short Air BFS
(`BFS_BUDGET` 96) looking for a side entrance / skylight. Sealed pockets
return false and never enlarge the weather walk.

### T6 — Return to pressurized vapour / geysers

**Humidity stores (landed):**

1. **Sky Humidity** — weather; open caves / overhangs share it (T5).
   Boiling under open sky is ordinary **accelerated evaporation** into this
   store — nothing special vs a hotter lake.
2. **`World.cave_humidity`** — second mini field, **underground sealed air
   only**. Conventional closed cave under rock: ambient moist Air, sparse +
   hard-capped. Cool surplus (above Magnus cell capacity) recondenses into
   Air sat; seats that open to sky hand off to weather H. **Not** pressure,
   **not** rain lottery.
3. **`World.steam`** — roofed flash / geyser pressure only. Never dumps
   into sky H.

**Open hot water:** accelerated film evaporization into sky Humidity
(evap climate rate ceiling rises near boil). Steam does **not** special-case
open seats.

**Pore pressure (sparse):** above 100 °C, phase-expansion drive spikes with heat (~×3 by +80 °C, not 1700×). Reverse seep follows highest permeability (path of least resistance) and can vent as a warm spring that drops Flowstone.

**Roofed flash:** free / pore water ≥100 °C under a roof may mint steam
(P3 pressure motor).

**T6b / P4 (FPS-gated):** episodic geyser jet from escape tubes.

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
