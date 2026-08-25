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

### Fix: field capacity (capillary retention)

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

**Model:** accumulate transported volume per cell, and make dissolution a
function of accumulated flux rather than instantaneous wetness.

- Storage: `Cell` has **no spare byte** (`material`, `sat`, `flags`,
  `_pad` = mycelium, `pore`). Use a **sparse per-cell map on `World`**, exactly
  like `mycelium_energy`: `pore_flux: HashMap<(i32, i32), u16>`. Only cells that
  have carried water are stored, so a dry world costs nothing. Serialize it —
  flux history is world state, and losing it on load would reset conduits.
- Accrual: seepage and gravity already compute a per-edge `move_amt`. Add it to
  the receiving (and/or donating) cell's flux. Saturating at `u16::MAX`.
- Decay: a slow global decay so abandoned paths fade and the map stays sparse.
- Dissolution: replace the wetness roll with a flux threshold plus rate, scaled
  by material solubility. Limestone dissolves at a modest flux; stone needs far
  more. Both slower than the surface path, which stays as-is.
- **The feedback loop is the point.** Dissolving raises the cell's `pore`, which
  raises permeability, which raises flux, which accelerates dissolution. That is
  what turns a diffuse front into a conduit. It also needs a brake — see below.

Acceptance: a scenario where two adjacent columns start with slightly different
pore values and, after a long soak, one has become a visibly faster conduit
while the other has barely changed.

## Feature B — dissolved load must come out of solution

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
2. Field capacity (Cause 3) — biggest single behavioural change, and it makes
   the pore field legible before any erosion work.
3. Fracture-tailed pore field + upward permeability widening (Cause 2) — cheap
   once field capacity exists, and needs a playtest to tune the tail.
4. Flux counter + flux-driven dissolution (Feature A).
5. Dissolved load + evaporative precipitation (Feature B), pressure-driven
   springs last.

Steps 2–3 are tuning-heavy and want playtest feedback between each. Steps 4–5
each add a serialized sparse map and a conservation audit, so they are naturally
separate PRs.

## Guards for all of it

```bash
GVSE_MASS_AUDIT=1 cargo test -p wk-voxel --lib
cargo test -p wk-voxel --test mass_audit_smoke
```

Water mass must stay flat throughout. Features A and B add a **second**
conserved quantity (mineral), which deserves the same treatment —
`audit.rs` should grow a mineral total once load exists.
