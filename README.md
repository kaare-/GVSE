# GVSE — World Kernel 0.1

Local-first 2D geological landscape research prototype.

## Question

Can a simple side-view landscape accumulate readable geological history without numerical instability, unbounded memory growth, or unexplained mass drift?

## Crates

| Crate | Role |
|-------|------|
| `wk-material` | Material vocabulary and property tables |
| `wk-world` | Columns, chunks, terrain generation, markers |
| `wk-sim` | Multirate scheduler, transfer buffers, hydrology/erosion |
| `wk-io` | Save/load v1 (postcard binary) |
| `wk-app` | MS Paint debug renderer and interactive controls |

## Run

```bash
# Interactive simulation
cargo run --release -p wk-app

# Headless soak (default 1M ticks in release)
cargo run --release -p wk-app -- --soak 1000000

# Scenario tests E1–E8
cargo test -p wk-sim --test scenarios

# Full 1M-tick soak test
WK_SOAK_TICKS=1000000 cargo test -p wk-sim --test scenarios e7_
```

## Controls

- **Space** — pause
- **.** — single step
- **1–4** — speed 1× / 10× / 100× / 1000×
- **A/D** — scroll
- **R** — toggle rain
- **[ / ]** — sea level
- **X** — x-ray strata
- **O** — cycle overlays
- **M** — drop marker on selected column
- **S / L** — save / load (`world_save.bin`)
- **Click** — select column (inspector panel)

## Scale (hypothesis)

- 1 horizontal sample = 0.25 m
- Chunk width = 64 samples (16 m)
- Max 8 layers per column
- Integer kg mass accounting

## Design notes

Durable design records for planned extensions and known constraints
live under [`docs/`](docs/README.md):

- [`docs/PLAN.md`](docs/PLAN.md) — consolidated roadmap: stages 1–11
  from current state through worldgen, streaming, performance, field
  layer, karst caves, ecology, burrows, creatures, and evolution.
- [`docs/WORLDGEN.md`](docs/WORLDGEN.md) — infinite left-right terrain
  via deterministic noise, chunk streaming (view / active / resident /
  evicted), initial hydrological state (water table, soil moisture,
  atmospheric humidity), boundary conditions preventing leakage at
  the sim edge.
- [`docs/UNDERGROUND.md`](docs/UNDERGROUND.md) — karst caves and
  burrows: void-annotation data model, soluble-material physics, roof
  collapse, cave ecology.
- [`docs/VOXELS.md`](docs/VOXELS.md) — voxels vs columns vs fields;
  why the hybrid (columns for material identity, coarser scalar/vector
  fields for smooth physics, extended voids for cavities) reaches
  the ambition inside real-time budget and a full voxel rewrite is
  reserved as an option.
- [`docs/PERFORMANCE.md`](docs/PERFORMANCE.md) — measured baseline
  throughput, ordered list of concrete optimisations, target headroom
  before adding the ecology and creature layers.
