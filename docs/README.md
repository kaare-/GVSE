# Design notes

Durable design records for GVSE World Kernel. Not user-facing documentation;
these are the "why did we do it this way" and "what are we planning next"
notes intended to survive re-reads of the codebase.

Each doc is dated at the top and versioned by additive edit. If a design
decision is reversed, note the reversal in a new section rather than
rewriting the original — future readers benefit from seeing what was
considered and rejected.

| Doc | Subject |
|-----|---------|
| [`PLAN.md`](PLAN.md) | Consolidated roadmap: vision, current state, stages 1–11 (worldgen → streaming → performance → field layer → karst → ecology → burrows → creatures → evolution), dependency graph, cross-cutting invariants, deliberately-deferred scope. |
| [`WORLDGEN.md`](WORLDGEN.md) | Infinite left-right terrain via deterministic noise, chunk streaming (view / active / resident / evicted), initial hydrological state (water table, soil moisture, humidity), boundary conditions preventing water leakage at the sim edge. |
| [`UNDERGROUND.md`](UNDERGROUND.md) | Karst caves and burrows: void-annotation data model, soluble-material physics, roof collapse, cave ecology, why voids can't be layers. |
| [`VOXELS.md`](VOXELS.md) | Would this whole simulation work as a voxel grid with heatmap-based physics? Answer: hybrid architecture (columns for material identity, coarser scalar/vector fields for smooth physics, extended voids for cavities). Full voxel rewrite deferred, with reasons and reserved as an option. |
| [`PERFORMANCE.md`](PERFORMANCE.md) | Measured baseline throughput, ordered list of concrete optimisations, target headroom before adding the ecology + creature layers. |

Read order for someone new to the project: root `README.md`, then
`PLAN.md` (the roadmap and how the docs fit together), then
`VOXELS.md` (the architectural constraint that motivates the whole
shape), then `WORLDGEN.md` (world shape) and `UNDERGROUND.md`
(planned physics extension), then `PERFORMANCE.md` (budget accounting
across all of the above).
