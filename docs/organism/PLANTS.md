# Plants

*Frozen land-plant, deep-root, and epiphyte references; the
`StemVsLeafVsRoot` allocation gene; stem integrity and the topple
pipeline. `land-plant` in the Organism Kernel plan.*

## Modules used

Only the palette entries from [`PALETTE.md`](PALETTE.md); nothing new
here. Plants are Atom-plus:

- `Nucleus` (black) — genome home. Default placement: root crown
  (see below).
- `Root` (sienna) — anchor + moisture drink.
- `Stem` (olive) — vertical stack, holds leaves into the light column.
- `Photosystem` (green) — leaves. Multiple 1×1 leaf pixels allowed.
- Optional `NeuralSoma` + `ChemoSensor` for shade-aware plants.
- Optional `Holdfast` (pink) on the epiphyte, layered onto a host's
  olive `Stem`.

Land plants are not a new kingdom mechanically. They are the atom
plus `Root` and `Stem` with fixed attachment instead of buoyancy.

## Reference creatures

The old design doc defines three visual targets. They stay canonical.

### C — Minimal land plant

MS Paint side view (ground at bottom, one leaf at top):

```
        ■ ■ green
        ■ olive
        ■ olive
        ■ black
        ■ sienna
   ~~~~ soil ~~~~
```

Two greens on top, two olive segments, nucleus at root crown, one
sienna anchor. Uses one column of the world. Reproduces by dropping
a messy clone near the parent (low `CloneFidelity` at first).

### D — Deep-rooted tree

Same modules, different allocation. Surplus energy goes into long
sienna chains toward the moisture gradient / water table:

```
        ■ ■ green
        ■ olive
        ■ olive
        ■ olive
        ■ black
        ■ sienna
        ■ sienna
        ■ sienna     <- seeks wetter cells
   ~ moist / water table ~
```

Tall olive + deep sienna is the expensive "tree" habit. Wins light
and drought; loses early races to short fast plants.

### E — Epiphyte on a host

Uses the host's olive as a skeleton. No own stem, no own root:

```
   host:    ■ green
            ■ olive <- ■ green    (epiphyte leaf)
            ■ olive <- ■ black    (epiphyte nucleus)
            ■ olive <- ■ pink     (holdfast on host olive)
            ■ sienna (host root)
```

Drinks from a thin surface film / rain on the host stem (see stem
wetness in [`FIELDS.md`](FIELDS.md)). Lives high enough to see light
that understory plants miss. Dies if the host stem segment is lost
or the host dies.

## Allocation gene

`StemVsLeafVsRoot` is three weights that sum to 1. Surplus energy is
spent according to the split:

- **Stem** — extend the olive stack upward by one pixel.
- **Leaf** — add a green pixel adjacent to the top of the stack.
- **Root** — extend the sienna chain into the neighbour cell with
  the highest moisture (usually down).

Wrong allocation is visible: tall bare poles, fat shaded bushes,
rooted stubs with no leaves. The player and the debug reader can
diagnose a species by looking at it.

## Nucleus placement

Working default: **root crown** (the pixel where sienna meets olive).
Rationale:

- Physically plausible: seeds' meristem sits there in most simple
  models.
- Mechanically stable: a topple (see below) that separates the crown
  from the leaves also kills the plant.
- Editor-friendly: the tool always drops a nucleus at the base when
  authoring a land plant template.

Mid-stem placement is allowed but not default; epiphytes carry their
nucleus wherever the pink holdfast anchors.

## Root elongation

Sienna extends into the neighbour cell with the highest `moisture`,
weighted by `RootDepthBias`:

```
target = argmax over neighbours n of:
    moisture(n) + RootDepthBias · (n.y_below_current ? 1 : 0)
```

- High bias → dives down into the wet band.
- Zero bias → sprawls shallow, wide.
- Substrate `{Rock, Loose, Organic, Void}` (see [`FUNGI.md`](FUNGI.md))
  gates penetrate cost. Loose fill is cheap, competent rock is
  expensive; **ghost roots** (prior root cavities) are effectively
  free.

Groundwater head field (already implemented in
[`crates/wk-field`](../../crates/wk-field)) provides the deeper
moisture gradient. Sienna reads `moisture` for the shallow layer and
`gw_head` for the deep, blending by depth.

## Stem integrity + topple

Every `Stem` pixel carries an `integrity: f32` in `0..1`. It is not
in the mass audit — it is a structural bookkeeping value.

| Event | Effect on `integrity` |
|-------|-----------------------|
| Living plant maintenance | Recharged toward 1.0 from the plant's energy budget. |
| Standing dead (nucleus absent or dead) | Slow abiotic decay: `-DEAD_DECAY_PER_TICK`. |
| Fungal digest in this cell | Fast decay: `-FUNGAL_DECAY_PER_TICK`. |
| Excess load above | Instant drop by a small factor per pixel of stem+epiphyte weight above. |

When any pixel's `integrity <= INTEGRITY_TOPPLE_THRESHOLD`, the stem
**topples at that pixel**. Working defaults:

- The stem stack above the failing pixel becomes a fallen-log
  band: pixels are re-projected horizontally onto the ground, one
  ground column per pixel, side chosen at random (L/R, seeded
  deterministically). Each fallen olive pixel becomes an `Organic`
  layer or `Ecology.dead_biomass` contribution — the exact bucket is
  fixed in [`FUNGI.md`](FUNGI.md).
- Attached epiphytes drop to the ground. On the ground they are
  usually in the wrong niche (too much shade for a canopy specialist,
  no root, wrong water). They usually die within a few ticks unless
  another host stem is adjacent (rare).
- The topple **opens the light column** where the trunk used to
  stand — canopy gap flash in the shade overlay. Nearby short plants
  and floor-level fungi get a burst.

No FEM, no wind throw, no Bezier fall animation. One integrity
number plus break-at-weakest is enough for the story.

## Shade-kill → rot → topple

Written as a graph in the design doc; kept mechanically minimal here:

1. Epiphyte or a taller neighbour shades the host's greens.
2. Host `Energy.current` drains faster than it harvests.
3. Host `Nucleus` despawns → all olive / sienna cells on the host
   are now standing dead.
4. Fungi (see [`FUNGI.md`](FUNGI.md)) invade the standing dead
   stems in place — cream hyphae grow into the olive pixels.
5. `integrity` collapses at whichever pixel the fungus reached first.
6. Topple fires.

That cascade is the fitness function for gentle vs smotherer
epiphytes — no kindness flag needed. See
[`SCENARIOS.md`](SCENARIOS.md) E42 and E43.

## Epiphyte-specific rules

- `Holdfast` (pink) may **share a Mid-lane cell** with an olive
  `Stem` — this is the attach exception in
  [`LANES.md`](LANES.md).
- Epiphyte `Photosystem` enters the same shade scan as host greens.
  If epiphyte greens sit above the host's greens and absorb hard
  enough, they starve the landlord. No mercy — that is the risk that
  makes freeloading interesting.
- `HostLeaveFraction` gene (see [`GENES.md`](GENES.md)) is one way
  gentle riders evolve: leave at least X of the light passing
  through your own stack for modules below. Smotherers are simply
  those with a low or zero `HostLeaveFraction`.

## Coupling to existing GVSE

- Shallow soil moisture reads / writes existing `column.moisture` and
  `moisture_cap`.
- Deep moisture reads the groundwater head field (already
  implemented) — same access pattern `run_ecology` uses today.
- Coarse column `Ecology.leaf_area` gets recomputed each tick from
  the pixel-count of live green on that column, so existing
  ET / infiltration / erosion feedback keeps working.
- Fallen log pixels contribute to `MassAudit::biomass_decay_total`
  via the same audit sink as ecological death today; no new bucket.
- Structural `integrity` is per-pixel scratch, `#[serde(default)]`
  for save-load.

## What is deliberately not here

- Branching morphogenesis (multiple stems from one nucleus). Later.
- Seasonal leaf drop (fake later by module loss when needed).
- Wood rings, thick bark structural mass. `Bark` slot reserved in
  [`PALETTE.md`](PALETTE.md); implementation waits.
- Compressive / shear tensors. One `integrity` scalar is enough.
- Directed wind throw. Topple side is random L/R (later cosmetic:
  toward heavier epiphyte side).
