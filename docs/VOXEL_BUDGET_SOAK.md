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
~+665k with `under=0` — flakes seat then leave the snow book.

**A vs B split — phase OFF residual vs phase ON thaw (2026-10-09):**
5k remeasure after snow-book enter/exit probes (`snow_enter_yield` /
`snow_exit_yield` / `net_leave` / `bare` / `to_ice`). Fall swaps both
enter and exit; `net_leave = exit − enter` matches `−Δsnow`. `bare` is
solid overwrite only (Sand→Snow sink, unit-tested). Units:
`thaw_to_air` / airborne thaw / snow-on-warm-water slush stay
TRACKED-flat; free credit matches snow leave.

| OFF | TRACKED /t | paid | Δsnow+ice | paid−phase | net_leave | bare | note |
|-----|------------|------|-----------|------------|-----------|------|------|
| *(none)* | **+130.86** | 860k | +219k | **+640k** | −208k | **0** | credit≈thaw; free −1.00M |
| `phase` | **+110.96** | 894k | +894k | **+19** | −894k | 2.8k | paid≈Δsnow; free −1.63M |
| `snowfall` | **+15.14** | 757k | +742k | +15k | −742k | 0 | aloft; late wins ~flat |
| `snow` | −0.01 | — | — | — | — | — | kill unchanged |

**A — residual with phase OFF (~+111/t):** descent on, thaw off.
`paid≈Δsnow`, `to_ice=0`, `bare≪mint`. Late windows still mint triad
`free+pore+hum+snow` (~+120k/500t) while snow banks grow — **not** a
snow-book exit. Vs `OFF=snowfall` (+15/t, late near-flat): ~+96/t is
**post-descent / landed-pack** coupling. Non-phase exits are not the
mint (bare ~0.5/t).

**B — extra when phase ON (~+20/t over A):** thaw returns snow→free.
Baseline free is ~0.63M higher than phase-OFF — ≈`paid−Δsnow` (~640k)
— so thaw **credits free correctly** (units agree). `paid−phase≈+640k`
matches **total** mint magnitude, not B; that coincidence is A-sized
snow leave while A still mints with snow held. `to_ice=0`, `bare=0`.

**Not fixed (no clear writer):** leftover is almost all **A**. Next:
post-descent coupling that raises the triad while snow remains in the
book (not thaw under/over-credit). Kill: `GVSE_SOAK_OFF=snow`.

**Residual A — landed-pack window + free_in (2026-10-09):** 5k on tip
`48b3925` after `free_other` / `steam_solid` probes and `OFF=gravity`.
Windowed triad (`GVSE_SOAK_WINDOW=500`): with `OFF=phase`, late wins
mint **~+100…127k/500t** triad; with `OFF=phase,snowfall` late wins
are near-flat / low tens — isolates **~+91/t** as landed-pack delta.

| OFF | TRACKED /t | paid−phase | free_other | steam_solid | note |
|-----|------------|------------|------------|-------------|------|
| `phase` | **+110.96** | +19 | −2.09M | 60k | paid≈Δsnow; triad late hot |
| `phase,snowfall` | **+20.32** | 0 | −1.94M | 72k | aloft; late triad ~flat |
| `phase,cond` | **+93.48** | +21 | — | — | thermal-surplus snow still A |
| `phase,steam` | **+101.89** | 0 | — | — | not steam double-count |
| `phase,gravity` | **+109.38** | +46 | −1.69M | 56k | chunk gravity **not** A |
| `phase,evap` | −4.25 | 0 | — | — | no H → no flakes |

**Accounting (phase):** `hum ≈ evap_add − paid` (within ~500);
`TRACKED ≈ (Δfree+evap) + Δpore + Δsteam + Δcave` with snow paired.
Landed vs aloft: extra **~+362k free_in** and **~+125k pore retained**
(−33k steam) ≈ **+453k TRACKED**. `free_other + evap_debit ≈ +80…90k`
on both — World::set_cell Air writers outside flow/seep/park are **not**
the mint-sized term (evap dominates `free_other` as a sink).
`steam_solid` peak ~60k is real (parallel Air→Snow skips evict) but
`OFF=steam` still **+102/t**.

**Ruled out for A:** thaw/B, snowfall-aloft residual, cond lottery
(majority), steam, gravity pulls, prior flow/seep/park/snowsurf/raft.
**Next:** chunk-direct / parallel writers other than gravity (grain
Air↔Snow sat integrity under multi-pass settle; any path that adds free
or pore without `note_set_cell`), or a snow-lid × sky-budget coupling
that is not a single OFF. Kill: `GVSE_SOAK_OFF=snow`.

**Residual A — settle + World snow overlay (2026-10-09):** 5k on tip
`f6d7f9c` after `par_air` / `grav_air` miss probes and `OFF=settle`.

| OFF | TRACKED /t | swap_snow | snow_in | snow_out | par_air | note |
|-----|------------|-----------|---------|----------|---------|------|
| `phase` | **+110.96** | +1.449M | +193.3M | −191.8M | **−157** | paid=894k; late triad ~+127k/500t |
| `phase,settle` | **+42.89** | +0.936M | +279.0M | −278.1M | **0** | settle off → A −68/t; airborne still lands |
| `phase,snowfall` | **+20.32** | +0.880M | +19.3M | −18.5M | −36 | aloft; late triad ~flat |

**Miss probes:** `par_air` / `par_snow` ≈ 0 (parallel free-Air sat
conserved). `grav_air` is a large sink (infiltration), not a mint —
agrees with prior `OFF=gravity` **+109/t**. Chunk-direct free-sat writers
are **not** residual A.

**Accounting identity:** `TRACKED ≈ swap_snow − paid` (±1k) on all three
rows. `paid≈Δsnow`. World Snow enter/exit traffic is huge (airborne /
drift / rise swaps); **net** World snow overlay exceeds nucleation by
exactly the mint. Unit: settle beside Snow pack stays TRACKED-flat
(no local free mint in a toy bank).

**Settle role:** multi-pass grain fall/repose is ~⅔ of landed-pack A
(+111 → +43 with settle off; snowfall still +20). Amplifies the World
snow-overlay bias; does not mint via `parallel::set_cell` Air sat.

**Call-site tag (2026-10-09):** `SnowSwapScope` on World snow_in/out.
5k `OFF=phase` before the fix — fall/drift/nucleate clean; **untagged
other net ≈ TRACKED**. Stage split: almost all other was **failure**.

| site | snow_in | snow_out | net | vs TRACKED |
|------|---------|----------|-----|------------|
| nucleate | +894k | 0 | +894k | = paid |
| fall / drift / rise / punch / raft / land / competent | huge | −huge | **0** | paired |
| **other (failure)** | +4.67M | −4.12M | **+555k** | ≈ TRACKED |

**Root cause:** Snow (`roof_span_max_m = 0`) took F1 roof collapse over
any Air cavity. Debris path treats `sat` as pore water; Snow capacity 0
→ wrote Snow(sat=0) below (thaw reads 255) and dumped thaw yield into
vacated Air → unpaired World snow_in. Ice same trap. Fix: exclude
Ice/Snow from `is_roof_candidate` (airborne fall owns descent). Unit:
`roof_collapse_ignores_snow_and_stays_tracked_flat`.

**5k after fix** tip `124a197` / baseline remeasure tip `45e6008`:

| OFF | TRACKED /t | swap_snow−paid | note |
|-----|------------|----------------|------|
| *(none)* | **−0.01** | 0 | full baseline flat; park=0 |
| `phase` | **−0.01** | 0 | paid=Δsnow; other net 0 |
| `phase,settle` | **−0.01** | ~0 | settle leftover gone |
| `phase,snowfall` | **−0.04** | 0 | aloft residual gone |

Residual A **closed** (baseline and phase-OFF). Kill switch `OFF=snow`
unchanged for other hunts.

### Phase 0 gate — 50k after roof-collapse Snow fix (2026-10-09)

Headless `short_budget_soak` tip `7838473` (`GVSE_SOAK_TICKS=50000`,
warm=40, period=60; wall ~56 min):

| Signal | Result |
|--------|--------|
| TRACKED `/t` | **−0.00** (d=−6) |
| min.tot `/t` | **+0.07** (d=+3405) |
| `park` / clamp / clip | **0** |
| snow / ice store | snow **+311610**, ice **0** |
| snow sites | fall/drift/other net **0**; `swap_snow−paid≈0` |
| flake underpay | `under=0` |
| read flags | late windows `UNEXPL-M` only (abs min leftover; rate ≪ 0.5/t) |

Phase 0 rate eps still met (`|rate W|` and `|rate M|` &lt; 0.5/t) with
snow precip on after the Ice/Snow roof-collapse exclusion.
