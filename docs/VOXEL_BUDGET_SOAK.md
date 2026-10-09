# Headless / Xvfb budget soaks

Overnight GUI `B` soaks are expensive. Prefer these for mineral / water leftover hunts.

## Headless BudgetSnap (default for mineral)

Uses the same `step_world` order as the play app (evap / cond / karst / phase on, organisms off) and prints overlay stores + probes.

Short CI:

```bash
cargo test -p wk-voxel --test budget_soak --release short_budget_soak -- --nocapture
```

Long / overnight (run in a **subagent** so the main agent keeps working):

```bash
./scripts/budget_soak.sh
# or
GVSE_SOAK_TICKS=250000 GVSE_BUDGET_PERIOD=60 \
  cargo test -p wk-voxel --test budget_soak --release long_budget_soak \
  -- --ignored --nocapture
```

Env: `GVSE_SOAK_TICKS`, `GVSE_BUDGET_PERIOD`, `GVSE_BUDGET_WARM`.

## Xvfb + screenshot (complex / visual)

When the leftover needs eyes on the world (geyser, lake ring, shatter):

```bash
./scripts/xvfb_app.sh /tmp/gvse-budget.png
```

Needs `Xvfb`, `xdotool`, and `import` (ImageMagick) or `scrot`. Presses `B` after launch.

Prefer a subagent for the long wait. Drop screenshots under `/opt/cursor/artifacts/` when attaching to a PR.

## Work cycle

- Oslo 09:00 local: decision list in this Cursor chat (timer is 07:00 UTC while CEST).

### TRACKED mint @5k when snow precip is on (2026-10-08)

Headless `short_budget_soak` on the Phase 3 ice tip showed TRACKED
`~+130…136/t` with `park=0`. Differential `GVSE_SOAK_OFF`:

| OFF | TRACKED /t | note |
|-----|------------|------|
| *(none)* | +131…136 | snow store rises |
| `snow` (`enable_snow_precip=false`) | ~0 | sharpest kill (harness hook) |
| `cond` + surplus gated (hunt-local) | ~0 | both flake paths off |
| `cond` alone | still + | thermal-surplus snow remains |
| `steam` / `phase` / `cadence` | still + | not the gate |

Flake humidity debit probes clean (`under=0`). Seat path ruled out
(`phase::airborne_snow_seat_over_steam_stays_tracked_flat`).

**Not the 5k mint:** dual-axis humidity flux over-donate when
`|vx|+capped|vy|>1` was real (unit-repro + joint leave scale in
`flux_both_into` / `flux_both`) but a 5k remeasure after the fix is still
TRACKED `+130.86/t` (identical stores to pre-fix baseline). Keep the
conservation fix; it is not the snow soak leftover.

**Still open (store split):** with `OFF=phase`, paid≈Δsnow yet TRACKED
`+111/t` — mint is post-seat coupling. Relative to `OFF=snow`, the leftover
lines up with **extra humidity** (~mint-sized) plus free/pore retained as
if evap slowed; not steam scrub (`OFF=steam` still +). Dense-slab
`take_around` is visible to `total_mass` (unit). Soak `hum_adv` on
advect_with_surface is ~0 (±tens) while TRACKED is +10…130/t — **not
advect**.

**Diffuse / evap try_add / orphan (2026-10-09):** 5k attribution on the
snow-mint probe tip. Probes: `hum_dif`, `evap_add`/`evap_debit`,
`orphan_rm`. `OFF=diffuse` (α=0) and `OFF=orphan` (crest-film 8× off).

| OFF | TRACKED /t | note |
|-----|------------|------|
| *(none)* | **+136.01** | `hum_dif≈−88`, `evap_add=evap_debit`, `orphan_rm≈25k` |
| `snow` | **−0.01** | kill unchanged |
| `diffuse` | **+132.13** | `hum_dif=0` — not the mint |
| `orphan` | **+133.98** | `orphan_rm=0` — not the mint |
| `phase` | **+110.96** | paid≈Δsnow; triad `Δ(free+pore+hum+snow)≈+mint` |

Evap apply pairs try_add with sat debit (`evap_add==evap_debit` on every
window). Defensive skip when a cell is no longer Air at apply (Snow thaw
sat×capacity-0 trap) is kept + unit-tested; collect→apply is back-to-back
in `step_world`, so that race is not the 5k leftover.

**Best remaining suspect:** post-seat coupling that raises the
free+pore+hum+snow triad once flakes exist — not diffuse, not orphan
boost, not evap try_add/debit asymmetry. Next: post-evap free-sat restore
/ snow-fall↔standing-water, or another writer that re-adds sat after a
paired free→hum move. Kill switch: `GVSE_SOAK_OFF=snow`.
- Automerge when CI green once GitHub auto-merge is available to the agent; until then mark ready and note merge wait.
- Hard problems: verify headless and/or Xvfb before asking to merge.
