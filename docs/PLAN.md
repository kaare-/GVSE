# Roadmap

*Consolidated stages from the individual design docs, plus dependency
graph, plus scope boundaries. Living document — expected to change as
each stage is scoped or implemented.*

## Vision

GVSE is a 2D side-view open-world simulation whose *physical substrate*
is honest first: mass is conserved, dynamics are deterministic, the
world stretches infinitely left and right, geology accumulates
readable history, and hydrology closes as a rain ↔ evaporation loop
without hidden pumps.

On top of that substrate:

- A subsurface with karst caves, burrows, and cave rivers, providing
  distinct ecological niches and multi-lane routing for creatures.
- An atmospheric and geothermal field layer (temperature, humidity,
  wind, subsurface heat) that fills in the physics currently
  approximated by scalar constants.
- A per-column ecology bucket driving plant-like producers with
  root/leaf/nutrient state feeding back into erosion, infiltration,
  and evapotranspiration.
- An entity/agent layer (ECS) hosting player-designed creatures.
- Selection pressure emerging from the physics + ecology + creature
  interactions such that "evolution takes over" is not a special
  subsystem but a consequence.

The physics honesty and the eventual creature layer are the two
things GVSE tries to do differently from Minecraft (which is
content-first, physics-second), Dwarf Fortress (which is content-first,
physics-third, mass-conservation-not-audited), and the powder-sim
family (which is physics-only, no ecology).

## Current state (World Kernel 0.1)

- 5.8k Rust lines across 5 crates (`wk-material`, `wk-world`,
  `wk-sim`, `wk-io`, `wk-app`).
- 11-material vocabulary with per-material property table.
- Columns up to 8 stratigraphic layers with age ranges; chunks of 64
  columns.
- 12 tick-scheduled subsystems: rain, weather, surface water,
  sediment, infiltration, groundwater, evaporation, layer merge,
  activity, lake level, phase change (snow ↔ water ↔ ice), slumping.
- Buffered writes + barrier commit + per-chunk cross-boundary
  outbox/inbox pattern.
- Mass audit invariant (`initial + rain − evap − boundary = current`)
  with <100 kg drift over 100k ticks in shipped tests.
- Postcard save/load with legacy-field migration.
- macroquad debug renderer with overlays and live settings.
- E1–E8 scenario tests plus a soak test.

Measured single-core throughput at the shipped 5632-column map:
~308 tps (~4× headroom against a 60 tps target). Per-column
throughput is roughly flat above ~1000 columns at ~1.7 M col·ticks/s.

## Stage graph

```
   [1 noise gen] ── [2 initial hydro] ── [3 streamer] ── [4 backlog boundary]
                                                              │
   [5 perf sweep] ─────────────────────────────────────────── │
                                                              │
                                                        [6 field layer]
                                                              │
                                                      ┌───────┼───────┐
                                                      │               │
                                              [7 karst caves]   [8 ecology]
                                                      │               │
                                              [9 burrows]             │
                                                      └───────┬───────┘
                                                              │
                                                       [10 ECS + creatures]
                                                              │
                                                       [11 evolution loop]
```

Later stages reference and depend on earlier ones; the graph is not
strictly linear. Stages 5 (perf) and 6 (fields) can proceed in
parallel with 2–4 (worldgen). Stages 7 (karst) and 8 (ecology) both
consume the field layer.

## Stages in detail

Each stage lands as one or more PRs; each PR leaves the sim in a
green, testable state.

### Stage 1 — Multi-scale noise terrain generator

*Doc: `WORLDGEN.md` §infinite terrain*

Replace fixed-x-cutoff profile in `continental_surface_y` with a
three-band deterministic noise composition (continental ~4000 m,
regional ~400 m, local ~40 m + 10 m). Regional wetness scalar added
at ~800 m stride. Existing E-tests continue to pass. New scenario
walks 400 chunks in either direction and asserts biome variety.

Old fixed profile preserved as `generate_chunk_demo_profile` for
scenario reproducibility.

### Stage 2 — Initial hydrological state at generation

*Doc: `WORLDGEN.md` §initial hydrological state*

At chunk-gen time, initialise water table, capillary fringe, soil
moisture, atmospheric humidity, and spring/wetland features. Mass
audit gains `soil_inject_total` bucket. New scenario
`E14_generated_land_starts_at_steady_state` verifies drift < threshold
over 10k ticks with rain off.

### Stage 3 — Chunk streamer

*Doc: `WORLDGEN.md` §chunk streaming*

Add view / active / resident / evicted tiers. In-memory chunk store
for evicted chunks. `AppState::new` stops pre-generating; streamer
generates on demand. Determinism test: unload, regenerate, assert
bit-identical output.

### Stage 4 — Frozen-chunk backlog + absorbing boundary

*Doc: `WORLDGEN.md` §boundary conditions*

Frozen chunks accept inflow into a persistent `FrozenBacklog` that
applies once on thaw. Halo values at the active-window boundary come
from frozen resident state (or generative steady state). New scenario
`E15_no_boundary_leak_across_freeze` verifies mass conservation across
freeze/thaw events.

### Stage 5 — Performance sweep

*Doc: `PERFORMANCE.md`*

In order of expected payoff: eliminate halo-update clone, parallelise
buffered subsystems via rayon, dirty flags on `run_lake_level` and
`run_slumping`, flatten `BTreeMap<i32, Chunk>` to `Vec`, remove
barrier-commit buffer clone, `MaterialRegistry::props` switch → const
array, preallocate snapshot buffer, review `run_activity` predicate,
optionally move per-column residual to a per-chunk sparse map.

Target: 80–100× real-time on one core at shipped map size before
starting the ecology + creature layer work.

### Stage 6 — Field/heatmap layer

*Doc: `VOXELS.md` §hybrid architecture*

Introduce coarse-resolution scalar fields decoupled from the column
grid, updated by stencil operations each tick. First field: air
temperature (2D grid over the loaded active window at ~0.5 m
resolution). Second: atmospheric humidity. Third: subsurface heat with
geothermal boundary condition from world bottom. Existing subsystems
that consume `HUMIDITY` and `temperature_at` migrate to sampling the
fields instead. New subsystem `run_thermal_field` does 5-point
Laplacian diffusion plus source terms per tick.

Deliberately additive: the columns are untouched, the fields sit
alongside. First test: `E16_geothermal_gradient_stable` verifies a
100 m depth column reaches steady-state gradient without oscillation.

### Stage 7 — Karst caves via void annotation

*Doc: `UNDERGROUND.md`*

`MaterialId::Limestone` + `solubility` + `roof_span_max_m` properties;
`Void` type and sparse `voids: SmallVec<[Void; 4]>` on `Column`;
`run_karst` driven by lateral flux through soluble layers;
`run_void_water_flow`; roof collapse unified with slumping under a
single unsupported-mass predicate; speleogenesis closing the
dissolved-mass audit loop. Scenarios E9–E12.

Depends on stage 6 for the coupled dissolved-mineral concentration
field.

### Stage 8 — Ecology bucket

*Doc: to be written (`ECOLOGY.md`)*

Per-column `Ecology { root_density, leaf_area, dead_biomass,
alive_biomass, nutrient }`. New `run_ecology` subsystem with plant-
like growth as a function of light (from surface exposure), water
(from `moisture`), temperature (from the field), and nutrient state.
Ecology feeds back into `run_sediment` (roots reduce erosion),
`run_evaporation` (leaves add evapotranspiration), `run_infiltration`
(roots boost permeability). Mass audit gains a biomass bucket.

Initial per-column values seeded from biome and wetness at chunk-gen
time.

### Stage 9 — Burrow API

*Doc: `UNDERGROUND.md` §burrows*

`world.dig(column_x, target_y, volume_kg)` extends or creates a
`Void { origin: Burrow }`. Removed mass becomes surface tailings.
Two burrows at similar elevations in adjacent columns treated as
connected. Testable without creatures via synthetic dig calls.

### Stage 10 — ECS layer for creatures

*Doc: to be written (`AGENTS.md`)*

Bring in `hecs` or `bevy_ecs` (probably `hecs` — smaller, no ecosystem
lock-in). Agents live outside the column stack; they read
column/void/field state and can call world APIs (`dig`, `eat`,
`drink`). Active-set augmentation: chunks containing agents stay
active regardless of camera position. First "creature" is a scripted
grazer with genome-driven trait vector; no evolution yet, this is the
substrate.

### Stage 11 — Species-selection loop

*Doc: to be written (`EVOLUTION.md`)*

Fitness = f(survival ticks, offspring produced, resource efficiency).
Mutation on reproduction. Selection is implicit: creatures that
starve/desiccate/freeze die, ones that meet resource thresholds
reproduce with mutation. No global evolution subsystem; the outcome
emerges from the interaction of the substrate + ecology + creature
layers. This is when "evolution takes over" is a phrase we can defend.

## Cross-cutting invariants

Preserved through every stage:

- Deterministic content-addressed world gen (`hash_u64(seed, x, y,
  salt)` primitives). Any coord regenerates identically.
- Mass audit invariant. Every new mass sink and source gets its own
  bucket so the equation stays exact.
- Buffered writes + barrier commit. New subsystems obey the same
  contract or are direct-mutation post-barrier passes (like
  `run_phase_change` and `run_slumping`).
- Save/load round-trip. New per-column or per-chunk fields carry
  `#[serde(default)]` so older saves migrate cleanly.
- Scenario tests as engineering artefacts. Each new subsystem earns
  its own scenario. Mass conservation asserted in every scenario.

## Deliberately deferred / out of scope for the current arc

- Full 3D world. GVSE is a 2D side-view; making it 3D is a rewrite.
- Multiplayer. The determinism substrate supports lockstep in
  principle but no network layer is planned.
- Native rendering pipeline. `wk-app` uses macroquad because it works.
  A shipping game would want its own renderer but not now.
- Voxelisation of the world. See `VOXELS.md` for the reasoning and
  for the hybrid path we take instead.
- Content authoring tools (biome editor, creature-genome editor UI).
  The genome format itself lands in stage 10; the UI for editing it
  is a later concern.
