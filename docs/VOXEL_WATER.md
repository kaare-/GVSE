# Voxel water (working model)

Canonical notes for free-surface water and pore hydrology in `wk-voxel`.
Runtime entry point: [`tick`](../crates/wk-voxel/src/rules/tick.rs) in `wk-voxel`.
Demo / draw: `wk-voxel-app`.

## Representation

- Free water is **`Air` + `sat`** (`0..=255`). There is no live `MaterialId::Water` cell in the voxel CA.
- Porous solids hold pore water in the same `sat` field, capped by **`World::water_capacity(material)`** (= material porosity for solids, `255` for Air), reading `World.hydro` overrides via `water_capacity_with`.
- **Bedrock / Ice / Snow** have capacity `0` — impermeable to the CA.

## Mass inventory

`wk_voxel::audit::sat_totals(world)` sums **free Air sat** and **pore
sat** over loaded chunks. Humidity + clouds are separate stores —
combine with `tracked_totals(world, humidity, clouds)`.

Physics `tick` should keep `cell_total` flat (closed basin). Opt-in
debug assert: `set_mass_audit_enabled(true)` or `GVSE_MASS_AUDIT=1`
(debug builds only; default off). Long smoke:
`cargo test -p wk-voxel --test mass_audit_smoke`.

Known sinks **outside** tick: bare evaporation, open-loop rain mint,
ice/snow cull, humidity OOB drop. Do not expect `tracked` flat across
those passes unless they are closed-loop.

## Tick pipeline

Each `tick`:

1. **Flow substeps** (×12): `plan_active` → clear dirty → **gravity fall** → **`apply_water_flow`**
2. Once: `plan_active` → **`apply_seepage`** → multi-pass grain settle (fall + repose, up to `GRAIN_SETTLE_PASSES`) → **`apply_roof_collapse`** (geotech F1; Tab → Geotech)
3. Opt-in (demo): **`apply_flow_erosion`** — cascade/head-drop water scours erodible beds/banks and deposits downhill

Dirty rectangles + a 1-cell halo drive the active set. Writes rebuild
dirty for the **next substep**. Important quiescence detail: dirty is
cleared at the *start* of every flow substep; if that substep then sees
an empty plan, the loop exits. A fully settled bed can therefore end the
tick with **no dirty rects left** — `plan_active` is empty going into
the next tick. That is intentional (idle water should not rescan the
world), not a missed wake. Setup / rain / editor writes must dirty
cells so the following tick re-enters the loop.

Interactive default (`PerfConfig::flow_quiet_early_out`, **on**) caps
the loop at `FLOW_SUBSTEPS_MIN` (8) and may stop after
`FLOW_SUBSTEPS_EO_AFTER` (4) when the dirty halo is empty or at most
`FLOW_QUIET_AREA` (512 cells). Do **not** early-out just because a large
halo shrank — that stuttered streams (a few hops, then a tick of rest)
while still letting quiet ponds and open basins park. `full_feel()`
keeps the full ×12 with no early-out. Underground seepage stays
cadence-gated (`SEEPAGE_EVERY` = 4 ticks) — every-other-tick smeared
stone wetting fronts instead of advancing them.

Thin-film hops stay soft (`sheet` 48 / `drain` 96 sat per pass at stack
depth 0–1) so runnels don't spike empty; stacked hillside water still
dumps hard (240–255).

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
4. **Throughflow** — weep through a saturated porous stack (≤24 deep) at seepage rate into the **nearest opening**: a side Air face (cliff / spring) or Air below the stack.
5. **Confined upward head** — Air-with-room sitting on a **full** wet-Air cell pulls from the connected free-surface donor when that body's max `hydraulic_head` exceeds the receiver. Pressure walks through full wet Air only (bedrock pipes / communicating vessels). Mass leaves the high reservoir surface so the pipe stays full and gravity cannot undo the rise. A **higher-row** donor always qualifies (1-wide or 2-wide shafts); same-row finish still requires a fully walled column. Open lakes stay with same-Y equalise. Deep oceans use **column climb** plus a periodic **full-chunk** wake (`wake_confined_head` — not the dirty halo, so ocean evaporation cannot starve a quiet shaft). The wake visits chunks with solid **and** standing water (`has_solid && has_standing_air`), then only the standing-air y band plus one row for the rising film. Dry sky in shore chunks is leftover. Unset bands (old saves) still scan the full rect. Rain-film sky, mid-ocean water with no rock, and groundwater-only crust already reject per-cell. Uncased drizzle on wet ground skips before the free-surface / BFS probes — seepage owns that infiltration; a walled well still rises. Casing is **rock / bedrock** (`is_competent_rock` or Bedrock). Plants, litter, ice, and loose grains do not case a well — a vegetated or sandy film is open ground and must not BFS the aquifer. Evap refreshes `has_standing_air` and tightens the y band so a drained lake drops out.

`apply_lateral_spill` remains as a narrower Air–Air half-gap helper for unit tests; **`tick` does not call it**.

### Seepage (`apply_seepage`)

Head-driven, permeability-capped soak on **cardinal** edges (`+x`, `+y` owned once per edge):

| Pair | Allowed |
|------|---------|
| Air ↔ porous solid | yes |
| porous ↔ porous | yes (rate = min of both) |
| Air ↔ Air | no (surface flow owns that) |

Rate: `((permeability * 32) / 255).max(1)` when permeability &gt; 0, else 0.
Each solid cell stores `pore: u8`, selecting both permeability and
porosity inside its material ranges. Worldgen fills it with coherent
noise plus mild depth compaction; editor / constructed cells use the
midpoint. See [`VOXEL_PORE_VARIATION.md`](VOXEL_PORE_VARIATION.md).
Fully saturated solid→Air faces get a ×3 spring boost (capped at 16) so cliff pores weep visibly.

This is what wets a dry beach **sideways** from a puddle, equalises pore sat between sand and clay/stone, and lets saturated hillsides drip into open Air. Vertical fill under a lake is dominated by **gravity**, not seepage.

### Grain fall + repose

- **Settle:** After seepage, every tick runs `wake_unsupported_grains` + `wake_unstable_slopes` then multi-pass fall and multi-pass repose (up to `GRAIN_SETTLE_PASSES`), then litter-centric `rise_buoyant_litter` + `soak_floating_litter` (only Snow/Ice/Organic cells — not a full-grid × height scan). Fall alone left Organic/sand as vertical cliff faces; repose now keeps avalanching until the pile is flat (max_step ≈ 0). Sand **and Soil** may repose through thin atmospheric haze (`sat ≤ GRAIN_REPOSE_HAZE_MAX`), walk sideways off ledges into open air, **and** avalanche into standing lake water (`sat ≥ GRAIN_REPOSE_LAKE_MIN`) so submerged banks are not frozen cliffs. Mid shore film (`HAZE_MAX+1 .. LAKE_MIN-1`) still blocks sand (fleck cycle); Soil may still sprawl through land mid-film so humid cliffs do not freeze. Repose uses a Moore-neighbour chunk ptr map (serial) so slides across chunk seams actually write — a pull-only `cy+1` map used to silently no-op one face of large F3 blobs. **Snow / Ice / Organic** float only on **grounded** full standing water; suspended mid-air full-sat is not a seat. Submerged buoyant litter rises through the column; floating Organic soaks from deeper lake water (surface stays full).
- **Repose** (`apply_grain_repose`): supported grains slide diagonally into Air when the drop exceeds `floor(repose_rise_m / SAMPLE_WIDTH_m)`. Sand≈0 (no 1-cell cliffs), Organic litter / Soil≈0 (sprawl instead of towers), LooseRock / LooseLimestone≥1 (short stairs). Wet grains (except Clay) loosen one step. **Clay** is pore-wetness gated: dry powder ≈ sand (max_step 0), semi-wet plastic holds steeper faces (max_step 2), near-saturated mud flows again (max_step 0). Dense grains (**including Soil**) and **submerged / waterlogged Organic** treat standing lake water as avalancheable relief (gentler UW banks); sand mid shore film stays refused. **Surface Organic** (rafts / beach litter with open air above) sprawls through land haze/film and floats on full standing water — it refuses lake / underwater film seats (no surface crawl into the lake). Snow avalanches on land, not into standing water. Underwater, dense grains sliding into lake water swap the seat (vacated cell stays wet); collapsing into empty/haze bubbles steals neighbour standing water (no sky-flash on the slope face).
- **Fall:** Sand / Gravel / Clay / Soil / LooseRock / LooseLimestone sink through Air (any sat). **Snow, Ice, and Organic** fall through empty Air *and* haze; they float only on **full** standing water (`sat == 255`) so unsupported pack does not hang mid-air and phase cannot melt→refreeze a misty seat into a ±1-cell pump. Full water seats also **pull submerged** Snow/Ice/Organic upward (buoyancy) so a refilled lake surface cannot trap a “glitch line” of litter below a floating raft. Float “grounded column” walks treat partial-sat water and missing lower chunks as still bedded (so soak drawdown / checkerboard halos cannot make Organic freefall through the ocean). Grain settle runs fall on the full active set (not checkerboard) for the same reason. **Dense grains punch through floating litter rafts** (Organic/Snow/Ice on water cannot carry Soil/Sand/LooseRock piles — cargo swaps down through the raft then sinks). **Wind / stream drift** (`drift_floating_organic` / `sail_plants_on_wind_rafts`) shoves floating Organic sideways from local climate wind **and** local stream push (per column / per bound raft — still-lake mats must not zero out river current). Taller piles and living plant sails raise the chance. Loose litter may tear apart or wash over cascade lips; **living roots bind** the plant’s full root-span of columns into one raft so trees sail with the mat (dispersal). Only plants with a holdfast in/on floating Organic translate — submerged or free plants are not hitchhiked when litter slides past. Destination must stay on a float seat with freeboard Air (never into the water column), except thin unbound film may wash onto an empty lip when current is strong. **Soak → waterlog → sink:** floating Organic fills pores from the lake (`soak_floating_litter`); once saturated a slow counter ([`CellFlags::WATERLOGGED`]) eventually lets the mat sink through standing water instead of floating forever.
- Ice is not a repose grain and not flow-erodible; hillside glaze can still peel in the cold-avalanche pass.

### Flow erosion + deposition (`apply_flow_erosion`)

Opt-in (wired in `wk-voxel-app` after `tick`, Tab → Grain / sediment):

- Only cells with **flow bias** (cascade lip or clear head drop to a neighbor). Still lakes do not scour.
- Chunk filter is `has_standing_air` when `min_flow_sat >= STANDING_AIR_SAT` (the default 180). Rain-wet land (`has_wet_air` only) is leftover — drizzle cannot scour. A lower custom `min_flow_sat` keeps the wet-air net.
- Targets dense [`is_flow_erodible`] grains: Sand / Gravel / Clay / **Soil** / LooseRock / LooseLimestone (`erosion_resistance < 150`). Not Ice / Stone / Snow.
- **Organic** is contextual: grounded beach litter and waterlogged/sunk mats scour under flow (deposits stay `WATERLOGGED` so they remain bedload). Thick / mycelium-bound floating rafts stay wind-owned. Thin unbound floating film is current-owned: deterministic drift under `flow_bias`, cascade-lip wash, scour, and a post-drift [`shove_floating_organic_with_current`] so mats do not dam free surfaces into comb teeth. Same-Y cascade pull looks past soft floating Organic lids. **Standing water washes through Organic spans** into Air beyond (Organic is a sponge, not a masonry dam). Mycelium felt does not hold vertical cliffs when Organic is wash-wet next to standing water.
- **Bed scour** under standing water → vacated cell becomes **empty Air** (gravity pulls the column down — no minted water); **bank undercut** → Air (pore sat released).
- Picked grain deposits on a solid-supported Air seat; any free water already in that seat soaks into the grain's pores or is pushed upward — deposit must not delete lake sat.
- Rate scales with `1 - resistance/180` and `GrainConfig.erosion_rate`; wet grains (pore sat) erode faster.

### Cold avalanche + ice load

Demo order after `tick`: thermal step → **`apply_cold_avalanche`** → **`apply_phase`**.

- **Cold avalanche** (Tab → Ice / snow / slush): at/under `freeze_point_c`, wet sand loosens and can smear onto an ice lid; snow may seat on a wet film **over ice** (still refuses open water); hillside ice glaze (on rock/sand, not floating lids) can peel into a diagonal seat.
- **Ice load break**: if Ice has grain or snow directly above and contiguous lid thickness `< ice_carry_thickness` (default 2), the contact ice becomes water. Thicker lids carry the debris. Hillside ice that lands on a lid merges into the sheet (does not count as overburden).

## Material hydrology (defaults)

| Material | porosity range | permeability range | seepage rate range / pass |
|----------|----------------:|-------------------:|--------------------------:|
| Sand | 83–137 | 72–120 | 9–15 |
| Gravel | 68–112 | 120–200 | 15–25 |
| Organic | 150–250 | 90–150 | 11–18 |
| Soil | 75–125 | 36–60 | 4–7 |
| Limestone | 30–50 | 105–175 | 13–21 |
| LooseRock | 19–31 | 30–50 | 3–6 |
| LooseLimestone | 23–37 | 38–62 | 4–7 |
| Clay | 45–75 | 6–14 | 1 |
| Stone | 15–25 | 1–9 | 1 |
| Bedrock | 0 | 0 | 0 |

Tab → **Material permeability / porosity** exposes min/max values and
writes ranges into `World.hydro` (`HydroOverrides`). Physics samples
them through `water_capacity_cell` / `permeability_cell`. Setting a
range to **0–0** makes that property zero for every cell; sand
porosity and permeability both at 0–0 make an impermeable lid.

## The tight end of the permeability spectrum

`seepage_rate_cell` is `((p * 32) / 255).max(1)`, so **every material below
permeability 8 floors to the same 1 sat-unit per pass**. Clay (10), flowstone
(12) and bentonite (1) all conducted identically. This quantised the whole
tight end of the spectrum into one value, and it is why:

- a real aquitard could not be expressed, and so there was no confined head to
  find with a well;
- pore variation on tight rock did nothing, defeating the point of the fracture
  tail (its whole range sits under the quantum);
- the 1/√2 diagonal path weighting had no effect — `(1 * 181) / 256` floors
  back to 1.

`seepage_stride_cell` recovers the resolution without fractional sat: below the
quantum a cell transfers one unit every `quantum / p` passes, so permeability 1
runs at exactly 1/8 of permeability 8. Phase comes from cell position so a seal
does not pulse in lockstep. Stride values (2, 4, 8) are coprime with the
seepage cadence (5), so the gate samples every residue and the ratio holds.

**Watch for this whenever tuning anything at the tight end.** A permeability
change below 8 does nothing on its own; it only moves behaviour through the
stride.

## Bentonite: the aquitard

Clay is not a seal. At permeability 10 against limestone's 140 it is only ~14×
tighter, which still equalises over a geological cadence. `Bentonite`
(permeability 1, field capacity 232, insoluble) is, and confinement is what
makes artesian head and perched tables happen by design rather than by
accident.

Worldgen lays it as a 2-cell cap just above the limestone stratum, and
deliberately **not** continuous (`lens >= 0.15`, ~85% coverage). A perfect seal
would also block recharge and the aquifer beneath would never fill; real
confined aquifers take their water where the aquitard is absent, and the gaps
are those windows.

Confinement is a *timescale* property, not an endpoint — given long enough,
water equalises through anything with non-zero permeability, and real confining
layers are leaky. Measured on a sealed reservoir, clay fully drains by 2000
ticks while bentonite passes nothing through 3000.

## Diagonal faces open only along veins

The seepage stencil is four-neighbour, so water in a **diagonal** vein had to
zigzag through the two corner cells between it. The vein was throttled to
*their* permeability rather than its own, and only grid-aligned veins could
conduct — channels could form along the world axes and nowhere else.

Adding diagonal faces everywhere is not the fix: in homogeneous rock it gives
every cell eight faces instead of four and roughly doubles vertical drainage.
That is a global retune of the water model, and it pulls the wrong way — water
should linger in permeable layers, not reach bedrock faster. Two seam tests
caught it at once, both asserting a bed wets and both failing because the water
had drained away.

So `diagonal_is_a_shortcut` opens the face only where the anisotropy bites:
both ends more permeable than either corner. In homogeneous material the
corners match the ends, nothing opens, and tuned behaviour is untouched.
Diagonals carry pore↔pore conduction only; infiltration and weep keep their
orthogonal faces, since those rules are tuned against a free surface.

Cost is roughly +0.3 ms/tick (demo) and +0.5 (stress) amortised over the
cadence.

## Seam coupling scans only wet seams

`apply_seepage_seam_coupling` exists because dirty rects are per chunk and do
not cross the `cy` boundary, so pore water would shelf at y=63|64.
`seam_seepage_regions` used to emit a 20-row full-width band for **every**
chunk pair unconditionally, which made it the most expensive pass in the
simulation: 7.4 ms/call on demo, 17.2 ms on stress, ~85% of the seepage bucket
while every other seepage component measured 1–3 ms.

It now gates each seam on the sticky `has_wet_pores` / `has_wet_air` chunk
flags and narrows the band to the local `x` span of columns where water could
cross. The seepage accumulate walk itself skips a region whose chunk has
neither flag (dry rock / empty sky still land in the flow halo from gravity
and body writes). Bootstrap — no flags set yet — keeps every region so a
legacy save cannot skip a wet chunk. Air cells whose +x / +y faces are
also Air skip the neighbour loop — a 64×64 pond interior cannot infiltrate
or weep, and diagonals are pore-only. The span predicate is deliberately looser than the per-column test in
`wake_vertical_chunk_seam_pores` — both sides able to hold or pass water, one
of them having some — so it cannot exclude a column the wake would couple.
Result: 0.45 ms demo, 0.83 ms stress.

The lake-bed and seam pore wakes were also being called twice per tick: once
under the seepage cadence, and again unguarded just before the seepage plan.
Only the later call site matters (its dirty is what the seepage plan consumes),
so the early copy is gone and the wakes ride the cadence as intended.

`wake_lake_bed_pores` further skips rain-film sky and a quiet saturated
water table. It walks chunks with standing water or an unsaturated pore
front (`has_standing_air || has_unsaturated_pores`) and refreshes both
flags from the same scan. Standing water still walks down into the bed
in the chunk below, so a full table does not need its own 64×64.
Mid-ocean / lake interiors (standing water on more water) peek
chunk-local and skip the pore walk. Standing-only chunks (no
unsaturated front) then scan only the standing-air y band — dry sky
in shore / mid-ocean surface chunks is leftover.

`wake_pore_weep_into_air` on buried crust (`!has_open_air`) only checks
the chunk perimeter — an interior pore cannot face Air. Neighbour reads
are chunk-local when they stay inside the 64×64. Digging a cavity writes
Air and raises `has_open_air`, so the next weep is a full scan again.

Deep / seam seepage skips pore↔pore faces that are **both at capacity**
before the fire-odds roll and head math. Those cannot transfer; Air faces
(infiltration / weep) still run.

Use `tests/seepage_split_probe.rs` before tuning any of this. The profiler
lumps five calls into one `seepage` bucket and the bucket cannot say which.

## What “working” looks like

- Rain / cascade on impermeable sand: films drain off shelves into lower pools; lake tops level via same-Y equalise.
- Trickle on default sand: sand saturates toward porosity, then free water runs over the wet cap.
- Lake bed: sand fills, then clay/stone/gravel/limestone under the cap fill to **their** capacity. Stone at `sat=20` with porosity 20 is **fully saturated**, not “barely wet”.
- Inspector shows `sat/{capacity}` and porosity/permeability so pore fill is not confused with `/255`.

### Karst (`apply_karst_dissolution`)

Demo toggle: **`K`**. Period default 32 ticks (geology, not every frame).

- **Surface** — limestone with a 4-connected wet Air neighbour
  (`sat >= min_wet_neighbour_sat`, default 200). Probability
  `prob_per_wet_neighbour × wet_count`. Unchanged cliff-face rule.
- **Underground** — limestone *and* stone, much slower. A cell
  dissolves when it is itself near-saturated (`sat / capacity >=
  min / 255`), when a porous neighbour is, or when a *roofed* damp
  cave Air cell (`0 < sat < min`, solid immediately above) sits next
  to it. Open-sky drizzle does not count. Contact weight is scaled
  by `pore_scale` (default 0.2 vs a surface wet-Air face) and again
  by `stone_scale` (default 0.125) for stone. Dry stone next to a
  lake film does **not** count — that stays mechanical flow erosion.
- Dissolved cells become Air and keep their pore sat (mass conserved).
  That leftover sat is usually below the surface threshold, so it
  feeds the underground path and lets conduits enlarge where water
  already is.
- Chunks without `has_soluble` are skipped. That flag is raised on
  limestone / flowstone / sandstone / conglomerate writes and cleared
  when a scan finds none left. Rain-soaked sand / soil used to enter
  via `has_wet_pores` and pay a full-chunk walk with nothing to
  dissolve. Per-cell porosity/permeability ranges
  ([`VOXEL_PORE_VARIATION.md`](VOXEL_PORE_VARIATION.md)) can later
  scale the same contact weight.

## Draw notes (`wk-voxel-app`)

- Standing / ocean wet Air paints at any `sat ≥ 1`.
- Palette floors tiny fills at faint blue-white `#B8D4EE`, then ramps toward lake blue.
- Mid-air sat stays invisible (cosmetic rain streaks come from clouds).

## Tests to keep green

- `same_y_equalize_flattens_stepped_lake_surface`
- `communicating_vessels_bedrock_l_pipe_equalizes`
- `confined_head_rises_in_two_wide_shaft`
- `confined_head_wake_scans_despite_unrelated_dirty`
- `confined_head_equalizes_across_large_deep_ocean`
- `closed_basin_lake_does_not_fountain_upward`
- `drizzle_film_on_wet_ground_does_not_confined_rise`
- `plant_sided_film_on_wet_sand_does_not_confined_rise`
- `sand_sided_film_on_wet_sand_does_not_confined_rise`
- `a_well_bottomed_in_a_confined_aquifer_rises`
- `solid_staircase_film_drains_left_into_lower_pool`
- `lake_bed_sand_wets_clay_and_stone_below_via_tick`
- `deep_stone_stack_keeps_wetting_after_surface_quiesces`
- `stamped_lake_bed_pores_wet_under_water`
- Karst: `wet_limestone_eventually_dissolves`, `saturated_buried_limestone_dissolves_without_air`, `saturated_buried_stone_dissolves_without_air`, `damp_cave_void_seeds_further_limestone_dissolve`, `karst_ignores_non_limestone_solids`
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
- **Freeze:** standing free-surface wet Air (`sat ≥ min_sat_to_freeze`,
  default **255** / full cell) when `temp ≤ freeze_point_c` → whole `Ice`
  cell (lake skin). Partial films must not freeze — thaw always yields a
  full water cell, so freezing mist would mint mass. **Cold lids then
  thicken downward** one cell / tick into **full** wet Air under Ice/Snow.
  Open-surface freeze is skipped when the column already has Ice/Snow
  below — prevents a second skin above a fallen/submerged flake (shore
  “float up” after break/fall).
- **Thaw:** top-of-stack Ice/Snow when `temp > freeze_point_c` → `Air+FULL`.
- **Rain on ice:** stays as a water film on top (no density-swap under the
  sheet — that lofted ice into the rain). Melts the ice when **warm** only
  (cold ponded rain no longer melts sheets — that churned ice towers).
- **Ice lid × evaporation:** intentional. Evap only runs on wet Air with
  **Air** above it (`dry_above_max`). An Ice/Snow sheet blocks that, so a
  capped lake loses far less mass and the humidity pump dries out — a
  useful cold-climate feedback even before a full thermal field.
- **Unsupported ice/snow:** empty Air below → **fall** as solids in
  `apply_grain_fall` (float on full water; drop through empty/haze Air).
  Phase break does **not** melt packs over empty or haze — fall owns those
  seats (melting haze used to fight freeze and pump flakes at the surface).
- **Snow precip:** climatic rain and condensation call
  `deposit_precip_on_surface`. **Air temp at precip origin** (`start_y` /
  cloud height) chooses flake vs drop. Snow that hits **warm ground**
  melts to liquid; cold air + cold ground → solid `Snow` pack (never
  pore-soaks). Warm air always rains (ponds may freeze later via phase).
  Solid seats cost a **full cell** (`min_budget_to_snow` default 255) —
  thaw always yields `Air+FULL`, so a 64-sat droplet must not mint a
  Snow cell. Short budget / full blanket → hold mass (`0`).
- **Condensation drizzle (`C`):** warm → liquid film; cold air on cold
  ground → thin `Ice` frost / rime (≤ `frost_coat_depth`, default 1;
  lateral `frost_spread_radius`, default 3), also paid as a full cell
  from the humidity tile. Never places `Snow` packs or ice towers —
  climatic rain (`W`) owns packed snow. Tab exposes both frost knobs.
- **Climatic rain (`W`):** **closed-loop by default** — deposits only what
  humidity can pay, and refuses columns already flooded above
  `sea_level + max_flood_above_sea`. Tab can reopen the legacy open
  faucet (`closed_loop` off) for experiments; prefer condensation (`C`)
  for weather. Atmosphere also has a soft per-tile cap
  (`Humidity::MAX_MASS_PER_TILE`). There is no cartoon cloud store.
  `CloudStore` only returns leftover save-file parcel mass to humidity.
  Evap also refuses a near-saturated vapor column
  (`Humidity::column_near_saturated`) and stops entirely when
  `Humidity::atmosphere_overfull` — buoyant rise used to empty the
  surface tile so the per-tile cap never tripped, and a multi-million
  tick soak filled the whole sky grid. Condensation then walked every
  wet column (demo `max_prob_per_tick = 0.10`) and collapsed to ~7 FPS.
  Drizzle is now capped at `CondensationConfig::max_events_per_tick`
  (default 48; `0` = unlimited).
  Rain / condensation will **not** stack a full one-cell film on a
  hillside that can still spread or cascade (wedge guard). A closed
  basin of full films (dry lake bed) *does* pond — otherwise a long
  soak rains forever while lakes stay empty, because condensation
  cannot land once every column wears a full film.
  Surface deposit walks up to 512 cells down from the precip origin
  (demo sky is 320 tall). A shorter 128-cell walk left ceiling
  condensation unable to reach sea-level lakes — looking at the ground
  only made evaporation win faster (higher FPS), which felt like a
  viewport cull.
- **Cloud / humidity floor:** `cloud_floor_y` clips the `H` haze to the
  occupied column top. It starts at the worldgen surface ±64, then climbs
  while the column is still rock / ice / wet so editor towers above that
  band (inland ~y 263) still bump the field instead of letting it pass
  through the stone. Damp air is not a floor.
- **Physics-leaning weather (still coarse tiles):** evap rate scales
  with surface temperature, wind, and local humidity deficit (same
  wet-chunk scan). Vapor rises harder when the lapse is unstable
  (warm under cold). Condensation prefers cold / supersaturated tiles
  and dew when a colder tile sits below. Leftover parcel mass from old
  saves returns to humidity. Event caps and the atmosphere budget stay
  in place so this cannot refill the 7 FPS soak.
- **Snow spread:** new flakes search ±`snow_spread_radius` columns and
  only seat where pack ≤ `snow_blanket_depth`. No slow spike growth past
  the blanket. Climatic rain uses a wider footprint when snowing.
- **Snow pack:** solid lid, no pore soak. Falls through empty Air; cold
  avalanche can spill onto lake ice (see Grain / cold avalanche sections).
- **Slush:** Snow on water — warm melts snow; cold freezes the water film
  under snow into ice (snow-on-ice pack).
- Rate limits: freeze / thaw / slush / break per column per tick.
- **Max Ice+Snow cells / column** — excess culled to empty Air (not melted).

Cold snap: Tab → Base temp below 0°C (with rain/clouds on). Warm snap: raise
base temp above freeze point.

## Organisms (brief)

- **Set A Atom** — wet-Air plankton (buoyancy / fission).
- **Set D minimal plant** — `Root` + `Stem` + leaves on moist porous
  ground; drinks cell `sat`, no buoyancy. Editor: F2 → `T` template,
  spawn on Air above sand/soil. Full canopy shade / elongation still
  deferred (`docs/organism/PLANTS.md`).

## Related docs

- [`VOXEL_MIGRATION.md`](VOXEL_MIGRATION.md) — isolation, dirty rects, historical spill vocabulary
- [`WORLDGEN.md`](WORLDGEN.md) — column-stack water table (separate from this CA)
