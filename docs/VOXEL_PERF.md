# Voxel performance — Phase 1 baseline

Companion to [`ROADMAP.md`](ROADMAP.md) Phase 1 and [`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md).
Numbers from `cargo test -p wk-voxel --test perf_profile --release -- --ignored --nocapture`
on the cloud agent host (2026-10-07), park-bank tip after Phase 0 close.

Harness: warm 40 / measure 200, `PerfConfig` FPS defaults, climatic stack via
`step_world`. Parallel physics **OFF** unless noted (rayon ON is slightly
slower on this dirty width).

## Stamp sizes

| Label | Size | Chunks | Cells |
|-------|------|--------|------:|
| short sky | 1024×320 | 80 | ~328k |
| demo (default) | 1024×1064 | 272 | ~1.09M |
| stress | 2048×1064 | 544 | ~2.18M |

## Wall / physics (0 plants)

| Stamp | wall ms/tick | ~sim-FPS | physics | parallel A/B |
|-------|-------------:|---------:|--------:|--------------|
| short sky | 27.4 | ~36 | 19.8 | — |
| demo | 33.0 | ~30 | 23.6 | FPS OFF 32.4 / ON 34.1; full_feel OFF 119 / ON 117 |
| stress | 56.2 | ~18 | 40.7 | — |

Demo + plants: +48 ≈ 34.0 ms, +256 ≈ 32.8 ms (org share ≤2%).

## Hottest physics sub-passes (demo 0 plants)

| Pass | ms/tick | Share of wall |
|------|--------:|--------------:|
| rock bodies | 8.49 | 26% |
| settle grains | 7.07 | 21% |
| seepage | 4.29 | 13% |
| gravity | 0.80 | 2% |
| plan+clear dirty | 0.69 | 2% |
| water flow | 0.53 | 2% |

Frame shell (outside physics): humidity.advect ~1.85, evap ~1.72, steam ~1.12,
temperature ~1.04 amortized, flow erosion ~1.03. Condensation ~0.28 — **do not**
coarsen weather / lottery before these CA tails shrink.

Active plan (demo): ~16 regions / ~2460 cells per flow substep; avg ~7.6
substeps/tick with quiet early-out.

## Surgical win (this branch)

**Hotspot chosen:** settle grains (#2). Multi-pass
`settle_loose_grains_regions_ex` already filtered sticky-`has_loose` for the
*repose* re-plan, but assigned the next fall pass from unfiltered
`plan_active` — so after seepage dirtied stone/limestone pores, every settle
pass re-walked that water halo.

**Change:** `cur = keep_loose_regions(world, &plan_active(world))` after each
settle pass (same gate as repose). No weather / condensation / `live_surface_y`
changes.

### Before → after (`perf_profile_sky_height`, same host)

| Stamp | wall | settle | bodies | physics |
|-------|-----:|-------:|-------:|--------:|
| short sky before | 27.4 | 4.65 | 5.68 | 19.8 |
| short sky after | 25.3 | 4.00 | 5.57 | 18.7 |
| tall/demo before | 32.7 | 6.97 | 8.51 | 23.2 |
| tall/demo after | 30.5 | 5.46 | 8.09 | 21.2 |

Settle **−1.5 ms/tick** on demo stamp (−22%); wall **−2.2 ms** (~30 → ~33
sim-FPS). Short `budget_soak`: TRACKED **0.00/t**, park=0.

## Out of scope for this cut

- Coarsening weather / skipping condensation lottery / changing `live_surface_y`
- Enabling rayon by default (still slower on narrow dirty)
- Rock bodies (#1) — next candidate once settle is re-profiled; wake/flood
  budgets already exist
