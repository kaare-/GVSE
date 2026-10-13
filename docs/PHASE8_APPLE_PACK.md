# Phase 8 — Apple packaging smoke *(notarized `.app`)*

**Status:** scripts landed (2026-10-13). **Pinned until 2026-11-15** —
owner Apple Developer membership expired; renew + notarize then (do not
block sim / Phase 8 present work). Linux CI still checks layout / plists
(`packaging/macos/check_layout.sh`).

**Goal:** prove stack A (`wk-voxel-client`) can become a Developer ID
**notarized** macOS app **outside** the Mac App Store. MAS sandbox is
out of scope for this wedge.

## What we ship in the smoke

| Piece | Path |
|-------|------|
| Bundle script | [`packaging/macos/bundle.sh`](../packaging/macos/bundle.sh) |
| Notarize script | [`packaging/macos/notarize.sh`](../packaging/macos/notarize.sh) |
| `Info.plist` | bundle id `com.kaare.gvse.wk-voxel-client` |
| Entitlements | Hardened Runtime (network client + unsigned exec mem) |
| Layout check | [`packaging/macos/check_layout.sh`](../packaging/macos/check_layout.sh) |

Default binary: **in-process Compact** viewer. Optional `--include-host`
copies `wk-voxel-host` into `Contents/MacOS/` for local IPC experiments
(not required for the smoke).

## One-time Apple setup (owner Mac)

1. Xcode CLT / full Xcode so `codesign`, `notarytool`, `stapler` exist.
2. **Developer ID Application** certificate in login keychain
   (developer.apple.com → Certificates).
3. Store notary credentials (recommended):

```bash
xcrun notarytool store-credentials "gvse-notary" \
  --apple-id "you@example.com" \
  --team-id "TEAMID" \
  --password "app-specific-password"
```

Or use App Store Connect API key env vars (see `notarize.sh`).

## Build → sign → notarize

```bash
# From repo root, on macOS:
./packaging/macos/bundle.sh \
  --sign "Developer ID Application: Your Name (TEAMID)"

# Optional: also pack the OOP host binary
# ./packaging/macos/bundle.sh --include-host --sign "…"

./packaging/macos/notarize.sh
# → dist/macos/WK Voxel.app  (stapled)
open "dist/macos/WK Voxel.app"
```

Gatekeeper check:

```bash
spctl --assess --type execute --verbose=4 "dist/macos/WK Voxel.app"
```

## Linux / CI

```bash
./packaging/macos/check_layout.sh   # dry-run .app + plist XML
```

Does **not** build Metal, codesign, or talk to Apple.

## Non-goals (this wedge)

- Mac App Store / App Sandbox
- Steam depots
- Universal2 fat binary automation (build native arch first; lipo later)
- Shipping `wk-voxel-app` (macroquad) as the store binary

## Failure knobs

| Symptom | Likely fix |
|---------|------------|
| Crash under Hardened Runtime | Adjust `entitlements.plist` (wgpu often needs unsigned-exec-mem) |
| `spctl` rejects | Notarize/staple missing; wait for notarytool success |
| Invalid signature | Re-sign binaries **then** the `.app`; use `--options runtime` |
| Bundle id clash | Change `CFBundleIdentifier` in `Info.plist` before first App Store Connect record |

## Pin (owner 2026-10-13)

Do **not** run notarize until **2026-11-15** (or earlier if membership
renews). Scripts stay in-tree; layout CI stays green. Unblock path:
renew Apple Developer → `bundle.sh --sign` → `notarize.sh`.

## Next after green smoke (post-pin)

1. Owner confirms notarized app launches on Apple Silicon (and Intel if needed).
2. Optional: Universal2 + DMG.
3. Steam packaging for the same binaries.
4. Resume sim / presentation backlog (unblocked during the pin).
