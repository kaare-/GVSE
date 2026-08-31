# Voxel fields / heatmaps for material physics — future plan

*Tentative. How coarse overlays could drive richer material behaviour
without abandoning the cell CA or importing `wk-field`. Companion to
[`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md) §6 / §9, [`VOXEL_WATER.md`](VOXEL_WATER.md),
and column ideas in [`organism/FIELDS.md`](organism/FIELDS.md).*

## Principle

**Cells stay the truth for mass.** Pore water, free water, solids, and
grain motion remain cell `material` + `sat`. Fields / heatmaps are
**derived or slow overlays** that *modulate* rates (evap, dissolve,
growth, failure) — they must not become a second water store.

Isolation stands: reimplement solvers inside `wk-voxel` (or grow the
existing `Humidity` / `Temperature` / `Heatmap` types). Do **not**
depend on `wk-field` / `wk-world`.

## What we already have

| Overlay | Role today | Material coupling |
|---------|------------|-------------------|
| Cell `sat` | Free water + pore water | porosity → capacity; permeability → seepage |
| `Humidity` | Atmospheric vapour tiles | Evap / rain / clouds / plant return |
| `Temperature` | °C tiles (Air / Surface / Buried) | `heat_capacity`, `albedo`; drives phase + cold avalanche |
| `Wind` | Climate + orographic helpers | Clouds / oro rain (not a pressure grid) |
| `Heatmap<T>` scaffold | Generic sparse patches | **Unused** by live climate (typed fields preferred so far) |
| `World::soft_litter` | Per-column fungus food | Digested before Organic cells |

Unused material knobs that fields could finally read: **`thermal_diffusivity`**,
**`solubility`**, **`cohesion`**, **`density`** (beyond grain settle).

Deliberately not ported from the column stack (migration §9): separate
moisture bucket, dissolved-mineral field, groundwater-head field.

## Candidate roadmap (ranked)

### 1. Wetness index (read-only) — first

Coarse heatmap: for each tile, average `sat / water_capacity` over
porous cells (and maybe free-water fraction in Air).

- **Feeds:** plant drink urge, fungus seat scores, HUD overlay, maybe
  snow stick / dust.
- **Does not** store water — rebuild from cells on a cadence (like temp
  period 20).
- **Risk:** low. No double-count if never written back into `sat`.

### 2. Wire `thermal_diffusivity` into Temperature

Today thermal uses a uniform `diffuse_alpha`. Column thermal already
scales α by material. Voxel should:

- Weight tile diffusion / buried lag by scanned `thermal_diffusivity`
  (and keep `heat_capacity` / `albedo` as now).
- Optional: anisotropic buried vs air tiles.

- **Risk:** low–medium. Cadence and determinism already exist; tune
  against current day/night feel.

### 3. Dissolved carbonate (slow field)

Column pattern: wet Limestone injects solute; field advects/diffuses;
reprecip or accelerates CA dissolve.

Voxel sketch:

- Coarse `Heatmap<f32>` or typed `Dissolved` tiles.
- Source term from karst-wet faces × `solubility` (voxel CA already
  dissolves limestone on a surface film and limestone/stone on
  groundwater; a dissolved field would modulate those rates).
- Sink: reprecip onto stone / reduce humidity? Prefer **modulating**
  `apply_karst_dissolution` probability/rate from local dissolved
  concentration rather than deleting cells from the field alone.

- **Risk:** medium. Easy to double-count with CA Limestone→Air. Keep
  CA as the mass of rock; field is chemistry state.

### 4. Pore-pressure / water-table sketch (careful)

Column has `GroundwaterHeadField` synced from `water_table_y`. Voxel
already has per-cell hydraulic head in the CA.

Possible uses without a second reservoir:

- Derive a **coarse head / pressure** overlay from saturated columns
  (for HUD, spring bias, landslide trigger).
- Let high pore-pressure tiles **boost** lateral spring weep / reduce
  cohesion for grain failure — still moving water via existing seepage
  / throughflow.

- **Risk:** high if the field is allowed to invent water. Treat as
  diagnostic + rate modulator only.

### 5. Nutrient / litter plume

Promote `soft_litter` (+ optional Organic density) into a coarse
heatmap for fungi / root foraging / HUD.

- **Risk:** low for hydro. Keep column soft_litter as source of truth
  or migrate carefully with save schema.

### 6. Shear and compressive failure (geotech CA)

We already have **ingredients**, not a failure model:

| Prop / rule | Mode it hints at | Today |
|-------------|------------------|--------|
| `repose_rise_m` + grain repose | **Shear** (angle of repose) | Landed for grains / snow / Organic |
| `cohesion` | Shear resistance (wet/dry) | Prop exists; unused in voxel |
| `density` + column height | **Compression** (overburden σᵥ) | Density used for grain settle only |
| `roof_span_max_m` | Compression / beam fail of roofs | Column burrows; **not** in voxel CA |
| Ice load break | Compression under debris | Thin ice lids only |
| Pore wetness / head | Both (effective stress) | CA `sat` + seepage; no σ′ field |

Treat the two modes separately — they look different in a side-view CA.

#### Compressive failure (crush / roof / compaction)

Trigger when **vertical effective stress** exceeds capacity:

```
σᵥ ≈ Σ (density × g × cell_height) above the cell
σ′ = σᵥ − u          (u from pore pressure / wetness)
fail if σ′ > f(cohesion, material class)
```

Outcomes (pick per material, keep mass local):

- **Roof collapse** — unsupported Air span under solid wider than
  `roof_span_max_m` → drop roof cells (→ LooseRock / Sand / Organic
  fill), open trench/doline (column burrow spirit).
- **Compaction** — soft sediment (Clay / Organic / wet Sand) under
  overburden: reduce porosity slightly or convert toward denser
  facies; squeeze pore `sat` upward/out (must conserve water).
- **Crush** — rare for Bedrock/Stone; mostly ice lids + later cave
  ceilings.

Derived overlay: coarse **overburden / σᵥ** heatmap (rebuild from
cells). Do **not** store a second mass — only gate the CA.

#### Shear failure (slide / slump / topple)

Trigger when **slope demand** exceeds strength:

```
demand ≈ local relief / run  (or unresolved repose debt)
strength ≈ tan(φ) + c'       (φ from repose_rise_m; c' from cohesion)
c' drops when wet / high pore pressure
fail if demand > strength
```

Outcomes:

- **Repose slide** — already: diagonal grain moves into Air.
- **Cohesive block fail** — Stone / Clay / Organic cliffs that repose
  alone will not touch: when shear demand is high, convert a face
  cell to LooseRock / Sand (or drop a 2×2 block) so repose can finish.
- **Lateral spread** — saturated terrace toes: high u + low c′ →
  boost spring weep + loosen bank grains (ties to side-seep work).

Derived overlay: coarse **slope / shear-demand** heatmap, or just
compute on the active dirty halo when checking faces.

#### How fields fit (same rule as water)

```
cells ──derive──► σᵥ, wetness/u, slope demand
                      │
                      ▼
              fail? → CA writes (drop, loosen, compact)
```

- Overlays **modulate**; cells still move the mass.
- Wetness / pore-pressure (candidates 1 and 4) feed **effective
  stress** and wet cohesion loss — that is the main field coupling.
- Cadence: failure pass **once per tick** (or every N), after CA
  water + grain, not inside flow ×12.

#### Implementation plan

Concrete phases, APIs, tick placement, and tests:
[`VOXEL_FAILURE.md`](VOXEL_FAILURE.md). Slow stress maps:
[`VOXEL_GEOTECH_MAP.md`](VOXEL_GEOTECH_MAP.md).

Short order: roof collapse → wet cohesion shear → geotech map (`G`) →
compaction → map-gated thin-dam shear.

#### Non-goals

- Full FEM / continuum plasticity
- Inventing water from compaction without pushing `sat` into neighbours
- Making Bedrock shear-fail in normal play

### 7. Air pressure (research)

Powder Toy–style gas. Migration already flags Air as first-class;
voxel wind is climate-scale mean + natural variance, plus a **local tile
heatmap** (terrain / thermal ∇T / swirl) rebuilt every
`WIND_FIELD_PERIOD` ticks on occupied humidity seats + a 1-tile halo + a
thin near-surface band. Humidity uses per-tick fractional flux through
`vector_at` (free-air height cached per column). Temperature stays on
period 20. Sun and night-sky radiation hit the **ground**; air sits on
the climate lapse and couples to that skin (no day/night sky swap).
Each air tile then upwind-mixes heat along the local wind. Warm
humid skin under colder air is the draft that lofts vapour. The T overlay uses a fixed −40..36 °C ramp (ice-white through
yellow-green at 18 °C to red). Humidity hold follows
Clausius–Clapeyron (Magnus): full tile at 40 °C, a few percent at
0 °C, a trace at −100 °C. Cloud / haze floors follow the live
column, not a global sea-level y. No pressure solver — defer cell
wind until hydro + thermal are boring.

## Architecture notes

```
cells (material, sat) ──derive──► wetness / head overlays
        │                              │
        │                              ▼
        ▼                         modulate rates
   CA rules (seepage, karst,          │
   grain, phase, springs) ◄───────────┘
        ▲
        │
   Temperature / Humidity (slow overlays)
```

- **Cadence:** fields step slower than flow substeps (temp/humidity
  already period-20). Rebuild derived overlays after the CA tick, not
  inside each flow substep.
- **Parallelism:** tile/column field steps fit Phase 2 of
  [`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md).
- **Save/load:** new overlays need `SimSnapshot` schema bump or
  `#[serde(default)]` + rebuild-on-load for derived layers.
- **Debug:** H/T overlays today; add wetness / dissolved toggles the
  same way.

## Suggested first slices

1. **Wetness heatmap** — derive, overlay key, optional plant bias.  
2. **Thermal diffusivity** — material-weighted `Temperature::step`.
   Climate coupling already live: evap × T/wind/deficit, thermal
   buoyant rise, condensation/dew vs saturation-at-T (still 4×4 tiles).  
3. **Roof-span compressive collapse** — voxel cavities use
   `roof_span_max_m` (readable caves / overhangs).  
4. **Wet cohesion shear** — pore fill weakens banks / cliffs via
   `cohesion` before a full stress field.  
5. **Dissolved + solubility** — after wetness/thermal feel solid.

## Non-goals

- Importing `wk-field` into `wk-voxel`
- Replacing pore `sat` with a moisture field
- Running field writes inside checkerboard gravity colours
