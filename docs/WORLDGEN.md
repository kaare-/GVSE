# World generation, streaming, and initial hydrological state

*Author: initial design proposal, mid-2026. Nothing in this document is
implemented yet; this is the design record for a planned extension.*

## Goals

- The map stretches "infinitely" left and right. There is no static
  world size the player can reach.
- Terrain is regenerated identically from a seed at any position. A
  chunk that has been forgotten and revisited must produce bit-for-bit
  the same generation output.
- Terrain generation produces a plausible initial hydrological state:
  a water table, ground moisture above it, atmospheric humidity, and
  spring/wetland features where the table meets the surface. New land
  is not bone-dry — it starts at the hydrological steady state its
  biome implies.
- Water leaving the currently simulated region cannot vanish silently
  and cannot pool up against an invisible wall. The boundary condition
  must be either physically transparent (flow continues into a real
  neighbour) or hydrologically absorbed (backlogged into a frozen
  neighbour that will apply the accumulated inflow when it re-enters
  the active window).
- The simulation cost is bounded regardless of world size: only a
  finite active window is ticking, everything beyond it is either
  frozen or evicted.

## Why columns, not voxels

The rationale for the column-of-layers representation is a per-tick
budget argument, and it directly constrains this design.

A shipped-map column costs roughly 600 ns per tick. At an active
window of ~30 chunks (≈1920 columns) that's ~1.1 ms of sim per tick,
well below the 16.7 ms of a 60 Hz frame. That leaves headroom for the
karst, ecology, and creature layers we're planning.

The equivalent voxel model — say 100 cells per column vertically —
would be ~192 k voxels in the same window. Even at aggressive
sparsity, per-voxel work makes real-time impossible without giving up
the mass-audit invariant that makes the whole thing debuggable.

So this document takes as fixed:

- Vertical geometry is per-column (mass in layers, plus sparse voids
  from `UNDERGROUND.md`).
- Horizontal geometry is per-column at 0.25 m resolution.
- Any design that would require an active window larger than ~50
  chunks or would raise per-column cost past ~1 µs is rejected on
  budget grounds.

## The current state, and what has to change

The current `continental_surface_y` in `crates/wk-world/src/terrain.rs`
defines a fixed profile:

```
if macro_x < 100.0 { abyss }
else if macro_x < 180.0 { slope }
else if macro_x < 260.0 { shelf }
else if macro_x < 340.0 { coast }
else if macro_x < 420.0 { plains }
else { mountain cordillera with 8 named peaks }
```

with absolute world-x cutoffs. Walking right past x=420+555 m hits the
last "mountain" branch forever; walking left of x=0 hits abyss
forever. This is not a bug for a demo but it is a hard block for an
infinite world.

The current `AppState::new` also pre-generates 88 chunks eagerly at
startup (`MAP_CHUNK_MIN = -8`, `MAP_CHUNK_MAX = 80`) and relies on all
of them being resident. There is no streaming code path.

However, the substrate is already halfway there:

- `World.chunks: BTreeMap<i32, Chunk>` is keyed by coord, so the map
  data model is already "infinite by coord."
- `generate_chunk_continental(coord, seed, ...)` is a pure function of
  `(seed, coord)`. Deterministic regeneration works today.
- `MAX_LOADED_CHUNKS = 96` and `World::insert_chunk` already evict the
  farthest chunk on overflow.
- `hash_u64(seed, x, y, salt)` gives us a deterministic content-
  addressed noise primitive at any coordinate, with no state.

What has to be built:

1. A stationary noise-based terrain generator that produces varied
   biomes at all world-x, not a fixed profile with hard cutoffs.
2. A chunk streamer with view / active / resident / evicted tiers.
3. An initial-hydrological-state pass at chunk generation time.
4. A boundary condition on the active window edge that either accepts
   flow into a frozen neighbour or reflects it against a physical
   wall.

## Infinite terrain via multi-scale deterministic noise

Replace the fixed profile with a composition of value noise at three
frequency bands. Each band is a smooth interpolation of the
`hash_f32(seed, world_x / stride, salt)` primitive already available.

**Band A — continental noise (very low frequency)**. Stride ≈ 4000 m,
produces slow variation between continental interior and oceanic
regions. This value determines the *macro* elevation offset — a broad
"land here vs. ocean here" bias that shifts every few thousand columns.

**Band B — regional noise (medium frequency)**. Stride ≈ 400 m,
produces coast / plains / hill / mountain regional structure. Larger
than a chunk, smaller than a continent. This gives the world visible
"regions" that a player recognises as they scroll.

**Band C — local noise (higher frequency)**. Stride ≈ 40 m and 10 m,
produces the ripple and column-scale variance that already exists in
`land_ripple`.

Composition:

```
elevation(seed, wx, sea_level) =
    sea_level
  + CONTINENTAL_AMP * value_noise(seed, wx, 4000_m, salt=101)
  + REGIONAL_AMP    * value_noise(seed, wx,  400_m, salt=102)
  + LOCAL_AMP       * (value_noise(seed, wx,  40_m, 103)
                      + value_noise(seed, wx,  10_m, 104) * 0.4)
```

Amplitudes tuned so:

- Continental: ±40 m (deep ocean ↔ high plateaus).
- Regional: ±20 m (coastal hills ↔ mountain ridges).
- Local: ±3 m (columns don't stray far from their neighbours).

The current fixed-profile logic becomes a special case if you clamp
band A to a stepped function of x, so migration is straightforward: we
can preserve the existing demo landscape shape as a scenario while
gaining the infinite generic case.

Sediment composition (`sediment_composition`) is unchanged in shape —
it's a function of elevation-relative-to-sea-level, and that's
independent of how the elevation was derived. Free.

**Determinism guarantee**: every noise call is
`hash_f32(seed, wx_bucket, salt)` — a pure function of seed, integer
bucket coordinate, and a compile-time salt constant. No state, no
RNG streams, no chunk-order dependency. `generate_chunk_continental`
becomes strictly self-contained.

## Regional climate variation

For the ecology layer to have interesting selection pressure, biomes
must differ in **wetness**, not just elevation. Add a wetness field
at the regional scale:

```
wetness(seed, wx) = clamp(0.5 + WETNESS_AMP * value_noise(seed, wx, 800_m, 105), 0.0, 1.0)
```

- `wetness ≈ 1.0`: humid forest / rainforest region. Deep root zones,
  shallow water table, high atmospheric humidity, frequent precipitation.
- `wetness ≈ 0.5`: temperate.
- `wetness ≈ 0.0`: arid / semi-desert. Deep water table, low humidity,
  rare precipitation.

Wetness is what drives every initial hydrological state parameter in
the next section. It's a per-column value derived from a global noise
field, so adjacent chunks blend smoothly and there are no biome cliffs.

This also gives the weather subsystem a natural extension point:
clouds passing over an arid region should be less likely to precipitate
than over a humid region (a "willingness to rain" scalar can be modulated
by local wetness at cloud position). But that's a weather extension, not
a worldgen one.

## Initial hydrological state at chunk generation

At chunk generation time, after topography and stratigraphy are
placed, run an initialisation pass over each column.

### Water table elevation

The water table sits somewhere between the bedrock and the surface, at
a depth determined by regional wetness. For each column:

```
depth_offset = mix(dry_offset_m, humid_offset_m, wetness(seed, wx))
water_table_y = clamp(
    surface_y − depth_offset,
    bedrock_y + MIN_TABLE_ABOVE_BEDROCK_M,
    surface_y                                // never above the surface
)
```

with defaults roughly:

- `humid_offset_m = 3.0` (a wet-region water table is 3 m below the
  surface — near-surface springs, wetlands, standing water in low
  spots).
- `dry_offset_m = 25.0` (an arid-region water table is 25 m deep —
  desert plateau, dry gullies).

Where the raw calculation would put the table **above** the surface,
clamp to surface and emit the excess as surface water: a spring or the
edge of a wetland (see below). Where the raw calculation would put it
below bedrock, clamp to bedrock — an aquifer bottom.

### Pore-water saturation below the table

Every porous solid layer entirely below `water_table_y` is initialised
at its full `moisture_cap` — saturated. This is the aquifer.

The current `Column.moisture` scalar handles pore-water in the
topmost porous layer. Extending to fill lower porous layers with
water needs either per-layer moisture (deferred to phase 2 of the
unification per the comment in `Column`) or a convention that the
whole column's moisture below the table is implicitly saturated and
we track only the moisture in the layer *containing* the water table.

The simpler approach for now: keep `Column.moisture` as-is but
interpret it against the layer containing the table, and treat
everything below as saturated by construction. The mass audit needs to
count that implicit water — otherwise the audit will "detect" the
sudden appearance of an ocean-worth of pore water at generation time
and fail. Add it to the audit at generation as a `soil_inject_total`
bucket parallel to `sea_inject_total`.

### Capillary fringe

Between the water table and `water_table_y + capillary_fringe_m` (typically
1–3 m), saturation decreases linearly from 1.0 to 0.0. This is the
vadose zone — moist soil that's not saturated but has significant water.
The linear ramp is a coarse but reasonable approximation of a
Brooks-Corey / van Genuchten curve at this fidelity.

For the current data model (`moisture` on the topmost porous layer
only), this reduces to: if the water table is inside the topmost porous
layer, set `moisture` proportional to how much of that layer's height
is below the table plus a partial capillary contribution above.

### Soil moisture above the fringe

Above the capillary fringe but below the surface, initial soil moisture
is set to the biome's steady-state value:

```
initial_soil_saturation = 0.05 + 0.35 * wetness   // 5%..40% of cap
```

In dry regions this is ~5% of moisture cap — enough that infiltration
still happens but the ground isn't dry to bedrock. In wet regions it's
~40% — moist forest floor with retained rainwater.

Sets `Column.moisture` to `initial_soil_saturation * moisture_cap`
where the water-table calculation doesn't already override it.

### Atmospheric humidity

Add a per-region humidity scalar. The simplest form is a chunk-level
scalar that relaxes toward a target driven by wetness and by proximity
to open water:

```
humidity_target(coord) = 0.3 + 0.4 * regional_wetness + 0.2 * near_ocean
```

where `near_ocean` is 1 if the chunk contains submerged terrain and
falls off with distance from the nearest chunk that does. This is
computed at generation time, cached, and updated only when new chunks
generate to either side (a very rare event once the world's active
region has stabilised).

At runtime, `humidity(coord)` relaxes toward `humidity_target(coord)`
with time constant ~1 in-game day. Local evaporation raises it,
precipitation lowers it. This replaces the hardcoded
`const HUMIDITY: f32 = 0.4` in `crates/wk-sim/src/subsystems.rs` with a
lookup, and the existing `run_evaporation` picks up regional variation
essentially for free.

### Springs and wetlands where the table meets the surface

For any column where the water-table calculation clamped to `surface_y`,
place a small `Water` layer on top with mass equal to a small fixed
initial amount (say 1–3 cm depth-equivalent). This gives generation-
time visual and hydraulic anchors: wetlands at the base of hills, coastal
marshes where the shelf water table intersects the emergent land,
oases in dry regions where a deep aquifer surfaces.

These initial water features will then obey ordinary surface flow /
evaporation / infiltration during simulation. Some will dry up over
time in arid regions and reappear during wet spells; some will drain
downhill and consolidate into permanent lakes; some will hold
throughout the simulation because they're fed by a spring from below.

### Clouds

Not initialised per chunk — clouds are a world-level list drifting
through, and their generation is orthogonal to terrain. But the
initial-cloud spawn logic (see `run_weather`) should be sensitised to
local wetness once wetness is available.

## Chunk streaming: view / active / resident / evicted tiers

Concretely, at any moment there are four sets of chunks:

- **View**: chunks currently visible in the viewport (derived from
  camera x and screen width).
- **Active**: view + `HALO_CHUNKS` buffer on each side (default 3
  chunks ≈ 48 m). All chunks in this set are simulated every tick by
  every subsystem.
- **Resident**: active + `RESIDENT_MARGIN_CHUNKS` buffer on each side
  (default 8 chunks ≈ 128 m). These chunks are in memory but *frozen*
  — skipped by the flow subsystems, only touched by barrier commit to
  absorb any inflow from the active edge.
- **Evicted**: everything outside resident. State is either persisted
  to a per-run in-memory chunk-store (phase 1) or a disk-backed store
  (phase 2), or discarded and regenerated on demand.

Behaviour on scroll:

| Transition | Action |
|------------|--------|
| A chunk enters `view` and is not resident | Generate it (or load from store) and mark active |
| A chunk enters `active` from `resident` | Thaw: add to active set; the halo backlog it accumulated is applied on the next barrier commit |
| A chunk leaves `active` for `resident` | Freeze: no more flow subsystems run on it, but barrier commit still applies any inflow from active neighbours into a small backlog buffer |
| A chunk leaves `resident` | Evict: persist state to store, drop from memory |
| A chunk enters `resident` from evicted | Load from store, or generate if never seen; frozen |

The active set size is roughly constant regardless of how far the
camera has travelled. At default halo/margin, `|active| = |view| + 6`
and `|resident| = |view| + 22`. For a `viewport_column_count` of ~350
columns (roughly one full-screen worth at 4 px per column on a 1440 px
window), that's ~6 chunks in view, ~12 chunks active, ~28 chunks
resident. Well under the current 96-chunk cap; well under the
performance envelope.

**Per-tick sim cost is bounded by `|active|`, not by world size.**
This is the crucial property. The player can scroll for hours and the
per-tick cost stays fixed.

## Boundary conditions: no water leaks

The specific concern is: without a physically consistent boundary, a
river flowing to the edge of the active window disappears the moment
the camera moves and the edge chunk gets frozen.

Three mechanisms in play:

- Surface runoff hitting a chunk edge with no active-neighbour.
- Groundwater lateral flow doing the same.
- Sediment carried with the flow.

The current code (`exchange_outboxes`) books all three into
`boundary_out_total` when no right/left chunk exists. That keeps the
mass audit intact but treats the world as if it ends at the loaded
region. We need better.

### Rule 1 — Frozen chunks absorb inflow (backlog)

When a chunk is in `resident` (frozen) state, the flow subsystems in
active neighbours still route water/sediment/moisture toward it via
`exchange_outboxes`. Instead of booking that into `boundary_out_total`,
book it into a **backlog** on the frozen chunk:

```rust
pub struct FrozenBacklog {
    pub water_in_left: i64,
    pub water_in_right: i64,
    pub sediment_in_left: SedimentLoad,
    pub sediment_in_right: SedimentLoad,
    pub moisture_in_left: i64,
    pub moisture_in_right: i64,
}
```

On thaw, the backlog is applied to the appropriate edge columns of the
now-active chunk exactly once, then cleared. Mass is conserved
throughout. This is the same shape as the existing `ChunkInbox` but
persists across many ticks instead of being drained every tick.

### Rule 2 — Active-window boundary is absorbing, not reflective

At the outermost active chunk, the halo values (surface_y, water_top,
water_table) are taken from the frozen resident neighbour if one
exists, from the generative steady-state values if not.

Because the frozen neighbour's state doesn't move, and because the
generative values are exactly the steady state (see initial-hydrological
state above), the head gradient at the active boundary is small and
self-limiting: water only crosses the boundary if the active side has
built up water above the generative steady state, which is exactly
correct.

Contrast with the current behaviour, where a fresh chunk generated on
scroll would show water table = surface_y = topography, moisture = 0,
and immediately act as an infinite sink for whatever water flowed into
it.

### Rule 3 — Newly-generated chunks start at hydrological steady state

Restated because it's the anchor of this whole design: the reason the
boundary doesn't leak is that new chunks are generated with a
plausible initial water table, capillary fringe, soil moisture, and
atmospheric humidity. So flow into a new chunk from an active neighbour
mostly finds "the level is already about right" and only the small
delta between the source's local state and the biome's steady state
actually transfers.

This lets rule 2 work: the boundary is transparent to gross flow
because the neighbour is already at the level the flow would push it
to. Only physically meaningful excess crosses.

### Rule 4 — Discharge at absolute world boundaries (there are none)

There is no absolute world boundary. If the sim ever needs to know
"where does water end up if it keeps flowing right forever?" — the
answer is: infinitely many chunks to the right, each one starting at
the steady state, each one absorbing the diminishing excess. The mass
audit gets `boundary_out_total = 0` in the ideal case; small residual
transfers into freshly-generated chunks are booked to a new
`soil_inject_total` bucket (see initial pore-water saturation above)
so the invariant equation stays exact.

## Persistence

**Phase 1**: in-memory chunk-store. When a chunk is evicted, its
serialised bytes (via the existing `postcard` path in `wk-io`) go into
a `HashMap<i32, Vec<u8>>` on the world. When re-loaded, deserialised
back into a `Chunk`. This is cheap and preserves state without disk
I/O; the trade-off is that quitting the app loses everything not in
the explicit save.

**Phase 2**: disk-backed chunk-store. Same interface, but the map is a
memory-mapped file or a directory of per-chunk files keyed by coord.
Preserves state across runs even without an explicit save. Fits the
existing postcard format cleanly.

**Determinism guarantee for regeneration**: a chunk that has never
been simulated (or was evicted and its state discarded) can be
regenerated identically at any time by calling
`generate_chunk_continental(seed, coord)`. The generator is a pure
function; content-addressed hashing means no state leaks across chunk
boundaries.

**Non-determinism from simulation**: once a chunk has been simulated,
its state depends on the history of what flowed through it. Two
different play sessions of the same seed will diverge as soon as
scroll patterns differ. That's expected and matches how every other
persistent open-world game works.

## Interaction with the rest of the roadmap

- **Karst caves** (`UNDERGROUND.md`): void placement is deterministic
  from `(seed, coord, elevation)`. `run_karst` operates on active
  chunks only; frozen chunks preserve their voids in the backlog / save
  representation without dissolving further. Same rule as any other
  active-only subsystem.
- **Ecology bucket**: `root_density`, `leaf_area`, `nutrient` all get
  a biome-driven initial value at chunk generation time (a wet region
  starts with denser vegetation, a dry one with sparser). This is
  exactly analogous to water-table initialisation.
- **Creatures**: agents live in their own layer (an ECS, per
  previous review). Their positions determine where the active window
  should be — the sim needs to keep an active window around every
  agent, not just the camera. That means the active-set becomes a
  union of viewports plus agent-halos.

## Suggested implementation stages

Each stage lands as its own PR; each leaves the sim in a working state.

1. **Multi-scale noise terrain generator.** Replace the fixed-profile
   `continental_surface_y` with the three-band noise composition,
   plus regional wetness field. Existing E-tests continue to pass;
   add a new scenario that walks 400 chunks in either direction and
   asserts variety of biomes. Fixed-profile can survive as a debug
   `generate_chunk_demo_profile` for reproducing the current showcase.

2. **Initial hydrological state pass.** Water table + capillary fringe
   + soil moisture + atmospheric humidity at generation. New scenario:
   `E14_generated_land_starts_at_steady_state` — verifies that a chunk
   generated cold and simulated with rain-off drifts by less than a
   small threshold over 10k ticks (i.e. it's really at steady state).

3. **Chunk streamer with view/active/resident/evicted tiers.** In-
   memory chunk-store for persistence. `AppState::new` no longer
   pre-generates chunks; the streamer generates on demand.
   Deterministic regeneration test: unload, regenerate, assert bit-
   identical terrain output.

4. **Frozen-chunk backlog + absorbing boundary.** Rule 1 and rule 2.
   New scenario: `E15_no_boundary_leak_across_freeze` — a river flowing
   through the active edge into a frozen chunk, run for 10k ticks,
   then thaw and verify mass conservation across the whole event.

5. **Disk-backed chunk-store** (optional, later). Same interface as
   phase 1 but persistent across runs.

## Trade-offs explicitly accepted

- **Regional wetness is per-column noise, not a simulated atmospheric
  model.** A proper atmospheric circulation (Hadley cells, orographic
  precipitation, rain shadow) would give more physically defensible
  biome patterns but is far beyond the current budget. The noise field
  produces plausible-looking wet/dry variation without simulating
  atmosphere, which is enough for the ecology payoff.

- **The water table is initialised from a static climate function,
  then evolves under simulation.** Over long play, aquifer drawdown /
  recharge dynamics will diverge from the noise field, and the field
  is only ever used at chunk-generation time. That's correct — the
  field represents "what the climate wants" and the simulation
  represents "what the water actually does under that climate."

- **Frozen chunks don't tick.** Their state as observed by the player
  will be exactly what it was when they were frozen, until they thaw.
  For a slow-drifting geological simulation this is fine; a player
  scrolling away from a river won't return to find it evaporated,
  they'll return to find its state at freeze time plus the accumulated
  backlog applied. For fast dynamics (creatures moving) this would be
  wrong, and the creature layer will need agent-tracking to keep
  chunks containing agents active regardless of camera position.

- **Two-band noise (continental + regional) may still produce
  monotonous stretches in unlucky seeds.** If this proves boring, add
  a fourth band (mesoregional, ~1500 m stride) at half the amplitude
  of regional. Cheap.
