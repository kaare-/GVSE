# Voxel multithreading — future plan

*Tentative. Landed pieces are already in tree; this records what to
do next and what not to do. Companion to [`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md)
§7 and Tab → Performance.*

## Already landed (do not re-invent)

| Piece | Where |
|-------|--------|
| 4-colour checkerboard over active chunks | `crates/wk-voxel/src/active.rs` |
| Rayon within a colour | `crates/wk-voxel/src/parallel.rs` |
| Parallel gravity / grain fall / repose (in-place) | `rules.rs` |
| Parallel water-flow + seepage **scans**; serial apply | `rules.rs` |
| Toggle `PerfConfig.parallel_physics` (default on) | Tab → Performance |
| Determinism: parallel ≡ serial multi-chunk fixture | `rules` / `perf_profile` tests |

Contract: pull passes write only **own chunk + `cy + 1`**. Flow /
seepage stay **compute-then-apply** (one snapshot → one apply) so
mass stays conserved. Wrap-x worlds need an **even** chunk span so
seam neighbours stay opposite colours.

## Phase 0 — Measure first

Before adding threads, profile a real save (terrace water + high
creature count):

1. Share of frame in `tick_with_perf` vs rain / evap / clouds / temp /
   phase / organisms / draw.
2. Whether the hot path is flow ×12 scans or the serial frame shell.
3. Gate: only invest where a pass is roughly ≥10–15% of frame time.

Harness notes live in `crates/wk-voxel/tests/perf_profile.rs`
(`busy play` = 48×6 chunks + plants + terrain-edit churn).

**Phase 0 snapshot (release headless, ~24Hz budget = 41.7ms):** busy play
was ~70ms/tick with **geotech roof** scanning every loaded chunk
(~53ms). After dirty-wake + periodic full-scan failure and cheaper
thermal props refresh, busy sits ~39ms/tick (≈93% of budget). Next
≥10% candidates on busy: failure full-scan ticks, karst, flow erosion,
water flow; organisms were negligible at ~140 plants.

## Phase 1 — Easy wins (same pattern as seepage)

`map_regions_parallel` scan + **serial** apply:

1. `apply_flow_erosion`
2. Evaporation / karst (already skip dry / non-limestone chunks)
3. Rain / condensation column or tile scans

Low risk: no new write-set proofs.

## Phase 2 — Fields outside the CA

Parallelize by column / tile when write sets are disjoint (no
checkerboard required):

- Humidity diffuse / advect
- Temperature step
- Cloud precip seating (careful with parcel-list mutation)

See also [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md).

## Phase 3 — Organisms (only if pop is the bottleneck)

HUD has already hit entity caps in the thousands. Suggested split:

1. Parallel **read-only** prepass: canopy / trunk / live-root indexes
2. Parallel per-atom metabolism **without** world writes
3. Serial (or sharded) apply for drink / grow / death / births

Hard: shared pore `sat`, root spacing, contacts. Do not checkerboard
atoms until write sets are proven.

## Phase 4 — Defer / avoid

- Parallelizing flow / seepage **apply** (mass bugs)
- Expanding pull write-set beyond own + `cy+1` without a new proof
- Odd wrap-x chunk counts (breaks colour disjointness)
- Multithreaded macroquad draw — batch / cull first instead

## Success criteria

- Same water mass and same entity outcomes with parallel on vs off
- Measurable frame-time win on a dirty, wide world (not idle sky)
- Toggle remains in Tab → Performance

## Suggested order of work

1. Profile one of the user's saves  
2. Phase 1 on the top offender  
3. Re-profile; only then organisms or fields  
