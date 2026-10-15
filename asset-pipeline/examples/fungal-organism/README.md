# Example: fungal organism

Isolated demo asset for the headless Aseprite pipeline. It does **not**
replace in-game organism rendering (`docs/organism/`).

## Contents

| Path | Role |
|------|------|
| [`../../specs/fungal-organism.json`](../../specs/fungal-organism.json) | Reproducible pixel spec (size, palette, frames) |
| [`source/fungal-organism.aseprite`](source/fungal-organism.aseprite) | Native editable source |
| [`export/fungal-organism-sheet.png`](export/fungal-organism-sheet.png) | Horizontal spritesheet |
| [`export/fungal-organism-sheet.json`](export/fungal-organism-sheet.json) | Frame metadata |
| [`provenance/`](provenance/) | Generation notes (never treat as artist master) |

## Regenerate

```bash
cd asset-pipeline
./art create --spec specs/fungal-organism.json --force
./art validate \
  --source examples/fungal-organism/source/fungal-organism.aseprite \
  --data examples/fungal-organism/export/fungal-organism-sheet.json \
  --expect-width 16 --expect-height 16 --expect-min-frames 4
```

When Aseprite is installed (`ASEPRITE_PATH` or discovery), create uses the
Lua script path. Without Aseprite, an offline fixture writer still produces
the `.aseprite` + sheet for review; mark that path untested against the real CLI.
