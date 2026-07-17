# Stratigraphic world model (artistic geology)

*Design record, mid-2026. Companion to [`WORLDGEN.md`](WORLDGEN.md).
Not a 4-billion-year plate tectonics sim — a **painterly cross-section**
that reads as Earth-like strata while staying inside the column/layer
budget (`MAX_LAYERS = 8`).*

## Intent

The product question in the root README is whether a side-view landscape
can accumulate **readable geological history**. Generation should hand
the sim a world that already *looks* like history happened: beds that
pinch out, shelves that stack mud over sand, mountains that expose
older rock, coasts that hold a thin living skin over older stone.

We do **not** simulate continental drift, orogeny over deep time, or
glacial cycles. We **compose** a believable cutaway the way a museum
diorama does — rules of thumb from real facies, authored for beauty
and play, then let erosion / karst / ecology write the next chapter
at runtime.

## Constraint: eight layers is the canvas

Each column holds at most eight material layers. Generation must
produce a stack that:

1. Reads clearly in the x-ray strata view.
2. Leaves 1–2 free slots for runtime deposits (rain sediment, ash,
   litter-as-Organic later, landslide toes).
3. Prefer **few thick, named beds** over many paper-thin ones that
   immediately merge.

Target authored stack (bottom → top), typically **5–6 layers** at
generation, never more than 7:

| Slot | Role | Typical materials |
|------|------|-------------------|
| 0 (bottom) | Crystalline basement | `Bedrock` |
| 1 | Deep crustal / metamorphic body | `Stone` (thick) |
| 2 | Older sedimentary package | `Limestone` *or* `Stone`/`Clay` |
| 3 | Basin fill / molasse | `Sandstone`-stand-in: `Sand`+`Gravel` mix, or `Clay` |
| 4 | Surficial cover | `Sand` / `Gravel` / `LooseRock` / `Clay` by facies |
| 5 | Optional cap | thin `Clay` (soil), or leave free |
| 6–7 | Reserved | runtime Water / Ice / Snow / Organic / spoil |

Water, ice, and snow still occupy layer slots when present — ocean
columns often use one Water slot on top of 4–5 solids.

## Facies belts (horizontal story)

Think of the ring (see topology in `WORLDGEN.md`) as one trip around
a small planet's equator cut open. Facies belts are **arcs** along
world-x, blended with smoothsteps so materials don't cliff:

```
… abyss → slope → shelf → lagoon/marsh → coastal plain →
  river lowland → upland → foothills → high range → rain shadow →
  interior basin → (seamless back into abyss or opposing coast) …
```

Each belt owns a **recipe**: surface height bias + preferred stack +
wetness + ecology seed. Belts are placed by a low-frequency
"province" signal (noise or authored ring spline), not by absolute
metre cutoffs like today's `continental_surface_y`.

### Belt catalogue (v1)

| Belt | Surface feel | Stack emphasis | Play payoff |
|------|--------------|----------------|-------------|
| Abyssal plain | Flat, deep | thin pelagic Clay over Stone/Bedrock | dark water, little life |
| Continental slope | Steep face | Gravel/LooseRock talus over Stone | turbidity "streaks", unstable |
| Shelf | Broad shallow | Sand over Clay/Limestone | reefs later; good Atom water |
| Lagoon / marsh | Near sea, low | Clay + organic-rich top, high table | wetlands, methane-later |
| Coastal plain | Low dunes/beach | thick Sand, thin soil | erosion demo, dunes |
| Alluvial / river | Broad valley | Sand/Gravel lenses, Clay floodplain | rivers, deltas |
| Upland plateau | Rolling | Stone near surface, thin soil | karst candidate if Limestone |
| Foothills | Rising | LooseRock + Gravel colluvium | slumps, fans |
| High range | Peaks + cirques | Stone/Bedrock exposed, thin scree | snowline, headwaters |
| Rain shadow | Dry lee | Sand/LooseRock, deep table | arid ecology contrast |
| Interior basin | Closed low | Clay playa ± salt-later, episodic water | endorheic lakes |

A single ring need not contain every belt. Seed picks a **palette** of
4–7 belts and arranges them so opposites (ocean ↔ range, wet ↔ dry)
face each other across the circle — readable after one lap.

## Vertical story without deep time

Instead of ages-in-Ma, each column gets a small **story vector**
derived from seed + belt:

1. **Basement relief** — long-wavelength Bedrock/Stone thickness.
2. **Platform or basin** — Limestone shelf vs Clay-filled trough
   (mutually exclusive in a column; neighbouring columns can grade).
3. **Cover** — facies-appropriate loose materials.
4. **Unconformity cue** — optional: skip the middle package and put
   thin cover directly on Stone ("truncated" look) in uplift belts.
5. **Surface trim** — ripple, terrace steps, cliff notches at coast
   (metres, not kilometres).

Unconformities are **missing layers**, not simulated erosion events.
A foothill column might be `Bedrock | Stone | Gravel | Sand`; a shelf
column `Bedrock | Stone | Limestone | Sand | Water`.

### Lateral continuity

Beds should **persist across many columns** then pinch:

- Pick contact elevations (or thicknesses) from low-frequency noise.
- Neighbouring columns share the same package IDs; only thickness
  varies smoothly.
- Pinch-out: thickness → 0, layer omitted (frees a slot).

This is what makes the x-ray view look like a real cliff face instead
of TV static.

## Materials we lean on (and gaps)

**Use now:** Bedrock, Stone, Limestone, Sand, Gravel, LooseRock, Clay,
Water, Ice, Snow.

**Artistic stand-ins (no new MaterialId required for v1):**

- "Sandstone" → thick compacted Sand (or Sand over Gravel).
- "Shale" → Clay.
- "Till" → LooseRock + Gravel mix in formerly glaciated belts (optional
  belt flag).
- "Soil" → thin Clay or future Organic layer.

**Defer new IDs** until a belt can't read without them (chalk, salt,
basalt). Prefer recipes over vocabulary explosion.

## Karst, aquifers, and ecology hooks

Generation should mark **intent**, then let subsystems act:

- Limestone packages in humid upland/plateau → high `run_karst`
  potential (already flux-driven; gen only places the rock).
- Clay aquitards under Sand → perched moisture after
  `seed_column_water_table` (today's table seed is a start; belt
  wetness from `WORLDGEN.md` modulates depth).
- Marsh/lagoon belts → high initial alive biomass / nutrient
  (ecology seed), shallow table, standing Water in lows.
- High range → snow-capable climate elevation; thin ecology.

## Ring-aware placement

On a wraparound world, facies must be **periodic**:

- Province/belt signal uses `theta = TAU * world_x / world_width_cols`
  (or chunk index mod `WORLD_CHUNKS`).
- Thickness noise is periodic (same value at x and x+width).
- Seam columns (last/first chunk) share neighbour recipes so Stone
  beds and shorelines meet without a material cliff.

Infinite mode (optional later) uses the same recipes keyed by
non-periodic noise; belts become irregular provinces along the line.

## Generator pipeline (one column)

```
1. province = belt_at(seed, x, topology)
2. surface  = height_for(province, seed, x)     // WORLDGEN elevation
3. wetness  = wetness_for(province, seed, x)
4. story    = story_vector(province, seed, x)   // which packages exist
5. stack    = thicknesses from story (sum to surface - basement)
6. deposit  Bedrock → … → cover  (fill_bathymetry-style, ordered)
7. sea fill + water table + ecology seed
8. leave ≥1 layer slot free when possible
```

Chunk generation stays a **pure function of (seed, coord, WorldGenParams)**
so streaming and ring regeneration both work.

## What we explicitly will not do in v1

- Plate motions, subduction, Wilson cycles.
- True stratigraphic ages or fossil indices.
- Per-layer pore moisture (still column scalar).
- Raising `MAX_LAYERS` (revisit only if x-ray readability demands it).
- Procedural mineral veins as separate materials (markers or dissolved
  field later).

## Falsification / scenarios (when implemented)

| ID | Claim |
|----|--------|
| WG-S1 | A shelf→coast transect shows Limestone or Clay under Sand over ≥32 consecutive columns (continuity). |
| WG-S2 | High-range columns expose Stone/Bedrock within 3 m of surface more often than coastal plain columns. |
| WG-S3 | Ring seam: material id sequence at chunk 0 local 0 matches neighbour continuity metrics vs chunk N−1 (no single-column material spike). |
| WG-S4 | Generated stacks use ≤7 layers before sea fill; ≥80% of land columns have a free slot. |
| WG-S5 | Two seeds produce recognisably different belt palettes (not only height jitter). |

## Relationship to current code

Today `sediment_composition` + `fill_bathymetry_column` already blend
materials by elevation — a single global ramp, not named belts or
pinching packages. This doc replaces that ramp with **belt recipes +
story vectors**, and keeps the deposit order / water-table seeding
ideas that already work.

Implementation should land behind a `WorldGenParams.profile` enum:

- `LegacyContinental` — current fixed transect (demos + scenarios).
- `RingFacies` — this model on a wraparound world.
- `InfiniteNoise` — `WORLDGEN.md` noise elevation + same facies recipes.
