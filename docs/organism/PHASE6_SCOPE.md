# Phase 6 — Optimise the biological loop *(baseline landed)*

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

## Proposed slice plan

1. ~~**Organisms-on perf baseline**~~ — Compact 0/48/256 + org splits.
2. **Hotspot cut #1** — thin `land` / `metab` (or skip if owner wants
   stream-on / buildup first); keep TRACKED flat + park=0 on
   `short_organisms_budget_soak`.
3. **Longer organisms-on soak** — 5k then 50k BudgetSnap with plants;
   only if short gate reopens or buildup shows a store drift.
4. **Stream-on life cost** — Compact stream+evict with plants; ensure
   park/restore stays cheap and residency still gates far work.
5. **Buildup / GC** — mycelium strain keys, soft litter, preferential
   overlays, corpse caps under long soak (park or cut).

## Acceptance (proposed)

- [x] Named Phase 6 scope frozen (this file).
- [x] Organisms-on Compact perf baseline logged under
      `/opt/cursor/artifacts/`.
- [ ] At least one measured life-pass cut with soak gate still green.
- [ ] Owner: accept Phase 6 wedge or pick a different Phase 5 follow-up.

## Non-goals

New Set B/C modules, creature locomotion, Phase 7 lighting, client
split — see [`ROADMAP.md`](../ROADMAP.md) and
[`CORE_FEATURES.md`](CORE_FEATURES.md).

## Decision list (2026-10-12, post-baseline)

1. **Default next: stream-on Compact + plants** — wall is physics-bound;
   life cost matters more under stream residency / larger rings than a
   sub-ms metab micro-cut on full-resident Compact.
2. Optional: surgical `land`/`metab` trim if owner wants a measured
   life-pass cut before stream work.
3. Optional longer organisms-on BudgetSnap — only if TRACKED reopens.
