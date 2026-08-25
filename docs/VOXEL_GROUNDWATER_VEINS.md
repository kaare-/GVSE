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

### Fix: fracture-tailed permeability, not a symmetric band

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

Not yet done: transport through surface flow and the gravity paths (groundwater
is wired, so the karst loop closes; surface streams still drop their load only
where they evaporate), and pressure-driven artesian discharge.

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
