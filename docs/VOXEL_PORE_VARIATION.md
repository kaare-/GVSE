# Variable permeability and porosity (plan)

**Status:** planned. Nothing in this document is implemented yet. The
prerequisite — coherent porous *lenses* in worldgen — has landed
(`lens_noise` in [`worldgen.rs`](../crates/wk-voxel/src/worldgen.rs)).

## Goal

Every material should carry a **range** for porosity and permeability rather
than a single fixed value, with the actual value varying spatially inside
that range. Example shape (numbers illustrative, not proposals): rock
porosity `0.1..0.9`, permeability `0.3..0.6`.

Why it matters: underground water then prefers the porous, permeable
regions, and because saturation improves conduction
([`seepage_conduct_rate_with`](../crates/wk-voxel/src/rules/head.rs) is
wetness-gated) that preference **reinforces itself**. Flow concentrates into
conduits instead of advancing as a uniform front. It also gives karst
somewhere natural to start dissolving.

## What already exists

| Piece | Where | Note |
|---|---|---|
| Fixed per-material `porosity` / `permeability` | `wk-material/src/lib.rs` (`MaterialProps`) | One value each. |
| World-level per-material overrides | `HydroOverrides` | Global, not spatial. Tab-tunable. |
| Coherent lens noise | `worldgen.rs::lens_noise` | Two-octave value noise; picks *which material* sits where. |
| Capacity lookup | `cell.rs::water_capacity_with(material, hydro)` | No cell / coordinate awareness. |
| Rate lookups | `head.rs::seepage_rate_with`, `seepage_uptake_rate_with`, `seepage_conduct_rate_with` | Same. |

So today variation exists only as *material choice*. Two neighbouring
limestone cells are hydrologically identical.

## Plan

### 1. Material data — ranges

Add to `MaterialProps`:

```rust
pub porosity_range: (f32, f32),
pub permeability_range: (u8, u8),
```

Seed both so **today's fixed value is the midpoint**. Nothing changes
behaviourally until step 4, which keeps steps 1–3 independently reviewable.

### 2. Per-cell value

Add `pore: u8` to `Cell` — position within the material's range, `0..=255`.

- `CellFlags` has **no spare bits**: four low flags (`ACTIVE_HINT`,
  `COMPACTED`, `WATERLOGGED`, `MOBILE_ROCK`) plus a high-nibble rock body
  tag. This needs its own byte.
- `Cell` is `material + sat + flags` today, so an extra `u8` should land in
  existing padding — confirm with `size_of::<Cell>()` before and after.
- `#[serde(default)]` so old saves load at the midpoint (`128`).

**It must be stored, not recomputed.** If capacity can change under a cell
that already holds `sat` — on a seed change, a tuning tweak, or a different
noise call order — that water has nowhere legal to go, and you get silent
creation or loss. Storing it also makes porosity savegame-stable.

### 3. Worldgen fills it

Set `pore` from `lens_noise` with a **different salt** from the material
choice, so a limestone lens can have a wetter core and drier edges rather
than being uniformly permeable. Consider a slight depth trend (compaction
reduces porosity with depth) — cheap and physically right.

Painted / editor cells and `Cell::solid()` default to the midpoint.

### 4. Make the consumers cell-aware

This is the invasive step and the reason the work is not a quick patch.

Current call surface (measured):

- **131** call sites of `water_capacity_with` / `water_capacity`
- **47** call sites of the three `seepage_*_rate_with` functions

Spread across `rules/seepage.rs`, `rules/water_flow.rs`, `rules/gravity.rs`,
`rules/grain.rs`, `failure.rs`, `fungi.rs`, `plant.rs`, `symbiosis.rs`,
`rules/spill.rs`, `audit.rs`, plus tests.

Suggested shape: keep the material-only functions as thin wrappers at the
midpoint (so tests and non-hydro callers need no edit) and add cell-aware
variants used by every pass that moves water:

```rust
pub fn water_capacity_cell(cell: &Cell, hydro: &HydroOverrides) -> u8;
pub fn seepage_rate_cell(cell: &Cell, hydro: &HydroOverrides) -> i32;
```

Most call sites already hold the `Cell`, so the edit is usually mechanical.

### 5. Hazard: mass conservation

**Every pass must agree on a cell's capacity.** If gravity thinks a cell
holds 90 and seepage thinks 20, water is silently created or destroyed. A
partial migration is worse than none: the symptom is a slowly drifting mass
total hours into a run, not a crash.

Guards to run continuously while doing step 4:

```bash
GVSE_MASS_AUDIT=1 cargo test -p wk-voxel --lib
cargo test -p wk-voxel --test mass_audit_smoke
```

See [`VOXEL_WATER.md`](VOXEL_WATER.md) § Mass inventory. `audit.rs` sums pore
sat against capacity, so it must be migrated in the same sweep.

Also check: any place that *writes* `sat` and clamps to capacity. A cell
whose stored `pore` implies a lower cap than its current `sat` (possible for
saves written before step 2) must shed the excess into a neighbour or the
audit will report a loss.

### 6. Acceptance

- `size_of::<Cell>()` unchanged, or the growth is understood and accepted.
- Old saves load and keep their mass total flat.
- A scenario test showing a wetting front **preferring** a high-`pore` lens
  over adjacent low-`pore` rock (this is the whole point — assert the
  preference, not just that water moves).
- Mass audit flat across a long smoke run.
- `perf_profile` seepage cost not materially worse; the lookups are on hot
  paths, so prefer arithmetic over branches and avoid re-reading the cell.

## Follow-on

**Underground karst** has a first cut in
[`apply_karst_dissolution`](../crates/wk-voxel/src/rules/karst.rs):
pore-saturated limestone and stone dissolve slower than a surface
film, and a damp cave void keeps the conduit growing. Rates are still
one number per material (`KarstConfig::pore_scale` /
`stone_scale`). Once per-cell `pore` exists, scale those contacts by
the stored value so conduits widen where the rock is already the
permeable path.
