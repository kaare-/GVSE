# Design notes

Durable design records for GVSE World Kernel. Not user-facing documentation;
these are the "why did we do it this way" and "what are we planning next"
notes intended to survive re-reads of the codebase.

| Doc | Subject |
|-----|---------|
| [`VOXEL_WATER.md`](VOXEL_WATER.md) | Working voxel water CA: gravity, surface flow, seepage, material capacities. |
| [`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md) | Multithreading: what landed, next phases, what to avoid. |
| [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) | Fields / heatmaps for richer material physics (future plan). |
| [`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md) | Greenfield voxel isolation, heatmaps, checkerboard, roadmap. |
| [`WORLDGEN.md`](WORLDGEN.md) | World topology (ring vs infinite), elevation, streaming, hydro init, wrap rules. |
| [`STRATA.md`](STRATA.md) | Artistic stratigraphic model: facies belts, 8-layer recipes, pinch-outs. |
| [`AGENTS.md`](AGENTS.md) | Creature / agent layer. |
| [`ECOLOGY.md`](ECOLOGY.md) | Plant / soil ecology bucket. |
| [`BURROWS.md`](BURROWS.md) | Dig / burrow API. |
| [`EVOLUTION.md`](EVOLUTION.md) | Reproduction and mutation loop. |
| [`organism/`](organism/) | Organism Kernel freeze docs. |

Read order for world shape: `WORLDGEN.md` → `STRATA.md`.
