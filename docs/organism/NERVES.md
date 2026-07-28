# Nerves

*Frozen neural graph inputs, outputs, and wire semantics.
`neural-min` in the Organism Kernel plan.*

## What "neural" means at kernel level

The gray blob is **not** a trained deep net. It is a tiny fixed graph
with a handful of thresholds and gains stored as genes. Mutation
tweaks the weights and, occasionally, which wires exist. Plastic
learning (a runtime training loop) is an explicit non-goal for the
kernel — later phases can upgrade the same gray visual language.

Every visible gray pixel in the creature has a job:

- `NeuralSoma` (2×2, `#7F7F7F`) — the controller. Reads its incoming
  wires each tick, applies a fixed activation, writes its outgoing
  wires.
- `Axon` (1-px line, `#AAAAAA`) — a signal path from a module output
  or another soma to a soma input. Each axon has a cost (upkeep per
  length) and is visible in the drawing.

Cream hyphae are the same line grammar but a different colour and a
different role (see [`FUNGI.md`](FUNGI.md)); they never carry neural
signal.

## Fixed I/O list (Core)

### Inputs into `NeuralSoma`

| Wire | Source | Payload |
|------|--------|---------|
| `life_stats` | `Nucleus` | `Energy.current / Energy.max`, `Energy.max`, clock phase (0..1) |
| `production` | `Photosystem` (per pixel) | Instantaneous harvest rate this tick |
| `chem_level[c]` | `ChemoSensor` (one per sensor module) | Scaled + thresholded reading of `Chem[c]` (see [`CHEM.md`](CHEM.md)) |
| `chem_gradient[c]` | `ChemoSensor` in gradient mode | `+x − −x` reading, signed |
| `light_local` | Available whenever the creature has a `Photosystem` | Remaining light at the top-most green pixel (see [`LIGHT.md`](LIGHT.md)) |
| `temp_local` | Always available on land / water | Sampled from `ThermalField` at pose |

Sensors that are not physically present on the creature simply have
no wire — the input is absent, not zero.

### Outputs from `NeuralSoma`

| Wire | Sink | Effect |
|------|------|--------|
| `emit_rate[c]` | `ChemoEmitter` | Drives release rate (see [`CHEM.md`](CHEM.md)) |
| `depth_target` | `Buoyancy` | Desired float depth in metres; `Buoyancy` pumps toward it |
| `metabolic_throttle` | `Photosystem` (green harvest) | Multiplies effective harvest 0..1; enables a "close stomata" analogue |
| `active_gate` | `Nucleus` clock | Optional soma override on the sleep window (see `CircadianPhase` in [`GENES.md`](GENES.md)) |
| `dig_drive` | archived column grazer | Bridging wire for legacy `wk_agents::Grazer` only; not used by voxel `OrganismStore` |

## Wire semantics

- Each wire has a **sign** (excite or inhibit) stored as a gene.
- Each wire has a **weight** in a small range (e.g. `-1..1` after
  gain scaling) with a `#[serde(default)]` middling default so
  Blueprints without weights load as neutral.
- Soma activation is a saturating linear sum then a soft threshold
  (`tanh` or a piecewise clamp; pick in Phase 3, the shape is a gene).

Because the graph is *drawn*, wire count is bounded by pixel count.
A blueprint with 12 axons has 12 weights. This keeps mutation
comprehensible.

## What lives in `Genome` for the neural side

Global (per-organism) neural genes:

| Gene | Meaning |
|------|---------|
| `soma_activation_shape` | Tanh / clamp / linear (`u8` enum). |
| `soma_bias` | Baseline offset before activation. |
| `axon_upkeep_per_px` | Upkeep tax on every axon-pixel. |

Per-axon (stored in the `Blueprint::wires` vector) genes:

| Gene | Meaning |
|------|---------|
| `sign` | `+1` excite, `-1` inhibit. |
| `weight` | Signed float in a fixed range. |
| `delay` | Optional integer tick delay (0 or 1 for Core). |

## What is deliberately not here

- Learned weights. `axon.weight` is a gene, not a runtime variable.
- Recurrent training loops.
- Anything you cannot draw. If a wire is not on the canvas, it does
  not exist in the sim.

## Debug story

The overlay in Phase 3 will draw each active axon coloured by
signed activation (blue = inhibit, orange = excite, brightness =
magnitude), so watching a creature "think" is a screenshot. Same
principle as the existing overlays cycled in `wk-voxel-app` (column-era
`O` cycle lived in archived `wk-app`).

## Coupling to existing GVSE

- Reads sample the same committed fields (`ThermalField`,
  `ChemField`, remaining-light column array) that other subsystems
  read post-barrier.
- Writes buffer through the same barrier as any other module-driven
  subsystem — a `run_neural` pass computes wire values into a scratch
  buffer, and effector modules apply them the next tick.
- `Genome::mutate` in
  [`crates/legacy/wk-agents/src/lib.rs`](../../crates/legacy/wk-agents/src/lib.rs)
  extends to jitter the neural genes above. The existing
  determinism seed (`hash_u64(world.seed, tick, entity_id, salt)`)
  applies unchanged.
