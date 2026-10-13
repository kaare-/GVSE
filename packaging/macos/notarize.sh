#!/usr/bin/env bash
# Notarize + staple a signed WK Voxel.app (macOS only).
#
# Prereqs:
#   - bundle.sh --sign "Developer ID Application: Your Name (TEAMID)"
#   - Apple notary credentials via one of:
#       xcrun notarytool store-credentials "gvse-notary" …
#     or env:
#       APPLE_API_KEY / APPLE_API_ISSUER / APPLE_API_KEY_PATH  (App Store Connect API key)
#       — or —
#       APPLE_ID / APPLE_APP_PASSWORD / APPLE_TEAM_ID
#
# Usage:
#   ./packaging/macos/notarize.sh
#   ./packaging/macos/notarize.sh dist/macos/WK\ Voxel.app
#   NOTARY_PROFILE=gvse-notary ./packaging/macos/notarize.sh
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
APP="${1:-$ROOT/dist/macos/WK Voxel.app}"
NOTARY_PROFILE="${NOTARY_PROFILE:-gvse-notary}"
ZIP="${TMPDIR:-/tmp}/wk-voxel-client-notarize.zip"

if [[ "$(uname -s)" != "Darwin" ]]; then
  echo "error: notarize.sh must run on macOS" >&2
  exit 1
fi
if [[ ! -d "$APP" ]]; then
  echo "error: missing app: $APP" >&2
  echo "run: ./packaging/macos/bundle.sh --sign \"Developer ID Application: …\"" >&2
  exit 1
fi

echo "zipping $APP …"
rm -f "$ZIP"
ditto -c -k --keepParent "$APP" "$ZIP"

submit_args=(submit "$ZIP" --wait)
if [[ -n "${APPLE_API_KEY:-}" && -n "${APPLE_API_ISSUER:-}" && -n "${APPLE_API_KEY_PATH:-}" ]]; then
  submit_args+=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY" --issuer "$APPLE_API_ISSUER")
elif [[ -n "${APPLE_ID:-}" && -n "${APPLE_APP_PASSWORD:-}" && -n "${APPLE_TEAM_ID:-}" ]]; then
  submit_args+=(--apple-id "$APPLE_ID" --password "$APPLE_APP_PASSWORD" --team-id "$APPLE_TEAM_ID")
else
  submit_args+=(--keychain-profile "$NOTARY_PROFILE")
fi

echo "notarytool ${submit_args[*]}"
xcrun notarytool "${submit_args[@]}"
echo "stapling …"
xcrun stapler staple "$APP"
xcrun stapler validate "$APP"
spctl --assess --type execute --verbose=4 "$APP" || true
echo "notarized + stapled → $APP"
