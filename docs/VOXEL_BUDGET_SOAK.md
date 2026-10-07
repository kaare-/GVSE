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
- Automerge when CI green once GitHub auto-merge is available to the agent; until then mark ready and note merge wait.
- Hard problems: verify headless and/or Xvfb before asking to merge.
