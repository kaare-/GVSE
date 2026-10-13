#!/usr/bin/env bash
# Build a macOS .app for wk-voxel-client (Phase 8 Apple packaging smoke).
#
# Usage:
#   ./packaging/macos/bundle.sh                  # release binary → dist/
#   ./packaging/macos/bundle.sh --dry-run        # layout only (Linux OK)
#   ./packaging/macos/bundle.sh --sign "Developer ID Application: …"
#   ./packaging/macos/bundle.sh --include-host
#
# Env:
#   MACOS_SIGN_IDENTITY  — codesign identity (optional; or --sign)
#   OUT_DIR              — default dist/macos
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
PKG="$(cd "$(dirname "$0")" && pwd)"
OUT_DIR="${OUT_DIR:-$ROOT/dist/macos}"
APP_NAME="WK Voxel"
BIN_NAME="wk-voxel-client"
HOST_BIN="wk-voxel-host"
DRY_RUN=0
INCLUDE_HOST=0
SIGN_IDENTITY="${MACOS_SIGN_IDENTITY:-}"
PROFILE=release

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --include-host) INCLUDE_HOST=1 ;;
    --debug) PROFILE=debug ;;
    --sign)
      shift
      SIGN_IDENTITY="${1:?--sign requires identity}"
      ;;
    -h|--help)
      sed -n '2,12p' "$0"
      exit 0
      ;;
    *)
      echo "unknown arg: $1" >&2
      exit 2
      ;;
  esac
  shift
done

APP="$OUT_DIR/${APP_NAME}.app"
CONTENTS="$APP/Contents"
MACOS="$CONTENTS/MacOS"
RES="$CONTENTS/Resources"

rm -rf "$APP"
mkdir -p "$MACOS" "$RES"
cp "$PKG/Info.plist" "$CONTENTS/Info.plist"

if [[ "$DRY_RUN" -eq 1 ]]; then
  # Placeholder so layout checks work off-Mac.
  printf '#!/bin/sh\necho "dry-run %s"\n' "$BIN_NAME" > "$MACOS/$BIN_NAME"
  chmod +x "$MACOS/$BIN_NAME"
  if [[ "$INCLUDE_HOST" -eq 1 ]]; then
    printf '#!/bin/sh\necho "dry-run %s"\n' "$HOST_BIN" > "$MACOS/$HOST_BIN"
    chmod +x "$MACOS/$HOST_BIN"
  fi
  echo "dry-run layout: $APP"
else
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: real bundle needs macOS (use --dry-run on Linux)" >&2
    exit 1
  fi
  cd "$ROOT"
  BINS=("$BIN_NAME")
  if [[ "$INCLUDE_HOST" -eq 1 ]]; then
    BINS+=("$HOST_BIN")
  fi
  cargo build -p wk-voxel-client --profile "$PROFILE" --bins
  TARGET_DIR="$ROOT/target/$PROFILE"
  for b in "${BINS[@]}"; do
    cp "$TARGET_DIR/$b" "$MACOS/$b"
    chmod +x "$MACOS/$b"
  done
fi

# Ad-hoc or Developer ID sign (macOS only).
if [[ -n "$SIGN_IDENTITY" ]]; then
  if [[ "$(uname -s)" != "Darwin" ]]; then
    echo "error: --sign requires macOS" >&2
    exit 1
  fi
  ENT="$PKG/entitlements.plist"
  codesign --force --options runtime --entitlements "$ENT" \
    --sign "$SIGN_IDENTITY" "$MACOS/$BIN_NAME"
  if [[ -x "$MACOS/$HOST_BIN" ]]; then
    codesign --force --options runtime --entitlements "$ENT" \
      --sign "$SIGN_IDENTITY" "$MACOS/$HOST_BIN"
  fi
  codesign --force --options runtime --entitlements "$ENT" \
    --sign "$SIGN_IDENTITY" "$APP"
  codesign --verify --deep --strict --verbose=2 "$APP"
  echo "signed: $APP ($SIGN_IDENTITY)"
else
  echo "unsigned: $APP (pass --sign or MACOS_SIGN_IDENTITY to codesign)"
fi

echo "bundle ok → $APP"
ls -la "$MACOS"
