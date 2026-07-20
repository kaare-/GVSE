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
2. Once: `plan_active` → **`apply_seepage`** → grain fall → **grain repose**
3. Opt-in (demo): **`apply_flow_erosion`** — cascade/head-drop water scours erodible beds/banks and deposits downhill

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

### Grain fall + repose

- **Fall:** Sand / Gravel / Clay / LooseRock sink through Air (any sat). Snow falls through *empty* Air only (floats on water).
- **Repose** (`apply_grain_repose`): supported grains slide diagonally into Air when the drop exceeds `floor(repose_rise_m / SAMPLE_WIDTH_m)`. Sand≈0 (no 1-cell cliffs), LooseRock≥1 (short stairs). Wet grains loosen one step. Snow avalanches on land, not into standing water.
- Ice stays rigid (phase break / thaw only) — not a repose grain and not flow-erodible.

### Flow erosion + deposition (`apply_flow_erosion`)

Opt-in (wired in `wk-voxel-app` after `tick`, Tab → Grain / sediment):

- Only cells with **flow bias** (cascade lip or clear head drop to a neighbor). Still lakes do not scour.
- Targets [`is_flow_erodible`] materials: Sand / Gravel / Clay / LooseRock (`erosion_resistance < 150`). Not Ice / Stone / Snow.
- **Bed scour** under standing water → vacated cell becomes **empty Air** (gravity pulls the column down — no minted water); **bank undercut** → Air (pore sat released).
- Picked grain deposits on a solid-supported Air seat; any free water already in that seat soaks into the grain's pores or is pushed upward — deposit must not delete lake sat.
- Rate scales with `1 - resistance/180` and `GrainConfig.erosion_rate`; wet grains (pore sat) erode faster.

### Cold avalanche + ice load

Demo order after `tick`: thermal step → **`apply_cold_avalanche`** → **`apply_phase`**.

- **Cold avalanche** (Tab → Ice / snow / slush): at/under `freeze_point_c`, wet sand loosens and can smear onto an ice lid; snow may seat on a wet film **over ice** (still refuses open water); hillside ice glaze (on rock/sand, not floating lids) can peel into a diagonal seat.
- **Ice load break**: if Ice has grain or snow directly above and contiguous lid thickness `< ice_carry_thickness` (default 2), the contact ice becomes water. Thicker lids carry the debris. Hillside ice that lands on a lid merges into the sheet (does not count as overburden).

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
- Grain repose: `sand_cliff_slides_diagonally`, `loose_rock_holds_single_step`, `snow_avalanches_off_cliff_but_not_into_water`, `sand_pile_flattens_over_ticks`

## Ice / snow / phase (milestones 1–3)

Module: `wk-voxel::phase` (`apply_phase`, `deposit_precip_on_surface`).
Demo toggle: **`I`**.

Pass order per column: **cull → break unsupported → water-on-ice/slush → thaw → freeze**.

- Uses the coarse **thermal field** (`Temperature`, 4×4 tiles, step every
  20 ticks). **Air** tracks climate day/night; **surface** water/rock use
  high heat capacity (lakes barely cool over a night); **buried** bedrock
  ignores night air, eases toward a geothermal profile, and slowly leaks
  heat upward by diffusion. Snow albedo still shades solar.
- **Freeze:** standing free-surface wet Air (`sat ≥ min_sat_to_freeze`) when
  `temp ≤ freeze_point_c` → whole `Ice` cell (lake skin). **Cold lids then
  thicken downward** one cell / tick into wet Air under Ice/Snow so deep
  ponds and peak “ice castles” freeze through instead of sitting liquid at
  −20 °C under a 1-px skin.
- **Thaw:** top-of-stack Ice/Snow when `temp > freeze_point_c` → `Air+FULL`.
- **Rain on ice:** stays as a water film on top (no density-swap under the
  sheet — that lofted ice into the rain). Melts the ice when warm, or when
  a full water cell has ponded (enough rain).
- **Ice lid × evaporation:** intentional. Evap only runs on wet Air with
  **Air** above it (`dry_above_max`). An Ice/Snow sheet blocks that, so a
  capped lake loses far less mass and the humidity pump dries out — a
  useful cold-climate feedback even before a full thermal field.
- **Unsupported ice:** Ice/Snow with dry air below breaks into water so
  trapped surface water can rejoin the basin.
- **Snow precip:** rain / drizzle / cloud downpour call
  `deposit_precip_on_surface`. Cold **ground** sample (skips snow/ice pack
  height) → one solid `Snow` cell on top of rock/sand/pack (wet surface
  films become Snow). **Never** falls back to liquid on cold columns —
  that used to soak mountain-top sand pores when the pack hit the cap or
  cloud shares were fractional. Short budget / full cap → hold mass (`0`).
- **Snow spread:** new flakes search ±`snow_spread_radius` columns and
  prefer packs ≤ `snow_blanket_depth` so cover blankets cold slopes
  before growing peak spikes. Cloud downpour uses a wider footprint when
  cold. Later: real drift / avalanche physics.
- **Snow pack (now):** static solid lid. No pore soak, no grain fall.
  Later: avalanche / settle physics, and ice/snow scour of sand & loose rock.
- **Slush:** Snow on water — warm melts snow; cold freezes the water film
  under snow into ice (snow-on-ice pack).
- Rate limits: freeze / thaw / slush / break per column per tick.
- **Max Ice+Snow cells / column** — excess culled to empty Air (not melted).

Cold snap: Tab → Base temp below 0°C (with rain/clouds on). Warm snap: raise
base temp above freeze point.

## Related docs

- [`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md) — isolation, dirty rects, historical spill vocabulary
- [`WORLDGEN.md`](WORLDGEN.md) — column-stack water table (separate from this CA)
