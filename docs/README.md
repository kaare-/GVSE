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
| [`WORLDGEN.md`](WORLDGEN.md) | World topology (ring vs infinite), elevation noise, chunk streaming, initial hydrological state, boundary / wrap rules. |
| [`STRATA.md`](STRATA.md) | Artistic stratigraphic model: facies belts, 8-layer stack recipes, pinch-outs, ring-aware continuity — geology as diorama, not deep-time sim. |

Read order for someone new to the project: root `README.md`, then
`WORLDGEN.md` (topology + shape), then `STRATA.md` (what the rocks
look like), then `UNDERGROUND.md` (karst/burrows), then
`PERFORMANCE.md` (budget).
