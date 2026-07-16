# Performance

*Author: initial review, mid-2026. Measured on a single mid-range core,
release build, `cargo test --release`.*

## Measured baseline

Reproduce with the ignored `bench_scaling` benchmark:

```
cargo test --release -p wk-sim --test scenarios -- --ignored --nocapture bench_scaling
```

| Chunks | Columns | Rain | tps | col·ticks/s |
|-------:|--------:|-----:|----:|------------:|
| 4      | 256     | off  | 11,029 | 2.82 M |
| 16     | 1,024   | off  | 1,683  | 1.72 M |
| 32     | 2,048   | off  | 868    | 1.78 M |
| 64     | 4,096   | off  | 416    | 1.70 M |
| 88     | 5,632   | off  | 308    | 1.73 M |
| 4      | 256     | on   | 10,579 | 2.71 M |
| 16     | 1,024   | on   | 1,616  | 1.65 M |
| 88     | 5,632   | on   | 248    | 1.40 M |

Reading the numbers:

- Column-tick throughput is roughly flat above ~1000 columns at
  ~1.7 M col·ticks/s single-core. Per-column work is essentially
  linear in column count. Good.
- The 4-chunk world runs ~1.6× faster per column-tick than the 88-chunk
  world. That gap is a real superlinearity, driven by BTreeMap lookups,
  halo cloning, and global O(N) sweeps that don't skip inactive chunks.
- Rain-on vs. rain-off costs ~20% at the shipped map size. Dominated by
  `run_surface_water` and `run_sediment`. The rest of the schedule is
  quiet when there's no water to move.

At **60 tps target** (real-time simulation at 1× speed with the whole
map live), the shipped 5632-column continental map has ~4× headroom on
a single core (308 / 60). That's not comfortable — the karst /
ecology / creature layers we plan to add will eat it.

The target after the optimisations below is **80–100× real-time on one
mid-range core** for the currently shipped map. That leaves budget for
the additions without going over frame time.

## Prioritised optimisation list

Ordered by expected payoff. Each is independent of the others; each can
land as a separate PR.

### 1. Kill the halo-update clone

`subsystems::update_halos` currently does:

```rust
let left = world.chunks.get(&(coord - 1)).cloned();
let right = world.chunks.get(&(coord + 1)).cloned();
```

That's cloning entire ~22 KB `Chunk` structs on both sides of every
loaded chunk, every tick, purely to plumb four scalars (surface_y,
water_top, water_table for left and right) across the seam.

At the shipped map size this is ~4 MB/tick of memcopy for nothing.
Replace with direct reads of just those four `f32`s from the
neighbour's edge column, using a scoped iteration pattern that reads
one chunk before it mutably borrows the next.

Expected payoff: 10–20% on wide maps.

### 2. Parallelise buffered subsystems across chunks

`run_surface_water`, `run_sediment`, `run_infiltration`,
`run_evaporation`, `run_groundwater_flow` all have no cross-chunk state
dependence — the outbox / inbox pattern already isolates the boundary
transfer. With `rayon::par_iter_mut` over `world.chunks.values_mut()`
these scale near-linearly with cores.

On an 8-core box, expected 5–6× speedup after item 4 (BTreeMap → Vec)
removes the borrowing-a-BTreeMap-in-parallel obstacle.

### 3. Dirty flags per chunk driving `run_lake_level` and `run_slumping`

Both currently do global O(N_columns) sweeps every tick.
`run_lake_level` walks every column across the whole world to detect
lake segments even when most chunks are bone dry. `run_slumping` does
two passes plus a `clamp_state` + `recompute_surface_y` on every column
of every chunk — including ones that saw no transfer this tick.

A single `dirty_this_tick: bool` bit on each `Chunk`, set when any
subsystem writes to that chunk's transfer buffer, gates both. Clean
chunks skip these passes entirely.

Expected payoff: 15–25%, higher on dry maps.

### 4. Flatten `BTreeMap<i32, Chunk>` to `Vec<Chunk>` + base-coord offset

`BTreeMap::get(&coord)` on the hot path is O(log N). It appears in
almost every subsystem loop. For contiguous chunk ranges (the normal
case), a `Vec` indexed by `(coord - base_offset)` is O(1) direct
indexing — a couple of instructions vs. a tree walk.

Save-time can still serialise as sorted (coord, chunk) pairs so save
format is unaffected.

Watch out: the chunk streamer (see `WORLDGEN.md`) needs the ability to
have "holes" (chunks not yet generated). A `Vec<Option<Chunk>>` or a
small `Vec<(i32, Chunk)>` with binary search + hot-index cache both
work.

Expected payoff: 15–20% on wide maps once item 2 is in.

### 5. `barrier_commit` buffer clone

```rust
if let Some(buf) = scratch.buffers.get(&coord).cloned() {
```

The clone exists to satisfy the borrow checker (`chunk` needs `&mut`,
buffer needs `&`). Fixes: (a) move buffers into a `Vec` indexed by
chunk-slot; (b) `HashMap::get_disjoint_mut`; (c) `.iter().unzip()` into
parallel arrays. Same pattern in `exchange_outboxes`
(`.map(|(&c, o)| (c, o.clone()))`).

Small per-tick cost individually but hot; worth doing when item 4
lands.

### 6. `MaterialRegistry::props` switch table → `const` array

`MaterialRegistry::props(m)` is called constantly (every subsystem, in
the inner loop). It's inlinable but not zero cost. A
`const PROPS: [MaterialProps; MATERIAL_COUNT]` indexed by `m as usize`
is strictly better — one indexed load, no branches. Same for
`colour_rgb`.

Small but essentially free.

### 7. Preallocated snapshot buffer for the renderer

`World::snapshot()` allocates a `Vec<ColumnView>` and a `Vec<Layer>`
per visible column every frame. Given the fixed-size viewport, a
preallocated snapshot buffer stops allocator churn from stealing frame
time on the main thread.

Not a sim speedup but a frame-time stability win.

### 8. `run_activity`'s "sediment > 0 keeps me awake" rule

Currently any column carrying suspended sediment stays
`HydrologyActive`. The Stokes-like settling never fully removes the
last kg of sediment on flat ground, so the dormancy optimisation rarely
fires in practice on a wet map. Either accelerate final-kg settling or
change the activity predicate to `sediment > small_threshold`. Worth
measuring whether the dormancy shortcut is paying for itself at all.

### 9. Move per-column `residual` and `activity` into a per-chunk sparse map

`ResidualAccumulator` state is only meaningful for the ~1 column per
chunk actually touched this tick. Storing it inline on every column
hurts cache density on the dominant `for i in 0..CHUNK_W` inner loops.
A per-chunk `HashMap<usize, ResidualBucket>` or a sparse pair list
would improve cache behaviour on the loops that don't need those
fields.

Lower priority; the layout is fine until items 1–5 are in.

## Projected end state

After 1–5:

- Halo clones eliminated.
- Buffered subsystems chunk-parallel.
- Dirty flags shortcut two of the biggest global sweeps.
- BTreeMap flattened to Vec.
- Barrier commit no longer clones per chunk.

Rough projection at the shipped map size on a single core: ~3–5k tps,
i.e. **80–100× real-time headroom on one core**. Add rayon-driven core
scaling and 8-core headroom is ~600–1000× real-time.

That's the target to hit before starting the karst + ecology work, so
those layers have room to grow.

## What already is good and shouldn't be touched

- Multi-rate scheduler (`SubsystemSchedule` with period + phase +
  fixed `SUBSYSTEM_ORDER`). New subsystems slot in with a two-line
  change. This shape is right.
- Buffered writes + barrier commit. This is exactly the pattern SPH
  and cellular-automata engines converge on for determinism.
- Residual accumulator for sub-integer rate integration. Standard fix
  for the "eats 0 kg per tick because 0.4 rounds down" bug family.
- Mass audit invariant with `bookkeeping_check`. Preserve this discipline
  as new mass sinks (dissolved carbonate, biomass) are added — extend
  the equation, don't skip it.
- Deterministic content-addressed hashing (`hash_u64`, `hash_f32`) with
  no `rand` dependency. Correct for replayable simulations.
- Inline arrays in `Column` and `Chunk` (`[Layer; MAX_LAYERS]`,
  `[Column; CHUNK_W]`). Cache-friendly, no heap indirection.
- Scenario tests as engineering artefacts (E1–E8). Extend, don't
  replace — each new subsystem earns its own scenario.
