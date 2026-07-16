# GVSE — World Kernel 0.1

Local-first 2D geological landscape research prototype.

## Question

Can a simple side-view landscape accumulate readable geological history without numerical instability, unbounded memory growth, or unexplained mass drift?

## Crates

| Crate | Role |
|-------|------|
| `wk-material` | Material vocabulary and property tables |
| `wk-field` | Scalar/vector field patches, stencils, solvers (stage 6) |
| `wk-world` | Columns, chunks, terrain generation, markers, field slots |
| `wk-sim` | Multirate scheduler, transfer buffers, hydrology/erosion |
| `wk-io` | Save/load (postcard binary; schema v2 adds optional fields) |
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
