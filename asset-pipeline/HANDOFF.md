# Handoff: remote ComfyUI → Aseprite boundary

StudioAssistant (rig-1) will expose a headless image service. GvsE should treat
it as an optional upstream of **PNG + manifest**, then run local Aseprite
processing. This pipeline implements only the **import / manifest** edge with
local fixtures.

## Planned HTTP surface

| Method | Path | Role |
|--------|------|------|
| `POST` | `/v1/art/jobs` | Submit a generation spec |
| `GET` | `/v1/art/jobs/{id}` | Poll job status + manifest fields |
| `GET` | `/v1/art/jobs/{id}/assets/{filename}` | Download PNG (or other asset bytes) |

Do not hardcode server URLs, tokens, or David’s internal hostnames in game code.
Optional client: `gvse_art.remote_client.RemoteClient.from_env()` reading:

- `GVSE_ART_BASE_URL` — e.g. `https://art-service.example` (no default)
- `GVSE_ART_TOKEN` — optional bearer token (no default)

The client is **not** a dependency of `wk-voxel` / `wk-voxel-app`.

## Manifest schema (v1)

Local fixture: [`fixtures/remote-art-job/manifest.json`](fixtures/remote-art-job/manifest.json).

```json
{
  "schema_version": 1,
  "job_id": "…",
  "status": "succeeded",
  "prompt": "optional text",
  "seed": 42,
  "tags": ["optional", "labels"],
  "assets": [
    {
      "filename": "sprite.png",
      "path": "sprite.png",
      "role": "png",
      "width": 16,
      "height": 16,
      "sha256": "optional"
    }
  ]
}
```

Flow:

1. Download / copy assets beside the manifest (or use `art import-remote`).
2. Stage into a scratch directory (`import-provenance.json` written automatically).
3. `art import --input staged.png --output …/processed.aseprite` (palette/size flags as needed).
4. `art export` → spritesheet + JSON for game packaging later.

## What stays local

- Pixel construction / cleanup Lua (`lua/`)
- Palette and dimension rules (spec / CLI)
- Native `.aseprite` masters and provenance
- Engine rendering — unchanged by this work

## Later integration sketch

**Cursor requests sprite → remote generation → PNG + manifest → Aseprite processing → GvsE asset.**

Keep generation metadata and original remote PNGs under provenance / stage dirs;
never silently overwrite artist-edited `source/` files.
