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

## Phase 0 — Mass budgets hold *(current)*

**Goal:** Overnight / long headless soaks show TRACKED and min.tot rates
near flat with `park=0`, `clip=0`, `clamp=0`. Short soaks are necessary
but not sufficient (50k-tick soak can reopen a −3/t water term after a
flat 120-tick run).

**Done recently**

- Park leftover banked; karst convert credited.
- Overlay body / crush dest salvage (#348).
- Headless BudgetSnap harness + Xvfb scripts (#349).
- Sandstone/Conglomerate cement emit on crush/shatter (`write_debris_cell`).

**In flight**

- UNEXPL-W ~−3/t on 50k-tick headless soak (mineral ~flat at +0.13/t).

**Exit criteria (proposed — confirm)**

| Signal | Gate |
|--------|------|
| Headless 50k+ ticks, default stamp | `\|rate W\|` and `\|rate M\|` under agreed eps |
| `park` / `clip` / `clamp` | stay 0 |
| GUI overnight optional | only after headless is green; subagent-owned |

Open: exact eps (e.g. `< 0.5/t` vs `< 1/t`), whether plant/organism
passes must be on for the gate.

---

## Phase 1 — Performance for larger worlds & long soaks

**Goal:** Tick cost stays playable as map size and soak age grow.
Buildup (dissolved maps, steam/pipe ledgers, dirty sets, landscape
bodies) must not produce the “fresh world fine → morning at 5 FPS” shape.

**Likely levers** (see also `VOXEL_PARALLEL.md`)

- Active-set / dirty discipline under long soak.
- Container growth (`dissolved`, steam, leftover pins, level-vacated).
- Wider maps: chunk streaming / ring already sketched in `WORLDGEN.md`.
- Profile before coarsening weather or skipping lottery.

**Exit criteria (proposed)**

- Profiled demo + larger stamp: ms/tick and memory vs soak age documented.
- No intentional mass-budget regressions (re-run short + mid headless).

---

## Phase 2 — Geysers / hydrothermal tighten

**Goal:** Feel local and readable; less glitchy route churn.

**Owner notes (to refine)**

- Changes routes too often.
- Walking to the surface feels a little **global-knowledge-ish**.
- Prefers **vertical** a bit too much.
- Should open **underwater hot springs**, not only aerial vents.

**Scope guard:** mass-flat mouth / park / sinter paths already chased;
this pass is behaviour and presentation, not reopening destroy holes.

**Exit criteria (proposed)**

- Headless leftover rates still hold near vents.
- Playtest / Xvfb: fewer route flips; visible UW hot-spring behaviour.

---

## Phase 3 — Ice revisit

**Goal:** Ice is an old material with quirks. Treat as a concentrated
pass, not drive-by tweaks.

**Owner notes**

- Behaves as a **semisolid powder** that lets water run through —
  looks strange sometimes.
- Needs coherent rules for powder vs solid, throughflow, melt/freeze
  presentation.

**Exit criteria (proposed)**

- Written ice model in docs + tests for throughflow / melt / load.
- Visual soak (Xvfb or playtest) accepted as “reads as ice”.

---

## Phase 4 — Optimisation again + larger worlds

**Goal:** After ice/geyser churn, re-profile and push map scale.
May include ring width / ceiling / streaming follow-ups from
`WORLDGEN.md`.

---

## Phase 5 — Biological system upgrade *(major)*

**Goal:** Big organism/ecology pass. **Requires its own scoping doc**
before implementation (genome/bodyplan, plants/fungi/creatures,
caps, perf).

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

**Goal:** Split sim presentation from ownership. Evaluate whether the
way we show the sim to the user can be upgraded. Heavy work and
testing; own architecture note before coding.

---

## Decision log (fill as answers land)

| When | Topic | Decision |
|------|--------|----------|
| | Phase 0 rate eps | |
| | Phase 0 organisms on/off for gate | |
| | Target playable map size for Phase 1/4 | |
| | Geyser “local knowledge” rule | |
| | Ice throughflow: keep / restrict / rewrite | |
| | Phase 5 scope owner doc | |
| | Client tech preference (Phase 8) | |

## Clarifications still needed

See the agent chat for the live question list. Answers get copied into
the decision log above.
