# Aseprite asset pipeline

Headless **Cursor → Aseprite → GvsE** pixel-art tooling. Isolated from the
voxel simulation crates; nothing here is loaded by `wk-voxel` / `wk-voxel-app`.

Official references:

- [Aseprite CLI](https://www.aseprite.org/docs/cli/)
- [Aseprite scripting API](https://www.aseprite.org/api/)

## Requirements

- Python 3.10+ (stdlib only; no pip packages)
- A **licensed** Aseprite build with CLI + Lua (for the primary path)

This repo does **not** redistribute Aseprite. Point the tooling at your install:

```bash
export ASEPRITE_PATH=/path/to/aseprite   # file or directory
./art which
```

Discovery also checks common Steam / `/opt` / Applications locations and `PATH`.

## CLI

```bash
cd asset-pipeline

./art which
./art create  --spec specs/fungal-organism.json
./art import  --input fixtures/remote-art-job/fungal-remote.png --output /tmp/in.aseprite
./art export  --source examples/fungal-organism/source/fungal-organism.aseprite \
              --sheet /tmp/sheet.png --data /tmp/sheet.json
./art validate --source examples/fungal-organism/source/fungal-organism.aseprite \
               --data examples/fungal-organism/export/fungal-organism-sheet.json \
               --expect-width 16 --expect-height 16 --expect-min-frames 4
./art import-remote --manifest fixtures/remote-art-job/manifest.json \
                    --stage-dir /tmp/remote-stage \
                    --output /tmp/from-remote.aseprite
```

Equivalent: `python3 -m gvse_art …` from this directory.

| Command | Purpose |
|---------|---------|
| `create` | Build `.aseprite` (+ optional sheet) from a JSON pixel spec via Lua |
| `import` | PNG → `.aseprite` (optional resize / palette JSON) |
| `export` | `.aseprite` → spritesheet PNG + Aseprite JSON (`--sheet` / `--data`) |
| `validate` | ASE header checks; with Aseprite, also `--list-layers` / `--list-tags` |
| `import-remote` | Local ComfyUI-style manifest fixture → stage → import |
| `which` | Print discovered binary |

Size, palette, frame count, durations, paths, and transparency come from the
**spec / CLI flags** — not from hardcoded engine constants.

## Layout

```text
asset-pipeline/
  art                 CLI entry
  gvse_art/           Python package
  lua/                Reusable Aseprite scripts
  specs/              Generation specs
  palettes/           Optional palette JSON
  examples/           Sample organism (source + export)
  fixtures/           Local remote-job fixtures
  tests/              unittest suite
  HANDOFF.md          Remote ComfyUI boundary notes
```

**Sources vs exports:** editable masters live under `examples/*/source/`.
Derived sheets live under `export/`. Remote imports stage into a separate
directory with `import-provenance.json` and never overwrite artist sources
unless `--force` is passed to an explicit output path.

## Offline vs Aseprite-verified

| Path | Without Aseprite | With Aseprite |
|------|------------------|---------------|
| Spec load / PNG util / ASE header validate | tested | tested |
| `art create` / `import` | offline fixture writer | Lua scripts (`lua/*.lua`) |
| `art export` | **unavailable** (needs CLI) | `--sheet` / `--data` |
| `validate` list-tags/layers | warning | exercised |

When Aseprite is missing, `create` still writes a minimal `.aseprite` so the
example can ship; regenerate with Aseprite installed to verify the Lua path.

## Tests

```bash
cd asset-pipeline
python3 tests/test_pipeline.py
```

Integration cases are skipped automatically if no binary is found.

## Remote generation (future)

See [`HANDOFF.md`](HANDOFF.md). Optional client: `gvse_art.remote_client`
(`GVSE_ART_BASE_URL`, optional `GVSE_ART_TOKEN`). No hostnames or credentials
are hardcoded.
