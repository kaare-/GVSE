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
| [`PERFORMANCE.md`](PERFORMANCE.md) | Measured baseline throughput, ordered list of concrete optimisations, target headroom before adding the ecology + creature layers. |
| [`UNDERGROUND.md`](UNDERGROUND.md) | Karst caves and burrows: void-annotation data model, soluble-material physics, roof collapse, cave ecology, why voids can't be layers. |
| [`WORLDGEN.md`](WORLDGEN.md) | Infinite left-right terrain via deterministic noise, chunk streaming (view / active / resident / evicted), initial hydrological state (water table, soil moisture, humidity), boundary conditions preventing water leakage at the sim edge. |

Read order for someone new to the project: root `README.md`, then
`WORLDGEN.md` (world shape), then `UNDERGROUND.md` (planned physics
extension), then `PERFORMANCE.md` (budget accounting for the above).
