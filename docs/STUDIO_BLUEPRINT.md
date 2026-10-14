# Studio ↔ host blueprint contract *(dig B)*

**Status:** design locked as defaults (2026-10-14). Dig order **B** from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Studio rewrite and any
player **Release** path target this note — do not invent a second
on-disk format or a free-form `SpawnOrganism` payload.

Live types: [`crates/wk-voxel/src/blueprint.rs`](../crates/wk-voxel/src/blueprint.rs).
Player gates: [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md).
Stale UX sketch: [`organism/EDITOR.md`](organism/EDITOR.md) (macroquad
F2 studio — rewrite target, not wire truth).

## Intent

| Surface | Job |
|---------|-----|
| **Studio** | Total freedom to paint modules + tune `Genome`; save `.gvsecrt` |
| **Lab** | Drop own (or disk) blueprints into a local world |
| **Operator** | Built-in presets only today (`SpawnPreset`); full library later |
| **Wild / Shadow guest** | No free spawn; future **Release** is validated + capped |

Studio is not the live Wild editor. The host always validates before
mass / organisms change.

## On-disk / wire shape *(source of truth)*

Postcard binary, extension **`.gvsecrt`**, directory `blueprints/`.

```text
BLUEPRINT_SCHEMA_VERSION = 1   // crates/wk-voxel

Blueprint {
  schema_version: u16,
  canvas_w, canvas_h: u16,
  modules: Vec<PlacedModule>,   // x,y,lane,ModuleId
  genome: Genome,               // #[serde(default)] on new fields
  name, notes: String,
}
```

| Rule | Default |
|------|---------|
| Schema bump | Breaking field/layout → increment `BLUEPRINT_SCHEMA_VERSION` |
| Older files | Load if `schema_version ≤` host; missing genes via `serde(default)` |
| Newer files | **Reject** (`from_bytes` already errors) |
| Encoding | Postcard only for durable files (same as today) |
| Name | Filesystem-safe; `Blueprint::save_path` |

**Not in v1 live struct** (despite [`EDITOR.md`](organism/EDITOR.md)):
`wires`, `per_module_gene`. Those land only with a schema bump +
migration note — not as silent postcard fields.

## Validity *(host reject path)*

| Gate | When | Rule |
|------|------|------|
| Decode | Always | `Blueprint::from_bytes`; newer schema → reject |
| Lab / studio spawn | Local drop | `can_editor_spawn` (nucleus + non-empty) — today’s editor |
| Habit check | Optional Lab warn | `is_valid_creature` (atom/plant/fungus/epiphyte) |
| **Release** (Wild/Shadow) | Dig D2 Operator stub | Must pass `is_valid_creature` + seat + caps |
| Seat | All live drops | Host habitat rules (solid crown, wet air, …) |

Reject → `CommandAck { ok: false, detail }` (or Release result). Never
partial mass write.

## Surfaces vs payloads

| Who | How organisms enter the world | Payload |
|-----|-------------------------------|---------|
| Operator (today) | `ClientCommand::SpawnOrganism { preset }` | Enum → built-in `Blueprint::*` on host |
| Lab local / macroquad F2 | In-process studio → `spawn_blueprint_free` | Full `Blueprint` in-process (not over Wild wire) |
| Studio rewrite | Edit + save `.gvsecrt`; Lab load/spawn | Disk file; host reads trusted path or bytes |
| **Release** (Dig D2) | `ClientCommand::ReleaseBlueprint { bytes }` | Validated blueprint bytes + quarantine rules |

Presets stay the Operator fast path. Raw `Blueprint` structs are not on
the Wild Viewer command bus — only postcard bytes through Release.

## Release to Wild / Shadow *(Dig D2 Operator stub)*

Locked defaults; player Wild Release stays allowlist-later:

| Dial | Default |
|------|---------|
| Who | Operator this wedge; later account that owns the blueprint |
| Where | Any host that accepts Operator cmds (Lab / owned Shadow / Wild ops) |
| Caps | `RELEASE_COOLDOWN_TICKS` + `RELEASE_LIVE_SOFT_CAP` (+ atom pop cap) |
| Quarantine | `Blueprint::quarantine_for_release` — Mid only + genome clamp |
| Audit | ACK detail includes name; no silent mutation of disk file |

Player Wild Release + per-account identity: dig order G / migrate pillar.

## Studio rewrite target *(product)*

Rebuild against **this** schema + egui/wgpu ship stack (Phase 8 A), not
a paint-over of macroquad F2. Minimum viable studio:

1. Canvas paint / erase `ModuleId` on Mid (Fo/Bk later)
2. Genome sliders for fields that exist on `Genome` today
3. Save / load / list `.gvsecrt`
4. **Spawn into Lab** (local host Operator-equivalent)
5. Export path clear enough for a future Release button

Out of this contract: brand, full axon/wire editor, Wild Presence UI.

## Implementation debt *(when coding)*

| Item | Notes |
|------|-------|
| Document `BLUEPRINT_SCHEMA_VERSION` in ship README / host banner | optional crumb |
| Shared validate helper used by app + client host | `Blueprint::validate_for_spawn(SpawnGate)` ✅ |
| Operator “load .gvsecrt” | later; presets enough now |
| `ReleaseBlueprint` cmd | Dig D2 ✅ (`COMMAND_SCHEMA` 8); Wild player row still later |
| Retire stale EDITOR wire/gene fields or implement with bump | pick one |

## Acceptance for the next studio PR

- [x] This file linked from product roadmap + Phase 8 backlog  
- [x] Shared `validate_for_spawn(Lab|Release)` on host + macroquad editor  
- [ ] Studio (or Lab path) round-trips `.gvsecrt` schema 1 on ship client  
- [x] Invalid / newer schema rejected with clear detail  
- [x] No raw blueprint on Wild Viewer command path (presets only)  
- [x] Preset spawn still works for Operator  

## Related

- [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) · [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md)  
- [`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md) · [`organism/GENES.md`](organism/GENES.md)  
- [`organism/EDITOR.md`](organism/EDITOR.md) · [`organism/PALETTE.md`](organism/PALETTE.md)  
