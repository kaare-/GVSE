# Underground: karst caves, burrows, cave ecology

*Author: initial design proposal, mid-2026. Nothing in this document is
implemented yet; this is the design record for a planned extension.*

## Motivation

Real ecosystems have distinct underground niches — cave systems, root
zones, burrows, aquifers. A side-scroller sim gains a lot from these:

- **Passing lanes.** In a 1D world, two creatures at the same
  elevation collide. Voids at multiple depths give creatures
  non-conflicting horizontal routes.
- **Distinct selection pressures.** Cave environments select for
  different traits (low light, stable temperature, chemoautotrophic
  food chains). That's exactly the pressure diversity that makes an
  evolution simulation actually diverge.
- **Niche construction.** Burrowing species modify the substrate they
  live in, changing infiltration, cover, and predator access for
  every other species. In real ecology this is a first-order driver.
- **Emergent geomorphology.** Karst caves, sinkholes, and cave rivers
  arise from a single soluble-material rule and produce landscape
  features (dolines, poljes, cave-fed springs) that are hard to hand-
  author but fall out of a simulation for free.

## Architectural problem

The current column model can't represent a cave directly:

- `Column.layers` is a stack of contiguous masses stacked bottom-up.
  Elevation is a summed prefix over that stack. There is no way to
  say "at height y=8..12 m in this column there is nothing."
- `settle_by_density()` sorts layers ascending by density every clamp.
  If we put `Air` as a middle layer, it immediately floats to the top
  of the stack — the cave vanishes as a side effect of the density
  invariant.
- `MAX_LAYERS = 8`. Two caves per column would use four slots on
  geometry alone, before any real stratigraphy.
- `merge_layers` actively collapses same-material adjacency. Two rock
  layers separated by a thin air gap tend to want to merge.

So the answer is **not** "add Air as a layer material." The layer stack
should keep representing mass distribution, and voids should live on a
parallel data path.

## Proposed data model: Void as a sparse column annotation

Add one field to `Column`:

```rust
pub voids: SmallVec<[Void; 4]>,
```

with

```rust
pub struct Void {
    pub top_y: f32,             // absolute elevation of ceiling
    pub height_m: f32,          // ceiling − floor
    pub water_mass: i64,        // kg pooled in the void
    pub roof_material: MaterialId,  // material of the layer above
    pub origin: VoidOrigin,     // Karst | Burrow | Collapse
    pub light: u8,              // 0..255, connectivity to surface
}
```

Rationale:

- **Layer stack is untouched.** Density settling, merge, clamp, mass
  audit all still operate on `layers` unchanged. Layers describe
  *what mass this column contains*.
- **Voids describe where that mass isn't.** A column with 30 m of rock
  plus a 4 m void at 12–16 m still has 30 m of rock in kg terms; the
  column's *top* is at 34 m absolute. A one-line adjustment in
  `recompute_surface_y` (add `voids.iter().map(|v| v.height_m).sum()`)
  keeps geometry consistent.
- **Sparse.** Most columns have zero voids. `SmallVec<[Void; 4]>` is
  inline for the small case and grows to the heap only for
  pathological caves. Memory delta is a rounding error at shipped
  map size.
- **Save/load costs zero.** `#[serde(default)]` on the new field;
  old saves round-trip with empty void arrays.

The mass audit gains one bucket, `dissolved_out_total`, symmetric to
`evap_out_total` and `boundary_out_total`. If we implement speleothems
(section 8), we also gain a `dissolved_return_total`.

## Karst physics: one new material, one new subsystem

Add two fields to `MaterialProps`:

- `solubility: u8` — 0 for everything currently in the table.
  `Limestone` gets ~40. This is the field that makes karst a first-
  class physical phenomenon.
- `roof_span_max_m: f32` — how wide a horizontal void this material
  can roof over before collapse. Bedrock/Stone ~15 m, Limestone ~10 m,
  Sand/Clay ~0 (immediately collapses into voids underneath),
  `f32::INFINITY` for the immovable substrate.

Add `MaterialId::Limestone`:

- density 2500
- permeability 140 (much higher than stone — the physical reason
  karst happens; water flows into it, not just past it)
- erosion resistance 150 (surface erosion doesn't strip it easily —
  real limestone forms cliffs)
- cohesion 180 (holds a cave ceiling up while thick)
- solubility 40
- roof_span_max_m 10

Then a new post-barrier direct-mutation subsystem `run_karst`:

```
for each column c
  for each layer l in c with solubility > 0
    flux_through_l = lateral water flux at l's elevation
    dissolved = flux_through_l * l.solubility * KARST_COEFF
    remove `dissolved` kg from l
    audit.dissolved_out_total += dissolved
    if dissolution accumulates enough
      spawn or grow a Void centred at l's mid-elevation
```

Two design choices worth highlighting:

**Dissolution is driven by lateral flux through the layer, not by
moisture-in-place.** This is the single most important choice for the
"multi-lane cave" property. Real karst caves form along the water
table because that's where water moves horizontally. Driving
dissolution by pore-water saturation instead would produce roughly
uniform limestone-eating everywhere, and no coherent horizontal
passages.

`run_groundwater_flow` already computes lateral head-gradient transfer
between neighbours' water tables. `run_karst` can reuse the same
gradient calculation restricted to soluble layers.

**Dissolution is slow.** `KARST_COEFF` should be small enough that a
metre of limestone takes tens of thousands of ticks to dissolve. That
leaves the tick budget for the fast subsystems and lets cave features
develop over the same visible time scale as the sediment layers today.

## Why the "multiple free lanes" property emerges naturally

Consider a limestone bed 4 m thick sitting under a sandstone caprock,
with a slight regional slope. Rain infiltrates the sand, hits the
limestone, spreads laterally along the caprock / limestone interface.
That's a horizontal water flux at a consistent elevation across many
columns. `run_karst` sees that flux, dissolves limestone along it,
opens a passage.

A few thousand ticks later: a horizontal cave stretches across dozens
of columns at approximately the water-table elevation. A creature at
column X can walk to column X+40 through the passage without changing
depth. **That's a passing lane.**

A second, deeper permeable bed produces a second lane. Vertical
connectivity emerges from:

- **Water-table drop** (drought): a cave that used to be full of water
  is now air. Water still infiltrates from above but drops down through
  the dry limestone as vertical seeps — new dissolution vector, vertical
  shafts form.
- **Roof collapse** (section 5): a doline / sinkhole opens a vertical
  connection between surface and cave, or between two cave levels.

## Cave water and interaction with surface hydrology

Voids need their own water because a cave river has different geometry
from a surface flow — confined passage vs. free surface. But it's the
same physics: head-gradient diffusion between adjacent voids at similar
elevation.

Two integrations with the current subsystems:

- **Surface water pours into voids** when a void breaches the surface.
  In `run_surface_water`, before the horizontal-flow calculation, check
  if the column has a void whose ceiling is at or above
  `surface_y − epsilon`. If yes, drain some fraction of the top-of-stack
  water into the void instead of flowing sideways. This is how
  sinkholes swallow rivers in reality.
- **Void water evaporates back up if the void connects to surface**,
  at a rate scaled by `light`. A deep sealed cave has ~100% humidity
  forever; a wide-mouthed cave dries out during a drought like anything
  else.

Groundwater and void water are related but distinct: groundwater is
pore water in soluble/porous layers, void water is free water inside a
cavity. They can exchange (the wall of a void is a permeable boundary
if the wall material has permeability > 0), but that's a small
extension of `run_infiltration`, not a new subsystem.

## Roof collapse (unified with slumping)

The current `run_slumping` subsystem handles a specific case of
unsupported mass — a slope steeper than a material's angle of repose.
Cave roof collapse is another case of unsupported mass — a horizontal
span wider than the material's `roof_span_max_m`.

Both share one predicate: **is this mass unsupported?** The response
differs (slump sideways vs. collapse downward), but the predicate is
one function. Unify them:

- If unsupport comes from a slope, apply the current slump transfer.
- If unsupport comes from a void beneath and the void is too wide,
  drop the roof into the void: convert some mass of the roof layer to
  `LooseRock` and deposit it at the void floor; shrink or eliminate
  the void; drop the layer above (and everything on top of it) by the
  removed height.

Collapse breaching the surface produces a **sinkhole**: the surface
subsides into the void, surface water starts pouring in, and rapid
further modification follows. That's an emergent feature, not a
hand-authored one.

## Speleothems: closing the mass loop

The dissolved carbonate mass has to go somewhere for the audit to
close. In reality it precipitates back out where water evaporates
(ceiling drips → stalactites, floor drops → stalagmites, wall seeps →
flowstone). A very cheap `run_speleogenesis`:

```
for each column with voids
  for each void
    if evaporation happens this tick
      convert `dissolved_in_water * SPELEO_FRAC` kg to Limestone
      deposit as a small Limestone layer inside the void,
        reducing void height
      audit.dissolved_return_total += converted
```

Turns karst from "material vanishes" into "material is redistributed
within the world." Audit invariant is preserved with the paired
`dissolved_out_total` / `dissolved_return_total` buckets, and you get
stalactites growing over time inside the caves the simulation carved.
Small code, significant "the world feels alive" payoff.

## Burrows: creatures as niche constructors

Burrows are structurally the same as karst voids — a `Void` on a
column, with `origin: Burrow` instead of `Karst`. A creature dig
action is one API call:

```rust
world.dig(column_x, target_y, volume_kg) -> DigResult
```

Rules:

- The dig removes mass from the layer at `target_y` and either extends
  an existing void at that elevation or creates a new one.
- Removed mass becomes "tailings" dumped on the surface of the source
  column (or an adjacent one), as a small deposit of
  `LooseRock`/`Sand`/`Clay` — whatever material was dug. This is
  physically correct: moles push dirt out of tunnels and it
  accumulates as mounds.
- Digging can't create a void wider than the roof material's
  `roof_span_max_m`. Attempting it breaks the roof immediately; the
  burrow becomes a trench.
- Two burrows at similar elevations in adjacent columns are treated as
  connected (same rule as karst passages). A chain of dig actions
  produces a tunnel.

The evo-sim payoff: a species that digs shallow burrows changes the
local ecology for every other species. That's the co-evolution pressure
that makes an evolution simulation diverge rather than settle.

## Cave ecology

Cave environments have three ecological properties that surface
environments don't:

- **Stable temperature.** Long-term surface mean, no diurnal swing.
  A slow moving-average of surface temperature at that column, weighted
  by depth. Selects for creatures that don't tolerate temperature
  variation.
- **High humidity, low evaporation.** Already falls out of the geometry
  (evaporation scaled by void `light`).
- **Zero light in the deep zone, gradient at the entrance.**
  `Void.light` computed as a decay-with-distance from any voids that
  touch surface. Zero-light regions can't support photosynthetic plants
  but can support chemoautotrophs (feeding on dissolved minerals — you
  already have that mass in circulation) and detritivores feeding on
  organic wash-in.

Combined with the planned per-column ecology bucket (root_density,
leaf_area, dead_biomass, nutrient), you get distinct habitat types
the selection rule can operate on:

- **Cave-adapted producers**: fungi, moss at entrances,
  chemoautotrophic mats deep inside. Grow on dead biomass and dissolved
  minerals rather than light.
- **Cave-adapted consumers**: eat producers, don't need eyes or pigment.
  Select for reduced sensory cost, cold tolerance, low metabolic rate.
- **Sinkhole opportunists**: surface species hunting near cave mouths.
- **Amphibious cave species**: follow cave rivers between surface waters.

Once these niches exist, the evolution simulation will select for
divergent trait profiles across them, which is the entire point.

## Interaction with existing subsystems (breakage check)

- **Density settling**: untouched. Voids aren't layers.
- **Layer merge**: untouched. Layer merge operates within the stack;
  voids are outside it.
- **Slumping**: extended to share its unsupported-mass predicate with
  roof collapse.
- **Surface water flow**: adds a "drain into open void" branch on the
  column. Two lines.
- **Groundwater**: unchanged; feeds soluble layers with the flux
  `run_karst` reads.
- **Infiltration**: unchanged.
- **Evaporation**: adds a "evaporate from void surface" branch gated on
  `light` (connectivity).
- **Phase change**: unchanged. Water inside a cave freezes as expected;
  the ice becomes an `Ice` deposit in the void.
- **Rain injection**: unchanged; falls on `surface_y`.
- **Weather**: unchanged.
- **Mass audit**: gains `dissolved_out_total` and
  `dissolved_return_total`. Both mirror the existing rain / evap
  bookkeeping.
- **Save/load**: adds `voids: Vec<VoidSnapshot>` per column with
  `#[serde(default)]`. Old saves round-trip unchanged.
- **Renderer**: `draw_terrain_column` needs to paint sky/dark inside
  void y-ranges. If a void has water, paint the water. Ten lines.

Nothing here demolishes an existing invariant.

## Performance envelope

Per-column overhead: `SmallVec<[Void; 4]>` inline is 4 × ~30 B ≈ 120 B
plus the discriminator. Doubles per-column footprint in the worst case,
unchanged when zero voids.

Per-tick cost:

- `run_karst`: only touches soluble layers. A chunk-level
  `has_soluble_layer` bit makes it near-zero cost for stone-only chunks.
- `run_void_water_flow`: proportional to total voids in the world, not
  to columns. Cave systems are sparse.
- `run_roof_collapse`: rare event, checked per tick but almost always
  no-op. Cheap.
- `run_speleogenesis`: same shape and cost profile as `run_karst`.

At shipped map size, if 10% of columns end up with 1–2 voids after
100k ticks of karst dissolution, that's ~1k active voids — negligible
compared to the surface-water sweep. **Karst adds no meaningful cost
until the world has actually built caves**, and once it has, the
incremental cost is well below what surface water already spends.

## Suggested implementation stages

Each stage lands as its own PR; each leaves the sim in a working state.

1. **Data model + Limestone.** Add `MaterialId::Limestone`, its
   property row, `solubility` + `roof_span_max_m` on `MaterialProps`,
   `voids` on `Column`, renderer cutout. Terrain gen places a limestone
   band in appropriate elevations. Nothing dissolves yet, but the model
   round-trips through save/load. Biggest structural change; better
   alone.

2. **`run_karst` + dissolution audit.** Introduce dissolution driven by
   lateral flux through soluble layers, `dissolved_out_total` audit
   bucket, void spawning. New scenario: `E9_karst_forms_horizontal_
   passage`. Verifies caves grow with mass conserved.

3. **`run_void_water_flow` + surface capture.** Cave rivers, sinkholes
   swallowing surface flow. New scenario: `E10_sinkhole_captures_river`.

4. **Roof collapse unified with slumping.** Single unsupported-mass
   predicate, two response handlers. New scenario:
   `E11_cave_roof_collapses`.

5. **Speleogenesis.** Close the dissolved-mass loop with
   reprecipitation. New scenario: `E12_stalactite_forms`.

6. **Burrow API + tailings.** Low-level `world.dig()`; no creatures yet.
   Testable via synthetic "dig this column at y=10 by 500 kg" calls.
   New scenario: `E13_burrow_produces_surface_tailings`.

7. **Cave ecology hooks.** (Requires the planned per-column `Ecology`
   bucket.) Void `light`, stable temperature, humidity properties feed
   species-selection rules.

## Trade-off explicitly accepted

Voids are rectangular per column. Transitions between adjacent voids'
elevations are steps, not curves. Rendered at the current 4 px column
width this looks fine — the surface already has the same step
resolution. If we later want scalloped cave walls or squeeze passages,
either a per-void wall-roughness scalar (cosmetic only) or sub-column
detail would be needed. Accepting the stepped look for now; revisit
only if visual demands push it. The gameplay properties (passing
lanes, depth levels, sinkhole capture, ecological niches) are
orthogonal to whether the outline is stepped or curved.
