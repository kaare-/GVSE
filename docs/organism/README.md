# Organism Kernel

*Design freeze for the pixel-blob organism vocabulary GVSE will grow
against. This directory is spec-only — no code lives here.*

## Design stance

Everything an organism *is* draws like **MS Paint at 1× zoom**. The
smallest useful mark is a 1×1 pixel; a module is a coloured pixel with
one job, one cost, and one visible tradeoff. If you cannot see it, it
is not a module.

Preferences (locked for Sets A–E ecology):

- Few `ChemType` channels over realistic biochemistry.
- Few wires over a learned neural net **in the open world**.
- Grow the palette in stages so the simplest creature stays two pixels.
- Phenotype is the drawing. Save the drawing, save the creature.

**Studio track:** trainable nets + muscle/bone benches live in
[`STUDIO.md`](STUDIO.md) on the **same** `wk-voxel` physics. Export
frozen bodies into the world; do not fork the CA.

## Petri = the current GVSE world

Sets A–E ecology still use the live world as the petri dish. The
Muscle / Bone / Neural **Studio** is an arena *mode* of that same
voxel physics (`wk-voxel-studio-app`), not a second engine.

Historical column-stack note (legacy `wk-app`):

- Columns, layers, chunks, and the material vocabulary in
  [`crates/wk-material`](../../crates/wk-material) and
  [`crates/wk-world`](../../crates/wk-world).
- Multirate scheduler + barrier-commit in
  [`crates/wk-sim`](../../crates/wk-sim).
- Field slots (thermal / humidity / pressure / wind / groundwater head
  / dissolved) in [`crates/wk-field`](../../crates/wk-field) — new
  organism fields (`light`, `chem[c]`) slot in the same way.
- Existing ECS creature store in
  [`crates/wk-agents`](../../crates/wk-agents) — evolves from the
  scripted `Grazer` into the module-pixel blueprint model in Phase 2.
- MS-Paint style creature editor lives in
  [`crates/wk-app`](../../crates/wk-app) as a new tab. Spec in
  [`EDITOR.md`](EDITOR.md).

## Reading order

| Doc | Freezes |
|-----|---------|
| [`PALETTE.md`](PALETTE.md) | The module colour atlas + exact RGB hex + pixel grammar |
| [`CHEM.md`](CHEM.md) | `ChemType` IDs, per-chunk chem field, sensor/emitter tuning |
| [`NERVES.md`](NERVES.md) | Fixed neural graph inputs / outputs / wire semantics |
| [`LIGHT.md`](LIGHT.md) | Column shade scan and the light competition rule |
| [`PLANTS.md`](PLANTS.md) | Rooted land plants, deep roots, epiphytes, topple pipeline |
| [`FUNGI.md`](FUNGI.md) | Litter fungi, hyphae, substrate memory, ghost roots |
| [`LANES.md`](LANES.md) | Fore / Mid / Back depth-lane occupancy for future animals |
| [`CORE_FEATURES.md`](CORE_FEATURES.md) | Feature Sets A–E lock and explicit non-goals |
| [`GENES.md`](GENES.md) | Gene table with tradeoffs; merge notes vs existing `wk-agents::Genome` |
| [`FIELDS.md`](FIELDS.md) | Petri fields (light, temp, chem, moisture, organic, substrate, stem wetness) |
| [`SCENARIOS.md`](SCENARIOS.md) | 16 falsification scenes numbered E30–E45 |
| [`EDITOR.md`](EDITOR.md) | MS-Paint editor tab UX, canvas, `Blueprint` save format, spawn flow |
| [`STUDIO.md`](STUDIO.md) | Muscle / bone / neural test studio — shared physics, GA, export |
| [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) | Active voxel plant/fungus port status |

## GVSE hook table

These are the anchors Phase 2 code will bind to. Naming them here
locks the coupling contract:

| Concept | Existing GVSE hook |
|---------|--------------------|
| Coarse column biomass | `Ecology { root_density, leaf_area, alive_biomass, dead_biomass, nutrient }` in [`crates/wk-world/src/column.rs`](../../crates/wk-world/src/column.rs) |
| Dead plant / root mass ledger | `MassAudit::biomass_decay_total` + `biomass_eaten_total` in [`crates/wk-world/src/world.rs`](../../crates/wk-world/src/world.rs) |
| Future organic material slot | `MaterialId::Organic` (reserved in the material vocabulary) |
| Per-chunk scalar / vector fields | Slot pattern in [`crates/wk-field`](../../crates/wk-field); enable flags on [`World`](../../crates/wk-world/src/world.rs) |
| Genome storage | `wk_agents::Genome` in [`crates/wk-agents/src/lib.rs`](../../crates/wk-agents/src/lib.rs). See [`GENES.md`](GENES.md) for the merge plan |
| Post-barrier subsystem slot | Direct-mutation section of `Simulation::step` in [`crates/wk-sim/src/sim.rs`](../../crates/wk-sim/src/sim.rs) — new `run_shade`, `run_chem`, module-driven `run_agents` sit here |
| Column moisture / water table | `column.moisture` + `moisture_cap` + `run_groundwater_head_field` |
| Existing scripted grazer | `wk_agents::Grazer` / `AgentStore::step_grazers` — Phase 2 replaces the scripted body with a module-pixel behaviour driven by `Blueprint` |
| App UI host | `state.rs::draw_settings_ui` in [`crates/wk-app/src/state.rs`](../../crates/wk-app/src/state.rs) — same `macroquad::ui` used for the editor tab |

## Cross-cutting invariants (unchanged)

The organism kernel obeys the four rules that carry the rest of GVSE:

1. **Determinism.** Blueprint spawn, mutation, and every organism
   subsystem seed off `hash_u64(world.seed, tick, entity_id, salt)`.
2. **Mass audit.** Every new mass sink or source gets its own bucket
   (`biomass_grow_total`, `biomass_decay_total`, `biomass_eaten_total`
   already exist; module upkeep and emitter cost land as new buckets in
   Phase 2). Creature body mass may stay outside `total_tracked` for
   the initial phases (as in stage 10/11); this is called out per
   subsystem.
3. **Buffered writes + barrier commit** or post-barrier direct
   mutation. Never both in one pass.
4. **Save/load round-trip.** Every new per-column, per-chunk, or
   per-entity field carries `#[serde(default)]` so pre-kernel saves
   still open.

## What is deliberately not here

This directory freezes the *design*, not the implementation. See the
[Organism Kernel plan](../../.cursor/plans) for phase timing:

- Phase 1 (this directory) — spec freeze.
- Phase 2 — Set A + editor scaffolding.
- Phase 3 — Set B (chem + nerves).
- Phase 4 — Set C (buoyancy + temp niche).
- Phase 5 — Set D (land plants + shade).
- Phase 6 — Set E (litter fungi + epiphytes + toppling + ghost roots).
- Phase 7 — animals (Fore-lane locomotion), outside the kernel arc.

No animal locomotion, no learned nets, no true mycorrhizae, no MUD
freeform drawing, no full GVSE geology coupling beyond `Organic` +
the substrate tag from [`FUNGI.md`](FUNGI.md). See
[`CORE_FEATURES.md`](CORE_FEATURES.md) for the explicit non-goals.
