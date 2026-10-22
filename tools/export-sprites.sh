#!/bin/zsh
# Turn art/sprites/*.aseprite into the PNG strips the app plays.
# Edit a frame in Aseprite, then run this. The phone build can wait.
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
ase="/Applications/Aseprite.app/Contents/MacOS/aseprite"
src="$root/art/sprites"
dst="$root/ios/MakeAcreature/Sprites"
mkdir -p "$src" "$dst"
"$ase" -b --script "$root/tools/make-sprites.lua"
for file in "$src"/*.aseprite; do
  name=$(basename "$file" .aseprite)
  "$ase" -b "$file" --sheet "$dst/$name.png" --sheet-type horizontal >/dev/null
  echo "$dst/$name.png"
done
