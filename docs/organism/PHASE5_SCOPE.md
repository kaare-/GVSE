# Phase 5 — Biological system upgrade *(E2 accepted)*

Phase 4 closed (2026-10-11): streaming + 5 km ≥30 sim-FPS. Headless
Phase 0 soaks stay **organisms-off**; Phase 5 soaks turn them **on**.

**Wedge (accepted 2026-10-12, daily default):** **Finish Set E2 on voxel**
— stem integrity / topple first; epiphytes + ghost-root fill follow.

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

## E2 slice plan

1. ~~**Stem integrity + rot topple**~~ — standing-dead woody corpses.
2. ~~**Living stem integrity**~~ — recharge from energy; load from
   modules above; tip when threshold hit.
3. ~~**Epiphytes scaffold**~~ — `Holdfast`, `attach_prefer` /
   `host_leave_fraction` on Genome, Mid-lane share with host Stem,
   `Blueprint::minimal_epiphyte`. Seating / HostLeave shade / reseat
   follow.
4. **Ghost-root fill** — Rock→Organic→Void→Loose + preferential re-root
   (E44).

Out of scope for E2: Set B chem/nerves, Set C niche, creature locomotion
([`CORE_FEATURES.md`](CORE_FEATURES.md) § Explicitly not in Core).

## Acceptance (wedge)

- [x] Named wedge + out-of-scope list frozen in this file.
- [x] Headless unit: fungal rot tips standing-dead stem.
- [ ] Organisms-on short soak gate (TRACKED / park) — follow-up.
- [ ] Stream-on: organism park still correct after bio work.
- [ ] Visual / studio pass for tipped grey logs (1× zoom).

## Non-goals (until Core freeze updates)

Learned nets, active predation chase, freeform drawing, full animal
locomotion, real wood rings / Bezier topple animation — see
[`CORE_FEATURES.md`](CORE_FEATURES.md) § Explicitly not in Core.
