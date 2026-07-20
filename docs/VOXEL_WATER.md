# Voxel water (working model)

Canonical notes for free-surface water and pore hydrology in `wk-voxel`.
Runtime entry point: [`tick`](../crates/wk-voxel/src/rules.rs) in `wk-voxel`.
Demo / draw: `wk-voxel-app`.

## Representation

- Free water is **`Air` + `sat`** (`0..=255`). There is no live `MaterialId::Water` cell in the voxel CA.
- Porous solids hold pore water in the same `sat` field, capped by **`water_capacity(material)`** (= material porosity for solids, `255` for Air).
- **Bedrock / Ice / Snow** have capacity `0` — impermeable to the CA.

## Tick pipeline

Each `tick`:

1. **Flow substeps** (×12): `plan_active` → clear dirty → **gravity fall** → **`apply_water_flow`**
2. Once: `plan_active` → **`apply_seepage`** → grain fall

Dirty rectangles + a 1-cell halo drive the active set. Writes rebuild dirty for the next substep.

### Gravity fall

Bottom-up **pull**: a cell with free capacity takes sat from the cell directly above (any material with capacity &gt; 0).  
Not permeability-limited — this is how a lake bed fills sand, then clay/stone underneath, one cell per pass.

### Surface flow (`apply_water_flow`)

Per wet Air cell (compute-then-apply, mass-conserving):

1. **Diagonal-down** into Air with room — dump.
2. **Cascade edge** — side Air whose below is Air with room — dump.
3. **Same-Y surface equalise**
   - Scan up to 12 standing cells for a cascade outlet; push toward it.
   - Pairwise head-equalise each `+x` standing edge (avoids checkerboard terraces on wide lakes).
4. **Throughflow** — weep through a saturated porous stack (≤24 deep) at seepage rate into Air beyond.

`apply_lateral_spill` remains as a narrower Air–Air half-gap helper for unit tests; **`tick` does not call it**.

### Seepage (`apply_seepage`)

Head-driven, permeability-capped soak on **cardinal** edges (`+x`, `+y` owned once per edge):

| Pair | Allowed |
|------|---------|
| Air ↔ porous solid | yes |
| porous ↔ porous | yes (rate = min of both) |
| Air ↔ Air | no (surface flow owns that) |

Rate: `((permeability * 32) / 255).max(1)` when permeability &gt; 0, else 0.

This is what wets a dry beach **sideways** from a puddle, and slowly equalises pore sat between sand and clay/stone. Vertical fill under a lake is dominated by **gravity**, not seepage.

## Material hydrology (defaults)

| Material | porosity (capacity) | permeability | seepage rate / pass |
|----------|--------------------:|-------------:|--------------------:|
| Sand | 180 | 160 | 20 |
| Gravel | 120 | 240 | 30 |
| Organic | 200 | 120 | 15 |
| Limestone | 40 | 140 | 17 |
| Clay | 60 | 10 | 1 |
| LooseRock | 25 | 40 | 5 |
| Stone | 20 | 5 | 1 |
| Bedrock | 0 | 0 | 0 |

Tab → **Material permeability / porosity** overrides these at runtime (`MaterialRegistry` hydro overrides). Setting sand porosity **and** permeability to 0 makes the sand cap an impermeable lid — pore water will not enter the body below.

## What “working” looks like

- Rain / cascade on impermeable sand: films drain off shelves into lower pools; lake tops level via same-Y equalise.
- Trickle on default sand: sand saturates toward porosity, then free water runs over the wet cap.
- Lake bed: sand fills, then clay/stone/gravel/limestone under the cap fill to **their** capacity. Stone at `sat=20` with porosity 20 is **fully saturated**, not “barely wet”.
- Inspector shows `sat/{capacity}` and porosity/permeability so pore fill is not confused with `/255`.

## Draw notes (`wk-voxel-app`)

- Standing / ocean wet Air paints at any `sat ≥ 1`.
- Palette floors tiny fills at faint blue-white `#B8D4EE`, then ramps toward lake blue.
- Mid-air sat stays invisible (cosmetic rain streaks come from clouds).

## Tests to keep green

- `same_y_equalize_flattens_stepped_lake_surface`
- `solid_staircase_film_drains_left_into_lower_pool`
- `lake_bed_sand_wets_clay_and_stone_below_via_tick`
- `deep_stone_stack_keeps_wetting_after_surface_quiesces`
- `stamped_lake_bed_pores_wet_under_water`
- Shore / cascade suite (`impermeable_shore_*`, `continuous_rain_on_*`)

## Ice / phase (milestones 1–2)

Module: `wk-voxel::phase` (`apply_phase`). Demo toggle: **`I`**.

Pass order per column: **cull → settle → thaw → freeze**.

- Uses the existing coarse `Temperature` field (not a per-cell heat sim).
- **Freeze:** standing free-surface wet Air (`sat ≥ min_sat_to_freeze`) when
  `temp ≤ freeze_point_c` → whole `Ice` cell.
- **Thaw:** top-of-stack Ice/Snow when `temp > freeze_point_c` → `Air+FULL`.
- **Settle:** wet Air above Ice/Snow swaps so liquid sinks and ice floats
  (blocks the column ice-pump / tower from rain-on-ice).
- Rate limits: 1 freeze and 1 thaw per column per tick by default.
- **Max Ice+Snow cells / column** — excess culled to empty Air (not melted).

Cold snap: Tab → Base temp below 0°C. Warm snap: raise base temp above freeze point.

Next: snow precip / slush (snow on water cools / melts with hard caps).

## Related docs

- [`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md) — isolation, dirty rects, historical spill vocabulary
- [`WORLDGEN.md`](WORLDGEN.md) — column-stack water table (separate from this CA)
