#!/usr/bin/env bash
# Linux-safe check: dry-run .app layout + plist well-formedness.
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG="$(cd "$(dirname "$0")" && pwd)"

"$PKG/bundle.sh" --dry-run --include-host
APP="$ROOT/dist/macos/WK Voxel.app"

test -x "$APP/Contents/MacOS/wk-voxel-client"
test -x "$APP/Contents/MacOS/wk-voxel-host"
test -f "$APP/Contents/Info.plist"

# Prefer plutil on macOS; xmllint / Python elsewhere.
if command -v plutil >/dev/null 2>&1; then
  plutil -lint "$APP/Contents/Info.plist"
  plutil -lint "$PKG/entitlements.plist"
elif command -v xmllint >/dev/null 2>&1; then
  xmllint --noout "$APP/Contents/Info.plist"
  xmllint --noout "$PKG/entitlements.plist"
else
  python3 - <<'PY'
import pathlib, xml.etree.ElementTree as ET
root = pathlib.Path("packaging/macos")
for p in [root / "Info.plist", root / "entitlements.plist"]:
    ET.parse(p)
    print(f"xml ok {p}")
PY
fi

ID=$(grep -A1 CFBundleIdentifier "$APP/Contents/Info.plist" | tail -1 | sed 's/.*<string>\(.*\)<\/string>.*/\1/')
echo "bundle id: $ID"
test "$ID" = "com.kaare.gvse.wk-voxel-client"
echo "macos packaging layout check OK"
