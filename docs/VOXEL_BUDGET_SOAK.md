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

Flake humidity debit probes clean (`under=0`). Suspect is **after**
`deposit_snow_in_air` seats Snow (not the pay path). Soak kill switch:
`GVSE_SOAK_OFF=snow` → `PhaseConfig::enable_snow_precip = false` until the
post-seat mint is found.
- Automerge when CI green once GitHub auto-merge is available to the agent; until then mark ready and note merge wait.
- Hard problems: verify headless and/or Xvfb before asking to merge.
