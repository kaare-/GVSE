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
- Source term from karst-wet faces × `solubility` (stop hard-coding
  Limestone-only when props are ready).
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

### 6. Stress / compaction (later)

Overburden from `density` + stack height; `cohesion` resists. Could
gate cave collapse / soft sediment compaction. Weakest fit until we
have a failure CA worth driving.

### 7. Air pressure (research)

Powder Toy–style gas. Migration already flags Air as first-class;
voxel wind is still climate-scale. Defer until hydro + thermal are
boring.

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
3. **Dissolved + solubility** — only after wetness/thermal feel solid.

## Non-goals

- Importing `wk-field` into `wk-voxel`
- Replacing pore `sat` with a moisture field
- Running field writes inside checkerboard gravity colours
