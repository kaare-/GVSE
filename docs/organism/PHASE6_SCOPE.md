# Phase 6 — Optimise the biological loop *(hotspot #2 deferred)*

Phase 5 E2 agent slices closed (2026-10-12). Phase 6 focuses on
**cost and buildup** of the life pass — not new genes.

## Intent

Make organisms-on soaks and stream-on play affordable on Compact and
wider rings without reopening TRACKED / park. Prefer measured cuts over
feature work.

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| E2 integrity / epiphytes / ghost-root | Live | [`PHASE5_SCOPE.md`](PHASE5_SCOPE.md) |
| Organisms-on short soak gate | Live | [`VOXEL_BUDGET_SOAK.md`](../VOXEL_BUDGET_SOAK.md) |
| Stream organism park (body span) | Live | [`WORLDGEN.md`](../WORLDGEN.md) |
| Phase 1/4 physics perf baselines | Live | [`VOXEL_PERF.md`](../VOXEL_PERF.md) |
| Compact organisms-on perf baseline | Live (slice 1) | this file + `perf_profile` |
| Compact stream-on + plants A/B | Live (slice 2) | this file + `perf_profile` |
| Hotspot cut #1 (plant-only contacts) | Live (slice 3) | this file + `organism.rs` |

## Compact baseline (2026-10-12)

Harness: `phase6_organisms_compact_baseline` (release, Compact
4096×1064, warm 40 / measure 200). Log:
`/opt/cursor/artifacts/phase6-organisms-compact-baseline.log`.

| plants | wall ms/tick | org ms/tick | org% |
|-------:|-------------:|------------:|-----:|
| 0 | 52.1 | 0.00 | 0% |
| 48 | 54.8 | 0.25 | ~0.5% |
| 256 | 55.2 | 1.30 | ~2.4% |

At **256 plants**, org splits (ms/tick): **land 0.46**, pose 0.14,
canopy 0.11, post 0.10. Land splits: **metab 0.32**, grow 0.07,
seat 0.04, disperse 0.01.

**Read:** Compact full-resident is still **physics-bound** (~52 ms
wall). Organisms at MAX-ish plant load are ~1.3 ms — the only ≥1 ms
life bucket. Inside that, `land` / `metab` dominates.

## Compact stream-on + plants (2026-10-12)

Harness: `phase6_stream_organisms_compact` (release). Focus at
`width/4`; plants seeded into ±resident (~14 chunks) so stream-on
keeps live org cost (ring-mid focus parks land seeds). Log:
`/opt/cursor/artifacts/phase6-stream-organisms-compact.log`.

| mode | wall ms/tick | org ms/tick | living | parked | chunks |
|------|-------------:|------------:|-------:|-------:|-------:|
| full-resident + 256 (focus band) | 53.3 | 0.61 | 256 | 0 | 1088 |
| stream view=320 + evict + 256 | **12.8** | 0.59 | 256 | 0 | 459 |

Stream org splits (ms/tick): **post 0.11**, pose 0.08, land 0.06,
canopy 0.05. Land: metab 0.02, seat 0.02.

**Read:** Stream cuts wall ~4× (physics/climate residency). With plants
inside the resident band, **org stays ~0.6 ms** (parked=0) — park only
helps far bodies. Under play-like stream+live load, post/pose lead;
ring-wide baseline still shows land/metab as the dense-seed hotspot.

## Hotspot cut #1 (2026-10-12)

Skip wasted plant-only work: `resolve_contacts` unless ≥2 mobile atoms;
epiphyte host/rider maps only when epiphytes exist; soft-mat pose
`HashSet` only when a stemless Photosystem exists.

| harness | before org | after org | post before→after |
|---------|-----------:|----------:|-------------------:|
| stream view=320 + 256 live | 0.59 | **0.29** | 0.11 → **0.007** |
| Compact ring-wide 256 | 1.30 | **0.83** | 0.10 → **0.010** |

Short soak (`short_organisms_budget_soak`, 600t / 48 plants): living=50,
TRACKED −0.01/t, **park=0**. Logs:
`phase6-stream-organisms-hotspot.log`,
`phase6-organisms-compact-hotspot.log`,
`phase6-hotspot-short-soak.log`.

**Read:** Post is no longer a hotspot on plant-only loads. Under
ring-wide seed, **land/metab** (~0.34 ms at 256) remains the largest
named org bucket.

## Hotspot cut #2 — Dig D1 revisit (2026-10-14)

Tried tip×count-always for grounded upright (metab **0.34 → 0.20** on
Compact 256) and stem-integrity every-other-tick. Both regress
`plant_size_is_limited_by_cost_not_by_the_ceiling` (river tip sample /
load-tip timing). Code reverted.

**Dig D1:** grounded upright multi-leaf plants full-sum every **4** ticks;
skip ticks scale cached sum by **tip-sky ratio** (not tip×count). See
[`BIOLOGY_DEPTH.md`](../BIOLOGY_DEPTH.md). Re-profile (2026-10-14):
Compact 256 land/metab **~0.26** ms (was ~0.34); river cost-limit test
+ short org soak park=0 green. Log:
`/opt/cursor/artifacts/phase6-d1-organisms-compact-baseline.log`.

## PreferentialRootPath soft-cap (2026-10-12)

Phase 6 buildup/GC slice 1. Soft-cap [`PREFERENTIAL_ROOT_MAP_MAX`]
(4096) with insert eviction; `clear_preferential_root`; drop competent-rock
stale marks in `fill_ghost_root_voids`.

Short soak 600t/48: living=50, TRACKED −0.01/t, **park=0**,
`preferential_root=0`, `root_residue=0`. Log:
`/opt/cursor/artifacts/phase6-pref-gc-short-soak.log`.

## Soft litter + root_residue soft-cap (2026-10-12)

Phase 6 buildup/GC slice 2. `SOFT_LITTER_MAP_MAX` / `ROOT_RESIDUE_MAP_MAX`
(4096) with insert eviction; `prune_stale_root_residue` each org step.

Short soak 600t/48: living=50, TRACKED −0.01/t, **park=0**,
preferential_root=0, root_residue=0. Log:
`/opt/cursor/artifacts/phase6-litter-gc-short-soak.log`.

## Proposed slice plan

1. ~~**Organisms-on perf baseline**~~ — Compact 0/48/256 + org splits.
2. ~~**Hotspot cut #1**~~ — plant-only contact / epi / soft-pile skips;
   soak still green.
3. ~~**Longer organisms-on soak**~~ — 5k organisms-on green (park=0,
   overlays=0); 50k only if TRACKED hard-gate reopens.
4. ~~**Stream-on life cost**~~ — Compact stream+evict with plants;
   park/restore cheap; residency gates far work; live-in-view ~0.6 ms
   pre-cut / ~0.3 ms post-cut.
5. ~~**Hotspot cut #2 (land/metab)**~~ — **deferred**: tip×count-always
   starves river tips (submerged tip sample); stem-integrity cadence
   delays load-tip that river plants need to outgrow cost caps. Revisit
   only with a harvest-neutral cut.
6. ~~**PreferentialRootPath soft-cap**~~ — map max 4096 + rock stale drop;
   soak park=0.
7. ~~**Soft litter + root_residue soft-cap**~~ — map max 4096 + stale
   Organic prune; soak park=0.
8. ~~**5k organisms-on soak**~~ — park=0, overlays=0, living=59.

## Acceptance (proposed)

- [x] Named Phase 6 scope frozen (this file).
- [x] Organisms-on Compact perf baseline logged under
      `/opt/cursor/artifacts/`.
- [x] Compact stream-on + plants A/B logged (focus-band seed).
- [x] At least one measured life-pass cut with soak gate still green.
- [x] Preferential + litter/residue soft-caps landed; short soak green.
- [ ] Owner: accept Phase 6 park or pick longer soak / Phase 7.

## Non-goals

New Set B/C modules, creature locomotion, Phase 7 lighting, client
split — see [`ROADMAP.md`](../ROADMAP.md) and
[`CORE_FEATURES.md`](CORE_FEATURES.md).

## 5k organisms-on soak (2026-10-12)

Harness: `short_organisms_budget_soak` with `GVSE_SOAK_TICKS=5000`
plants=48. Log: `/opt/cursor/artifacts/phase6-organisms-5k-soak.log`.

living=59, corpses=5, TRACKED −0.33/t, d_min=+260, **park=0**,
preferential_root=0, root_residue=0. Overlays stay flat under GC soft-caps.

## Decision list (2026-10-12, Phase 6 agent park)

1. **Agent park:** baselines, stream+plants, hotspot #1, preferential +
   litter/residue GC landed; hotspot #2 deferred (river tip/growth).
2. **Optional next:** 5k organisms-on BudgetSnap if owner wants longer
   overlay drift proof (artifact in flight / landed below).
3. **Default roadmap next:** Phase 7 surface spice, or owner visual on
   E2 tipped grey logs ([`PHASE5_SCOPE.md`](PHASE5_SCOPE.md)).
