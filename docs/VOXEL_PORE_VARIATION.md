# Variable permeability and porosity

**Status:** implemented. Cells store a coherent `pore: u8`; all live
voxel hydrology samples per-material porosity / permeability ranges.
Old `.gvsesim` snapshots are intentionally rejected at schema v13 —
F5 writes a fresh snapshot.

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

## Implementation

| Piece | Where | Note |
|---|---|---|
| Material ranges | `wk-material::MaterialHydrology` / `HydroRange` | Defaults are ±25% around the old fixed value (minimum width 4); zero stays `0..0`. |
| Per-cell selector | `Cell::pore` | `0` selects both minima; `255` both maxima; constructors use `128`. |
| World-level overrides | `HydroOverrides` | Tab sets min/max per material. `0..0` seals it. |
| Coherent generation | `worldgen.rs::pore_coordinate` | Independent broad/fine noise plus mild depth compaction. |
| Capacity | `cell.rs::water_capacity_cell` | Authoritative cell-aware lookup for movement, clamps and audit. |
| Rates | `head.rs::*_cell` | Seepage samples cell permeability; material-only wrappers remain midpoint helpers for tests/legacy. |

The one stored coordinate intentionally correlates porosity and
permeability: open fabric both holds and conducts more water. Material
choice uses separate noise, so a limestone body has internal texture.

### 1. Material data — ranges

Ranges are kept beside the legacy `MaterialProps` scalar table:

```rust
pub struct MaterialHydrology {
    pub porosity: HydroRange,
    pub permeability: HydroRange,
}
```

The legacy scalar remains the midpoint for the archived column stack.

### 2. Per-cell value

`pore: u8` is the position within the material's range, `0..=255`.

- `CellFlags` has **no spare bits**: four low flags (`ACTIVE_HINT`,
  `COMPACTED`, `WATERLOGGED`, `MOBILE_ROCK`) plus a high-nibble rock body
  tag. This needs its own byte.
- `_pad` already stores mycelium, so `Cell` grows from 4 to 5 bytes
  (16 KiB → 20 KiB cell slab per 64×64 chunk).
- Save schema v13 rejects old snapshots; there is no migration.

**It must be stored, not recomputed.** If capacity can change under a cell
that already holds `sat` — on a seed change, a tuning tweak, or a different
noise call order — that water has nowhere legal to go, and you get silent
creation or loss. Storing it also makes porosity savegame-stable.

### 3. Worldgen fills it

Worldgen sets `pore` from `lens_noise` with a **different salt** from the material
choice, so a limestone lens can have a wetter core and drier edges rather
than being uniformly permeable. Consider a slight depth trend (compaction
reduces porosity with depth) — cheap and physically right.

Painted / editor cells and `Cell::solid()` default to the midpoint.

### 4. Cell-aware consumers

Water movement, saturation clamps, wetness decisions and the mass
inventory use cell-aware helpers:

```rust
pub fn water_capacity_cell(cell: Cell, hydro: &HydroOverrides) -> u8;
pub fn seepage_rate_cell(cell: Cell, hydro: &HydroOverrides) -> i32;
```

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

Changing a range does not delete existing water. A temporarily
over-capacity cell drains through normal rules; audit continues counting
all sat.

### 6. Acceptance

- `Cell` growth to 5 bytes is accepted.
- Old saves are rejected; F5 overwrites with v13.
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
one scale per material (`KarstConfig::pore_scale` / `stone_scale`).
Water now reaches high-pore cells first through cell-aware seepage, so
dissolution already follows preferential paths; a future chemistry
field can additionally scale reaction rate by local permeability.
