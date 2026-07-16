# Integrating the field layer without a mess

*Author: integration design memo, mid-2026. This document is the
concrete architectural plan for stage 6 in `PLAN.md`: adding
heatmap-style scalar and vector fields to the existing column-based
sim without letting the codebase deteriorate.*

## The rule that keeps this clean

Every field added must obey the same contract the current subsystems
obey:

1. **Chunk-local state.** Fields live per-chunk with a halo of edge
   values from neighbours. No global 2D array covering the whole
   world; the streamer's freeze/thaw already handles chunks as the
   unit of persistence, and fields must fit that.
2. **Own subsystem, own file.** Each field has one subsystem that
   updates it (a Laplacian or advection pass over the field grid).
   That subsystem is the *only* code that writes to the field's
   cells directly. Everything else that wants to modify the field
   adds source terms to a per-tick source buffer.
3. **Read-only coupling from other subsystems.** A subsystem that
   consumes a field (say, evaporation reading humidity) does so via
   a named accessor that returns `f32`, never `&mut`. Writes go
   through the source buffer.
4. **Buffered writes + barrier commit.** Field subsystems obey the
   same pattern as the current water/sediment machinery. Reads at
   tick start, writes to scratch, commit at barrier, halo exchange
   afterwards.
5. **Same-shape save/load and mass audit.** Fields serialise as
   per-chunk arrays with `#[serde(default)]`; fields with mass
   semantics (dissolved minerals, water content) get their own audit
   bucket; fields without mass semantics (temperature, pressure,
   wind) don't touch the audit at all.
6. **Determinism.** Fixed iteration order per field pass (chunk coord
   ascending, then row-major within a chunk). No `HashMap` iteration
   over field cells. Parallel reductions use a fixed tree order or
   Kahan summation.
7. **One scenario test per field.** Same discipline as E1–E8. A field
   subsystem lands with a test that proves it converges to a
   deterministic steady state under known boundary conditions.

If a new physics idea can't be expressed under these rules, the
right response is "it's not a field, it needs different
scaffolding" — not "loosen the rules." The rules are load-bearing.

## Refactor prep: split `subsystems.rs` before touching physics

The current `crates/wk-sim/src/subsystems.rs` is 1171 lines. Adding
five field subsystems on top of that would push it past 2000 lines
and lose all navigability. **Do this first, as a pure refactor PR
with no behaviour change.**

Target layout:

```
crates/wk-sim/src/
├─ lib.rs
├─ sim.rs                    (Simulation struct, step, run_ticks)
├─ clock.rs                  (SimClock, SubsystemId, SUBSYSTEM_ORDER)
├─ barrier.rs                (commit_chunk_buffer, barrier_commit)
├─ buffer.rs                 (CellTransferBuffer, WorldTransferScratch)
├─ residual.rs               (ResidualAccumulator)
├─ audit.rs                  (mass-audit helpers)
├─ ports.rs
└─ subsystems/
   ├─ mod.rs                 (public re-exports)
   ├─ rain.rs                (run_rain_inject)
   ├─ weather.rs             (run_weather, Cloud helpers)
   ├─ surface_water.rs       (run_surface_water)
   ├─ sediment.rs            (run_sediment)
   ├─ infiltration.rs        (run_infiltration)
   ├─ groundwater.rs         (run_groundwater_flow)
   ├─ evaporation.rs         (run_evaporation)
   ├─ lake_level.rs          (run_lake_level + LakeCell helpers)
   ├─ phase_change.rs        (run_phase_change)
   ├─ slumping.rs            (run_slumping + slumping_pass helpers)
   ├─ layer_merge.rs         (run_layer_merge)
   ├─ activity.rs            (run_activity)
   ├─ shared.rs              (SimParams, common constants like
   │                          WATER_MASS_PER_METRE_DEPTH)
   └─ halos.rs               (update_halos, exchange_outboxes)
```

Each file 50–200 lines. `pub use` in `subsystems/mod.rs` keeps the
existing API surface unchanged. All E1–E8 tests pass after the split.

**This is the single most important step for keeping things clean.**
1171-line files are where architectures go to die. Every subsequent
addition slots into this structure trivially; without the split, they
all pile up in one file.

## The `wk-field` primitive crate

New workspace crate. Contains the field data type and the stencil
operations, no simulation logic.

```rust
// crates/wk-field/src/lib.rs

pub struct FieldPatch {
    pub cells: Vec<f32>,       // row-major, width_cells × height_cells
    pub width_cells: u16,
    pub height_cells: u16,
    pub cell_size_m: f32,
    pub origin_x_m: f32,       // world-space origin of cell (0,0)
    pub origin_y_m: f32,
    pub halo: FieldHalo,
}

pub struct FieldHalo {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub top: Vec<f32>,
    pub bottom: Vec<f32>,
}

impl FieldPatch {
    pub fn sample(&self, x_m: f32, y_m: f32) -> f32 { ... }
    pub fn sample_bilinear(&self, x_m: f32, y_m: f32) -> f32 { ... }
    pub fn cell_at(&self, cx: usize, cy: usize) -> f32 { ... }
    pub fn set_cell(&mut self, cx: usize, cy: usize, value: f32) { ... }
}

pub mod stencil {
    pub fn laplacian_5point(field: &FieldPatch, cx: usize, cy: usize) -> f32;
    pub fn gradient(field: &FieldPatch, cx: usize, cy: usize) -> (f32, f32);
    pub fn divergence(vx: &FieldPatch, vy: &FieldPatch, cx: usize, cy: usize) -> f32;
}

pub mod solvers {
    pub fn explicit_diffusion(
        field: &FieldPatch,
        alpha: &FieldPatch,       // per-cell coefficient
        source: &FieldPatch,      // per-cell source
        dt: f32,
        out: &mut FieldPatch,
    );

    pub fn semi_lagrangian_advect(
        field: &FieldPatch,
        vx: &FieldPatch,
        vy: &FieldPatch,
        dt: f32,
        out: &mut FieldPatch,
    );
}
```

Newtypes per field, in `wk-world`, to keep the type system doing our
categorisation for us:

```rust
// crates/wk-world/src/fields.rs

pub struct ThermalField(pub FieldPatch);          // deg C
pub struct HumidityField(pub FieldPatch);         // 0..1 relative humidity
pub struct PressureField(pub FieldPatch);         // Pa or arbitrary units
pub struct WindField {
    pub vx: FieldPatch,
    pub vy: FieldPatch,
}
pub struct GroundwaterHeadField(pub FieldPatch);  // m
pub struct DissolvedField(pub FieldPatch);        // kg/m³ concentration
```

Reason for newtypes: prevents "oops I passed the humidity field to a
temperature-diffusion function." Costs zero at runtime. Standard
Rust hygiene.

## Chunk integration

Each field lives on `Chunk` as `Option<T>` so we can roll them out
one at a time, and so old saves with no fields still load:

```rust
pub struct Chunk {
    // existing fields...
    pub thermal:   Option<ThermalField>,
    pub humidity:  Option<HumidityField>,
    pub pressure:  Option<PressureField>,
    pub wind:      Option<WindField>,
    pub gw_head:   Option<GroundwaterHeadField>,
    pub dissolved: Option<DissolvedField>,
}
```

Field resolution is chosen once per field family and never changes
(anti-mess rule 5). Suggested defaults:

| Field | Resolution | Cells per chunk (16 m × 100 m extent) |
|-------|-----------:|--------------------------------------:|
| Thermal | 0.5 m | 32 × 200 = 6.4 k |
| Humidity | 2 m | 8 × 50 = 400 |
| Pressure | 2 m | 8 × 50 = 400 |
| Wind (vx, vy) | 2 m | 2 × 400 = 800 |
| Groundwater head | 1 m | 16 × 100 = 1.6 k |
| Dissolved | 0.5 m | 6.4 k |

Total per chunk: ~15 k cells × 4 bytes = 60 kB. At 30 active chunks:
1.8 MB of live field state. Trivial.

Vertical extent: `origin_y_m = bedrock_floor - 5 m` up to
`origin_y_m + 155 m`, so we cover 5 m below bedrock (dead zone with
geothermal source) through the whole terrain up to 30 m above sea
level. Chosen once, world-scale constant.

## The field-subsystem shape

Every field has one file, and every file has exactly this shape:

```rust
// crates/wk-sim/src/subsystems/fields/thermal.rs

use wk_field::{solvers, FieldPatch};
use wk_world::world::World;
use crate::buffer::WorldTransferScratch;

pub fn run_thermal_field(
    world: &World,
    scratch: &mut WorldTransferScratch,
    tick: u64,
) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        let chunk = world.chunks.get(&coord).unwrap();
        let Some(field) = &chunk.thermal else { continue };

        // 1. Build the per-cell diffusivity map from material grid.
        let alpha = build_alpha_from_materials(chunk, field);

        // 2. Build the per-cell source map from boundary conditions.
        let source = build_thermal_source(chunk, field, world, tick);

        // 3. Compute the diffused output into scratch.
        let out = scratch.thermal_out_mut(coord);
        solvers::explicit_diffusion(&field.0, &alpha, &source, DT, out);
    }
}

fn build_alpha_from_materials(chunk: &Chunk, field: &ThermalField)
    -> FieldPatch { ... }

fn build_thermal_source(
    chunk: &Chunk,
    field: &ThermalField,
    world: &World,
    tick: u64,
) -> FieldPatch {
    // Boundary conditions:
    //   top row  = solar/night radiation (uses climate::temperature_at
    //              at the top of the field as the sky value)
    //   bottom row = geothermal target
    //   sides    = halo values from neighbours
    // Interior cells = 0 (no source; pure diffusion)
    ...
}
```

The subsystem is *never* an inline PDE solve. It's a **composition of
stencil ops** from `wk-field`. The stencil ops are unit-tested once;
subsystems just wire them together with the right coefficients and
boundary conditions. This is what prevents each subsystem from
degenerating into 400 lines of index arithmetic.

Barrier commit for fields is separate from the material barrier
commit but follows the same pattern:

```rust
// crates/wk-sim/src/barrier.rs (extended)

pub fn barrier_commit_fields(
    world: &mut World,
    scratch: &mut WorldTransferScratch,
) {
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    for coord in coords {
        if let Some(out) = scratch.thermal_out.remove(&coord) {
            if let Some(chunk) = world.chunks.get_mut(&coord) {
                if let Some(field) = chunk.thermal.as_mut() {
                    field.0 = out;
                }
            }
        }
        // ...same for humidity, pressure, wind, gw_head, dissolved...
    }
    update_field_halos(world);
}
```

The material barrier commit and the field barrier commit are called
in sequence from `Simulation::step`. Order matters — see below.

## Where field boundary conditions come from

Three kinds:

- **Halo (chunk-to-chunk)**: read from neighbour chunk's edge cells,
  written into this chunk's halo before its diffusion pass. Same
  pattern as the current `update_halos`. Extend `update_halos` to
  handle fields.
- **Ground (bottom of vertical range)**: per-field constant.
  Temperature = geothermal. Humidity = 0 (ground absorbs, doesn't
  emit air moisture). Pressure = high (weight of column above).
  Wind = zero (rock is impermeable to air).
- **Sky (top of vertical range)**: per-field time-dependent function
  from `ClimateSettings`. Temperature = sky temp from
  `climate::temperature_at` at the top elevation. Humidity = regional
  target from wetness noise (see `WORLDGEN.md`). Pressure = ambient.

Each field's boundary conditions live in the same file as its
subsystem. `climate.rs` remains the *supplier* of sky temperature and
regional humidity target; the thermal and humidity subsystems just
sample it at the top row of their field.

**This is the key point**: the existing climate function
(`temperature_at`) doesn't go away — it becomes the boundary condition
for the field. The field's *interior* is now a proper diffused
temperature, so the temperature under 5 m of stone can be different
from the surface, and the surface itself can lag the sky by hours
(diurnal thermal inertia). No existing code that currently calls
`world.temperature_at` needs to change; internally the accessor
switches from "call climate function" to "sample field, fall back to
climate function if field disabled." That's the migration.

## Coupling: fields ↔ material

Every coupling is **one direction at a time** and has a specific
mechanism:

### Material → field coefficient

Material properties (thermal diffusivity, permeability, solubility)
determine a field's coefficient. Rebuilt on any material change.
Cheap: a chunk material change is rare (dissolution, deposition), and
the alpha map is a small array.

```rust
fn build_alpha_from_materials(chunk: &Chunk, field: &ThermalField) -> FieldPatch {
    // For each field cell, sample the material grid at cell centre.
    // Look up thermal diffusivity from MaterialProps.
    // Store in a same-shape FieldPatch to hand to the solver.
    ...
}
```

Optimisation: cache the alpha maps and invalidate only on material
change. Not needed initially; add when a profiler says so.

### Field → material rate

Fields drive rates in existing subsystems. `run_evaporation` reads
the humidity field at each surface column and computes:

```rust
let humidity_here = chunk.humidity
    .as_ref()
    .map(|h| h.0.sample(col.world_x as f32 * SAMPLE_WIDTH_M, col.surface_y))
    .unwrap_or(HUMIDITY);  // fallback to the old constant
let evap_factor = 1.0 - humidity_here;
```

The fallback is what makes this a safe incremental change: if the
humidity field is `None`, we get the current behaviour exactly. Field
enabled? Better behaviour. Field disabled? No regression.

### Field → field coupling

Some fields drive other fields (wind advects humidity, pressure
gradients drive wind). Same pattern: read at cell centre, integrate
per stencil step. Never mutate another field's cells directly from
your own subsystem — write to the *target field's source buffer* if
you need to inject, and let its own subsystem incorporate.

## Execution order

Within one tick, once fields are in, the order becomes:

```
For each subsystem in SUBSYSTEM_ORDER {
    match subsystem {
        MaterialWrite  => run_(rain|weather|surface_water|sediment|...)
        FieldRead      => (subsystems above may sample fields)
        FieldWrite     => run_(thermal|humidity|pressure|wind|gw_head|dissolved)
    }
}
barrier_commit_materials(world, scratch, tick);
barrier_commit_fields(world, scratch);
run_direct_mutation_passes();  // phase_change, lake_level, slumping, karst
```

Concretely, the augmented `SUBSYSTEM_ORDER`:

```rust
pub const SUBSYSTEM_ORDER: [SubsystemId; 15] = [
    // Field passes that ADD to material state (source terms).
    // Not present yet — but this is where they'd slot if any
    // material subsystem needs field-driven inputs.

    // Existing material subsystems, unchanged.
    SubsystemId::RainInject,
    SubsystemId::Weather,           // now reads WindField
    SubsystemId::SurfaceWater,
    SubsystemId::Sediment,
    SubsystemId::Infiltration,
    SubsystemId::Groundwater,       // may migrate to head-field driven
    SubsystemId::Evaporation,       // now reads HumidityField, writes source to it
    SubsystemId::LayerMerge,
    SubsystemId::Activity,

    // Field passes (write to fields via source buffers).
    SubsystemId::ThermalField,
    SubsystemId::HumidityField,
    SubsystemId::PressureField,
    SubsystemId::WindField,
    SubsystemId::GroundwaterHeadField,
    SubsystemId::DissolvedField,
];
```

Multi-rate schedule: fields can (and should) run less often than
material subsystems if the physics is slow. Temperature diffusion at
Δt = 1 tick and α = 1e-6 m²/s gives a stability limit at 0.5 m
resolution of Δt < 0.25 s = 15 ticks at 60 Hz. So the thermal field
can run every 10 ticks with no accuracy loss. Same trick as the
current `Infiltration` period=60 subsystem.

Recording it in the schedule table:

```rust
SubsystemSchedule { id: SubsystemId::ThermalField,   period: 10, phase: 0 },
SubsystemSchedule { id: SubsystemId::HumidityField,  period: 10, phase: 3 },
SubsystemSchedule { id: SubsystemId::PressureField,  period: 30, phase: 5 },
SubsystemSchedule { id: SubsystemId::WindField,      period: 30, phase: 6 },
SubsystemSchedule { id: SubsystemId::GroundwaterHeadField, period: 30, phase: 10 },
SubsystemSchedule { id: SubsystemId::DissolvedField, period: 6,  phase: 2 },
```

Phase-staggered so field subsystems don't all fire on the same tick
and beat against each other (learned lesson from the RainInject +
LakeLevel beat-frequency bug).

## Save / load / audit

Save format bumps to `SCHEMA_VERSION = 2`. Each field serialises as
`Option<FieldPatchSnapshot>` on the chunk snapshot. Loader reads
`Option` — `None` means the field wasn't enabled at save time and
lands as `None` today, which the subsystem tolerates.

```rust
#[derive(Serialize, Deserialize)]
pub struct ChunkSnapshotV2 {
    // v1 fields...
    #[serde(default)]
    pub thermal: Option<FieldPatchSnapshot>,
    #[serde(default)]
    pub humidity: Option<FieldPatchSnapshot>,
    // ...
}
```

Legacy v1 saves round-trip: `#[serde(default)]` supplies `None` for
every field. On load, `Simulation::sync_params` triggers a
regeneration pass that initialises any field currently enabled by the
world's `Feature` flags but missing from the save.

Mass audit:

- Thermal, pressure, wind, humidity: **no audit change**. These are
  not mass. Adding them to `by_material` would be a type error.
- Groundwater head: **no audit change**. Head is a pressure, not a
  mass. Actual water mass stays in `Column.moisture` and in the
  layer stack.
- Dissolved minerals: **new audit bucket** `dissolved_total`. Paired
  with the material `by_material` entry for the source material
  (Limestone). Audit invariant becomes:
  `initial + rain + sea + soil = current_solid + dissolved + evap + boundary`.

## Determinism

Field passes obey the same fixed-iteration-order rule as the current
sim:

- Outer loop: chunk coords in ascending BTreeMap order.
- Inner loop: cells row-major within the field.
- Reductions (e.g. total energy for a stability check) use tree-order
  Kahan summation. No `sum::<f32>()` on a HashMap iteration.

Parallelisation via rayon (stage 5 performance work) still respects
this: `.par_iter_mut()` on a `Vec<Chunk>` (post-BTreeMap-flatten,
stage 5.4) gives deterministic per-chunk work; inside a chunk the
loop is sequential and cheap enough that no further parallelism is
needed. Cross-chunk reductions use `.par_iter().fold(...).reduce(...)`
with associative operators only.

## Debugging aids

New render overlays, one per field:

- `OverlayMode::TemperatureField` — colour ramp from cold blue to hot
  red, sampled at the field grid.
- `OverlayMode::HumidityField` — grey → cyan.
- `OverlayMode::PressureField` — contour lines.
- `OverlayMode::WindField` — arrows.
- `OverlayMode::DissolvedField` — dark → yellow.

Cycle-through with the existing `O` key. These are *essential* for
debugging PDE stability issues (an oscillating temperature field is
visually obvious; a subtly wrong one in a log line is not).

Each overlay is <50 lines in `wk-app/src/render.rs`, all pattern-
matched off the existing `OverlayMode` enum. No new render code
architecture.

## What could still make this a mess

Every failure mode I can name has a specific mitigation:

| Failure mode | Prevention |
|--------------|------------|
| One-file bloat | Splitting `subsystems.rs` first; new subsystems get new files |
| Field-lookup sprawl | Document which subsystem writes which field; read-only elsewhere |
| Coupled subsystems fighting over field state | Source-term buffers, not direct writes; barrier commit for fields |
| Numerical instability from too-large timestep | Multi-rate scheduling with periods chosen against α·Δt/Δx² stability |
| Determinism drift under parallelism | Fixed iteration order, tree-order Kahan reductions |
| Save-format churn | Schema versioning; `#[serde(default)]` on every field |
| Type confusion (humidity function taking temperature) | Newtype per field |
| PDE code re-implemented per subsystem | Stencils in `wk-field`, subsystems just compose |
| Field state divergence from material state | Coupling is one-way at each site; material→field via alpha map, field→material via rate |
| "It works but I don't know why" | Scenario test per field with a known steady state |
| Silent field-off-by-default breakage | Feature flag reads a documented enum; fallback branches use the current behaviour |

## Migration inventory: what changes in existing files

For the reader, this is what actually gets edited when stage 6.2
(thermal field) lands:

- `crates/wk-material/src/lib.rs` — add `MaterialProps::thermal_diffusivity`.
- `crates/wk-world/src/fields.rs` — new module, `ThermalField`.
- `crates/wk-world/src/chunk.rs` — add `pub thermal: Option<ThermalField>`.
- `crates/wk-world/src/climate.rs` — no change; `temperature_at` becomes
  the sky boundary source.
- `crates/wk-world/src/world.rs` — accessor
  `fn temperature_at(&self, world_x: i32, y: f32, tick: u64) -> f32`
  that samples the field (or falls back to `climate::temperature_at`
  if the field is `None`).
- `crates/wk-sim/src/clock.rs` — new `SubsystemId::ThermalField`,
  entry in `SUBSYSTEM_SCHEDULES` and `SUBSYSTEM_ORDER`.
- `crates/wk-sim/src/subsystems/fields/thermal.rs` — new file.
- `crates/wk-sim/src/sim.rs` — dispatch new subsystem in `step`.
- `crates/wk-sim/src/barrier.rs` — `barrier_commit_fields` extension.
- `crates/wk-sim/src/subsystems/phase_change.rs` — read from
  `world.temperature_at(world_x, y, tick)` accessor instead of
  `world.temperature_at(elev, tick)` (signature migrated).
- `crates/wk-io/src/lib.rs` — bump `SCHEMA_VERSION`; add
  `Option<FieldPatchSnapshot>` fields to `ChunkSnapshot` with
  `#[serde(default)]`.
- `crates/wk-app/src/render.rs` — new `OverlayMode::TemperatureField`
  branch.
- `tests/scenarios/e20_thermal_steady_state.rs` — new scenario.

That's ~12 files touched; each edit is small and localised. No file
becomes larger than ~300 lines. No new global state. No architectural
churn — this is exactly the shape the existing subsystems already
take.

Every subsequent field (humidity, pressure, wind, groundwater head,
dissolved) is the same shape, roughly the same size of change, and
the migration inventory is a straight copy of the thermal one.

## Rollout order (sub-stages of stage 6 in `PLAN.md`)

Each is a self-contained PR with tests.

### 6.0 Refactor: split `subsystems.rs` into `subsystems/*.rs`

Pure refactor, no behaviour change. All E1–E8 pass. **This must land
first.** No new physics before this.

### 6.1 `wk-field` crate + newtypes

Introduce `FieldPatch`, `FieldHalo`, stencil ops, solver
skeletons. Add `Option<FieldFamily>` fields to `Chunk` for every
planned field. All fields default `None`. Save/load extended to
round-trip the `Option`s. Existing tests unchanged.

### 6.2 Thermal field

First real field. Geothermal at bottom, sky-driven boundary at top.
`phase_change` reads from the field via the accessor. New scenarios
`E20_geothermal_steady_state` and
`E21_diurnal_thermal_inertia_at_depth`.

### 6.3 Humidity field

Replaces `const HUMIDITY: f32 = 0.4`. Advected by wind if wind field
exists, otherwise diffusion only. `evaporation` reads humidity, adds
to source. Scenario `E22_humidity_near_water_body`.

### 6.4 Pressure + wind fields

Baroclinic pressure driven by temperature gradients; wind derived
from `−∇p/ρ`. `weather` reads wind from the field instead of
`climate.wind_speed`. Scenario `E23_convection_cell`.

### 6.5 Groundwater head field

Replaces `Column::water_table_y()` internals with sampling of the
head field. `run_groundwater_flow` becomes a Darcy diffusion pass on
the field. Column moisture is still the mass storage; the field is
just the pressure. Existing groundwater E-tests continue to pass with
tightened drift bound. Scenario `E24_darcy_pressure_equilibration`.

### 6.6 Dissolved minerals field

Concentration field. Sourced by dissolution at soluble material
voxels (limestone from stage 7). Advected by groundwater flow (from
6.5). Precipitates in speleothems when supersaturated. **Enables
stage 7 karst** to be a clean field-driven subsystem instead of
ad-hoc per-column limestone-eating.

## Total cost of the stage

Cumulative code added across 6.0 through 6.6:

- `wk-field` crate: ~400 lines (data types + stencil + solver
  primitives).
- Newtypes and chunk integration in `wk-world`: ~200 lines.
- Field subsystems in `wk-sim/subsystems/fields/`: ~600 lines total
  (6 files × ~100 lines).
- Barrier extension, halos extension: ~150 lines.
- Save/load extension: ~100 lines.
- Render overlays: ~250 lines.
- Scenarios: ~300 lines total (5 new).

Total new code: **~2000 lines**, spread across ~30 small files, each
under ~250 lines. Compared to the current 5.8 k-line codebase, this
is a ~35% growth for a very significant capability expansion, and
the split into small files keeps navigability high.

## Summary of what makes it clean

Three specific practices carry all the weight:

- **Split `subsystems.rs` before anything else.** One-file-per-
  subsystem is the practice that stops the codebase from silently
  ossifying.
- **Same contract for every field subsystem.** Read at tick start,
  write source buffer, barrier commit, halo exchange. Identical shape,
  no bespoke architectures.
- **Fields never write across boundaries.** One subsystem owns each
  field's cells; source terms are how anyone else contributes.

Everything else — newtypes, feature flags, versioning, scenario
tests, render overlays, multi-rate scheduling, deterministic
reductions — is standard hygiene that pays for itself the first time
it prevents a bug. The three above are the load-bearing pieces.
