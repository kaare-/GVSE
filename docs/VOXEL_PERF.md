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

Phase 0 close (before Phase 1 surgical wins):

| Stamp | wall ms/tick | ~sim-FPS | physics | parallel A/B |
|-------|-------------:|---------:|--------:|--------------|
| short sky | 27.4 | ~36 | 19.8 | — |
| demo | 33.0 | ~30 | 23.6 | FPS OFF 32.4 / ON 34.1; full_feel OFF 119 / ON 117 |
| stress | 56.2 | ~18 | 40.7 | — |

### Size sweep after settle + bodies + seam-apply win

Same host, warm 40 / measure 200, `PerfConfig` FPS defaults, parallel **OFF**
unless noted (`perf_profile_demo_and_stress` / `perf_profile_sky_height`):

| Stamp | wall ms/tick | ~sim-FPS | physics | seepage | settle | bodies |
|-------|-------------:|---------:|--------:|--------:|-------:|-------:|
| short sky | 19.3 | ~52 | 13.4 | 3.15 | 3.90 | 1.70 |
| demo | 21.4 | ~47 | 13.9 | 2.89 | 5.48 | 2.16 |
| stress (2048×1064) | 34.0 | ~29 | 21.2 | 4.78 | 6.27 | 4.76 |

Demo parallel A/B (0 plants): FPS OFF 20.1 / ON 19.3; full_feel OFF 73.6 / ON 64.4.
Demo + plants: +48 ≈ 21.0 ms, +256 ≈ 25.1 ms (org share ≤3%).

### Size sweep after settle Air-dest (dirty-clear experiment)

Same harness on seep tip + Air-dest trim (`perf_profile_demo_and_stress`).
Figures include a multi-pass `clear_all_dirty` that was later reverted — see
§4; re-profile for the shipped Air-trim-only settle path:

| Stamp | wall ms/tick | ~sim-FPS | physics | seepage | settle | bodies |
|-------|-------------:|---------:|--------:|--------:|-------:|-------:|
| short sky | 16.3 | ~61 | 10.1 | 3.26 | 0.53 | 1.80 |
| demo | 17.2 | ~58 | 9.3 | 2.93 | 0.78 | 2.26 |
| stress (2048×1064) | 27.2 | ~37 | 14.2 | 4.93 | 0.36 | 3.86 |

Demo parallel A/B (0 plants): FPS OFF 17.0 / ON 16.9; full_feel OFF 33.0 / ON 30.1.
Demo + plants: +48 ≈ 17.3 ms, +256 ≈ 18.2 ms (org share ≤3%).

## Hottest physics sub-passes (demo 0 plants)

Baseline at Phase 0 close (before Phase 1 surgical wins):

| Pass | ms/tick | Share of wall |
|------|--------:|--------------:|
| rock bodies | 8.49 | 26% |
| settle grains | 7.07 | 21% |
| seepage | 4.29 | 13% |
| gravity | 0.80 | 2% |
| plan+clear dirty | 0.69 | 2% |
| water flow | 0.53 | 2% |

Re-profile after settle + bodies (#353), before seam-apply win — demo hotspots
were **settle ~7.4**, **seepage ~4.4**, bodies ~2.1 (docs had guessed seepage ~3.9 /
settle ~5.5; settle was noisier on this host).

Frame shell (outside physics): humidity.advect ~1.7, steam ~1.1, temperature
amortized ~0.9, evap ~0.7. Condensation ~0.22 — **do not** coarsen weather /
lottery before these CA tails shrink.

Active plan (demo): ~16 regions / ~2470 cells per flow substep; avg ~7.2
substeps/tick with quiet early-out.

## Surgical wins (this branch)

### 1. Settle grains (sticky-loose dirty)

Multi-pass `settle_loose_grains_regions_ex` already filtered sticky-`has_loose`
for the *repose* re-plan, but assigned the next fall pass from unfiltered
`plan_active` — so after seepage dirtied stone/limestone pores, every settle
pass re-walked that water halo.

**Change:** `cur = keep_loose_regions(world, &plan_active(world))` after each
settle pass (same gate as repose). No weather / condensation / `live_surface_y`
changes.

| Stamp | wall | settle | bodies | physics |
|-------|-----:|-------:|-------:|--------:|
| short sky before | 27.4 | 4.65 | 5.68 | 19.8 |
| short sky after | 25.3 | 4.00 | 5.57 | 18.7 |
| tall/demo before | 32.7 | 6.97 | 8.51 | 23.2 |
| tall/demo after | 30.5 | 5.46 | 8.09 | 21.2 |

Settle **−1.5 ms/tick** on demo stamp (−22%); wall **−2.2 ms**.

### 2. Rock bodies (sleep seated strata floods)

After settle, rock bodies remained #1 (~7.9 ms/tick on this host). Probe on the
demo stamp showed ~10k `flood_cells`/tick with ~80% from `FLOOD_GATHER_CAP`
strata bailouts: empty hang after a 2048-cell gather never slept the cells, so
every topology pass re-flooded the same hillside.

**Change** (in `build_components`):

- On untagged seated strata bailout with empty hang: settle the gather (and
  same-pass `settle_pending`) so later passes/ticks skip those seeds.
- Seated tag-0 floods refuse settled / pending neighbours (stops re-absorbing
  the hillside).
- Sleep only when the gather is **not** `cell_set_floating` (sky islands
  must stay eligible to peel). Soft-bed hung shards wake; tagged
  partial-support cantilevers hang-peel so leftovers finish falling.

No weather / condensation / `live_surface_y` / mass-ledger changes.

### Before → after bodies win (`perf_profile_sky_height`, same host)

Tip after settle win → after strata sleep (warm 40 / measure 200):

| Stamp | wall | settle | bodies | physics |
|-------|-----:|-------:|-------:|--------:|
| short sky before | 23.3 | 3.68 | 5.35 | 16.7 |
| short sky after | 20.9 | 4.07 | 2.11 | 14.1 |
| tall/demo before | 27.2 | 5.17 | 7.87 | 19.2 |
| tall/demo after | 22.0 | 5.49 | 2.63 | 14.2 |

Bodies **−5.2 ms/tick** on demo stamp (−67%); wall **−5.2 ms** (~37 → ~45
sim-FPS). Short `budget_soak`: TRACKED **0.00/t**, park=0.

### 3. Seepage seam apply (skip full↔full + run bands)

Re-profile after #353: seepage ~4.4 ms/tick on demo. `seepage_split_probe`
showed **seam_coupling ~2.5 ms/call** (hottest seepage component). On a warmed
demo world every wet seam band was full-width 64, and ~56% of face columns were
quiet pore↔pore **both at capacity** — the accumulate walk no-ops those faces,
but `seam_coupled_span` still emitted a min..=max rect over them. HashMap-merging
runs by x also re-filled dry gaps; merging a middle chunk’s top+bottom strips
could balloon y to full height.

**Change** (apply band only — wake still visits every wet column for downward
fronts):

- `seam_coupled_runs`: emit contiguous x-runs of face columns that can still
  transfer; skip both-at-capacity pore↔pore (same gate as deep accumulate).
- ±1 x halo so a full neighbour still owns the +x face into a column with room.
- One `ActiveChunk` per run (no HashMap min/max merge).

No weather / condensation / `live_surface_y` / cadence changes.

### Before → after seam-apply win (`perf_profile_sky_height`, same host)

Tip after bodies win → after seam runs (warm 40 / measure 200):

| Stamp | wall | seepage | settle | bodies | physics |
|-------|-----:|--------:|-------:|-------:|--------:|
| short sky before | 20.5 | 4.87 | 3.72 | 1.59 | 14.9 |
| short sky after | 19.2 | 3.15 | 3.84 | 1.70 | 13.4 |
| tall/demo before | 24.7 | 4.42 | 7.45 | 2.14 | 17.5 |
| tall/demo after | 21.0 | 2.85 | 5.31 | 2.11 | 13.6 |

`seepage_split_probe` seam_coupling: demo **2.47 → 0.35 ms/call**; stress
**4.77 → 0.73 ms/call**. Seepage bucket **−1.6 ms/tick** on demo (−35%); wall
**−3.7 ms** (~40 → ~48 sim-FPS). Short `budget_soak`: TRACKED **0.00/t**, park=0.

### 4. Settle Air destinations (keep seepage dirty)

Re-profile after seam-apply: settle still **~5.3 ms/tick** on demo (docs’
~5.5–7.4 band). `settle_air_probe` showed the sticky-loose plan was **~71%
non-Air** — seepage pore dirty inside `has_loose` chunks. Fall and repose only
pull into Air, so those solid visits were pure waste.

**Change** (in `settle_loose_grains_regions_ex` only):

- Trim each settle scan to **Air destinations** (sparse bitset).
- Multi-pass re-plans use sticky-loose + Air-dest (`settle_scan_regions`) so
  wet-pore cells are not walked ×N.
- Do **not** `clear_all_dirty` inside settle: that wiped seepage pore dirty
  that next tick’s flow/seepage (and lake-bed / beach / well wakes) need.
  Filter the scan mask; leave global dirty for the wetting wake.

No weather / condensation / lottery / `live_surface_y` changes.

### Before → after settle Air-dest win (`perf_profile_sky_height`, same host)

Tip after seam-apply → after Air-dest trim (warm 40 / measure 200). Numbers
below include a brief dirty-clear experiment that was reverted for seepage
correctness; expect settle closer to the Air-trim-only band than the cleared
re-plan extreme:

| Stamp | wall | seepage | settle | bodies | physics |
|-------|-----:|--------:|-------:|-------:|--------:|
| short sky before | 19.5 | 3.21 | 3.87 | 1.71 | 13.5 |
| short sky after (Air-trim+clear*) | 16.3 | 3.25 | 0.53 | 1.81 | 10.1 |
| tall/demo before | 21.3 | 2.93 | 5.29 | 2.14 | 13.8 |
| tall/demo after (Air-trim+clear*) | 16.8 | 2.87 | 0.76 | 2.24 | 9.1 |

\*dirty-clear inside multi-pass settle broke lake-bed / beach / well soak
tests; shipped path keeps Air-dest trim without clearing seepage dirty.
Air-trim alone still drops the ~71% non-Air visits. Re-profile after the
revert when hunting the next settle leftover.

## Out of scope / next

- Coarsening weather / skipping condensation lottery / changing `live_surface_y`
- Enabling rayon by default (still slower on narrow dirty)
- Next CA tails once re-profiled: seepage wakes (lake-bed / seam wake / weep),
  humidity.advect / steam / temperature amortized, rock bodies — settle is
  no longer the top CA cost
