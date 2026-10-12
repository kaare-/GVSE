# Phase 6 — Optimise the biological loop *(draft)*

Phase 5 E2 agent slices closed (2026-10-12). Phase 6 starts when the
life pass is the product focus — **cost and buildup**, not new genes.

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

## Proposed slice plan

1. **Organisms-on perf baseline** — `perf_profile` / Compact with
   seeded plants; name the top life-pass wall buckets (pose, canopy,
   fungi field, elongate, corpse).
2. **Hotspot cut #1** — largest ≥1 ms life bucket from (1); keep
   TRACKED flat + park=0 on `short_organisms_budget_soak`.
3. **Longer organisms-on soak** — 5k then 50k BudgetSnap with plants;
   only if short gate reopens or buildup shows a store drift.
4. **Stream-on life cost** — Compact stream+evict with plants; ensure
   park/restore stays cheap and residency still gates far work.
5. **Buildup / GC** — mycelium strain keys, soft litter, preferential
   overlays, corpse caps under long soak (park or cut).

## Acceptance (proposed)

- [ ] Named Phase 6 scope frozen (this file).
- [ ] Organisms-on Compact perf baseline logged under
      `/opt/cursor/artifacts/`.
- [ ] At least one measured life-pass cut with soak gate still green.
- [ ] Owner: accept Phase 6 wedge or pick a different Phase 5 follow-up.

## Non-goals

New Set B/C modules, creature locomotion, Phase 7 lighting, client
split — see [`ROADMAP.md`](../ROADMAP.md) and
[`CORE_FEATURES.md`](CORE_FEATURES.md).

## Decision list (2026-10-12)

1. **Accept Phase 6 as next** (default) — start slice 1 baseline.
2. Defer Phase 6 until owner visual / playtest of E2 tipple — agent
   parks and only runs daily lists.
3. Optional Phase-4 FPS leftovers instead — only if owner asks.
