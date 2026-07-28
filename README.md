# GVSE — World Kernel

Local-first 2D side-view geological / ecological research prototype.

**Active stack:** voxel cellular automata (`wk-voxel` + `wk-voxel-app`).
The older column stack lives under `crates/legacy/` (`wk-world` /
`wk-sim` / `wk-app` / …) — reference only, not the development path.

## Question

Can a simple side-view landscape accumulate readable geological history
without numerical instability, unbounded memory growth, or unexplained
mass drift?

Cell-sat inventory: `wk_voxel::sat_totals` (see `docs/VOXEL_WATER.md`
§ Mass inventory).

## Crates (active)

| Crate | Role |
|-------|------|
| `wk-material` | Shared material vocabulary and property tables |
| `wk-voxel` | 2D cell grid: water, grain, climate, organisms, geotech |
| `wk-voxel-app` | Macroquad demo / editor for `wk-voxel` |

Design notes: [`docs/README.md`](docs/README.md) (start with
`VOXEL_WATER.md`, `VOXEL_FAILURE.md`, `VOXEL_GEOTECH_MAP.md`).

## Run

```bash
cargo run --release -p wk-voxel-app

# Library tests
cargo test -p wk-voxel --lib
cargo test -p wk-voxel-app

# Isolation: wk-voxel must not depend on the column stack
bash scripts/check-voxel-isolation.sh
```

Toolchain is pinned in `rust-toolchain.toml` (1.83). License: `LICENSE-MIT`.

## Controls (voxel app)

- **Tab** — settings (geotech, climate, materials, …)
- **Space** — pause
- **R** — regenerate world
- **G** — cycle geotech overlay (shear → σᵥ → wet → off)
- **H / T / N** — humidity / temperature / clouds
- **F1** — HUD chrome · **F2** creature editor · **F3** terrain editor
- **F5 / F9** — save / load
- **Click** — block inspector · arrows — pan

## Scale

- 1 cell ≈ 0.25 m (`SAMPLE_WIDTH_M`)
- Chunk = 64×64 cells
- Water is `Air + sat` (free surface + pore fill)

## Legacy column stack

`wk-world`, `wk-field`, `wk-sim`, `wk-io`, `wk-app` remain buildable but
are not maintained as the product path. See `docs/VOXEL_MIGRATION.md`.
