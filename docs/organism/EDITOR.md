# Editor

*Frozen UX for the MS-Paint style creature studio / editor.
Active host: [`wk-voxel-app`](../../crates/wk-voxel-app). The column-era
host was [`crates/legacy/wk-app`](../../crates/legacy/wk-app) — archive
only.*

## Purpose

The editor is a studio tab inside
[`wk-voxel-app`](../../crates/wk-voxel-app). It lets a user construct a
creature by painting modules on a small grid using the palette from
[`PALETTE.md`](PALETTE.md), name it, save it to disk, and drop it into
the running GVSE world at a chosen cell.

The world *is* the petri dish. There is no separate sandbox — you
paint the creature, click a cell, and it lives.

## Activation

- **Key: `C`** — toggles the editor tab. Working default; if `C`
  clashes with a later feature, remap; add to the status line the
  moment it lands.
- While the editor is open, the world sim is paused and rendered
  behind the editor at half brightness. Same paused state the
  existing `Space` key produces. The tab does not replace the world
  view; it overlays.
- Pressing `C` again closes the editor and resumes the sim at the
  previous speed.

## Layout

Three-panel modal, macroquad UI (same pattern as the voxel settings /
editor panels in `wk-voxel-app`; column-era reference was
[`state.rs::draw_settings_ui`](../../crates/legacy/wk-app/src/state.rs)):

```
+---------------------------+---------------------+---------------+
|                           |                     |               |
|    Canvas (16 x 16)       |    Palette drawer   |  Blueprint    |
|                           |                     |  library      |
|    - 1 px = 1 module cell |    - grouped list   |               |
|    - grid lines           |    - hex swatch     |  - name       |
|    - lane tabs (Fo/Mi/Bk) |    - one-line job   |  - save       |
|    - selection cursor     |    - shortcut key   |  - load       |
|                           |                     |  - delete     |
+---------------------------+---------------------+---------------+
|                                                                 |
|   Tools row: Paint / Erase / Axon / Soma / Attach / Wire /      |
|              Eyedropper                                         |
+-----------------------------------------------------------------+
|                                                                 |
|   Info bar: current lane · pixel count · genome preview ·       |
|              spawn button (opens column-picker mode)            |
+-----------------------------------------------------------------+
```

## Canvas

- **Grid.** Initial 16×16 module cells. Expandable to 32×32 for tall
  trees (Set D). Grid size is a per-blueprint value on disk so a
  tree loads with the right canvas.
- **1 module cell = 1 world pixel** at 1× zoom. Editor renders at,
  say, 24 screen-px per module cell so the reader can pick a cell
  with the mouse.
- **Lane tabs** at the top of the canvas: Fo / Mi / Bk (see
  [`LANES.md`](LANES.md)). Editing happens on the active lane; the
  other two lanes render at half opacity so relative position is
  visible.
- **Ground line** is a horizontal reference at row `y = 0`. Modules
  below the line are underground when spawned; above the line, above
  ground.
- **Selection cursor.** A single-cell reticle. Arrow keys nudge it;
  clicking on a canvas cell moves it there.

## Palette drawer

- Grouped by job (Identity & metabolism, Chemistry, Nervous system,
  Physiology, Detritus loop, Land body), exactly matching
  [`PALETTE.md`](PALETTE.md).
- Each entry shows:
  - A 24×24 hex swatch.
  - The module name.
  - A one-line job description.
  - A keyboard shortcut for the paint tool
    (e.g. `1..9` cycle within a group; digit + letter picks it).
- Reserved modules (`Bark`, `Fruit`, …) are greyed out with a
  "reserved slot" tooltip.
- **Bone / Muscle / Skin** are first-class (Wave K): hotkeys `7` /
  `8` / `9`, full paint / inspect / mutate / aggregate.
- The user can drag a palette entry onto the canvas (drops on the
  hovered cell) or pick + click.

## Gene panels (Wave K)

Right of the canvas in `wk-voxel-app` (`gene_inspector.rs`):

| Panel | Job |
|-------|-----|
| **Gene Inspector** | Click a painted pixel. Sliders for that cell's `PixelTraits` (only traits meaningful for its `ModuleId` are shown). |
| **Body Plan** | Live readout of `Blueprint::body_plan()` — `total_mass`, `metabolic_rate`, `clone_fidelity`, `reproduce_at`, `photo_capacity`, `has_repro_gate`. |
| **Mutation Preview** | Rolls `mutate_child(seed=0, tick=0, parent_id=0)`, shows Δpixels / Δmass / Δmetabolic and a half-size child glyph. |

Hotkeys in the live editor: `1` Nucleus · `2` Photosystem · `3` Root
· `4` Stem · `5` Digest · `6` Hypha · `7` Bone · `8` Muscle · `9` Skin.

## Tools

Left-side row, standard MS-Paint-style modal tools:

| Tool | Effect | Notes |
|------|--------|-------|
| `Paint` | Places the selected module in the hovered cell. | Fails silently on lane-occupancy conflict. |
| `Erase` | Clears the hovered cell (all lanes). | Also removes attached axons / wires. |
| `Axon` | Drags a gray 1-px line from cell A to cell B. | Records a `wires` entry. |
| `Soma` | 2×2 stamp — must land in a 2×2 free area. | Occupies four cells. |
| `Attach` | Layers a pink `Holdfast` on top of an olive `Stem`. | Mid-lane only. |
| `Wire` | Sets sign / weight on an existing axon. Opens a small popup. | Excite / inhibit + slider. |
| `Eyedropper` | Picks the module at the hovered cell into the palette. | For duplication. |

Undo (`Ctrl-Z`) and redo (`Ctrl-Shift-Z`) are essential. Store a
ring buffer of the last 32 canvas states.

## Blueprint save format

Postcard binary, extension `.gvsecrt`, one file per blueprint:

```rust
pub const BLUEPRINT_SCHEMA_VERSION: u16 = 1;

pub struct Blueprint {
    pub schema_version: u16,
    pub canvas_w: u16,
    pub canvas_h: u16,
    pub modules: Vec<PlacedModule>,
    pub wires:   Vec<Wire>,
    pub genome:  Genome,
    pub name:    String,
    pub notes:   String,
}

pub struct PlacedModule {
    pub x: i16,       // relative to canvas origin
    pub y: i16,
    pub lane: LaneId, // Fore / Mid / Back
    pub module: ModuleId,
    pub traits: PixelTraits, // Wave K per-pixel genes (serde default)
}

pub struct Wire {
    pub from_pixel_idx: u16,
    pub to_pixel_idx:   u16,
    pub kind:           WireKind, // Axon (neural) or Hypha (nutrient)
    pub sign:           i8,       // +1 or -1
    pub weight:         f32,
    pub delay:          u8,       // 0 or 1
}
```

- Files live in `blueprints/*.gvsecrt` next to `world_save.bin`.
- `schema_version` is bumped on breaking changes; readers keep two
  versions back.
- Old blueprint on new build: opens. Missing genome fields default
  via `#[serde(default)]`. Reserved modules referenced by ID that
  the binary does not know: refuses to open with `"unknown module
  0xNN"` and the editor logs the error to the status bar.

## Library panel

- Lists all `blueprints/*.gvsecrt` files.
- Click to load a blueprint (replaces canvas contents; undo restores
  the previous canvas).
- `Save` writes the current canvas over the selected blueprint.
- `Save As` writes to a new file. Name is asked in an inline text
  field.
- `Delete` moves the file to `blueprints/trash/` (never rm -rf,
  never lose a design to a fat-finger).
- Thumbnail is a 48×48 render of the blueprint canvas (top-of-file
  cache written alongside the postcard payload).

## Spawn flow

1. Click **Spawn** in the info bar. Editor enters cell-picker mode.
2. The world view highlights the cell under the mouse (same
   selector already used by the voxel app input path).
3. Click a cell. The blueprint is instantiated via
   `OrganismStore::spawn_blueprint_free` (or habitat-aware spawn) on
   `wk-voxel`:
   - Genome is applied.
   - Land plants / fungi snap to a surface Air crown when needed.
4. Editor closes and world sim resumes.

If the user tries to spawn a plant with no solid seat, or a water
Atom with no wet Air, the info bar warns and refuses to spawn
(voxel seating rules in `wk-voxel` plant / organism modules).

Column-era note (archive): the old host used
`agent_keep_awake` in
[`crates/legacy/wk-world`](../../crates/legacy/wk-world) so hydrology
stayed active under a grazer — not used on the voxel path.

## Determinism

- Blueprint instantiation uses `hash_u64(world.seed, tick,
  spawn_serial)` to seed any per-entity RNG (mutation rolls,
  per-axon initial jitter, etc.). Two spawns of the same
  blueprint at the same tick on the same seed produce identical
  creatures.

## Rendering the creature in-world

- Modules render as 1×1 pixels in Mid lane by default (Fore for
  future animals, Back for cover creatures).
- 2×2 soma renders as a 2×2 block in the same lane it was authored
  in.
- Axons render as 1-px gray lines between the module cells listed
  in `wires` (kind = `Axon`).
- Hyphae render the same but with the cream palette entry.

Voxel draw already paints module pixels from `OrganismStore::draw_list`
in `wk-voxel-app`. Column-era reference pass lived in
[`crates/legacy/wk-app/src/render.rs`](../../crates/legacy/wk-app/src/render.rs).

## Debug overlays

Cycled by `O` (existing overlay key):

- `LightRemaining` (see [`LIGHT.md`](LIGHT.md)) — how much light is
  reaching each height in each column.
- `ChemChannel[c]` — per-channel field colour ramp.
- `Neural` — active axons coloured by signed activation (see
  [`NERVES.md`](NERVES.md)).

## What is deliberately not here

- Freeform brush strokes. Modules are grid-aligned pixels.
- Multi-select transform. One cell at a time; copy / paste is
  Phase 2 stretch.
- Animation preview. A tick / step preview inside the editor is a
  nice-to-have, not a requirement.
- In-editor mutation preview. Save + spawn is fast enough.
- Multiplayer editor sync. Never.
