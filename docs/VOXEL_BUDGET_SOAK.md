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

**Snow descent / landing (2026-10-09):** 5k on tip after
`OFF=snowfall|snowwet|slush` hooks (`GrainConfig::enable_airborne_snow_fall`
/ `enable_snow_wet_fall`, `PhaseConfig::enable_slush`). Window onset
still ~2.0–2.5k. Unit: snow↔haze swap stays TRACKED-flat.

| OFF | TRACKED /t | note |
|-----|------------|------|
| *(none)* | **+130.86** | paid=860k, Δsnow+ice=219k, paid−phase≈+640k≈mint |
| `snowfall` | **+15.66** | flakes nucleate but do not descend; Δsnow≈paid |
| `snowwet` | **+134.94** | empty-Air fall only — haze/film swap **not** the mint |
| `slush` | **+139.72** | water-on-ice / snow-on-water phase off — not the mint |
| `snow` | **−0.01** | kill unchanged |

**Narrowed:** mint is **post-descent / near-surface** coupling once flakes
can leave the sky — not nucleation underpay, not haze/film swap, not
slush. With `OFF=snowfall`, late windows are near-flat while snow banks
aloft; baseline late windows mint large **free** (+150k/500t) with hum
barely dropping. Residual `OFF=snowfall` ~+16/t may be drift or weak
sky-snow coupling. Kill: `GVSE_SOAK_OFF=snow`.

**Post-descent lid / crest (2026-10-09):** 5k after `OFF=snowraft|snowsurf`
(`GrainConfig::enable_snow_float`, `set_peel_seated_snow`). Windowed free
still explodes late when flakes can land. Vs `OFF=phase` (descent on,
thaw off) still **+111/t** — mint is not melt accounting.

| OFF | TRACKED /t | note |
|-----|------------|------|
| *(none)* | **+130.86** | late free ≈+154k/500t |
| `snowraft` | **+159.02** | Snow sinks through lakes — **worse**; lake float lid not the mint |
| `snowsurf` | **+140.85** | live_surface/live_skin peel seated Snow — weather crest not the mint |
| `snowfall` | **+15.66** | descent gate unchanged |
| `snow` | **−0.01** | kill unchanged |

**Ruled out (post-landing):** lake raft / evap lid from floating Snow;
weather `free_air_hy` / orographic crest from seated pack. Blocked evap
alone cannot raise TRACKED (free↔hum). Next: free-sat **writers** beside
landed Snow (rain `drain_tile` vs deposit near pack, water_flow/seep /
park adjacent to Snow solids) — spatial free attribution or one writer
OFF at a time. Kill: `GVSE_SOAK_OFF=snow`.

**Condensation liquid underpay (2026-10-09):** lottery apply used a
**snapshot** `mass`/`take_mass` while an earlier freezing hit's
`take_around` could already have drained that tile — `deposit_water_in_air`
wrote free sat, then `drain_tile` underpaid → TRACKED mint. Unit:
`liquid_deposit_after_neighbor_snow_take_stays_tracked_flat` (stale path
mints ≈+245; live clamp flat). Fix kept: re-read `at_tile` before
deposit (`deposit_liquid_paid`); probe `dep_add`/`dep_debit`.

| OFF / tip | TRACKED /t | note |
|-----------|------------|------|
| *(none)* post-fix `d6124f3` | **+130.86** | identical to pre-fix baseline |
| probe | dep_add=dep_debit=**534** | liquid path balanced; ≪ mint |
| late win 500t | free **≈+154k** | hum −14k; free not from H/dep |
| `snowfall` | +15.66 | descent gate unchanged |
| `snow` | −0.01 | kill unchanged |

**Not the 5k leftover** (real conservation bug, soak-irrelevant):
condensation liquid underpay. Next: free-sat writers beside landed Snow
other than cond deposit — water_flow / lateral seep / park_orphan, or
same-mat Air sat writes near Snow solids. Kill: `GVSE_SOAK_OFF=snow`.

**Free writers beside Snow (2026-10-09):** 5k on tip after
`OFF=flow|seep|park` + `FreeSatScope` probes (`flow_air` / `seep_air` /
`park_air`). Units: flow / contact-seep / take→park beside Snow stay
TRACKED-flat. `OFF=seep` must skip contact apply too (cadence-else used
to leave contact on).

| OFF | TRACKED /t | note |
|-----|------------|------|
| *(none)* | **+136.01** | `flow_air≈−2.6M`, `seep_air≈+3.3M`, `park_air≈+197k` |
| `flow` | **+196.47** | **worse** — surface flow not the mint |
| `seep` | **+145.24** | `seep_air=0`; still + — not the mint |
| `park` | **+132.60** | ≈same; `park` leftover huge, mint stays |
| `snowfall` | +15.66 | descent gate unchanged |
| `snow` | −0.01 | kill unchanged |

**Ruled out:** water_flow / lateral equalise, pore seep / contact wet,
`park_orphan` free placement (and same-mat Air Δ under those scopes).
Scope nets are free↔pore moves, not TRACKED. `paid−phase` still
~+665k with `under=0` — flakes seat then leave the snow book. Next:
post-seat phase / melt / thaw-yield path once flakes land. Kill:
`GVSE_SOAK_OFF=snow`.
