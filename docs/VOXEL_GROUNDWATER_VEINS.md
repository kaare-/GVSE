# Groundwater veins, flux erosion, and mineral deposition (plan)

**Status:** diagnosis complete, one fix landed, three features designed and not
yet built. Written from playtest feedback on
[`VOXEL_PORE_VARIATION.md`](VOXEL_PORE_VARIATION.md): per-cell pore values
landed, but groundwater still advances as a broad saturated front that drains to
bedrock and stacks upward, rather than establishing veins.

## The observation

> We still get a large mostly saturated front moving slowly from the lake
> through the ground. I had hoped we would get more variability — that veins and
> faster narrower fronts would be established — but gravity is dominant, pulling
> everything down to bedrock and just building up a saturated layer that grows
> with time, regardless of the variable saturation.

Three separate causes, in order of how much they matter.

## Cause 1 — infiltration ignored the pore field (**fixed**)

`rules/gravity.rs` rate-limits Air → porous infiltration, but was calling the
**material-level** `seepage_rate_with` / `seepage_uptake_rate_with` rather than
the cell-aware variants. Lake-bed infiltration is the dominant way water enters
the ground, so the entry rate was a material average and the front was uniform
by construction no matter what the pore field said underneath.

Now uses `seepage_rate_cell` / `seepage_uptake_rate_cell`.

## Cause 2 — the rate formula quantizes the variation away

`seepage_rate_cell` is `((permeability * 32) / 255).max(1)`. That yields only 32
distinct rates over the whole 0–255 permeability range, and the `.max(1)` floor
flattens everything below permeability 8 to the same value:

| Material | permeability range | rate range | effective contrast |
|---|---|---:|---|
| Stone | 1–9 | 1–1 | **none** |
| Sand | 72–120 | 9–15 | 1.7× |
| Limestone | 105–175 | 13–21 | 1.6× |

Stone is the bulk of deep rock, and the default ±25% band (`centered_range`)
lands entirely inside one rate bucket. Even where variation survives, a 1.6×
contrast reads as a slightly ragged front, not a vein.

### Fix: fracture-tailed permeability — **landed**

Permeability now widens **upward only** (`fracture_range`: floor at the table
value, ceiling ~8× clamped) and is sampled through
`HydroRange::sample_fracture`, which treats the whole lower half of the pore
domain as matrix and ramps the upper half quadratically to the ceiling.

Two properties that fell out of implementing it:

- `pore = 128` — the `Cell::solid()` default — must land *exactly* on the matrix
  value, or every painted and constructed cell silently becomes more permeable
  when the range widens. A linear sample over an asymmetric range broke this;
  the curve restores it. Guarded by
  `fracture_sampling_keeps_the_default_cell_at_matrix`.
- The **curve** carries the tail, so the pore *field* stays a readable coherent
  lens pattern (and porosity stays centred on it). Cubing the field instead
  worked but compressed the lens structure and coupled the two properties.

Stone now spans rate 1 in the matrix to ~5 in a fracture; ~62% of the pore
domain stays matrix-tight (`fracture_sampling_is_mostly_matrix`).

### Original design

Real rock does not have a ±25% spread around a mean — matrix permeability is
tiny and flow concentrates in a **small fraction** of much more conductive
fractures. So the distribution matters more than the bounds:

1. **Heavy-tail the pore field**, not the range. `worldgen::pore_coordinate`
   currently produces a roughly centred value, so most cells sit near the
   material midpoint. Applying a power curve (`v³`-ish) leaves most cells near
   the low end with a thin high tail — a fracture network rather than noise.
2. **Widen permeability upward only**, keeping the floor at today's value so
   typical rock still feels tight: `min = mid`, `max ≈ mid × 8` (clamped 255).
   With a cubic field the median lands near `mid × 1.9` and the top percentile
   reaches `mid × 8`. For stone that is rate 1 in the matrix and rate ~7 in a
   fracture — a real conduit.
3. Leave **porosity** symmetric. It controls storage, not path selection, and
   several tests assert bed saturation against it.

Do not simply raise the rate multiplier: it speeds up all seepage and changes
the tuned water feel everywhere.

## Cause 3 — no counterforce to gravity

Seepage equalises hydraulic head (`y + sat/capacity`), so water sinks until it
stacks from bedrock upward. That *is* a water table and is physically right, but
nothing holds water against gravity, so every cell eventually drains and the
only stable state is a growing saturated wedge.

### Fix: field capacity (capillary retention) — **landed**

`MaterialProps::field_capacity` (0–255, a share of porosity) is the fraction
held against gravity. Downward pore→pore seepage now moves only the amount
*above* it (`cell::drainable_sat_cell`), replacing the flat 10% residual film.
Retention scales with the cell's own capacity, so a high-`pore` cell both stores
and retains more.

Values: gravel 20 (nearly free-draining), sand and stone 51, soil 128,
organic 166, clay 188 (perches), limestone and loose rock 38 (drain through
their conduits). Regression: `clay_retains_water_that_gravel_lets_go`.

Knock-on seen immediately: a wet beach absorbs a surface film more slowly and
sheds the rest sideways, which is real runoff — `beach_film_drains_into_ocean_not_inland`
now bounds the lateral trace below the visible-puddle threshold instead of
demanding exactly zero.

### Original design

Give each material a **retained fraction** — the share of capacity held against
gravity by capillary action. Only saturation *above* that threshold is mobile
downward; below it, water moves only by much slower diffusion (or is taken by
roots / evaporation).

- Clay retains most of its pore water, gravel almost none. That single number
  is what makes a clay lens perch water and a gravel lens drain — the visible
  behaviour difference the pore field was supposed to produce.
- Seepage already has a crude stand-in: the wetting-front plug refuses downward
  pore flow when the donor is ≤10% or ≤2 sat. Replacing that hard-coded residual
  with a per-material (and pore-scaled) field capacity is the natural upgrade.
- Expect a **perched water table** above clay-rich lenses and genuinely
  unsaturated rock between conduits, instead of one advancing front.

Hazard: mass conservation. Retained water is still water — it must be counted by
`audit.rs` and remain reachable by roots and evaporation, or long runs will look
like a slow leak.

## Feature A — flux counter drives erosion (veins, not confetti)

> I think we might need a counter on every block, counting how much water has
> passed, and calculate erosion from that. Erosion should be slower for both
> rock and limestone in the groundwater path. Now we get random erosion events
> all over, but what we want is the formation of veins along the most travelled
> paths.

Today `apply_karst_dissolution` rolls a per-cell probability against contact
wetness. Wetness is nearly uniform inside the front, so dissolution scatters —
exactly the "random erosion events all over" being reported. Nothing rewards a
cell for being on the path that actually carries water.

**Model:** make dissolution a function of transported volume rather than
instantaneous wetness — and use **`pore` itself as the aperture state** instead
of a separate counter.

A dedicated `flux: HashMap<(i32, i32), u16>` (in the style of `mycelium_energy`)
was the first sketch, but `pore` is the better accumulator:

- **No third quantity and no new serialization.** `pore` already lives in `Cell`
  and already saves.
- Raising it raises capacity *and* permeability together, which is the physical
  claim: a widened aperture both stores and conducts more. Conflating "how much
  has flowed here" with "how open is this rock" is the model, not a bug.
- It only ever **increases**, so it can never strand saturation above a
  shrinking capacity. That was the main hazard in the original pore migration
  ([`VOXEL_PORE_VARIATION.md`](VOXEL_PORE_VARIATION.md) §5) and this avoids it.
- **Precipitation is the counter-process** (Feature B), so no decay heuristic is
  needed: flux opens the aperture, deposition closes it.

So: water passing through a soluble cell nudges `pore` up by a slow increment
scaled by solubility (limestone readily, stone far slower, both much slower than
the surface path). Once `pore` saturates, the cell dissolves to Air and emits its
mineral as load.

- **The feedback loop is the point.** More flow → more open → more flow. That is
  what turns a diffuse front into a conduit. Feature B is its brake.

**Landed** as `mineral::widen_aperture`, called from the seepage apply loop on
the receiving cell. `APERTURE_GROWTH_SCALE` (2.0) is the odds for a *full*
throughput through limestone; the roll scales with how much water moved and with
solubility relative to limestone, so stone opens ~40× slower. Deterministic given
`(seed, position, tick)` like the rest of karst.

### The mass identity this rests on

`MINERAL_PER_CELL` is 255 and `pore` is a `u8`, so **one pore step is exactly one
unit of mineral**. That makes the whole ledger exact with no scaling factor:

- Widening by a step releases 1 unit of load; a rock cell's remaining mineral is
  `255 - pore` (`mineral::cell_mineral`).
- Occluding by a step consumes 1 unit.
- Dissolution emits only `255 - pore`, not a full cell — a cell already widened
  has released most of its mineral incrementally, and emitting a full load would
  mint mineral. This is why `emit_from_dissolved_rock` takes the pre-dissolve
  *cell* rather than just its material.

It also means a porous rock cell genuinely contains less rock than a dense one,
which is physically right and applies to worldgen porosity too, not just to
dissolution.

Regressions: `one_pore_step_is_one_mineral_unit`,
`a_widened_cell_does_not_emit_a_second_full_load`,
`full_aperture_dissolves_the_cell_and_conserves`,
`precipitation_closes_the_aperture_it_opened`, and
`throughput_widens_a_conduit_and_conserves_mineral` (a fed limestone shaft opens
while an identical unfed one does not).

Acceptance: a scenario where two adjacent columns start with slightly different
pore values and, after a long soak, one has become a visibly faster conduit
while the other has barely changed.

## Feature B — dissolved load must come out of solution (**first cut landed**)

Implemented in [`mineral.rs`](../crates/wk-voxel/src/mineral.rs): dissolution
emits load, load rides seepage transfers pro rata, and evaporation precipitates
it — concentration ceiling first, whole load once the cell goes dry. Deposits
seat only on solid ground (no mid-air flowstone) and mint `Limestone`.
`audit::mineral_total` tracks rock + load so the loop is checkable, and the
inspector reports the load on a clicked cell.

Transport now covers **seepage, surface flow, and both gravity paths**, so a
spring's load runs downstream and deposits where the water finally dries rather
than at the point it left the ground
(`a_stream_carries_mineral_downhill_and_conserves_it`). The concentration brake
also runs on seepage receivers, so a conduit that stalls cements itself shut.

Gravity collects its load moves and applies them after the parallel section — the
hot loop works over raw chunk pointers and cannot touch the sparse map.

Cost: these hooks sit on a hot path, so both are guarded by checks the caller
already has — "is anything dissolved at all" (once per pass, not per transfer)
and "is the receiver even soluble" (from the cell already in hand). Without those
the demo tick regressed ~10%; with them the residual is ~0.7 ms, which is the
aperture-growth roll itself.

**Artesian discharge landed.** Confined upward flow already carried load (it
shares `commit_air_sat_xfers`); what was missing was the pressure drop. Water
that just *rose* against gravity arrived under pressure, so
`mineral::precipitate_artesian` applies a ceiling `ARTESIAN_CEILING_DIVISOR`
times lower and drops the difference. Without it a rising spring carried its
mineral off to wherever it eventually dried instead of building a mound.

An outlet is open Air with no pore to cement, so a sub-cell deposit goes into the
**floor beneath** — which is where travertine actually forms, and which matters
for more than realism: load banked in an Air cell stays *mobile* and the next
transfer carries it away, so nothing would ever accumulate. Occlusion is
restricted to soluble rock, since that is what `mineral_total` counts; cementing
anything else would consume load with no solid gaining it.

## Soak finding: the brake was beating the growth

A 160 k-tick soak showed no conduits and no cave development. The inspector on a
water-bearing cell explained it:

```
material=looselimestone  sat=27/28
pore=86  ->  pore=46      (over ~1800 ticks)
dissolved mineral=15..24  (holds 6)
```

Load sat 2.5–4× over the carrying ceiling, so precipitation fired continuously
and the aperture **closed**. Erosion needs throughput over a threshold and scales
with its square (deliberately hard); cementing triggered on any excess at all.
Every water-bearing cell was slowly sealing itself.

Cause: `SOLUBILITY_PER_SAT` was 4. Pore water has small `sat`, so a near-saturated
cell's ceiling was `27 × 4 / 16 = 6` — load precipitated the moment it entered
rock instead of travelling. Raised to 24 (~40 for that cell), so load moves and
drops only where water is genuinely lost. Deposits belong at discharge points,
not spread through the aquifer.

**Open (landed later):** flowstone is now its own material and colour. Limestone
keeps a single material colour as karst opens it — a sage permeability wash used
to step across the bed on the geology cadence and read as a blink.

## Hard rock, so channels can form

Playtest: continuous limestone and stone need to be **harder**, or erosion
spreads out and you get a slightly more porous aquifer instead of pipes.

Two changes to `widen_aperture`:

- **Flow threshold** (`APERTURE_MIN_THROUGHPUT`). Below it competent rock does
  not yield at all. Wetted rock carrying a trickle stays solid.
- **Superlinear response.** Odds scale with the *square* of throughput above the
  threshold, so doubling the water through a cell opens it more than four times
  faster. A small head start compounds into a conduit while neighbours stay
  effectively solid. A linear response widened everything evenly.

`APERTURE_GROWTH_SCALE` is therefore not a uniform erosion rate — it sets how
sharply flow focuses. Raising it erodes everything and loses the channels.

This retired an integration test that fed a *sealed* limestone shaft: such a
shaft fills and flow stops, so it was passing on residual trickle — exactly the
diffuse erosion the threshold exists to remove. Replaced by mechanism tests
(`rock_does_not_yield_below_the_flow_threshold`,
`erosion_is_superlinear_so_flow_focuses_into_channels`) which state the claim
directly and do not depend on a long soak.

## Artesian head through saturated rock (**landed**)

`transmits_pressure` now treats **fully saturated pore space** as a pressure
conductor alongside water-filled Air, in the confined-head walk *and* in the
receiver's "what am I standing on" check. That second one was the blocker for a
hand-dug well: the shaft bottoms on rock, so requiring Air below meant the
aquifer it reached could never feed it.

Partially saturated rock deliberately does **not** conduct — there is air in the
pores, so there is no continuous column to push through. Donors are still
free-surface wet Air, so the rise is a transfer from a real surface and cannot
invent water.

Two behaviours worth knowing, both correct:

- An **uncased** hole does not rise past the confining layer. Confined rise is
  refused where water could spread sideways instead
  (`allows_confined_rise` / `open_air_both_sides`), so a bare hole in open ground
  fills its sump and stops. A cased shaft climbs.
- A well **draws its aquifer down**. If recharge cannot keep the path fully
  saturated, the pressure chain breaks and the rise stops — which is what a real
  over-pumped well does. Sustained rise needs sustained intake.

Regression: `a_well_bottomed_in_a_confined_aquifer_rises`. Confined-pass cost is
unchanged (~0.3–0.7 ms/tick).

## Why rate fixes could never produce structure (**the root cause**)

Everything aimed at the underground — pore variation, the fracture tail, ridged
veins, competitive allocation, fractional rates — changed how *fast* a cell
reached its endpoint. **The endpoint was uniform**, so given a night it all got
there. Playtest, after all of it: "seepage doesnt seem to find our more permeable
layer much interesting and it seems more like decoration at this point."

The endpoint is retention, and it was inverted. `retained = capacity *
field_capacity / 255`, and capacity *grows* with pore — so opening a cell bought
capacity and retention in equal measure, and a fractured cell held **more**
absolute water than a tight one. Conduits stored water instead of transmitting it,
which is backwards: low storage and high flux is what a conduit *is*.

Retention fraction now falls as pore opens (60% shed at fully open), so a vein
drains nearly dry between flows while the matrix beside it perches. **Competent
rock only** — shedding from fine sediment took open clay from 74% retention to
29%, which is not a seal.

Expect the visual to invert: veins read as the *dry* paths through wet rock, not
wet paths through dry rock. Combined with the ochre permeability tint on stone,
a conduit shows as tinted-and-dry inside matrix-and-wet. Limestone is left on
its material colour so karst opening the bed does not flash.

**General lesson.** A diffusive process with a fixed uniform endpoint erases any
structure its rates create, given time. If a soak keeps flattening, look at the
equilibrium before tuning the rate.

## Two erosion chains, kept separate (**landed**)

The material list was being asked to carry two independent processes at once,
and conflating them meant silicate rock dissolved into the load that
precipitates as flowstone — the sim was quietly converting granite into
carbonate.

**Chemical (carbonate only).** Limestone and flowstone dissolve, travel as
dissolved load, and precipitate as flowstone. Membership is now read straight
off `MaterialProps::solubility`, which already said exactly this (40 for both,
0 for stone). The old hardcoded `| MaterialId::Stone` in `is_soluble_rock`, and
a second hardcoded pair in karst, were the entire conflation. Removing them also
fixed flowstone never dissolving, which had made a sealed conduit permanent.

**Mechanical (everything competent).** A fracture carrying water does open in
silicate rock, just far slower and by abrasion. `widen_aperture` is gated on
`is_competent_rock` rather than solubility, with carbonate scaled by its
solubility and silicate by `MECHANICAL_ABRASION_REF` (a tenth of limestone).
Abrasion releases nothing into solution, which is self-consistent because stone
is outside the mineral ledger — `cell_mineral` returns 0 for insoluble material.

**Known gap:** abraded stone is therefore untracked. Grinding rock produces
*suspended* sediment, and that species does not exist yet. This is the honest
version of the gap rather than the hidden one (mineral appearing as carbonate).

`KarstConfig::stone_scale` is retired, kept only so existing presets
deserialize.

## Cementation: why loose sediment could never hold a channel (**landed**)

Repose and grain settle destroy any void in loose material the moment it opens.
So conduits could only ever form in competent rock — never in the near-surface
layer where water actually runs, which is why groundwater erosion produced no
visible pipes near the surface however well the aperture growth worked.

Cementing sediment gives near-surface channels somewhere to persist, and closes
the loop: water deposits mineral → sediment sets → set rock holds a void → the
void becomes a conduit → the conduit concentrates flow.

Two materials, not one per loose type:

| loose | cemented | note |
|---|---|---|
| `Sand` | `Sandstone` | |
| `Gravel`, `LooseRock` | `Conglomerate` | |
| `LooseLimestone` | `Limestone` | needs no new material |
| `Soil`, `Organic` | — | they rot rather than set |

Both are clastic, so unlike tight silicate stone they stay decent aquifers
(permeability 60 and 80 against stone's 5) while still being competent enough to
hold a roof. That combination is the point.

**Conservation shaped the mechanism.** An insoluble sediment carries no mineral
in the ledger, so there is nowhere to bank a partial amount — there is no such
thing as half-cemented sand. Cementation is therefore one atomic step whose
arithmetic is exact: the load consumed becomes the new cell's cement, with
`pore` set to its complement. Everything after that first step is ordinary pore
occlusion, because the resulting rock *is* soluble. `CEMENT_MIN_LOAD` keeps a
trace of mineral from setting a whole cell.

**Reversible in all three directions**, or the world monotonically petrifies:

- the cement dissolves (solubility 20, half limestone's, since only the matrix
  is soluble);
- a dissolving clastic rock returns its **sediment, not a void** — both the karst
  path and full aperture opening route through `loose_parent`, or dissolving
  sandstone would delete the sand;
- shattering breaks it back to the sediment it was cemented from.

## Three transport modes, kept apart (**suspension landed**)

| mode | mechanism | drops out when |
|------|-----------|----------------|
| bedload | `grain::apply_flow_erosion` relocates whole cells | it stops being pushed |
| dissolved | `mineral` — carbonate in solution | the water **leaves** or concentrates |
| suspended | `sediment` — clay held up by turbulence | the water **slows** |

Clay is not dissolved, it is entrained, and that single distinction determines
everything about the module:

- **It drops when the water slows, not when the water leaves.** Slack water holds
  nothing at all (`SUSPEND_PER_SLACK_SAT` is 0); only moving water carries. This
  is what puts mud where a river slows rather than where it dries.
- **Pore space filters it out.** `sediment::carry_with_water` refuses a non-Air
  destination, so fines strain out at a gravel bed instead of silting an aquifer.
  Dissolved load has no such restriction, and should not.

Modelling clay as dissolved load would have cemented mud in place instead of
letting it settle — the opposite of a delta.

**Only clay-grade material suspends.** Sand and gravel are too coarse and travel
as bedload. Bentonite is excluded deliberately: an aquitard that washes away is
not a seal. Restricting the species to one material is also what keeps
`audit::sediment_total` exact — entrainment takes a cell and yields
`SEDIMENT_PER_CELL`, settling spends `SEDIMENT_PER_CELL` and returns a cell.
Atomic for the same reason cementation is: there is nowhere to bank a partial
cell.

### Cost, and the shape that got it down

1.35 ms/tick on the demo world at its half-tick cadence, from 2.34 in the first
version. Two changes did it, and both are worth preserving:

- **Scan for the fines, not for the water.** An ocean world is overwhelmingly
  water with nothing beneath it to lift, so iterating water cells does work
  proportional to the sea rather than to the erodible bed. Chunks need both
  `has_wet_air` and `has_loose`.
- **Drive settling from the sparse load map**, not from a scan, so it costs what
  is actually in transit.

A clay-specific chunk flag would cut the remaining scan further.

### Still open: abraded silicate

Abrasion of silicate rock should produce suspension — that is the gap left by
making dissolution carbonate-only. It is not wired up yet because the sediment
ledger would first have to close over every silicate conversion
(sand↔sandstone, stone↔looserock, gravel↔conglomerate). Adding the emission
without that would turn an exact audit into a false one.

## Superseded notes on the original limitation

A hand-dug well fills from the sides but shows **no upward pressure**, and that
is expected with the current rules rather than a bug:

- `apply_confined_upward_regions` walks pressure through **full wet Air** only —
  water-filled voids, i.e. communicating vessels. It does not traverse saturated
  pore space, so an aquifer cannot push water up a shaft.
- Seepage cannot do it either, and the reason is structural:
  `hydraulic_head = y + sat / capacity`, so the pressure term is capped at one
  cell of elevation. For water to rise from `y` to `y+1` it would need
  `frac_below > frac_above + 1`, which is impossible when `frac <= 1`. **A
  saturated column can never push water above its own top.**

Real artesian flow needs head that is not bounded by local saturation — the
recharge area is higher than the outlet. The contained version is to let the
existing confined-head walk traverse *fully saturated porous* cells as well as
full wet Air: the machinery for "find the connected body's maximum head and
compare it to the receiver" already exists, it simply refuses to cross rock.
Hazard to respect: that walk must not invent water, so it stays a transfer from
a real donor surface ([`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) §4).

### Original design

> Even more importantly, the dissolved rock or limestone needs to come out of
> solution at some point. So we might need pressure and artesian springs with
> mineral rich water that deposits on the surface.

This is the missing half of karst: today dissolved rock simply ceases to exist.
Real karst is a mass-transport loop — carbonate dissolves upstream and
reprecipitates downstream as tufa, travertine, flowstone, and spring mounds.

**Model:** water carries a dissolved mineral load; the load precipitates when
the water can no longer hold it.

- Storage: a second sparse map, `dissolved: HashMap<(i32, i32), u16>`, in the
  same style. (A dedicated field is sketched in
  [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) §3; the sparse-map form is cheaper and
  matches the flux counter.)
- Source: dissolution adds load proportional to the rock removed. **This makes
  karst mass-conserving for the first time** — rock does not vanish, it becomes
  load.
- Transport: load rides existing water transfers, pro rata with `move_amt`.
- Precipitation triggers, in rough order of value:
  1. **Evaporation** — water leaving at the surface must drop its whole load.
     This is what builds spring mounds and tufa terraces, and it is the most
     visible payoff.
  2. **Concentration** — load exceeding a solubility ceiling precipitates.
  3. **Pressure / degassing** — an artesian discharge point drops load as it
     depressurises. Needs the confined-head machinery that already exists in
     `water_flow.rs` (`apply_confined_upward_regions`).
- Deposit: precipitation raises `pore` downward (occluding the pore) and,
  at full occlusion, converts Air → `Limestone` — flowstone sealing a conduit.
  **This is the brake on Feature A's feedback loop:** conduits widen where flow
  is fast and seal where it stalls, which is what keeps a karst system from
  running away into a single drain.

Acceptance: a spring that deposits a visible mineral mound at its outlet, and a
total-mineral audit (rock + dissolved load + deposits) that stays flat, in the
same spirit as the water mass audit.

## Suggested order

1. ~~Cell-aware infiltration~~ **landed**.
2. **Dissolved load + precipitation (Feature B)** — first, by decision. It is
   what makes karst mass-conserving at all (today dissolved rock ceases to
   exist), and it is the brake every later amplification needs.
3. Field capacity (Cause 3) — biggest single behavioural change, and it makes
   the pore field legible before erosion tuning.
4. Fracture-tailed pore field + upward permeability widening (Cause 2) — cheap
   once field capacity exists, and needs a playtest to tune the tail.
5. Aperture growth from throughput (Feature A), now safe because deposition can
   close conduits again.

Steps 3–4 are tuning-heavy and want playtest feedback between each.

## Guards for all of it

```bash
GVSE_MASS_AUDIT=1 cargo test -p wk-voxel --lib
cargo test -p wk-voxel --test mass_audit_smoke
```

Water mass must stay flat throughout. Features A and B add a **second**
conserved quantity (mineral), which deserves the same treatment —
`audit.rs` should grow a mineral total once load exists.
