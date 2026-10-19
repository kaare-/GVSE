#!/bin/zsh
# Render the opening room from RoomArt and hand it to Aseprite.
# A one-pixel check stays on the Mac: ./tools/preview-room.sh
# Small animations (scissors, power light) live in art/sprites.
# Edit those, then ./tools/export-sprites.sh
# Pass --open to show the tumbler crop in Aseprite.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
out="$root/art/preview"
mkdir -p "$out"
bin="$root/tools/.preview-room"
swiftc \
  -framework AVFoundation \
  -framework CoreGraphics \
  -framework ImageIO \
  -o "$bin" \
  "$root/ios/MakeAcreature/RoomArt.swift" \
  "$root/tools/preview-room.swift"
"$bin" "$out"
ase="/Applications/Aseprite.app/Contents/MacOS/aseprite"
if [[ -x "$ase" ]]; then
  "$ase" -b "$out/glass@8.png" --save-as "$out/glass.aseprite"
  "$ase" -b "$out/room@4.png" --save-as "$out/room.aseprite"
  echo "$out/glass.aseprite"
  echo "$out/room.aseprite"
fi
if [[ "${1:-}" == "--open" ]]; then
  open -a Aseprite "$out/glass.aseprite"
fi
