# Organism Kernel

*Design freeze for the pixel-blob organism vocabulary GVSE will grow
against. This directory is spec-only — no code lives here.*

## Design stance

Everything an organism *is* draws like **MS Paint at 1× zoom**. The
smallest useful mark is a 1×1 pixel; a module is a coloured pixel with
one job, one cost, and one visible tradeoff. If you cannot see it, it
is not a module.

Preferences (locked):

- Few `ChemType` channels over realistic biochemistry.
- Few wires over a learned neural net.
- Grow the palette in stages so the simplest creature stays two pixels.
- Phenotype is the drawing. Save the drawing, save the creature.

## Petri = the current GVSE world

There is no separate sandbox crate. The petri dish is the live world
you scroll through — **today that is [`wk-voxel-app`](../../crates/wk-voxel-app)**
on the greenfield cell grid ([`wk-voxel`](../../crates/wk-voxel)). See
[`VOXEL_MIGRATION.md`](../VOXEL_MIGRATION.md).

Creature studio / editor UX lives in `wk-voxel-app` (see
[`EDITOR.md`](EDITOR.md)). Atom-bloom and other kernel falsification
scenes (E30+) are studio / follow-up work, not a physics-wave
prerequisite.

### Column stack = archive only

The original design anchors (column ecology ledger, field slots,
scripted grazer ECS) live under **[`crates/legacy/`](../../crates/legacy/)**.
They remain for reference and for the column scenario suite in
`tests/scenarios/`. They are **not** the active runtime. Do not add
features there; do not import them from `wk-voxel` / `wk-voxel-app`.

| Archived column hook | Path |
|----------------------|------|
| Columns, layers, chunks | [`crates/legacy/wk-world`](../../crates/legacy/wk-world) |
| Multirate scheduler | [`crates/legacy/wk-sim`](../../crates/legacy/wk-sim) |
| Field slots (thermal / humidity / …) | [`crates/legacy/wk-field`](../../crates/legacy/wk-field) |
| Scripted grazer ECS | [`crates/legacy/wk-agents`](../../crates/legacy/wk-agents) |
| Column UI host | [`crates/legacy/wk-app`](../../crates/legacy/wk-app) |
| Shared material vocabulary | [`crates/wk-material`](../../crates/wk-material) (still shared) |

Live Set A / Set D organisms are implemented in `wk-voxel`
(`organism.rs`, `plant.rs`, `fungi.rs`) and drawn/edited in
`wk-voxel-app`. Plant notes: [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md).

## Reading order

| Doc | Freezes |
|-----|---------|
| [`PALETTE.md`](PALETTE.md) | The module colour atlas + exact RGB hex + pixel grammar |
| [`CHEM.md`](CHEM.md) | `ChemType` IDs, per-chunk chem field, sensor/emitter tuning |
| [`NERVES.md`](NERVES.md) | Fixed neural graph inputs / outputs / wire semantics |
| [`LIGHT.md`](LIGHT.md) | Shade scan and the light competition rule |
| [`PLANTS.md`](PLANTS.md) | Design reference: rooted plants, epiphytes, topple (check [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) for live status) |
| [`VOXEL_PLANTS.md`](VOXEL_PLANTS.md) | What landed on the voxel stack (Set D / E1 fungi, saplings, Symbiont) |
| [`FUNGI.md`](FUNGI.md) | Litter fungi, mycelium field, Symbiont trade, spore bank |
| [`LANES.md`](LANES.md) | Fore / Mid / Back depth-lane occupancy for future animals |
| [`CORE_FEATURES.md`](CORE_FEATURES.md) | Feature Sets A–E lock and explicit non-goals |
| [`GENES.md`](GENES.md) | Gene table with tradeoffs; merge notes vs archived `wk_agents::Genome` |
| [`FIELDS.md`](FIELDS.md) | Petri fields (light, temp, chem, moisture, organic, substrate, stem wetness) |
| [`SCENARIOS.md`](SCENARIOS.md) | Falsification scenes (E30–E45 skeletons; column E1–E17 archived) |
| [`EDITOR.md`](EDITOR.md) | MS-Paint editor / studio UX, `Blueprint` save format, spawn flow |

## Archive hook table (column stack)

Historical anchors the kernel specs grew against. **Reference only** —
paths are under `crates/legacy/`. New work binds to `wk-voxel` instead.

| Concept | Archived column hook |
|---------|----------------------|
| Coarse column biomass | `Ecology { … }` in [`crates/legacy/wk-world/src/column.rs`](../../crates/legacy/wk-world/src/column.rs) |
| Dead plant / root mass ledger | `MassAudit` biomass buckets in [`crates/legacy/wk-world/src/world.rs`](../../crates/legacy/wk-world/src/world.rs) |
| Organic material slot | `MaterialId::Organic` in [`crates/wk-material`](../../crates/wk-material) (shared; used by voxel too) |
| Per-chunk fields | Slot pattern in [`crates/legacy/wk-field`](../../crates/legacy/wk-field) |
| Genome / scripted grazer | [`crates/legacy/wk-agents`](../../crates/legacy/wk-agents) — superseded by module-pixel `Blueprint` + `OrganismStore` in `wk-voxel` |
| Post-barrier subsystem slot | `Simulation::step` in [`crates/legacy/wk-sim/src/sim.rs`](../../crates/legacy/wk-sim/src/sim.rs) |
| Column moisture / water table | `column.moisture` + groundwater head in the legacy stack |
| App UI host | Studio in [`crates/wk-voxel-app`](../../crates/wk-voxel-app); column-era host was [`crates/legacy/wk-app`](../../crates/legacy/wk-app) |

## Cross-cutting invariants (unchanged)

The organism kernel obeys the four rules that carry the rest of GVSE:

1. **Determinism.** Blueprint spawn, mutation, and every organism
   subsystem seed off `hash_u64(world.seed, tick, entity_id, salt)`.
2. **Mass audit.** Every new mass sink or source gets its own bucket.
3. **Buffered writes + barrier commit** or post-barrier direct
   mutation. Never both in one pass.
4. **Save/load round-trip.** New fields carry `#[serde(default)]` so
   older saves still open.

## Non-goals (this directory)

- Implementing systems (code lives in `wk-voxel` / `wk-voxel-app`)
- Re-activating the scripted column grazer
- A separate Petri crate
