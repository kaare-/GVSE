# Genes

*Pixel-gene model (Wave K). Every painted module cell carries a
[`PixelTraits`](../../crates/wk-voxel/src/blueprint.rs) payload; the
organism's global scalars are **aggregates** of those pixels
([`BodyPlan`](../../crates/wk-voxel/src/aggregate.rs)).*

## Pixel-gene model

- A **pixel gene** is one `PlacedModule`: `(x, y, lane, ModuleId, PixelTraits)`.
- There is no separate authoritative global gene table for new work.
  The vestigial `Genome` on `Blueprint` / `Atom` still drives live
  physics (metabolism, plant alloc, digest, …) until follow-up waves
  rewire those reads to `BodyPlan`.
- **Bone / Muscle / Skin** are first-class studio kinds: paint,
  inspect, aggregate, mutate. Sim physics for them lands in Wave L
  (world `MaterialId` + differential decay).

### `PixelTraits` (per cell)

All fields default to `1.0` (`#[serde(default)]`) so older mental
models and missing postcard fields upgrade cleanly:

| Trait | Role |
|-------|------|
| `mass` | Local mass contribution |
| `density` | Scales mass into `total_mass` (`mass × density`) |
| `stiffness` | Bone column capacity (Wave N live fragility) |
| `strength` | Scales Bone capacity; Muscle contraction later |
| `upkeep_bias` | Contributes to aggregate metabolic cost |
| `absorb_bias` | Photosystem harvest lean |
| `drink_bias` | Root drink lean |
| `clone_fidelity_bias` | Mutation tightness |
| `reproduce_at_bias` | Repro energy gate lean |
| `buoyancy_bias` | Float / sink lean |
| `alloc_stem` / `alloc_leaf` / `alloc_root` | Nucleus growth habit (Wave O) |
| `root_depth_bias` | Root dive lean (Wave O) |
| `shade_efficiency` | Photosystem dim-light lean (Wave O) |
| `digest_rate` | Digest / Hypha litter rate (Wave O) |

Kinds ignore traits they do not use; unused fields stay inert until
a physics wave binds them.

### `BodyPlan` aggregates

Computed by `body_plan_from` / `Blueprint::body_plan` /
`Atom::recompute_body_plan`:

| Aggregate | Formula (first cut) |
|-----------|---------------------|
| `total_mass` | Σ (`mass × density`) |
| `metabolic_rate` | Σ `upkeep_bias` (floor 0.05) |
| `clone_fidelity` | Mass-weighted mean of `clone_fidelity_bias` |
| `reproduce_at` | Mass-weighted mean of `reproduce_at_bias` |
| `buoyancy_bias` | Mass-weighted mean of `buoyancy_bias` |
| `photo_capacity` | Σ `absorb_bias` over Photosystem pixels |
| `nucleus_count` / `has_repro_gate` | Nucleus presence |

Nucleus is a functional kind (anchor + repro gate), not a schema
requirement for painting. Anchor rule: first Nucleus if present,
else bounding-box centre.

### Mutation (`Blueprint::mutate_child`)

Deterministic in `(world_seed, tick, parent_id)`. Ops:

1. **Jitter** — every pixel's traits wiggle; sigma scales with
   `(1 − clone_fidelity)`.
2. **Chain-grow** — rare append of a same-kind orthogonal neighbour
   with jittered traits.
3. **Kind-swap** (Wave Q) — rarer rewrite of one pixel to a related
   kind (`kind_swap_partners`); never the last Nucleus.
4. **Delete** — rarer removal of a non-last-Nucleus pixel.

Studio **Mutation Preview** rolls `mutate_child(0, 0, 0)` and shows
Δpixels / Δmass / Δmetabolic beside a half-size child glyph.

Live Atom fission builds a temporary blueprint via
`Atom::to_mutation_blueprint` and runs the same `mutate_child` path.

### Wave M — live physics binding

Spawn copies painted traits onto `Atom.body_traits`. Aggregates drive:

| Read | Source |
|------|--------|
| Upkeep | `body_plan.metabolic_rate` (= Σ `upkeep_bias`) |
| Photo harvest | `body_plan.photo_capacity` (= Σ Photosystem `absorb_bias`) |
| Repro gate / threshold | `has_repro_gate` / `reproduce_at` |
| Buoyancy / clone fidelity | mass-weighted means (synced onto `Atom` pose knobs) |
| Root drink energy | mean Root `drink_bias` |
| Leaf shade cast | `leaf_absorb_effective()` (painted absorb, else `Genome::leaf_absorb`) |

`PixelTraits` defaults for buoyancy / fidelity / repro match the old
`Genome` defaults (floater, 0.9, 0.85) so unpainted bodies behave as
before.

### Wave N — Bone fragility

Live `ModuleId::Bone` capacity =
`3.5 × stiffness × density × strength`. Column load is Σ
(`mass × density`) of modules stacked above in the same body `dx`.
Overloaded Bone fractures (drops world `Sand`, removes the pixel).
Dead world Bone crush is a separate opt-in geotech pass — see
[`VOXEL_BIOLOGY.md`](../VOXEL_BIOLOGY.md).

### Wave O — plant / fungus knobs on pixels

Remaining `Genome` plant fields are painted onto kinded traits and
aggregated on `BodyPlan`. Live growth / shade / digest **read the plan**,
not `atom.genome` (which stays a Tab / blueprint mirror).

| Knob | Pixel home | BodyPlan aggregate |
|------|------------|--------------------|
| `alloc_stem/leaf/root` | Nucleus | Mean Nucleus (else mean all) |
| `root_depth_bias` | Root | Mean Root (default 0.55) |
| `shade_efficiency` | Photosystem | Mean Photosystem (default 0.40) |
| `digest_rate` | Digest / Hypha | Mean of those kinds (default 0.8) |
| `leaf_absorb` (Tab) | Photosystem `absorb_bias` | via `photo_capacity` / `leaf_absorb_effective` |

`apply_genome` writes Tab / blueprint values into the matching pixels,
then `recompute_body_plan` mirrors aggregates back onto `Genome`.
Studio Gene Inspector exposes the new sliders per kind. Full deletion of
the `Genome` struct is deferred until blueprint postcard migration.

### Wave Q — kind-swap mutation

`Blueprint::mutate_child` may rewrite one pixel to a related kind via
[`kind_swap_partners`](../../crates/wk-voxel/src/blueprint.rs)
(Photosystem↔Stem/Skin, Root↔Stem, Digest↔Hypha, Bone↔Muscle, …).
Never swaps away the last Nucleus. Live Atom fission uses the same
pipeline through `Atom::to_mutation_blueprint`.

### Wave P — visual trait feedback

Draw path tints module RGB from local `PixelTraits` via
[`modulate_module_rgb`](../../crates/wk-voxel/src/blueprint.rs):

- Frozen [`ModuleId::rgb`](PALETTE.md) remains the save-format identity.
- **Default traits → identical RGB** (no drift for unpainted bodies).
- Kind-aware leans: Photosystem absorb/shade, Root drink/depth, Bone
  density/stiffness, Muscle strength, Digest/Hypha rate, Stem mass, Skin
  density. Nucleus stays `#000000`.
- Living `OrganismStore::draw_list` and studio canvas / mutation preview
  use the tint. Corpses drop traits → frozen palette then `corpse_rgb`.

## Studio surfaces

See [`EDITOR.md`](EDITOR.md): Gene Inspector (selected pixel
sliders), Body Plan readout, Mutation Preview. Hotkeys `7` / `8` /
`9` paint Bone / Muscle / Skin.

## Superseded — global `Genome` table

The sections below document the pre-Wave-K organism-wide gene table
and the archived column `wk_agents::Genome`. Kept for provenance;
new design work should extend `PixelTraits` / aggregates instead.

### Rules for genes (legacy framing)

- A gene has a **name**, a **domain**, a **tradeoff**, and a
  **default**.
- Genes were stored on `Genome` (per organism) or on a specific
  module blueprint entry (per-module tuning like emitter
  `tuned_type`).
- Mutation was deterministic per trait via `Genome::mutate`.
- Every new gene got `#[serde(default)]` so old blueprints load.
- No gene is silently free — a high value must **cost** something on
  screen (upkeep, waste, wrong-niche death).

### Archived `wk_agents::Genome` fields

For reference, the column-stack fields (stage 10 / 11) in
`crates/legacy/wk-agents`:

```rust
pub struct Genome {
    pub move_speed: f32,
    pub graze_rate: f32,
    pub drink_rate: f32,
    pub dig_drive: f32,
    pub graze_efficiency: f32,
    pub metabolism: f32,
    pub repro_drive: f32,
}
```

These map into the kernel table as follows:

| Existing | Kernel gene | Change |
|----------|-------------|--------|
| `metabolism` | `MetabolicRate` | Rename only. Same meaning. |
| `repro_drive` | (rolled into) `ReproduceAt` | Energy-fraction threshold is primary. |
| `move_speed` | `LocomotionSpeed` (Phase 7) | Retained; unused until locomotion. |
| `graze_rate` | `BrowseRate` (Phase 7) | Animal browse; not Set A–E. |
| `graze_efficiency` | `BrowseEfficiency` (Phase 7) | As above. |
| `drink_rate` | `DrinkRate` | Water-column / walker; plants use moisture. |
| `dig_drive` | (removed) | Replaced by module-triggered dig later. |

### Kernel gene table (still mirrored on vestigial `Genome`)

#### Metabolism & life cycle

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `MetabolicRate` | `f32 ≥ 0` | Wins blooms; starves fast. | Thrifty; loses races. |
| `ReproduceAt` | `f32 in 0..1` | Reproduces often, offspring weak. | Rare, fat offspring. |
| `CloneFidelity` | `f32 in 0..1` | Costly precise clones. | Cheap messy / mutative clones. |
| `CircadianPhase` | `f32 in 0..1` | Locks activity into a band. | Always-on cost. |
| `ActiveWindow` | `f32 in 0..1` | Long active window. | Narrow window. |

#### Physical niche

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `BuoyancyBias` | `f32 in 0..1` | Heavy: sinks. | Buoyant: rides under free surface. |

#### Land plants

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `StemVsLeafVsRoot` | `[f32; 3]` | Skew surplus into stem / leaf / root. | |
| `LeafAbsorb` | `f32 in 0..1` | Strong shade cast. | Passes light. |
| `ShadeEfficiency` | `f32 in 0..1` | Dim-light harvest. | Full-sun specialist. |
| `RootDepthBias` | `f32 in 0..1` | Deep dive. | Shallow sprawl. |

#### Fungi

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `DigestRate` | `f32 ≥ 0` | Fast litter clearing. | Starves when litter scarce. |

## What is deliberately not here

- Species labels. Species emerge from clusters in trait space.
- Fitness functions. No global fitness — see
  [`docs/EVOLUTION.md`](../EVOLUTION.md).
- Meta-genes (mutation rate of the mutation rate). Later.
- Kind-specific Bone / Muscle / Skin world physics — Wave L.
- Runtime-modifiable weights outside mutation. See
  [`NERVES.md`](NERVES.md) — no plastic learning.
