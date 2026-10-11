# Phase 5 — Biological system upgrade *(scope draft)*

*Planning only. No implementation until this doc is accepted and a
follow-up slice PR names acceptance tests + soak hooks.*

Phase 4 closed (2026-10-11): streaming + 5 km ≥30 sim-FPS. Headless
Phase 0 soaks stay **organisms-off**; Phase 5 soaks turn them **on**.

## Intent

Fundamental changes to the live voxel organism stack (`organism.rs`,
`plant.rs`, `fungi.rs`, studio in `wk-voxel-app`) — not a thin skin.
Kernel vocabulary stays MS Paint 1× ([`README.md`](README.md)).

## What already landed (do not re-litigate)

| Area | Status | Docs |
|------|--------|------|
| Set A atoms | Live | [`CORE_FEATURES.md`](CORE_FEATURES.md) |
| Set D plants (saplings, shade, roots) | Live | [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |
| Set E1 fungi (litter, mycelium, Symbiont, spores) | Live | [`FUNGI.md`](FUNGI.md) |
| Organism park under stream (body span) | Live | [`WORLDGEN.md`](../WORLDGEN.md) |
| Studio / blueprint editor | Live | [`EDITOR.md`](EDITOR.md) |

## Open scoping picks *(daily 09:00 Oslo)*

Pick **one** primary wedge before code. Default if no pick: **(1)**.

1. **Finish Set E2 on voxel** — epiphytes, stem integrity / topple,
   ghost-root fill ([`CORE_FEATURES.md`](CORE_FEATURES.md) Set E
   “still later”). Closest to existing plant/fungi code; falsify with
   E40–E44 skeletons in [`SCENARIOS.md`](SCENARIOS.md).
2. **Set B chemical talk + nerves** — `ChemoSensor` / `ChemoEmitter` /
   soma on live atoms ([`CHEM.md`](CHEM.md), [`NERVES.md`](NERVES.md)).
   Needs chem field + energy cost wiring; scenarios E32 / E35.
3. **Set C vertical niche** — buoyancy / temp tolerance / chemosystem
   thin pass ([`CORE_FEATURES.md`](CORE_FEATURES.md)). Niche, not the
   star of the show.
4. **Creature locomotion / lanes** — explicitly **out of Core** today
   ([`CORE_FEATURES.md`](CORE_FEATURES.md) § Explicitly not in Core);
   only promote if owner overrides the freeze. Otherwise defer to a
   later phase (lanes note Phase 7 in [`LANES.md`](LANES.md)).

Secondary (after primary wedge named):

- Soak harness: organisms-on short `budget_soak` + TRACKED / park
  invariants with stream-on Compact (and optional 5 km).
- Studio falsification scenes (E30+) as playable / headless hooks.
- Mass-audit buckets for any new bio sink/source.

## Proposed acceptance (fill when wedge picked)

- [ ] Named wedge + out-of-scope list frozen in this file.
- [ ] At least one headless scenario or soak gate (organisms **on**).
- [ ] Mass audit: new sinks/sources have buckets; short soak `park=0`.
- [ ] Stream-on: organism park still correct (body span); no full-ring
  bio scan regressions on Compact stretch stream+evict.
- [ ] Visual / studio pass called out per Set (1× zoom readable).

## Non-goals (until Core freeze updates)

Learned nets, active predation chase, freeform drawing, full animal
locomotion, real wood rings / Bezier topple animation — see
[`CORE_FEATURES.md`](CORE_FEATURES.md) § Explicitly not in Core.

## Next step after accept

Open the first **implementation** slice PR against
`cursor/wk-voxel-park-bank-fdf9` with code + tests for the chosen
wedge only. Keep daily decision lists until that PR lands.
