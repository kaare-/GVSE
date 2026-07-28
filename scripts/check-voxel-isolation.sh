#!/usr/bin/env bash
# Enforce docs/VOXEL_MIGRATION.md isolation: wk-voxel / wk-voxel-app must not
# depend on the column-stack crates (directly or transitively).
#
# Prefer this targeted check over a workspace-wide ban: legacy crates are
# still first-class workspace members and legitimately depend on each other.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

FORBIDDEN_RE='wk-world|wk-field|wk-agents|wk-sim|wk-io|wk-app'
PACKAGES=(wk-voxel wk-voxel-app)

fail=0
for pkg in "${PACKAGES[@]}"; do
  # Package line itself is "wk-voxel v…" — strip the root, keep dependencies.
  tree="$(cargo tree -p "$pkg" --edges normal,build 2>/dev/null || true)"
  if [[ -z "$tree" ]]; then
    echo "error: could not resolve dependency tree for $pkg" >&2
    fail=1
    continue
  fi
  hits="$(printf '%s\n' "$tree" | tail -n +2 | grep -E "$FORBIDDEN_RE" || true)"
  if [[ -n "$hits" ]]; then
    echo "ISOLATION VIOLATION: $pkg depends on a column-stack crate:" >&2
    printf '%s\n' "$hits" >&2
    fail=1
  else
    echo "ok: $pkg has no column-stack dependencies"
  fi
done

exit "$fail"
