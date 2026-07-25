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
   `tick_*` / field path as `wk-voxel-app`. No forked CA rules.
2. **One look.** MS-Paint 1× pixels, same cell draw path as the world
   demo (`cell_color` + tissue overlay). No anti-aliased limbs.
3. **Paint → activate.** The editor paints tissue / fixture / joint /
   sensor pixels. **Activate** builds the runtime body graph. While
   painting, the sim may be paused; after activate, the world ticks.
4. **Export is a strip, not a rewrite.** Export drops studio-only
   fixtures and bench sensors, keeps bone / muscle / skin / nerve /
   neural weights, and spawns into the world organism store (or a
   dedicated body store) on the same grid rules.
5. **Bugfix once.** Any physics or coupling fix lands in `wk-voxel`
   (or shared studio types that both apps call) — never “studio-only
   water” or “world-only joints.”

Isolation: studio crates depend on `wk-voxel` + `wk-material` only
(same guardrail as `wk-voxel-app`). No column-stack imports.

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

Tissue and bench pixels are a **studio layer** (`TissueKind`) drawn
and saved with the body. They are **not** new geological
`MaterialId`s (those stay rock / sand / water). After activate, the
body **occupies** grid cells for collision and hydro coupling the
same way world organisms do — so a flapping fin pushes water.

| Kind | Role | Default RGB | Notes |
|------|------|-------------|-------|
| `Bone` | Rigid skeletal cluster | `#EFE7DA` | Aligns with palette `Bone` `0x15` |
| `Muscle` | Contractile link between anchors | `#C33C3C` | Aligns with palette `Muscle` `0x14` |
| `Skin` | Membrane / soft surface | `#FFDBAC` | Aligns with palette `Skin` `0x13` |
| `Nerve` | 1-px signal thread | `#B08A8A` | Pink-gray; studio thickens into soma |
| `NeuronBlob` | ≥2×2 nerve mass = processing | `#9A7070` | Holds neurons / weights |
| `Fixture` | Infinitely strong bench mount | `#2A2A2A` | Studio-only; stripped on export |
| `JointFull` | Free hinge | `#FFFFFF` + symbol | Rotation limit = τ |
| `Joint3_4` | ±3/4 turn | `#F5F5F5` + symbol | |
| `JointHalf` | ±1/2 turn | `#EBEBEB` + symbol | |
| `JointQuarter` | ±1/4 turn | `#E0E0E0` + symbol | |
| `ForceSensor` | Uniaxial force on fixture | `#4A6FA5` | Studio-only; feeds fitness |

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
| **S2** | Joints + muscle actuation (open-loop / scripted) | Scripted flap moves water enough to move a tracer |
| **S3** | Nerve graph + neuron blobs + force sensors | Sensor time series readable in UI |
| **S4** | Neural training regime (fixed morphology) | Net learns a flap above a fitness floor |
| **S5** | GA morphology search (parallel episodes) | Better-than-seed fin after N generations |
| **S6** | Export `.gvsebody` → spawn in `wk-voxel-app` | Same weights behave in open water |

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
