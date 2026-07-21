# Voxel migration — design notes for `wk-voxel`

Working design document for a greenfield 2D cellular-automata sim
living in `crates/wk-voxel/` alongside column-based GVSE. This
document is the **intent map** the plan referred to: it transcribes
what each subsystem in the current column stack is trying to model,
in voxel-cellular terms, so we know what to build when we get to it.

Nothing in this document is a promise to *ship* every feature. The
priority order is: get the fluid core working, then move outward.

## 1. Purpose and context

Column-based GVSE keeps producing water bugs. Each patch of the water
system exposes another (see the thread of screenshots and the
"honest evaluation" in the water-prune PR). The failure pattern is
architectural, not a series of unrelated bugs: column geometry can't
express caves or free fluid volumes without proliferating buckets
(top water layer, `moisture` scalar, `void.water_mass`, sediment
carrier, dissolved field, humidity field, cloud mass), and every
subsystem picks a different reference elevation.

The user's proposed model — 2D block map for everything, with
overlaid scalar heatmaps — is a much better fit for a 2D side-view
sim. `wk-voxel` is the greenfield attempt.

We do this **in parallel** with the existing column stack. Both live
in the same workspace; both build; neither imports the other. When
the voxel sim is far enough along to run the app, we swap `wk-app`
over. Until then, column GVSE keeps running.

## 2. Isolation Guardrails

This is the *contract* between the two sims. Both are enforced by
structure (Cargo.toml + guardrail comments) rather than trusted to
intent.

### Structural rules

- `crates/wk-voxel/Cargo.toml` depends on **exactly one** existing
  crate: `wk-material`. Material IDs and property tables are pure
  data and safe to share. No other GVSE crate is a dependency.
- No file under `crates/wk-app/`, `crates/wk-world/`,
  `crates/wk-agents/`, `crates/wk-sim/`, `crates/wk-io/`, or
  `crates/wk-field/` gains a `wk-voxel` dependency.
- Every `.rs` file in `crates/wk-voxel/src/` begins with the block:

  ```rust
  //! wk-voxel is an isolated greenfield sim. It MUST NOT import from
  //! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
  //! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
  ```

- Every column-crate `lib.rs` (`wk-world`, `wk-sim`, `wk-agents`,
  `wk-io`, `wk-field`) and `wk-app`'s `main.rs` carries the reverse
  guardrail:

  ```
  //! Column-based GVSE. MUST NOT import from wk-voxel. See
  //! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
  ```

- `wk-voxel` has its own test folder `crates/wk-voxel/tests/`.
  It never touches `tests/scenarios/`.

### Behavioural rules

- No shared runtime state. Two sims running in one process would
  need to serialise/deserialise through a well-defined format;
  today they don't communicate at all.
- Reused **inputs** are allowed: material IDs (`MaterialId`),
  property tables, biome enums, colour palette values. Reused
  **runtime state** is not.
- Config files and JSON assets may be duplicated if their shape
  differs. Prefer duplication over shared config where the two sims
  would need different fields.

### Why the guardrails

The plan describes this as "we don't intermix or an agent
misunderstands and starts mixing things up." Concrete failure modes
these rules prevent:

- A subsequent agent imports `Column::flowable_water` into a voxel
  rule and now the two representations have to stay in sync.
- Someone extends `MassAudit` (a wk-world type) with voxel-side
  counters. The audit becomes bimodal and every reader has to know
  which mode.
- `wk-app`'s renderer gains an optional voxel path. State ownership
  becomes ambiguous.

The guardrails make each of these show up as a Cargo dependency
change during review.

## 3. Intent map — column GVSE → voxel model

One subsection per existing design doc under `docs/`. Each row calls
out what the intent is, and how it lands in voxel terms. Items marked
"deferred" are not in the initial voxel work; they'd come later.

### 3.1 [WORLDGEN.md](WORLDGEN.md)

Ring topology, macro biome belts, bathymetry from abyss to peaks,
stratigraphy per column, karst / ecology / creature layers stacked on
top.

Voxel intent:

- **Ring topology.** `World::wrap_width` maps every world-x into
  `[0, width)` so physics and the demo camera join left↔right.
  Humidity sets `wrap_x` to match. The stamped profile puts deep
  ocean on both edges so the seam is seamless.
- **Continental profile.** Ring-fraction `continental_surface_y`:
  thick bedrock floor barrier, stratified stone / limestone / clay /
  gravel / loose-rock body, sand cap, water above submerged beds,
  extra sky headroom.
- **Chunk generation.** Analogous to `generate_chunk_continental` in
  `crates/wk-world/src/terrain.rs`. One cellular chunk = 64×64
  cells. Generation is deterministic on `(seed, cx, cy)`.
- **Deferred.** Streaming chunk load/unload — start with the whole
  ring in memory (the column build already fits at CHUNK_W=64).

### 3.2 [STRATA.md](STRATA.md)

Column layers carry `age_start` / `age_end`. Merges are pruned by
`MERGE_GAP` / `MERGE_MAX_THICKNESS`. Provides a first-class geologic
epoch view.

Voxel intent:

- Per-cell material is a small enum — full per-cell age tags would
  blow up memory.
- Instead: **stratum-id grid** at coarse resolution. A separate
  `Heatmap<u32>` (already scaffolded) sampled every 4–8 cells stores
  a stratum id; a small side table maps id → (age_start, age_end).
  Erosion / karst clears the id when a cell is dug out.
- Loses per-cell resolution on strata boundaries. Acceptable — real
  strata are already thick, and the geologist inspector doesn't need
  cell-level resolution.

### 3.3 [BURROWS.md](BURROWS.md)

Digging APIs, tailings on the surface, burrow origin tagging.

Voxel intent:

- Digging is just `world.set_cell(x, y, Cell::air())`. No sparse
  annotations, no `Void` struct, no `VoidOrigin`. The material *is*
  the fact.
- Tailings: when a dig pass converts a solid cell to air, it emits a
  `SedimentEmit(origin_material, mass)` event that a surface-
  deposition rule consumes at the topmost non-air cell in that
  column.
- Burrow origin classification (Karst / Burrow / Collapse in
  column-GVSE) becomes optional per-cell flag bits or a coarse-
  resolution heatmap for whatever gameplay wants to distinguish
  them.

### 3.4 [ECOLOGY.md](ECOLOGY.md)

Per-column ecology bucket (root density, leaf area, biomass,
nutrient, water/air CO₂ + O₂).

Voxel intent:

- Ecology reads cell material + saturation heatmap directly.
  "Plant grows on a wet substrate" becomes: at the top-of-solid
  cell, if the cell is Organic/Clay/Sand and the moisture heatmap
  sample says saturation > threshold, spawn biomass.
- **Visible life = Set A module pixels** (`organism.rs`): Nucleus
  `#000000` + Photosystem `#2ECC40`, same as column-GVSE / 
  `docs/organism/`. Not a green terrain biomass wash — column
  `Ecology.alive_biomass` is a hidden substrate scalar there too.
- Nutrient / CO₂ heatmaps and land-plant Set D still deferred.

### 3.5 [AGENTS.md](AGENTS.md)

ECS creature layer (`AgentStore`), reads world state, calls `dig`,
`eat_biomass`, `drink_water`.

Voxel intent:

- Agents keep their continuous world-space position but query the
  cell grid instead of the column stack for spatial state.
- `dig` becomes `world.set_cell(x, y, Cell::air())`.
- `eat_biomass` reads the biomass heatmap at the agent's cell.
- `drink_water` reads the saturation heatmap at the agent's cell.
- **Deferred** until fluid + ecology work.

### 3.6 [EVOLUTION.md](EVOLUTION.md)

Genome-driven reproduction, mutation, phenotype from blueprint.

Voxel intent:

- Agent-level, independent of the terrain representation. Ports
  as-is once `AGENTS.md` migrates.

### 3.7 [docs/organism/](organism/)

Editor UX, palette, gene set, plant / fungi / lane / light / chem /
nerves / scenarios docs.

Voxel intent:

- Editor and palette are UI. They live in `wk-app` today; when
  `wk-app` swaps to voxel, they carry over verbatim modulo the
  pixel-to-cell mapping (already 1:1 in intent).
- Chem / gases: heatmaps.
- Lanes / fore-back drawing: retained. Voxel cells are 2D but the
  render can still paint back / body / fore in three passes.
- **Deferred** until the fluid + agent ports land.

## 4. Coordinate system and chunk sizing

- **World grid.** Cell indices are `(gx: i32, gy: i32)`. `gx`
  positive is right, `gy` positive is up (sky). This is the
  opposite of column-GVSE's `surface_y` semantics but agrees with
  the falling-sand convention where "bottom-up" gravity walks
  ascending y.
- **Chunk.** 64×64 cells (Noita: Purho GDC 2019). Small enough for
  one thread to own during a checkerboard sub-tick, large enough
  that per-chunk overhead is negligible.
- **Cell size.** Initially 1 m per cell. This matches column-GVSE's
  `SAMPLE_WIDTH_M = 0.25` if we later halve horizontal cell size,
  but 1 m is a good starting resolution — memory is ~4 B/cell so a
  full 4000×512 world footprint is ~8 MB, trivial.
- **Ring wrap.** As above, world-x wraps on a configurable width.
  `World::split` already uses `rem_euclid` so negative and
  cross-boundary lookups are correct without extra branching.
- **Sparse chunks.** A chunk exists only after first write to any
  of its cells (`HashMap` keyed by `ChunkCoord`).

## 5. Cell layout

```rust
struct Cell {
    material: MaterialId, // u8 discriminant
    sat:      Sat(u8),    // 0..255 water saturation
    flags:    CellFlags,  // u8 reserved bits
    _pad:     u8,         // reserved
}
```

Total: 4 bytes.

- `material` is the same enum column-GVSE uses. Water / Ice / Snow
  / Air / rock materials all live here.
- `sat` is the *saturation*, not a separate mass. Cell capacity
  depends on material: an Air cell holds a full unit of free water;
  a porous solid holds `porosity × cell_volume`. `sat` is the
  fraction of that capacity currently occupied.
- `flags` reserves bits for future rules (frozen, sediment
  carrier, momentum tag). Kept as a plain `u8` bitfield so the
  cell's memory shape doesn't need to grow.

Design implication: one scalar (`sat`) covers everything the column
model used to split across `Water` layer mass, `Column::moisture`,
and `Void::water_mass`. That's the whole point.

## 6. Heatmap overlays

Overlays are typed `Heatmap<T>` layers keyed by chunk coordinate.
Each patch has its own `cells_per_side` — some fields sample at
cell resolution (saturation is the cell itself), some at 4×4 tiles
(temperature, humidity, wind).

Initial overlays we plan to introduce, in rough order:

1. **Moisture / saturation.** Really the cell's own `sat`; no
   separate heatmap needed. Listed for completeness.
2. **Temperature (thermal field).** °C per tile, coarse (`cells_per_side = 4`),
   stepped on a ~20-tick cadence. Layered: air ↔ climate skin; surface
   water/rock with high `heat_capacity` / `albedo`; buried rock on a
   geothermal gradient with a slow upward heat leak. Enough inertia for
   phase change and future organics — not a full volumetric solver yet.
3. **Humidity.** Atmospheric water mass per tile (sparse
   `Humidity` map, `tile_cols = 4`). Evaporation deposits every
   tick; **diffusion runs on a schedule** (`humidity_diffuse_due`,
   period 20 / phase 3 — same cadence as column-GVSE
   `HumidityField`). The map is **clamped** to the stamped world
   tile bounds; ring worlds also set `wrap_x` so the atmosphere
   joins at the seam.
4. **Wind.** Vector `(vx, vy)` per tile. Drives cloud advection and
   surface stress.
5. **Sediment concentration.** Per cell. Only needed once we do
   sediment transport.
6. **Chemical / dissolved.** Per cell. Deferred; not in the initial
   scope.

The scaffold's `heatmap.rs` supports arbitrary `T` so any of these
slot in without an ABI change.

## 7. Update order sketch

The full rule pass will look like this (deferred implementation):

1. **Determine active chunks.** Any chunk whose dirty rectangle is
   non-empty *or* whose neighbour's dirty rectangle abuts its
   boundary participates in the tick. Others are quiescent.
2. **Four-pass checkerboard.** Divide the ring of active chunks
   into four passes: even-cx even-cy, odd-cx even-cy, even-cx
   odd-cy, odd-cx odd-cy. Adjacent chunks never share a colour
   (Purho, Noita, 2019). Within a colour, gravity/grain (and
   spill/seepage scans) run on **rayon** via disjoint chunk
   pointers (`crates/wk-voxel/src/parallel.rs`). Toggle with
   `set_parallel_enabled`. Spill/seepage still apply once from a
   snapshot so a seam edge is not re-equalised mid-rule.
3. **Within a chunk, bottom-up pull.** Gravity and grain walk
   `y = 0 → CHUNK_CELLS_H` and **pull** from the cell above into
   the current cell. Pull keeps cross-chunk seams one-step under
   checkerboard (the lower chunk owns the destination write).
4. **Rules per cell (in order):**
   a. Gravity fall. Destination cell pulls `sat` from above up to
      its free capacity.
   b. Lateral spill. If cell surface elevation
      `= gy + sat / capacity_of_cell` exceeds a neighbour's, push
      water across proportional to head difference.
   c. Density swap. If a solid particle sits on top of a lighter
      fluid, swap them (sand sinks through water). Same rule as
      column-GVSE `settle_by_density` but at cell granularity.
   d. Porosity absorb. Where a wet cell borders a porous unsat
      solid, transfer at rate proportional to material permeability.
5. **Dirty rectangle update.** Every write via `Chunk::set` extends
   the rect. Rules never touch cells outside the current rect.
6. **Clear.** After a rule pass consumes the rect, reset it to
   `None`. Next tick starts from writes only.

## 8. Determinism

- Per-chunk RNG seeded with
  `hash(world.seed, chunk_coord.cx, chunk_coord.cy, tick / period)`.
- Rules within a chunk always visit cells in the same order for a
  given tick.
- The checkerboard partition itself is deterministic (fixed by
  parity of `cx` and `cy`).
- Cross-chunk boundaries are the only source of ordering
  ambiguity; the four-pass sub-tick eliminates it by design.

Save format for `wk-voxel` will be one flat serialise of the world +
seed + tick — same shape as `wk-io` for column GVSE. Not in scope
for the foundation PR.

## 9. What we deliberately DO NOT port

- **Sub-column state buckets.** No `moisture` scalar. No
  `void.water_mass`. Cell material + saturation is the full model.
- **`MassAudit`.** The audit invariant will be recomputed each tick
  from the cell grid (walk cells, sum saturation × capacity), so
  audit counters as a separate bookkeeping surface aren't needed.
  If a scenario requires "mass in transit" bookkeeping, we'll
  reintroduce a much smaller counter set at that point.
- **Dedicated dissolved-mineral field.** Karst dissolution in the
  voxel sim converts limestone cells to air cells directly. If we
  want reprecipitation later, we add it as a per-agent chemistry
  hook rather than a spatial field.
- **`hydraulic_bed_y` / `solid_bed_y` / `climate_elevation`.** The
  cell grid has one truth: the cell at `(gx, gy)`. No parallel
  elevation formulas.
- **Chunk halo / outbox / inbox exchange.** Cellular rules only
  read adjacent cells; the world map handles cross-chunk lookup
  natively.

## 10. Research bibliography

Consulted while writing this document. Findings are already baked
into the design above; the URLs are recorded so future work can go
back and read the source.

### Noita / Falling Everything engine

Petri Purho, *Exploring the Tech and Design of 'Noita'*, GDC 2019.

- [YouTube recording](https://www.youtube.com/watch?v=prXuyMCgbTc)
- [GDC Vault](https://www.gdcvault.com/play/1025695/Exploring-the-Tech-and-Design)
- [80.lv writeup / interview](https://80.lv/articles/noita-a-game-based-on-falling-sand-simulation)
- [Rock Paper Shotgun overview](https://www.rockpapershotgun.com/from-falling-sand-to-falling-everything-the-simulation-games-that-inspired-noita)

Findings we use:

- 64×64 chunks with a per-chunk **dirty rectangle** for quiescence.
- **Four-pass checkerboard** sub-tick order for safe parallelism.
  Each pass picks alternating 64×64 chunks with a 32-cell margin
  so a thread can freely mutate its area without locks.
- **Bottom-up update order** inside a chunk so gravity moves a
  falling cell only once per tick.
- **Density comparison for swaps** (heavy fluid displaces lighter):
  "liquid pixels first check if they can go down. If not, they
  check left and right." Density is the tiebreaker for stacked
  fluids.
- Rigid bodies via marching squares — deferred, but noted for when
  we do movable "chunks of solid" (e.g. a boulder).

### Powder Toy

Open-source falling-sand C++ reference implementation.

- [GitHub repository](https://github.com/The-Powder-Toy/The-Powder-Toy)
- [`src/simulation/Simulation.cpp`](https://github.com/The-Powder-Toy/The-Powder-Toy/blob/master/src/simulation/Simulation.cpp)

Findings we use:

- **Element table with per-material update functions.** A big
  discriminated dispatch on material id. The core loop is
  procedural (no OOP, no virtual dispatch, no RAII in hot path) —
  we do the same in Rust with a plain `match` on `MaterialId`.
- **Air pressure / heat coupled to particles.** Air is a
  first-class simulated field, not a background. Consistent with
  our plan to make `Air` a cell and stack heatmaps for temperature
  / humidity / pressure over the same grid.
- **Free-particle recalculation.** They maintain a small list of
  active particles (as a compressed index) rather than scanning
  every cell every tick. Related to our dirty-rectangle strategy
  but at a different granularity — worth revisiting when we
  optimise.
- License: GPL-3. We are **not** copying code, only reading for
  ideas. `wk-voxel` remains under the workspace's MIT license.

### Height-field water over terrain

Nikita Lisyarus, *Simulating water over terrain*.

- [Blog post](https://lisyarus.github.io/blog/posts/simulating-water-over-terrain.html)

Findings we use:

- **Virtual-pipes model** for cell-to-cell flow: neighbouring water
  columns are treated as connected by imaginary pipes whose flow
  rate depends on head difference. Same mathematical shape as our
  planned "lateral spill" rule.
- **Non-negativity clamp** to avoid mass loss under explicit time
  integration: never remove more water than the cell has. We'll
  apply this at cell level.
- **Outflow scaling** so the sum of outflows can't exceed cell
  content. Useful when a cell tries to move mass to multiple
  neighbours in one pass.

Related lattice-gas / lattice-Boltzmann literature for porous flow:

- Rothman, *Cellular-automaton fluids: a model for flow in porous
  media*, Geophysics 1988.
- Di Pietro et al., *Modeling water infiltration in unsaturated
  porous media by interacting lattice gas-cellular automata*,
  Water Resources Research 1994. [DOI 10.1029/94wr01307](https://doi.org/10.1029/94wr01307)

Findings we use:

- **Mass-conserving porosity/permeability rules** where Darcy's
  law emerges as a large-scale limit of local cell interactions.
  Motivates the porosity absorb rule design.

### Starbound-style 2D water

Community reference (a game we're not copying, but a nice sanity
check on 2D CA water at gameplay speeds).

- [gamedev.stackexchange thread](https://gamedev.stackexchange.com/questions/130861/2d-rain-creation-and-fluid-dynamics)

Findings we use:

- **Rain drops are decorative particles.** Accumulation happens by
  a per-cell counter, not by inspecting drop trajectories. Same
  approach we'll take: `RainSource` writes to the top-of-air cell
  of each column every tick; visual streaks are cosmetic.

### Voxel engine optimisation talk

*500 million voxels/sec* (creator writes their own Rust + Bevy
engine), various optimisation techniques.

- [YouTube](https://www.youtube.com/watch?v=ru_oz09Zo-s)

Findings we use:

- **Extremity bounds check.** Skip any chunk where the whole
  chunk is one material (fully solid rock, fully air sky). Cheap
  early-out during worldgen and streaming.
- **Noise caching along invariant axes.** In 2D side-view this
  applies to the horizontal-slice worldgen: compute the surface
  height once per column, cache it, then walk down. Free win.
- **Run-length encoding** for cache efficiency. Deferred — our
  chunks are already small enough to fit in L1.

### Skipped

- Sparse voxel octrees / DAG (`voxelis`, `vx_bevy`, Laine + Karras
  2010 "Efficient Sparse Voxel Octrees"). Overkill for a flat 2D
  grid; 64×64 chunks with a sparse `HashMap` are simpler and
  cheaper. Noted for the record; not part of `wk-voxel`.

## 11. Order of work

Foundation (this PR): scaffold + this document. **No rules.**

Follow-ups, each its own PR:

1. Falling gravity rule (single-material water cells fall into air
   cells).
2. Lateral spill rule (virtual pipes head equalisation).
3. Density swap (sand falls through water).
4. Porosity absorb / seepage (water enters porous solid up to
   saturation cap; permeability-limited) + head-based spill.
5. Worldgen (stamp continental profile into cells).
6. Rendering (macroquad-based, matches column-GVSE's palette).
7. Rain / evaporation sources.
8. Karst dissolution (limestone → air).
9. Multithreading via the checkerboard partition.
10. Ecology + agents port.

Items 1–8 (through karst + seepage/head-spill), dirty-rect
active-chunk planning, four-pass checkerboard, **rayon
parallelism within each colour**, **chunk occupancy skips** for
evap / karst, and **Set A Atoms** (`OrganismStore` — isolated, no
`wk-agents` import) are landed in `wk-voxel`, plus **Set D plants**
(D1–D4 plants + E1 litter/fungi + lingering corpses → Organic — see
`plant.rs`, `fungi.rs`, `organism.rs`, and
`docs/organism/VOXEL_PLANTS.md`). Remaining organism focus is Set E2
(epiphytes / topple).

Each PR keeps the isolation contract and passes headless tests
before touching rendering.

## 12. What could still go wrong

Honest concerns kept close so we don't discover them at commit
time:

- **Cell-level cost.** A 4000-cell-wide ring × 256 cells tall =
  ~1M cells. At 60 Hz and a naive full scan, that's 60 M cell
  visits/sec. Feasible in Rust, but the dirty-rectangle + chunk
  quiescence tricks aren't optional; they're load-bearing.
- **1D → 2D memory step.** Column-GVSE keeps ~192 chunks × 64
  columns × ~8 layers = ~100 K cells of geology state. Voxel is
  10× larger by cell count. Still ~8 MB, but every field / heatmap
  overlay adds another slab. We'll audit total memory at each PR.
- **Rendering perf.** 65 K on-screen cells naively drawn as
  rectangles is fine on desktop, but we should batch as one draw
  call (macroquad or bevy). Rendering PR will settle the choice.
- **Stratigraphy resolution.** Coarse `Heatmap<u32>` stratum ids
  might read as blocky. If it looks bad, we bump the resolution.

Not concerns we plan to solve up front — flagged so an agent
picking this up in a month knows where the friction is.
