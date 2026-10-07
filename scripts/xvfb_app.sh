#!/usr/bin/env bash
# Run wk-voxel-app on a virtual display and grab a PNG.
# Needs: Xvfb, xdotool, and import (ImageMagick) or scrot.
set -euo pipefail
cd "$(dirname "$0")/.."
display="${DISPLAY_NUM:-99}"
out="${1:-/tmp/gvse-xvfb.png}"
export DISPLAY=":${display}"

if ! pgrep -f "Xvfb :${display}" >/dev/null 2>&1; then
  Xvfb ":${display}" -screen 0 1280x720x24 >/tmp/gvse-xvfb.log 2>&1 &
  sleep 0.5
fi

cargo build --release -p wk-voxel-app
cargo run --release -p wk-voxel-app >/tmp/gvse-app.log 2>&1 &
app_pid=$!
trap 'kill "$app_pid" 2>/dev/null || true' EXIT
sleep 8
xdotool search --name 'wk-voxel' windowactivate --sync key b || true
sleep 2

if command -v import >/dev/null 2>&1; then
  import -window root "$out"
elif command -v scrot >/dev/null 2>&1; then
  scrot "$out"
else
  echo "need ImageMagick import or scrot to grab $out" >&2
  exit 1
fi
echo "wrote $out" >&2
