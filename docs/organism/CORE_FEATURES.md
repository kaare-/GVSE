# Core features — Sets A–E

*Frozen order of feature sets and the explicit non-goals for the
kernel arc. `core-features` in the Organism Kernel plan.*

Each Set adds a small vocabulary. The simplest creature (Set A Atom)
stays two pixels through every later addition. Add colours and rules
only when a scenario justifies them.

## Set A — Alive in light

Modules: `Nucleus`, `Photosystem`.

Rules:

- Energy budget, upkeep, harvest from the light field.
- Day / night (or light on / off) drive the harvest curve.
- Nucleus emits life stats (energy, health, clock phase).
- Death dumps organic in place (contributes to
  `MassAudit::biomass_decay_total`).

Genes in play (from [`GENES.md`](GENES.md)):

- `MetabolicRate`, `ReproduceAt`, `CloneFidelity`, `CircadianPhase`,
  `ActiveWindow`.

Visual pass:

- Two 1×1 squares. Green dots fill the lit band by day and thin at
  night. Bloom / crash is readable at 1× zoom.

Scenarios: E30 Atom bloom, E31 messy boom (from
[`SCENARIOS.md`](SCENARIOS.md)).

## Set B — Chemical talk + nerves

Adds: `ChemoSensor`, `ChemoEmitter`, `NeuralSoma`, `Axon`.

Rules:

- Per-chunk `ChemField` with 4 channels (see [`CHEM.md`](CHEM.md)).
- Sensor reads local or gradient; emitter buffers add-into-cell
  writes.
- Fixed soma graph (see [`NERVES.md`](NERVES.md)); gene weights,
  no learning.
- Emitter cost is energy.

Genes: sensor / emitter `tuned_type`, `gain`, `threshold`; soma
`bias`, `activation_shape`; per-axon `sign`, `weight`.

Visual pass:

- Creature "talks" into the water. Neighbours' sensors light up. A
  proto-quorum forms when many wells emit into the same channel.

Scenarios: E32 scent flag, E35 dialect split.

## Set C — Vertical niche + climate

Adds: `Buoyancy`, `TempTolerance`, `Chemosystem`.

Rules:

- Buoyancy pumps a target depth (energy cost).
- Temperature tolerance couples to the existing `ThermalField`.
- `Chemosystem` is a `Photosystem` alternate — consumes a `ChemType`
  for energy instead of light. Fits alongside sensors / emitters
  without breaking Set A.

This Set is deliberately *thin* — GVSE is column-first and mostly
land. Water-column life is a niche the world supports rather than a
star of the show.

Genes: `BuoyancyBias`, `TempOptimum`, `TempWidth`.

Scenarios: E33 day-float / night-sink, E34 heat edge.

## Set D — Land plants + shade

Adds: `Root`, `Stem`, and the pixel-shade rule.

Rules:

- Column top-down attenuation (see [`LIGHT.md`](LIGHT.md)).
- `StemVsLeafVsRoot` allocation surplus (see [`PLANTS.md`](PLANTS.md)).
- Root elongation tropism into `moisture` and groundwater head.
- Plants tagged **Mid**-lane (see [`LANES.md`](LANES.md)).
- Optional soma reads local remaining light and biases stem vs leaf
  spend.

Set D promotes the coarse per-column `Ecology` bucket already
present in GVSE to the module-pixel world without deleting the
existing feedbacks (leaf ET, root infiltration, erosion resistance).

Genes: `StemVsLeafVsRoot`, `LeafAbsorb`, `ShadeEfficiency`,
`RootDepthBias`, plus reused Set A/B genes.

Scenarios: E36 canopy race, E37 understory persists, E38 root
drought.

## Set E — Litter fungi + epiphytes + toppling

Adds: `Digest`, `Hypha`, `Holdfast`, plus stem `integrity`, the
topple event, the substrate enum, and the ghost-root lifecycle.

Rules:

- `organic(x,y)` field surfaces surplus from plant deaths (see
  [`FUNGI.md`](FUNGI.md)).
- Cream hyphae extend through litter *and* standing-dead stems and
  roots.
- Fungal boom / crash after plant die-offs.
- Pink holdfast on host stem; epiphytes may fully shade-kill their
  landlord (no mercy rule from [`PLANTS.md`](PLANTS.md)).
- Stem `integrity` on every olive; fungal digest accelerates rot;
  topple when a pixel fails.
- Dead roots → organic → fungal cavity → loose fill → preferential
  re-root path (`PreferentialRootPath`).

Genes: `DigestRate`, `AttachPrefer`, `HostLeaveFraction`.

Scenarios: E39 litter bloom, E40 epiphyte free-ride, E41 allocation
trap, E42 smother → rot → topple, E43 gentle wins long game, E44
ghost roots.

## Order lock

The order is fixed: A → B → C → D → E. The dependency graph in
[`README.md`](README.md) enforces:

- B needs A (soma reads life stats + production).
- C needs A (harvest niche gates buoyancy target).
- D needs the shade rule which is authored on top of A photosystem.
- E needs D (dead plants supply organic surplus). Fungi should not
  be invented before organic surplus exists on screen.

## Explicitly not in Core

The kernel arc **does not** include:

- Horizontal swimming, hunting, or full animal locomotion.
  (Lane passability from [`LANES.md`](LANES.md) is a locked
  design constraint for when they arrive in Phase 7.)
- Learned neural nets (plastic training loops) **in open-world
  ecology**. Weights are genes there. The
  [`STUDIO.md`](STUDIO.md) arena **does** train nets + run GA, then
  exports frozen weights into the world.
- Active predation / chasing. `Digest` of *living* tissue can wait.
- True mycorrhizae (mutualist hypha↔root sugar/water exchange).
  Plausible follow-on, slot reserved.
- Real compressive / shear tensors, wind throw, or Bezier falling
  animation. One `integrity` scalar + collapse event is enough.
- True wood rings, branching morphogenesis, seasonal leaf drop.
  Fake with module loss later.
- Bones, muscle, multi-cell tissues as **world ecology animation**
  without the studio track. Slots reserved; studio owns the first
  implementation (shared physics — [`STUDIO.md`](STUDIO.md)).
- Freeform drawing (MUD). Grid pixels only.
- Full GVSE geology coupling beyond `MaterialId::Organic` +
  substrate tag. Karst, burrows, karst-integrated roof rules already
  live in [`docs/BURROWS.md`](../BURROWS.md) and Fungi.md picks the
  minimal handshake there.

Any of these can graduate later; adding to Core requires updating
this file and [`README.md`](README.md) so the freeze is honest.

## Cross-cutting invariants (repeat for emphasis)

Even in a doc-only phase, these get named up front so Phase 2+ code
never drifts:

1. Deterministic content-addressed generation.
2. Mass audit invariant: any new sink or source gets its own bucket.
3. Buffered writes + barrier commit **or** direct-mutation post-barrier;
   never both in one pass.
4. Save / load round-trip preserved by `#[serde(default)]`.

See [`../PLAN.md`](../PLAN.md) if the roadmap lives there; otherwise
the four rules are restated in [`README.md`](README.md).
