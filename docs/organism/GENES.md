# Genes

*Frozen gene table with tradeoffs and merge notes against the
**archived** column [`wk_agents::Genome`](../../crates/legacy/wk-agents/src/lib.rs).
Live voxel genomes live on `wk-voxel` organisms / blueprints.
`gene-table` in the Organism Kernel plan.*

## Rules for genes

- A gene has a **name**, a **domain**, a **tradeoff**, and a
  **default**.
- Genes are stored on `Genome` (per organism) or on a specific
  module blueprint entry (per-module tuning like emitter
  `tuned_type`).
- Mutation is deterministic per trait. See `Genome::mutate` in
  [`crates/legacy/wk-agents/src/lib.rs`](../../crates/legacy/wk-agents/src/lib.rs)
  for the hash-and-jitter template that all new genes follow.
- Every new gene gets `#[serde(default)]` so old blueprints load.
- No gene is silently free — a high value must **cost** something on
  screen (upkeep, waste, wrong-niche death).

## Archived `wk_agents::Genome` fields

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
| `repro_drive` | (rolled into) `ReproduceAt` | `repro_drive` was a per-tick roll gate; the kernel `ReproduceAt` is an energy fraction threshold. Keep `repro_drive` as an *additional* gate for compatibility, but the primary trigger becomes the energy threshold. |
| `move_speed` | `LocomotionSpeed` (Phase 7) | Retained; only used when locomotion arrives. Zero for plants and fungi. |
| `graze_rate` | `BrowseRate` (Phase 7) | Retained; used by the animal `Browse` interaction on Mid plant cells. Not used by Set A–E organisms. |
| `graze_efficiency` | `BrowseEfficiency` (Phase 7) | As above. |
| `drink_rate` | `DrinkRate` | Retained for water-column and thirsty walker use. Plants use moisture read, not drink rate. |
| `dig_drive` | (removed) | The scripted grazer's dig drive is replaced by a module-triggered `world.dig` call in Phase 6+ (fungal cavity + burrow already have an API). Kept until Phase 7 for backwards compat. |

Nothing is deleted immediately. Phase 2 renames `metabolism →
metabolic_rate` inside `Genome` with a `#[serde(alias = "metabolism")]`
so pre-kernel saves still load. Everything else is left in place;
new kernel genes append.

## Kernel gene table

### Metabolism & life cycle

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `MetabolicRate` | `f32 ≥ 0` | Wins blooms; starves fast. | Thrifty; loses races. |
| `ReproduceAt` | `f32 in 0..1` (energy fraction of max) | Reproduces often, offspring weak. | Rare, fat offspring. |
| `CloneFidelity` | `f32 in 0..1` | Costly precise clones. | Cheap messy / mutative clones. |
| `CircadianPhase` | `f32 in 0..1` (phase within day) | Locks activity into a specific band; saves upkeep. | Always-on cost. |
| `ActiveWindow` | `f32 in 0..1` (fraction of day active) | Long active window; more harvest, more upkeep. | Narrow window. |

### Physical niche (water column and land)

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `TempOptimum` | `f32` (°C) | Comfort centre for photo / fission (default 22). | Same. |
| `TempWidth` | `f32 ≥ 0` (°C) | Wide comfort; reproduces in more climates (default 18). | Narrow specialist; cold/hot throttles fission. |
| `BuoyancyBias` | `f32 in 0..1` | Heavy: sinks toward the water bed. | Buoyant: rides ~1 m under the live free-water surface; rises/falls with water via weight vs buoyancy (not a constant `sea_level` snap). **Circadian:** active window pulls toward the float side; inactive (night) pulls deeper — day-float / night-sink (E33 interim, before Set C soma wiring). |

### Chemistry (per sensor / emitter module)

| Gene | Domain | Meaning |
|------|--------|---------|
| `tuned_type` | `ChemTypeId` (0..3) | Which channel this sensor / emitter binds. |
| `gain` | `f32 ≥ 0` | Response amplitude. |
| `threshold` | `f32 ≥ 0` | Deadzone. |
| `sensor_mode` | `enum { Level, Gradient }` | Sensor reads local level or spatial gradient. |

### Nerves (global + per-axon)

| Gene | Domain | Meaning |
|------|--------|---------|
| `soma_activation_shape` | `u8` enum | Tanh / clamp / linear activation. |
| `soma_bias` | `f32` | Baseline pre-activation offset. |
| `axon_upkeep_per_px` | `f32 ≥ 0` | Upkeep tax on every axon pixel. |
| `axon.sign` | `+1` / `-1` | Excitatory / inhibitory. |
| `axon.weight` | `f32` in a small range | Signed weight. |
| `axon.delay` | `u8` (0 or 1 for Core) | Optional one-tick delay. |

### Land plants

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `StemVsLeafVsRoot` | `[f32; 3]` summing to 1 | Skew of surplus into stem / leaf / root. Wrong split is visible: pole, bush, or leafless stump. | |
| `LeafAbsorb` | `f32 in 0..1` | Strong shade cast; self-hunger if stacked. | Passes light through; understory friendly. |
| `ShadeEfficiency` | `f32 in 0..1` | Harvest well in dim light; lower peak in sun. | Full-sun specialist. |
| `RootDepthBias` | `f32 in 0..1` | Deep dive into water table; slow in dry season race. | Shallow sprawl; wins wet seasons, dies in drought. |

### Epiphytes and fungi

| Gene | Domain | High | Low |
|------|--------|------|-----|
| `AttachPrefer` | `f32 in 0..1` | Seek olive host stems (epiphyte establishment). | Ignores hosts; needs its own root. |
| `HostLeaveFraction` | `f32 in 0..1` | Gentle rider: passes X of light through own stack to host below. | Smotherer: takes everything. |
| `DigestRate` | `f32 ≥ 0` | Fast litter clearing; boom-crash cycles. | Starves when litter is scarce. |
| `sym_water` | `u8` | Treaty water side of opt-in Symbiont deal (editor `,` / `.`). | Low W / high E skews parasitism toward the fungus. |
| `sym_energy` | `u8` | Treaty sugar side (editor `-` / `=`). | High W / low E skews toward the plant. |

Both partners need painted `ModuleId::Symbiont`. Match is assortative
similarity of `(sym_water, sym_energy)`; see [`FUNGI.md`](FUNGI.md).

### Reserved gene slots

Kept named for editor readability, not yet used by any subsystem:

- `LocomotionSpeed`, `BrowseRate`, `BrowseEfficiency` (Phase 7).
- `Woodiness` (`Bark` slot in [`PALETTE.md`](PALETTE.md), Phase 6+).
- `MutualistDrive` — superseded by `sym_water` / `sym_energy` + Symbiont.

## Merge and rename plan

Phase 2 (Set A code slice) is the moment to reshape `Genome`. The
mechanical plan is:

1. Rename `metabolism → metabolic_rate` with
   `#[serde(alias = "metabolism")]`.
2. Add the Set A / B / C / D / E kernel genes above as new fields
   with `#[serde(default)]` sensible defaults and a `Genome::default()`
   that produces a viable Atom.
3. Column `Genome` graze/dig/repro fields stay on the **archived**
   `crates/legacy/wk-agents` type so column scenarios (E16/E17) keep
   compiling. Voxel organisms do not grow those fields.
4. `Genome::mutate` extends to include the new fields; the salt list
   keeps growing but each gene has its own `trait_i` so mutation is
   stable across additions.
5. **Blueprint / body mutation** (`mutate_body`) runs on the same
   `clone_fidelity` knob: **double** an existing tissue block into a
   free neighbour, **add** a new module from the habit palette, or
   **delete** a non-essential module. After each edit the body is
   repaired to stay **Moore-contiguous** from the Nucleus — a deleted
   trunk gap collapses (canopy lowers / roots raise) so kids never
   inherit floating tissue. `clone_fidelity = 1.0` is an identical
   chassis; default (~0.9) applies at least one morph edit so
   spore/sapling kids diverge on `Atom::growth_target` before they
   grow out. Habit never flips (plants keep Root+Photosystem; fungi
   keep Digest and never gain Root/Stem). Kind-swap (Stem→leaf, etc.)
   is deferred. Wired into rhizome sprouts, plant wind spores, fungal
   spores, and Atom fission.

## What is deliberately not here

- Species labels. Species emerge from clusters in trait space.
- Fitness functions. No global fitness — see
  [`docs/EVOLUTION.md`](../EVOLUTION.md).
- Meta-genes (mutation rate of the mutation rate). Later.
- Runtime-modifiable weights outside mutation. See
  [`NERVES.md`](NERVES.md) — no plastic learning.
- Cross-habit chimeras (plant↔fungus body swaps). Deferred.
