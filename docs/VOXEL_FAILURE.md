# Voxel shear & compressive failure — implementation plan

*Actionable plan. Concept background lives in
[`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) §6. Water CA:
[`VOXEL_WATER.md`](VOXEL_WATER.md). Column roof spirit:
[`BURROWS.md`](BURROWS.md).*

## Goal

Readable side-view **geotech** without FEM:

1. **Compression** — roofs / overhangs collapse when span exceeds
   material capacity; soft stacks can compact and squeeze pore water.
2. **Shear** — wet / tall faces that repose alone cannot touch eventually
   loosen or block-fail so grain repose can finish.

**Invariant:** cells own mass. Any “stress” number is derived or a rate
gate — never a second water or rock store.

## Material props we will use (already in `wk-material`)

| Prop | Role |
|------|------|
| `roof_span_max_m` | Max unsupported horizontal Air span a roof can hold |
| `cohesion` | Shear strength bonus; wetness reduces effective c′ |
| `repose_rise_m` | φ proxy (already drives grain repose) |
| `density` | Overburden contribution per cell |
| `porosity` / `sat` | Wetness → u and wet cohesion loss |
| `erosion_resistance` | Optional: hard rock resists block-fail |

Rough voxel mapping: `SAMPLE_WIDTH_M = 0.25` → span in cells ≈
`roof_span_max_m / 0.25` (Stone ~60 cells, Limestone ~40, Sand/Clay 0).

## Tick placement

New passes run **once per world tick**, after water + grain, before
(or just after) phase — not inside flow ×12:

```
tick_with_perf:
  flow ×12 (gravity + water flow)
  seepage → grain fall → grain repose
  ★ apply_roof_collapse        // F1 compression
  ★ apply_shear_weaken         // F2 shear (optional same tick)
  // later: apply_compaction   // F3
world.tick += 1

app frame (after tick):
  cold avalanche → phase (ice load already compressive)
  organisms …
```

Dirty-rect: scan active chunks + 1-cell halo (same as seepage). Prefer
compute-then-apply so parallel scan stays possible later
([`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md) Phase 1).

## Phase F1 — Roof / overhang collapse (compression)

**Highest payoff, no new field.**

### Rule

For each solid cell that has **Air directly below** (ceiling):

1. Measure contiguous horizontal Air span at `gy - 1` (and optionally
   the open cavity band) bounded by solids / map edge.
2. Let `max_span_cells = floor(roof_span_max_m / SAMPLE_WIDTH_M)`.
   - `0` → any ceiling over Air fails (Sand/Clay/Organic).
   - `∞` → never (Bedrock).
3. If `span > max_span_cells` for the **weakest** roof material along
   the span (min of roof cells’ limits), collapse.

### Collapse write (mass-local)

Compute-then-apply list of `(gx, gy_roof)`:

- Roof cell → becomes **Air** (preserve sat 0) **or** converts to a
  fallable grain (`LooseRock` for Stone, `LooseLimestone` for Limestone;
  keep Sand/Organic as themselves) and is left for `apply_grain_fall` next tick.
- Prefer **convert-to-grain + fall** so debris piles read clearly.
- Cap events per tick (`FailureConfig.max_roof_events`, default ~32)
  so a whole mountain doesn’t vanish in one frame — process lowest
  ceilings first or deterministic hash order.

### API sketch

```rust
// crates/wk-voxel/src/failure.rs (new) or rules.rs section
pub struct FailureConfig {
    pub enable_roof_collapse: bool,
    pub enable_shear_weaken: bool,
    pub enable_compaction: bool,       // F3
    pub max_roof_events: u32,
    pub max_shear_events: u32,
}

pub fn apply_roof_collapse(world: &mut World, cfg: &FailureConfig);
pub fn roof_span_cells(world: &World, gx: i32, cavity_y: i32) -> i32;
pub fn roof_span_limit_cells(material: MaterialId) -> i32; // i32::MAX = ∞
```

Wire into `tick_with_perf` behind `FailureConfig` (default: roof **on**,
shear/compaction **off** until F2/F3 land). Tab → Performance or a
small “Geotech” tree later.

### Tests (F1)

| Test | Expect |
|------|--------|
| `sand_ceiling_over_one_air_collapses` | Sand roof over 1-cell cavity → falls |
| `bedrock_bridges_arbitrarily` | Bedrock roof over wide Air stays |
| `stone_holds_short_overhang` | Span under Stone limit stays |
| `stone_collapses_wide_karst_room` | Span over limit → debris |
| `roof_collapse_conserves_solid_count` | Solids become grains/Air+debris, no delete-to-void without fallable mass |
| `roof_events_capped_per_tick` | Wide failure progresses over multiple ticks |

### Acceptance (F1)

- Karst / dug overhangs in Sand trench immediately; Stone holds short
  lips; wide rooms eventually drop a ceiling.
- No water mass invented; pore sat in collapsed cells moves with the
  grain swap rules already used by repose/fall.

---

## Phase F2 — Wet cohesion shear weaken

**Make cliffs that aren’t “grains” eventually fail when wet/steep.**

### Rule

After grain repose, scan solid face cells (solid with diagonal-down or
side Air):

1. `demand` = local drop in cells toward open Air (1 or 2).
2. `phi_step = grain_max_stable_step(mat)` — ∞ materials use a large
   sentinel so dry Stone never fails from repose alone.
3. `wet = sat/capacity` (0 if impermeable).
4. `c_eff = cohesion * (1 - k_wet * wet)` (k_wet ~ 0.5–0.8).
5. Fail if `demand > phi_step` **and** `c_eff < c_threshold(demand)`
   (simple table: demand 1 needs c_eff < 40; demand 2 needs c_eff < 100 —
   tune in tests).

### Write

- Convert face cell Stone → `LooseRock`, Limestone → `LooseLimestone`
  (Clay→Clay grain path: Clay is already `is_grain` — moisture plasticity
  via `grain_repose_max_step`; block-fail only for `repose_rise_m`
  infinite materials).
- Split implementation:
  - **F2a:** wet grains: extra `max_step` loosen already exists; scale
    loosen by `cohesion` (low cohesion → always wet-loose). Clay uses a
    dry-powder / plastic / mud curve instead of simple loosen.
  - **F2b:** competent rock faces: rare convert to LooseRock /
    LooseLimestone when wet + steep + random/hash gate so mountains
    don’t melt.

### Tests (F2)

| Test | Expect |
|------|--------|
| `wet_sand_bank_loosens_faster_than_dry` | Same geometry; wet fails repose sooner |
| `dry_stone_cliff_stable` | Vertical Stone face holds without F2b spam |
| `wet_stone_overhang_lip_can_loosen` | Saturated Stone lip above Air → LooseRock within N ticks |
| `shear_events_capped` | Progression, not instant mountain deletion |

### Acceptance (F2)

- ✅ Terrace toes / wet low-c′ banks loosen via F2a repose scaling.
- ✅ Dry inland Stone cliffs stay scenic; wet demand-2 lips can F2b → LooseRock.
- Rock-face shear is **off by default** (Tab → Geotech); chance + event cap tune melt rate.

### F2c — Competent rock rigid fall (implemented)

Industry-style **connected-component rigid bodies** on the voxel grid:

- **Static vs dynamic** — only air/soft-adjacent competent clusters up to
  `MAX_DYNAMIC_BODY_CELLS` are simulated; larger same-material masses stay
  as static strata. Flood gathers up to `FLOOD_GATHER_CAP` then applies a
  **morphological open** (erode→label→dilate) so touching boulder chains
  split into separate bodies instead of freezing as one welded pillar.
  Residual pebble necks are peeled; only editor paint / geology should weld.
- **Free fall** — multi-cell drop through Air (rocks sink in lakes).
- **Impact** — bottom face → `LooseRock` / `LooseLimestone` on hard beds.
- **Tip vs slide** — 90° pivot only when COM overhangs *and* the bed drops the
  same way; tiny/needle bodies never tip (kills flat-floor flip-flop). Otherwise
  slide down-slope (tiny bodies only with a real step down).
- **Crush specs** — large movers pulverize tiny competent clusters
  (`≤ CRUSH_SPEC_MAX`) instead of welding or getting stuck on them.
- **Thin fracture** — long thin sticks/slabs snap at 1-cell necks into debris.
- **Cargo** — soft/loose caps and embedded cells ride with fall, tip, and slide.
- **Mobile mark** — fallen / tipped / slid rock sets `CellFlags::MOBILE_ROCK`.
  Flood-fill only merges same mobility class, so a boulder cannot glue into
  unmarked painted strata or gain mass by contact.
- **Hanging peel** — void-ceiling slabs above carved caverns peel as whole
  chunks (not row-by-row), excluding bedrock-rooted pillar columns; void-below
  seeds are processed before other competent floods so hill-sized strata cannot
  starve arch floaters. Per-tick body caps truncate excess work but **re-dirty
  leftovers** and wake air-below rock every tick so large collapses finish
  across subsequent ticks instead of hanging forever.

Tab → Geotech: **Competent rock rigid fall** + fall cells / impact / roll sliders.
F1 defers when `enable_competent_fall` and material is Stone/Limestone over Air.
A cheap **floating wake** (air-below only) re-dirties sky boulders every few
ticks so they cannot hang when the dirty set is empty.

---

## Phase F3 — Overburden compaction (compression + water)

**Soft sediment under load squeezes water — no porosity field yet.**

### Rule (v1, minimal)

For Clay / Organic (optional wet Sand) with ≥ H cells of solid above
(H ~ 8–12):

- Each compaction pulse: move up to R sat units from the cell into the
  nearest upward Air / unsaturated pore path (reuse gravity-style
  push), and optionally flag the cell (CellFlags bit) so it doesn’t
  pulse every tick.
- Do **not** change porosity at runtime until we have a safe override
  story; water squeeze alone reads as “compacting”.

### Tests (F3)

| Test | Expect |
|------|--------|
| `deep_wet_clay_exudes_sat_upward` | Sat leaves deep Clay; total water conserved |
| `shallow_clay_does_not_compact` | < H overburden → no-op |
| `bedrock_never_compacts` | — |

### Acceptance (F3)

- ✅ Deep wet Clay under high σᵥ exudes sat upward (water conserved).
- ✅ Shallow / Bedrock never compact. Off by default (Tab → Geotech).

---

## Phase F4 — Derived overlays (optional HUD / modulators)

Concrete plan + S1 implementation: [`VOXEL_GEOTECH_MAP.md`](VOXEL_GEOTECH_MAP.md).

| Overlay | Source | Use |
|---------|--------|-----|
| Wetness | `sat/capacity` on faces | Map channel; feeds F2 c′ |
| Overburden σᵥ | Σ density above | HUD; gate F3 (S2) |
| Shear demand | face relief + hydro column | HUD `G`; F2b gate (S3) |

Rebuild on cadence (period 20) like Temperature. Key `G` toggles
geotech overlay.

---

## Config / UI

```text
Tab → Geotech (or Performance)
  [x] Roof collapse
  [ ] Shear weaken (rock faces)
  [ ] Compaction
  Max roof events / tick
  Max shear events / tick
```

Defaults: roof **on**, shear rock-face **off** until tuned, compaction
**off**.

## Parallelism

F1/F2 start serial compute-then-apply. When hot, scan via
`map_regions_parallel` (same as seepage). No in-place parallel writes
across chunk seams for roof spans that cross chunks — either inflate
halo or serialize span measurement per row.

## Save / schema

`FailureConfig` lives in app settings (like `PerfConfig`), not
necessarily in `SimSnapshot`. No schema bump for F1–F3 if no new cell
fields. If we add a `CellFlags::COMPACTED` bit, document in
`VOXEL_WATER.md` / cell docs; old saves default flags 0.

## Out of scope

- Continuum FEM / Mohr–Coulomb solvers
- Bedrock failing in normal play
- Inventing water from compaction
- Full dig API (burrows) — roof rule should work for karst Air today;
  dig can call the same helper later

## Implementation order (PR chain)

| PR | Deliverable |
|----|-------------|
| **F1** | ✅ `FailureConfig` + `apply_roof_collapse` in tick + Tab → Geotech |
| **F2a** | ✅ Wet cohesion scales grain repose loosen (`wet_repose_loosens`); Clay plasticity via `grain_repose_max_step` |
| **F2b** | ✅ Competent-face → LooseRock / LooseLimestone (`apply_shear_weaken`, Tab toggle) |
| **F3** | Deep Clay/Organic sat squeeze |
| **F4 / S1–S4** | `GeotechMap` + map-gated F2b + F3 compaction ([VOXEL_GEOTECH_MAP.md](VOXEL_GEOTECH_MAP.md)) |

## Done when

1. Wide karst rooms in Stone eventually drop debris; Sand never roofs.
2. Rain-soaked banks retreat faster than dry ones without melting the
   whole continent.
3. Water mass audits still pass on compaction / collapse fixtures.
4. Parallel on/off determinism tests still green.
