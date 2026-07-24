# Chemistry

*Frozen `ChemType` count, per-chunk chem field slot, and sensor /
emitter tuning genes. `chem-types` in the Organism Kernel plan.*

## Working default: 4 ChemTypes

```rust
pub type ChemTypeId = u8; // 0..N, N small
pub const CHEM_TYPE_COUNT: usize = 4;
```

Four is the freeze. Not two — a proto-quorum needs more than a single
"food / not food" scalar. Not sixteen — every added channel costs a
per-cell scalar in the field plus editor palette clutter. Add a
channel only when a scenario explicitly needs a new *colour* of
behaviour, and record the addition in [`CORE_FEATURES.md`](CORE_FEATURES.md).

The IDs are opaque `u8`. There are **no named molecules** at kernel
level. Species-specific chemistry is *not* a special system — lineages
tune their emitter and sensor genes to the same four channels by
descent, so "dialect A ignores dialect B" emerges from mutation.

## Where the field lives

`ChemField` slots on the chunk in the same shape as
[`ThermalField`](../../crates/wk-field/) and the other existing
fields, but with a per-cell array of `CHEM_TYPE_COUNT` concentrations:

```rust
pub struct ChemField {
    // width * height cells, row-major.
    // Per cell: kg/m^3 concentration per ChemTypeId.
    pub cells: Vec<[f32; CHEM_TYPE_COUNT]>,
    pub width: u32,
    pub height: u32,
}
```

`ChemField` follows the standard field lifecycle — chunk-local storage
with a halo swap, diffusion each due tick post-barrier, source and
sink terms buffered like any other write. `World::dissolved_fields_enabled`
is the precedent: an opt-in flag `World::chem_fields_enabled` follows
the same pattern so scenario tests without chemistry (E1–E17) keep the
constant-zero path.

## Cadence

`Chem[c]` diffuses on its own subsystem clock. Working defaults:

- `run_chem_field` — period 6 ticks, phase 3, post-barrier.
- Grid resolution 1× column stride horizontally, 0.5 m vertically —
  same as `ThermalField`.
- Diffusion coefficient per channel is a `MaterialProps`-adjacent
  constant; water conducts, solids do not.

Emitters buffer add-into-cell writes; sensors read the committed field
in the following tick (one-tick sample delay makes the sample-order
determinism trivial). Both use the same halo the other fields use.

## Sensor and emitter genes

Each `ChemoSensor` or `ChemoEmitter` module carries a small trait
struct on the entity, stored as part of the `Genome` block for that
module slot:

| Gene | Type | Meaning |
|------|------|---------|
| `tuned_type` | `ChemTypeId` | Which channel the module reads / writes. |
| `gain` | `f32` | Response amplitude (sensor: input → neural signal; emitter: neural drive → concentration). |
| `threshold` | `f32` | Deadzone / activation level. Sensor fires only above threshold; emitter releases only above threshold. |

Mutation may retune `tuned_type` (deterministic per-trait, same
mechanic as `Genome::mutate` in
[`crates/wk-agents/src/lib.rs`](../../crates/wk-agents/src/lib.rs))
with a small probability, and jitter `gain` / `threshold` by a
relative sigma the same way existing traits are jittered. Retuning
across all four channels is what produces the "dialect split"
scenario in [`SCENARIOS.md`](SCENARIOS.md).

## Sensor reads

- **Local level** — `Chem[c](x, y)` at the sensor's world cell.
- **Gradient (optional)** — `Chem[c](x+1, y) − Chem[c](x-1, y)` for a
  cheap orientation cue. Second sensor gene `mode: LevelOrGradient`
  chooses. Gradient sense costs the same upkeep but reads two cells.

Sensor output is a scalar wire into the neural graph (see
[`NERVES.md`](NERVES.md)). If `tuned_type` and the field's actual
concentration disagree by more than `threshold`, output is zero.

## Emitter writes

Rate per tick, in kg / m³, buffered into the chem field:

```
release(t) = gain * clamp(neural_drive(t) - threshold, 0, 1)
cost(t)    = release(t) * EMIT_ENERGY_COST_PER_KG
```

Emitter cost is **energy**, not biomass — the module's presence has an
upkeep drawn from `Energy.current` in
[`crates/wk-agents/src/lib.rs`](../../crates/wk-agents/src/lib.rs).
Biomass cost is deferred to a later stage; too many stacked costs at
Set B kills the "you can watch it work" story.

## Mass audit

Chem mass is *not* part of `total_tracked` in the initial phases —
concentrations are treated like humidity, a bookkeeping scalar rather
than a mass ledger. This mirrors how humidity currently sits outside
the water mass audit. If Phase 3 tests reveal we need it in the
ledger, add a `chem_total` bucket then, not now.

## Substrate coupling

Emitters and sensors work in **water and moist soil** (a cell is
"conductive" if `moisture > 0` or `MaterialId::Water` is present).
Fully dry rock cells have no chem field storage — writes go to the
nearest wet cell, reads return zero.

## What is deliberately not here

- Named molecules ("glucose", "nitrate"). Never in Core.
- Per-species chemistry. Comes from descent, not a special table.
- Dozens of channels. Add channels one at a time, in a PR, with a
  scenario that justifies it.
- Chemical mass conservation. Deferred until it matters.
