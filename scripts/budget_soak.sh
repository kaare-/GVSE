#!/usr/bin/env bash
# Headless BudgetSnap soak. Prefer a subagent so the main agent keeps working.
set -euo pipefail
cd "$(dirname "$0")/.."
ticks="${GVSE_SOAK_TICKS:-50000}"
period="${GVSE_BUDGET_PERIOD:-60}"
warm="${GVSE_BUDGET_WARM:-40}"
echo "budget soak ticks=$ticks period=$period warm=$warm" >&2
GVSE_SOAK_TICKS="$ticks" GVSE_BUDGET_PERIOD="$period" GVSE_BUDGET_WARM="$warm" \
  cargo test -p wk-voxel --test budget_soak --release long_budget_soak \
  -- --ignored --nocapture
