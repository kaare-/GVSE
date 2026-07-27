# Design notes

Durable design records for GVSE World Kernel. Not user-facing documentation;
these are the "why did we do it this way" and "what are we planning next"
notes intended to survive re-reads of the codebase.

| Doc | Subject |
|-----|---------|
| [`VOXEL_WATER.md`](VOXEL_WATER.md) | Working voxel water CA: gravity, surface flow, seepage, material capacities. |
| [`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md) | Multithreading: what landed, next phases, what to avoid. |
| [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) | Fields / heatmaps for richer material physics (future plan). |
| [`VOXEL_FAILURE.md`](VOXEL_FAILURE.md) | Shear + compressive failure: phased implementation plan. |
| [`VOXEL_GEOTECH_MAP.md`](VOXEL_GEOTECH_MAP.md) | Slow shear/wetness/σᵥ stress maps; `G` overlay; dam hydro proxy. |
| [`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md) | Greenfield voxel isolation, heatmaps, checkerboard, roadmap. |
| [`WORLDGEN.md`](WORLDGEN.md) | World topology (ring vs infinite), elevation, streaming, hydro init, wrap rules. |
| [`STRATA.md`](STRATA.md) | Artistic stratigraphic model: facies belts, 8-layer recipes, pinch-outs. |
| [`AGENTS.md`](AGENTS.md) | Creature / agent layer. |
| [`ECOLOGY.md`](ECOLOGY.md) | Plant / soil ecology bucket. |
| [`BURROWS.md`](BURROWS.md) | Dig / burrow API. |
| [`EVOLUTION.md`](EVOLUTION.md) | Reproduction and mutation loop. |
| [`organism/`](organism/) | Organism Kernel freeze docs. |
| [`organism/STUDIO.md`](organism/STUDIO.md) | Muscle / bone / neural test studio (shared `wk-voxel` physics). |

Read order for the **active voxel stack:** `VOXEL_WATER.md` →
`VOXEL_FAILURE.md` / `VOXEL_GEOTECH_MAP.md` → `VOXEL_FIELDS.md`.
World shape: `WORLDGEN.md` → `STRATA.md`.

The column stack docs below remain historical context; product work
is on `wk-voxel`.
