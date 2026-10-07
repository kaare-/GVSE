# wk-voxel roadmap (next ~1–2 months)

Owner-facing plan. Dates are not estimates — phases are ordered gates.
Mass budget confidence is the entry ticket for everything after Phase 0.

Companion docs: [`VOXEL_WATER.md`](VOXEL_WATER.md) (B overlay / leftover),
[`VOXEL_BUDGET_SOAK.md`](VOXEL_BUDGET_SOAK.md) (headless / Xvfb soaks),
[`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md) (threading), [`WORLDGEN.md`](WORLDGEN.md)
(world size / streaming), organism docs under [`organism/`](organism/).

## Operating loop (already running)

- Daily **09:00 Oslo**: decision list in the Cursor agent chat (only
  blockers / forks that need Kaare).
- Prefer **headless BudgetSnap** soaks (`./scripts/budget_soak.sh`) via
  subagents; use **Xvfb** (`./scripts/xvfb_app.sh`) when eyes are needed.
- Stacked draft PRs → CI green → ready → **Enable auto-merge** on the PR
  (repo setting alone is not enough). Bases are often not `main`, so
  filter the PR list by head branch if a number “disappears”.

---

## Phase 0 — Mass budgets hold *(done 2026-10-07)*

**Goal:** Overnight / long headless soaks show TRACKED and min.tot rates
near flat with `park=0`, `clip=0`, `clamp=0`. Short soaks are necessary
but not sufficient (50k-tick soak can reopen a −3/t water term after a
flat 120-tick run).

**Done**

- Park leftover banked; karst convert credited.
- Overlay body / crush dest salvage (#348).
- Headless BudgetSnap harness + Xvfb scripts (#349).
- Sandstone/Conglomerate cement emit on crush/shatter (`write_debris_cell`).
- Wet-film snow/frost credit; lateral ice/snow relocate; no vapour-sat
  park on Ice/Snow; alpine film-park skip (#351).

**Exit (met)** — headless 50k on tip `0d7b6a6` / `#351`:

| Signal | Result |
|--------|--------|
| `\|TRACKED\|/t` | **0.03** (d=−1465) |
| min.tot `/t` | **+0.12** (d=+5827) |
| `park` | **0** |
| wall | ~66 min |

---

## Phase 1 — Performance for larger worlds & long soaks *(current)*

**Goal:** Tick cost stays playable as map size and soak age grow.
Buildup (dissolved maps, steam/pipe ledgers, dirty sets, landscape
bodies) must not produce the “fresh world fine → morning at 5 FPS” shape.

**Targets** (2026-10-07)

- At least **30 FPS** playable; raise the bar as we test.
- Grow world size until diminishing returns, then discuss next levers.
- Stretch aim: **~5 km+** horizontal at **~1064** world height — may be
  unrealistic; treat as aspiration, not a Phase-1 exit requirement.

**Likely levers** (see also `VOXEL_PARALLEL.md`)

- Active-set / dirty discipline under long soak.
- Container growth (`dissolved`, steam, leftover pins, level-vacated).
- Wider maps: chunk streaming / ring already sketched in `WORLDGEN.md`.
- Profile before coarsening weather or skipping lottery.

**Exit criteria**

- Profiled demo + larger stamp: ms/tick and memory vs soak age documented.
- No intentional mass-budget regressions (re-run short + mid headless).
- Stop when returns flatten; owner discussion before heroic further cuts.

---

## Phase 2 — Geysers / hydrothermal tighten

**Goal:** Feel local and readable; less glitchy route churn.

**Owner notes** (2026-10-07)

- Changes routes too often — needs testing and tuning.
- Walking to the surface feels a little **global-knowledge-ish** → prefer
  **local pressure / pathfinding only**, plus hard limits as needed
  (max horizontal seek / connected-void). Tune in play.
- Prefers **vertical** a bit too much.
- Open **underwater hot springs** into standing lake water **and**
  **seafloor vents** into the ocean column.

**Scope guard:** mass-flat mouth / park / sinter paths already chased;
this pass is behaviour and presentation, not reopening destroy holes.
Side note for later: geyser water cooling too fast in sub-zero air is
tied to water not carrying temperature (see Phase 3).

**Exit criteria**

- Headless leftover rates still hold near vents.
- Playtest / Xvfb: fewer route flips; visible lake UW springs + seafloor vents.

---

## Phase 3 — Ice revisit

**Goal:** Ice is an old material with quirks. Treat as a concentrated
pass, not drive-by tweaks.

**Owner notes** (2026-10-07)

- Direction: toward a **brittle solid**, not powder-throughflow as the
  default look.
- Biggest suspect: **water particles don’t carry temperature**, so
  standing water on ice can’t really melt it. May need a larger rewrite;
  **discuss when we get here**, don’t pre-commit the design.
- Related: geyser discharge cooling too fast in mostly sub-zero climates
  (same missing water-T bookkeeping).

**Exit criteria**

- Agreed ice model in docs + tests after the design discussion.
- Visual soak (Xvfb or playtest) accepted as “reads as ice”.

---

## Phase 4 — Optimisation again + larger worlds

**Goal:** After ice/geyser churn, re-profile and push map scale.
May include ring width / ceiling / streaming follow-ups from
`WORLDGEN.md`.

---

## Phase 5 — Biological system upgrade *(major)*

**Goal:** Big organism/ecology pass. Lots of **fundamental changes** to
the system we have — not a thin skin. **Requires its own major planning
session** and scoping doc before implementation.

Headless soaks for this phase turn **organisms on** (Phase 0 gate stays
organisms-off).

Do not start coding until a Phase-5 scope PR exists with acceptance
tests and soak hooks.

---

## Phase 6 — Optimise the biological loop

**Goal:** After the upgrade lands, cost and buildup of the life pass
under long soak / larger maps.

---

## Phase 7 — Surface spice (atmosphere & light)

**Goal:** Hopefully simple but important presentation:

- Colors; sunrise / sunset
- Moon + moon cycle
- Seasons
- Stars that move over time and seasons
- Shadows / shading from sundown
- Moonlight glitter on water

**Exit criteria (proposed):** visual acceptance on Xvfb/playtest;
no sim-mass regressions from lighting-only work.

---

## Phase 8 — Server / client *(huge)*

**Goal:** Split sim presentation from ownership. Review alternatives;
prefer a **production-compatible client** that can pass **Apple and/or
Steam** inspection. Heavy work and testing; own architecture note
before coding.

---

## Decision log

| When | Topic | Decision |
|------|--------|----------|
| 2026-10-07 | Phase 0 rate eps | Tight: `\|rate W\|` and `\|rate M\|` &lt; **0.5/t** on 50k+ headless |
| 2026-10-07 | Phase 0 organisms | **Off** until Phase 5 bio rewrite |
| 2026-10-07 | Phase 0 closed | 50k headless: TRACKED **0.03/t**, min **+0.12/t**, park=0 (#351) |
| 2026-10-07 | Phase 1 FPS / size | ≥**30 FPS**; grow size until diminishing returns; stretch **5 km+ @ ~1064 h** |
| 2026-10-07 | Geyser locality | Local pressure/pathfinding (+ hard seek limits); tune in play |
| 2026-10-07 | UW / ocean vents | Lake underwater springs **and** seafloor ocean-column vents |
| 2026-10-07 | Ice direction | Toward **brittle solid**; water-T rewrite likely; discuss at Phase 3 |
| 2026-10-07 | Phase 5 bio | Fundamental look + fundamental changes; own major planning session |
| 2026-10-07 | Phase 8 client | Review alts; production path for Apple/Steam inspection |

## Clarifications later

- Exact Phase-1 “diminishing returns” stop rule (ms/tick vs world cells).
- Ice water-temperature design options when Phase 3 starts.
- Phase-5 scoping agenda (plants/fungi vs creatures first — open until planning session).
- Phase-8 shortlist of client stacks when that review starts.
