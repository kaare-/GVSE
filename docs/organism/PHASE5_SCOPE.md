# Phase 5 — Biological system upgrade *(E2 agent-parked)*

Phase 4 closed (2026-10-11): streaming + 5 km ≥30 sim-FPS. Headless
Phase 0 soaks stay **organisms-off**; Phase 5 soaks turn them **on**.

**Wedge (accepted 2026-10-12):** **Finish Set E2 on voxel** — stem
integrity / topple, epiphytes, ghost-root fill. **Agent slices 1–7 +
soak/park gates landed** (2026-10-12). Owner visual for tipped grey
logs remains open; not a code blocker.

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
| E2 stem integrity (live + corpse topple) | Live (slices 1–2) | this file + [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |
| E2 Holdfast Mid-lane share + attach genes | Live (slice 3 scaffold) | this file + [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |
| E2 epiphyte host-Stem seating | Live (slice 4) | this file + [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |
| E2 HostLeave smother shade | Live (slice 5) | this file + [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |
| E2 attach_prefer reseat | Live (slice 6) | this file + [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |
| E2 ghost-root fill (E44) | Live (slice 7) | this file + [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) |

## E2 slice plan

1. ~~**Stem integrity + rot topple**~~ — standing-dead woody corpses.
2. ~~**Living stem integrity**~~ — recharge from energy; load from
   modules above; tip when threshold hit.
3. ~~**Epiphytes scaffold**~~ — `Holdfast`, `attach_prefer` /
   `host_leave_fraction` on Genome, Mid-lane share with host Stem,
   `Blueprint::minimal_epiphyte`.
4. ~~**Epiphyte seating**~~ — habitat spawn requires Holdfast on a
   living host Stem; unseated riders die within a few ticks (photo
   only).
5. ~~**HostLeave shade**~~ — same-column riders attenuate host light by
   `(1 − leave) × cast` (`epiphyte_rider_transmit`).
6. ~~**attach_prefer reseat**~~ — unseated / near-miss Holdfast seeks
   nearby Stem (`try_epiphyte_reseat`).
7. ~~**Ghost-root fill**~~ — Organic residue→Void→Loose + preferential
   re-root (E44); competent Stone stays rock on death.

Out of scope for E2: Set B chem/nerves, Set C niche, creature locomotion
([`CORE_FEATURES.md`](CORE_FEATURES.md) § Explicitly not in Core).

## Acceptance (wedge)

- [x] Named wedge + out-of-scope list frozen in this file.
- [x] Headless unit: fungal rot tips standing-dead stem.
- [x] Organisms-on short soak gate (TRACKED / park) —
  `short_organisms_budget_soak` 600t / 48 plants: TRACKED **−0.01/t**,
  park=0, living=50 (log
  `/opt/cursor/artifacts/phase5-e2-organisms-soak.log`).
- [x] Stream-on: organism park still correct after bio work —
  `sync_organism_residency_*` unit tests green on tip after E44.
- [ ] Visual / studio pass for tipped grey logs (1× zoom) — owner /
  play session (not headless).

## Non-goals (until Core freeze updates)

Learned nets, active predation chase, freeform drawing, full animal
locomotion, real wood rings / Bezier topple animation — see
[`CORE_FEATURES.md`](CORE_FEATURES.md) § Explicitly not in Core.

## Decision list (2026-10-12, post-E2)

E2 agent work is parked. Open forks (defaults in **bold**):

1. **Owner visual** for tipped grey logs at 1× — play/Xvfb when Kaare
   has eyes; agent does not block on this.
2. **Next phase default: Phase 6 scope** — optimise the biological loop
   (organisms-on cost / buildup under longer soak + larger maps). Draft
   [`PHASE6_SCOPE.md`](PHASE6_SCOPE.md) + organisms-on perf baseline
   before deep cuts.
3. Optional: longer organisms-on BudgetSnap (5k / 50k) — only if TRACKED
   reopens after short gate; short gate already flat (−0.01/t).
4. Optional Phase-4 FPS leftovers (5 km wind.rebuild / rock bodies) —
   only if owner wants more stream FPS; not a Phase-5 blocker.
