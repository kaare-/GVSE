# Petri fields

*Frozen list of scalar / vector fields the Organism Kernel needs,
each mapped to an existing GVSE slot where possible. `petri-fields`
in the Organism Kernel plan.*

## Reuse policy

For every field below, the freeze names either:

- **REUSE** — the existing GVSE hook that already carries this data.
- **NEW** — a new field slot that follows the same pattern as
  existing ones in
  [`crates/wk-field/`](../../crates/wk-field/).

Preference is always REUSE — the petri and the GVSE world are the
same substrate.

## Field list

### Light

- **Kind:** per-column remaining-light array.
- **Slot:** NEW. `Simulation` scratch `light_remaining:
  BTreeMap<i32, Vec<f32>>`, one `Vec<f32>` per active column,
  indexed top-down.
- **Feeds:** `Photosystem` harvest, epiphyte perch decisions,
  overlay drawer.
- **Producer:** `run_shade` post-barrier, every tick (see
  [`LIGHT.md`](LIGHT.md)).
- **Not** stored on `Column` — the arrays would balloon save files.
  Rebuilt from geometry each tick; no `#[serde]` needed.
- **Not** in the mass audit — light is a bookkeeping scalar.

### Temperature

- **Kind:** 2D scalar field per chunk.
- **Slot:** REUSE. `ThermalField` on `Chunk`, already implemented,
  gated by `World::thermal_fields_enabled`.
- **Feeds:** `TempTolerance` module, plant / fungus growth curves,
  phase change (existing).
- **Producer:** `run_thermal_field` (existing).

### Chemistry

- **Kind:** 2D vector field per chunk, one scalar per `ChemTypeId`.
- **Slot:** NEW. `ChemField` on `Chunk`, gated by
  `World::chem_fields_enabled`. Same shape as `HumidityField` /
  `PressureField` for the scalar case, but with a
  `[f32; CHEM_TYPE_COUNT]` per cell (see [`CHEM.md`](CHEM.md)).
- **Feeds:** `ChemoSensor`, `ChemoEmitter`, `Chemosystem`.
- **Producer:** `run_chem_field` post-barrier, period 6 ticks,
  phase 3.

### Moisture (shallow)

- **Kind:** per-column scalar.
- **Slot:** REUSE. `column.moisture`, `column.moisture_cap` in
  [`crates/wk-world/src/column.rs`](../../crates/wk-world/src/column.rs).
- **Feeds:** `Root` elongation tropism, `Digest` (litter dries out),
  `Photosystem` term in Set A/D (moisture gates growth).
- **Producer:** existing `run_infiltration`, `run_evaporation`,
  `run_lake_level`.

### Moisture (deep / water table)

- **Kind:** 2D field per chunk.
- **Slot:** REUSE. Groundwater head field, gated by
  `World::gw_head_fields_enabled`, already implemented.
- **Feeds:** deep `Root` elongation, tree-habit deep-root behaviour
  in Set D.
- **Producer:** `run_groundwater_head_field` (existing).

### Organic litter

- **Kind:** per-column scalar (aboveground) + per-column dead-root
  bucket (belowground) initially; per-cell layer eventually.
- **Slot:** REUSE for aboveground: `column.ecology.dead_biomass`
  in the existing ecology bucket (see
  [`docs/ECOLOGY.md`](../ECOLOGY.md)). NEW small bucket for
  belowground dead-root mass, or once Set E lands, promote to
  `MaterialId::Organic` layers.
- **Feeds:** `Digest`, `Hypha` reachability, ghost-root cavity
  lifecycle (see [`FUNGI.md`](FUNGI.md)).
- **Producer:** plant / creature deaths, existing decay in
  `run_ecology`, new fungal digest sink.
- **Accessor:** a single `column.organic_at(y_m)` helper hides
  whether the value comes from the ecology bucket, a dead-root
  bucket, or an `Organic` layer. Phase 6 picks the storage; the
  interface stays the same.

### Substrate tag

- **Kind:** per-column enum, per-cell later if needed.
- **Slot:** NEW small field on `Column`, `substrate:
  SubstrateTag = SubstrateTag::Rock` with
  `#[serde(default = "SubstrateTag::rock")]`. See the full enum
  in [`FUNGI.md`](FUNGI.md).
- **Feeds:** `Root` penetrate cost, karst-style void handling,
  ghost-root preferential paths.
- **Producer:** ghost-root lifecycle (`Void` → `Loose` on fill),
  karst dissolution, burrow dig.

### Stem wetness

- **Kind:** per-`Stem`-pixel small scalar.
- **Slot:** NEW, lives on the module entity, not on the world.
- **Feeds:** epiphyte drink (`Holdfast` reads local stem wetness
  instead of column moisture).
- **Producer:** rain events + humidity field top BC. Bumped on rain
  ticks; decays with vapor pressure deficit sampled from the
  humidity field.
- **Skip until epiphytes land.** Working default before Set E: a
  fixed `stem_wetness = 0` scalar (no water) is fine.

## Cadence and priority

| Field | Cadence | Read priority | Write path |
|-------|---------|---------------|------------|
| Light | Every tick | Post-barrier before `run_agents` | Scratch (`Simulation`) |
| Temperature | Every 10 ticks (existing) | Post-barrier | Committed to chunk |
| Chem | Every 6 ticks | Post-barrier | Committed to chunk |
| Moisture (shallow) | Every tick (existing) | Barrier / direct | Existing hydrology |
| Moisture (deep) | Every 30 ticks (existing) | Post-barrier | Committed to chunk |
| Organic | Same cadence as ecology / mass audit refresh | Post-barrier | Ecology bucket or `Organic` layer |
| Substrate | On event (root grow / die / dig / dissolve) | Anytime | Direct mutation |
| Stem wetness | Every 60 ticks (rain sampler) | Post-barrier | Per module scratch |

## Save-load

- `chem_fields_enabled: bool` and `ChemField` payload live in the
  existing world save the same way `thermal_fields_enabled` +
  `ThermalField` do today.
- `substrate: SubstrateTag` on `Column` is `#[serde(default)]`, so
  pre-kernel saves default to `Rock`.
- `light_remaining` and per-module `stem_wetness` are not saved —
  they rebuild from geometry on load.
- `PreferentialRootPath` overlay tag on the substrate is stored
  even when the cell is currently `Rock` again (it is the memory
  of a former ghost root — see [`FUNGI.md`](FUNGI.md)).

## What is deliberately not here

- Wavelength / colour of light. See [`LIGHT.md`](LIGHT.md).
- Chemical mass audit. Concentrations are scalars; add a bucket
  only when a scenario shows drift.
- Per-lane fields. Fields stay column-based; lanes are drawing +
  collision only (see [`LANES.md`](LANES.md)).
- Air / atmosphere as a fine-grained volumetric field. Above-ground
  cells share the coarse column above-surface values.
