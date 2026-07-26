# Muscle / Bone / Neural Test Studio

*Major feature track on the active voxel stack. Spec + phased roadmap.
Companion to [`CORE_FEATURES.md`](CORE_FEATURES.md) Sets A–E and
[`PALETTE.md`](PALETTE.md) reserved `Skin` / `Muscle` / `Bone` slots.*

## Why this exists

Train a creature’s **morphology + neural controller** in a controlled
arena, then **export the same body into the world** and watch it fail
or succeed under identical physics.

If a fin that flaps beautifully in the studio sinks in the ocean, that
is a studio/world bug — not “two sims.” The studio is not a toy
physics sandbox with different springs.

## Non-negotiable contract

1. **One physics.** The arena is a `wk_voxel::World`. Water, gravity,
   seepage, grain, phase, temperature, and karst use the **same**
   rule implementations as `wk-voxel-app`. No forked CA maths.
   Studio may **gate** which passes run (see Physics controls) so a
   dry walking bench is not paying for ocean flow — gates skip work,
   they do not replace rules.
2. **Full material vocabulary.** Every `MaterialId` available in the
   world is paintable in the studio (sand, stone, gravel, water, ice,
   …) so you can build rough terrain, pools, ice sheets, etc. Tissue
   is a separate overlay layer, not a replacement for geology.
3. **Scalable arena.** Width/height are configurable (chunk multiples
   welcome). A flapping-fin tank and a long rough-terrain walk track
   are the same system at different sizes.
4. **One look.** MS-Paint 1× pixels, same cell draw path as the world
   demo (`cell_color` + tissue overlay). No anti-aliased limbs.
5. **Paint → activate.** Terrain paints `MaterialId` into the world.
   Tissue paints the body overlay. **Activate** builds the runtime
   body graph. While painting, the sim may be paused; after activate,
   gated world ticks + body step run.
6. **Export is a strip, not a rewrite.** Export drops studio-only
   fixtures and bench apparatus, keeps bone / muscle / skin / nerve /
   neural weights (+ optional terrain seed separately). Body spawns
   into the world on the same grid rules.
7. **Bugfix once.** Physics/coupling fixes land in `wk-voxel` (or
   shared studio helpers both apps call).

Isolation: studio crates depend on `wk-voxel` + `wk-material` only
(same guardrail as `wk-voxel-app`). No column-stack imports.

## Physics controls

Default training benches turn **most** world passes off and keep only
what the scenario needs:

| Preset | Typical gates |
|--------|----------------|
| `body_only` | CA off — morphology / net debug |
| `sandbox` | gravity + water flow + grain (default paint bench) |
| `dry_walk` | gravity + grain (+ repose); flow/seepage/failure off |
| `hydro_fin` | gravity + water flow (+ optional seepage); failure off |
| `full` | same as world demo CA path |

Painting water (or `W` flood) calls `enable_water_physics` so columns
do not sit unflowed under `dry_walk` / `body_only` (also re-wakes
settled wet chunks).

Karst / rain / clouds / temperature stay opt-in (off unless the
scenario wires them). More toggles can land without changing rule code.

## Sensors (evolving)

| Priority | Sensor | Role |
|----------|--------|------|
| **v1** | **Muscle feedback** | Length, commanded actuation, tension proxy — proprioception for the net |
| later | Fixture force / contact | Bench thrust / ground reaction |
| later | Vestibular, touch, chem, vision-ish | Creature inputs for gait / behaviour |

Until the richer set exists, training fitness and net inputs come from
**muscle feedback** (and simple kinematics). Fixture `ForceSensor`
paint remains reserved.

## Product shape

| Piece | Crate / binary |
|-------|----------------|
| Shared body / nerve / GA / export types | `crates/wk-voxel-studio` |
| Pixel studio UI | `crates/wk-voxel-studio-app` (`wk-voxel-studio`) |
| World host (import target) | `crates/wk-voxel-app` (later: spawn / load exported body) |
| Shared CA + hydro | `crates/wk-voxel` |

The old kernel line “petri = the world, no sandbox” still holds for
Sets A–E ecology. The **Studio** is an *arena mode* of that same
world, not a second physics engine.

## Paint vocabulary

Two layers:

1. **Geology** — full `MaterialId` set (sand, stone, gravel, water, ice,
   …) painted into the `World` for rough terrain, pools, etc.
2. **Tissue** — studio overlay (`TissueKind`) for the creature / bench.

After activate, the body **occupies** grid cells for collision and
hydro coupling the same way world organisms do — so a flapping fin
pushes water.

| Kind | Role | Default RGB | Notes |
|------|------|-------------|-------|
| `Bone` | Rigid skeletal cluster | `#EFE7DA` | Aligns with palette `Bone` `0x15` |
| `Muscle` | Contractile link between anchors | `#C33C3C` | Aligns with palette `Muscle` `0x14` |
| `Skin` | Membrane / soft surface | `#FFDBAC` | Aligns with palette `Skin` `0x13` |
| `Nerve` | 1-px signal thread | `#B08A8A` | Pink-gray; studio thickens into soma |
| `NeuronBlob` | ≥2×2 nerve mass = processing | `#9A7070` | Holds neurons / weights |
| `Fixture` | Infinitely strong bench mount | `#2A2A2A` | Studio-only; stripped on export |
| `JointFull` | Free hinge | cyan `#2EE0F0` | Rotation limit = τ; **not** ForceSensor |
| `Joint3_4` | ±3/4 turn | cyan `#2EE0F0` | |
| `JointHalf` | ±1/2 turn | cyan `#2EE0F0` | Paint between fixture/bone and distal bone |
| `JointQuarter` | ±1/4 turn | cyan `#2EE0F0` | |
| `ForceSensor` | Uniaxial force on fixture | steel `#4A6FA5` | Studio-only; **does not hinge** |

Symbols on joint pixels are 1× overlays (tick marks), not separate
materials. Exact glyph atlas lives next to the studio palette module.

### Paint → activate pipeline

```
Paint grid (TissueKind[][])
        │
        ▼
Activate
  · connected Bone components → rigid bodies
  · Joint* between bones → hinge with angle limits
  · Muscle runs → contractile springs (actuation 0..1)
  · Skin → membrane constraints / collision hull
  · Nerve + NeuronBlob → directed graph + weight tensors
  · Fixture cells → world-anchored immovable body
  · ForceSensor on fixture edges → sample contact/actuator force
        │
        ▼
Runtime BodyGraph stepped each tick *after* voxel CA
  (read hydro / write occupancy; muscles driven by neural out)
```

## Example scenario — flapping fin bench

1. Paint a dark-gray **fixture** wall on one side of a small arena.
2. Mount **bones** + **joints** + **muscles** into a fin; add a
   **nerve** path into a **neuron blob**.
3. Place a **force sensor** on the fixture between two fixture pixels
   (measures force along one axis — proxy for thrust / reaction).
4. Fill the arena with water (same rain / fill tools → `Air+sat` CA).
5. **Train** the neural net (generational / episodic) to maximize
   sensor-derived fitness (e.g. mean directed force, or work).
6. Run a **genetic algorithm** in parallel that mutates bone/muscle
   placement and lengths (and optionally net weights) under the same
   fitness.
7. **Export** the best body (no fixture/sensors) into `wk-voxel-app`
   and spawn it free in the ocean.

## Neural training vs world kernel

[`CORE_FEATURES.md`](CORE_FEATURES.md) and [`NERVES.md`](NERVES.md)
freeze **world** Sets A–E as gene-weight nerves with **no plastic
training**. The Studio **explicitly lifts that ban inside the arena**:

| Context | Neural policy |
|---------|----------------|
| World ecology (Atoms / plants / fungi) | Gene weights, no backprop / no fitness loop |
| Studio bench | Trainable nets + GA; weights become genes/export payload |
| Exported body in world | Frozen weights (genes); may later allow slow world-side plasticity as a separate Set |

Studio training does not require rewriting Set B for plankton; it
adds a body controller path used by animal-like exports.

## Genetic algorithm (studio)

- Population of `StudioGenome` = morphology deltas + neural weights.
- Mutation: nudge muscle endpoints, bone length, joint type, small
  weight noise (deterministic hash seeds, same as the rest of GVSE).
- Selection: tournament / elite on fitness from sensors (+ optional
  energy / damage penalties).
- Evaluation: each individual gets a short arena episode on the
  **same** `World` tick path (parallel jobs must not share a mutable
  world — clone arena or shard episodes).
- No global world fitness function for ecology; GA stays studio-scoped
  until we deliberately add a world challenge mode.

## Save / export formats

| Format | Contents |
|--------|----------|
| `.gvsestudio` | Full bench: tissue paint, fixtures, sensors, water seed, training config, RNG |
| `.gvsebody` | Export: tissue + joints + muscles + nerve graph + weights; **no** fixture/sensors |
| Future | World spawn reads `.gvsebody` into the voxel organism/body store |

Postcard + schema version, same spirit as `.gvsecrt`.

## Shared-physics checklist (review every PR)

- [ ] Arena uses `wk_voxel::World` + production tick order for CA.
- [ ] Body step runs at a documented point in the frame (after CA,
      before or after organisms — pick one and keep both apps aligned).
- [ ] Water fill / empty uses existing rain or cell paint, not a
      studio-only fluid.
- [ ] Any new coupling (drag, buoyancy on body cells) lives in
      `wk-voxel` or `wk-voxel-studio` called from both apps.
- [ ] Export strip is tested: fixture/sensor absent; body present.

## Phased delivery

| Phase | Deliverable | Done when |
|-------|-------------|-----------|
| **S0** | This doc + `wk-voxel-studio` types + empty studio app arena | ✅ Arena ticks water with world rules; paint enum + colours locked |
| **S1** | Paint UI + activate → `BodyGraph` (bones, fixtures) | ✅ `Enter` activates; hung bones stay, free bones fall (discrete gravity); no muscle yet |
| **S2** | Joints + scripted muscle + hydro push + muscle feedback | ✅ Hinge water displace; free-body buoyancy/drag/inertia; serial chains; soft tissue follow; `tests/hydro_body.rs`, `tests/hinge_*.rs` |
| **S3** | Nerves / richer sensors | ✅ Nerve strands + neuron blobs on activate; muscle feedback is v1 net input; fixture force later |
| **S4** | Neural training (fixed morphology) | ✅ Hill-climb + live `StudioNet` drive; app `H` / `C` / `N` |
| **S5** | GA morphology search | ✅ Paint mutate + evaluate; app `M` |
| **S6** | Export `.gvsebody` → world spawn | ✅ Studio `E` export with net; world spawn hook next |

Do not start S4 until S2 proves hydro coupling on the shared CA.

## Relationship to reserved palette slots

Studio `Bone` / `Muscle` / `Skin` RGB match [`PALETTE.md`](PALETTE.md)
`0x13`–`0x15` so editor drawings and world modules stay one atlas.
Studio-only kinds (`Fixture`, joints, `ForceSensor`) are **not**
world `ModuleId`s and must not be allocated in the core 0x00–0x15
band.

## Out of scope (for now)

- 3D, continuum FEM, or replacing the voxel water CA.
- Training loops inside the open world ecology tick.
- Full animal predation / hunting AI.
- Column-stack (`wk-app`) port — voxel stack only.

## Code anchors

| Concept | Location |
|---------|----------|
| Tissue / joint / sensor kinds | `crates/wk-voxel-studio` |
| Arena + training/GA (later) | `crates/wk-voxel-studio` |
| Studio binary | `crates/wk-voxel-studio-app` |
| World CA | `crates/wk-voxel` |
| World demo / eventual import | `crates/wk-voxel-app` |
