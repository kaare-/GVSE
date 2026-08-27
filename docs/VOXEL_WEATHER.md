# Voxel weather: vapour, rain, clouds

Canonical notes for the water cycle above ground. Companion to
`VOXEL_WATER.md` (below ground) and `VOXEL_GROUNDWATER_VEINS.md`.

## Why this document exists

"Rain looks broken" took five separate fixes, and every one lived in the seam
between a **representation** of weather and the weather itself. That is the
signal worth remembering: when bugs cluster in the seams of an abstraction, the
abstraction is the defect.

The five, in the order they were found:

1. Clouds were placed by taking the globally wettest sky tiles up to
   `max_parcels`. Wet tiles cluster, so 716 tiles were eligible, 36 were drawn,
   and all 36 landed inside **17% of the map**. The other 83% rained invisibly.
2. Droplets smaller than one cell are *refused* by
   `deposit_condensate_on_surface`, and a refused deposit drains no humidity — so
   sub-cell drizzle spent rain events achieving nothing and filled the sky.
3. Rain streaks were sized from `cell_px`, so at whole-world zoom a drop was a
   1×1.6 pixel speck.
4. `occupies_cloud_floor` counted any damp air as floor, so in a humid sky the
   "ground" that streaks clip against climbed to just under the deck and every
   drop was skipped. Same function set the cloud lift floor, so haze was also
   shoving clouds around instead of terrain.
5. Parcels were cleared and rebuilt every tick, so a cloud had no identity: it
   could not drift, build or clear, only blink. `vis_mass` was documented as an
   EMA and never got to be one.

## Rain is real water now (**landed**)

Rain used to teleport: condensation scanned up to 512 cells down from the sky
ceiling and placed water on the ground, so the falling part *had* to be a
cosmetic overlay drawn over an event that had already finished. That split is
what made "is it actually raining?" hard to answer even from the code.

A droplet now nucleates in the air cell that held the vapour
(`deposit_water_in_air`) and descends under the same gravity as any other water.
The terrain pass draws mid-air sat above the haze band; `draw_falling_rain`,
`draw_falling_snow` and `precip_tier` are gone. Frost keeps the surface path,
because rime genuinely forms *on* things.

Measured on the demo world over 3000 ticks:

| | before | after |
|---|---|---|
| equilibrium humidity | 360,453 | 200,472 |
| condensation cost | 0.328 ms/tick | 0.229 ms/tick |
| `sum(physics)` | 12.33 ms/tick | 13.01 ms/tick |

The atmosphere holds 44% less water because rain now lands and drains it. The
0.7 ms is falling rain's churn, and it is affordable because rain is **sparse** —
only columns under raining tiles activate.

`CloudParcel::raining` is now only a HUD readout; nothing draws from it.

**`nimbus` is the parcel *count*, not a rain count** — "how many N echo parcels
are drawn (cap ~36)". It sits pinned at 36 because banding puts one parcel in
each of 36 bands, so any moist sky fills them all. It is not a weather signal and
was misread as one for several rounds. Use `hum` (total humidity mass) as the
dial instead: it fell from 183k to 95k in playtest once rain started landing.

### Pinned: droplets fall too fast

*Deliberately parked* — playtested and judged acceptable for now. Kept here
because the analysis is done, not because it is blocking.

Confirmed in playtest — rain is visible, and "very fast drops". A droplet is
moved by `apply_gravity_fall` on every flow substep, and there are 8 substeps per
tick, so it covers many cells per tick and reads as a flicker rather than a fall.

That speed is *correct* for a column of water draining and wrong for a single
droplet in air. The fix is the same shape as the one competent bodies already
have (`BODY_FALL_CELLS_PER_TICK`): a terminal velocity for isolated mid-air
water, so a droplet descends at a legible rate while a draining column keeps its
current behaviour.

**Do not simply slow gravity fall.** It is shared with waterfalls, column
drainage and lake filling, all of which are tuned. The cap has to apply only to
water that is isolated in air — no water directly above or below it — which is
what distinguishes a droplet from a column. `apply_gravity_fall_regions` is a
parallel region loop over raw chunk pointers with unsafe access and special cases
for lake interior, surge and slope runoff, so this is not a drive-by edit.

**Cheaper alternative worth trying first: nucleate smaller, more often.** The
whole-cell droplet rule (`mass_per_droplet` 255) exists because
`deposit_condensate_on_surface` *refuses* a sub-cell budget — frost needs a whole
cell. That constraint is **surface-only**. `deposit_water_in_air` goes through
`fill_air_sat`, which takes partial sat happily, so in-air nucleation has no such
floor. A smaller in-air droplet at a higher rate would read as a continuous
stream rather than fast-moving blobs, without touching gravity at all.

Measure equilibrium humidity either way (`tests/rain_probe.rs`): drainage per
event times event rate is what sets it, so smaller-and-more-often should be
roughly neutral, but that is an assumption to check rather than assume.

## Per-cell vapour is *not* affordable (**measured, do not build**)

The obvious next step — vapour stored in every Air cell rather than on coarse
humidity tiles — was measured and rejected.

| | cells | share |
|---|---|---|
| demo world | 327,680 | |
| sky (above sea level) | 245,760 | 75% |
| active set today | 27,889 | 8.5% |

Rain is cheap because it is sparse. Vapour is the opposite: it fills the sky and
**advects with wind, so it changes everywhere every tick**. Dirty rects cannot
help with a field that is uniformly in motion, so the active set would go from
8.5% toward 75% — roughly a ninefold increase — to buy detail invisible at cloud
scale.

The coarse tile field is therefore not a hack. For a diffuse quantity that
changes everywhere at once, coarse tiles are the right abstraction. Keep it.

## Decided: no cloud animation at all — render the vapour field

**Design call (playtest, 2026-08-26).** Drop cloud parcels entirely rather than
replacing them with a derived deck, and tune the *vapour field's own rendering*
instead. The glitchy look of water condensing out of the field was judged
visually interesting on its own and expected to improve with convection.

This is "a cloud is just an air block with saturation" taken to its conclusion:
if the field is the cloud, there is no reason to derive an intermediate object
from it. What goes:

- `CloudStore`'s visual half — parcels, persistence, drift, lift, dissipation
- `draw_clouds` and the three `draw_depth_cloud_layer` call sites
- quite possibly `deck_from_field` too (added just before this call — it derives a
  banded deck, which is still an intermediate representation)

What replaces it: draw humidity saturation directly as the sky's look, the way
the `H` overlay already does but as the default presentation rather than a
diagnostic.

Consequences worth knowing before starting:

- `pick_spread_across_x` and the banding exist to make a *parcel* selection
  spatially stable. Rendering the field needs none of it — the field is already
  spatially coherent everywhere. Banding can go with the parcels.
- `CloudParcel::raining` and the `nimbus` HUD field lose their last purpose.
- Cloud floor / ridge clearance (`cloud_floor_y`) exists to stop parcels sinking
  into terrain. A field has no such problem, so that machinery may also go — which
  would remove the function whose unbounded upward scan cost roughly two million
  hashmap lookups per frame.

## Superseded plan: clouds drawn from the field, parcels deleted

"A cloud is just an air block with saturation" is a claim about where clouds
*come from* — an observation about the field, not an object placed on top of it.
That can be honoured at tile resolution without per-cell vapour, and it deletes
the most code.

Today `CloudStore` maintains parcels that must be placed, persisted, drifted,
lifted and dissipated — five concerns, each of which has already been a bug. If
the deck is derived from humidity saturation at draw time, all five stop existing:
the field already has position, density and motion, because
`Humidity::advect` moves it.

Order of work:

1. Expose the deck as a **derivation** of `(humidity, temperature)`: for each
   band, saturation ratio → density, and condensation level → altitude
   (`condensation_level`, already written and tested).
2. Rewire the four app draw sites (`draw_depth_cloud_layer` ×3, `draw_clouds`) to
   read that derivation instead of `CloudStore::parcels`.
3. Delete parcel persistence, drift, dissipation, and `CloudStore`'s visual half.

**Watch for:** the derivation must be *spatially stable* or the jitter of fix 5
returns. Banding by world x (`pick_spread_across_x`) is what made it stable;
without that, top-N selection by mass moves discontinuously. Keep a test that
advecting the field moves the deck smoothly.

## It works (**playtest, full diurnal cycle**)

Observed over one day once convection landed, unscripted:

- humidity 200k before sunrise, **sunrise rain** off the overnight high band
- dry morning as humidity climbs
- after midday, clouds appear **first on the lee side of the hill**, then a second
  formation over the lake/land boundary
- afternoon **local** rain event on the lee side
- evening rain as humidity passes 440k, continuing into the night as it cools
- rain stops when air just above sea level reaches −2 °C, humidity 322k
- overnight decline to 277k, a rise, then **morning rain** at sunrise driving it to
  252k before climbing again

Lee-side formation is the tell: the orographic rule was always there and could not
express itself until the sky had both headroom (rain that lands) and horizontal
structure (convection). Neither alone was enough.

Still missing: **snow**. `precip_forms_snow_at_air` and the phase config exist, and
the temperature range now genuinely reaches freezing, but cold condensation still
takes the *surface* frost path — nothing falls. Extending phase 1 to nucleate snow
in the air needs care about mass: a `Snow` cell has to carry the humidity mass it
was made from or `audit::sat_totals` loses it, which is why `deposit_frost_coat`
pays in full cells.

## Convection is the *difference* between columns

Buoyancy existed and was local, but keyed only on the **vertical** lapse — and
temperature falls smoothly with altitude everywhere, so lift was near-identical
over every column. Vapour rose uniformly, which spreads moisture rather than
organising it, and no amount of extra diurnal forcing could change that.

Lift now scales by each column's temperature anomaly against **the mean of its own
row**. Two mistakes were made getting there, both measured in playtest within
minutes:

- `Temperature::mean()` called *inside* the per-tile loop — every humidity tile
  scanning the whole field. Collapsed the frame rate.
- The anomaly taken against a *global* mean, which mixes altitudes. Every high
  tile reads as cool, lift was suppressed aloft, and vapour piled into a dense
  unmoving layer near the ground — the opposite of convection.

And a third caught by a test: averaging over the tiles that *hold vapour* makes a
lone cloud its own mean, so it never convects, and the reference drifts with
wherever the vapour is. Average across the world's tile row.

## The diurnal cycle works — it needed headroom, not more forcing

**Superseded conclusion.** Earlier measurements said the day/night swing was
swamped (rain fraction day 80% vs night 76%) and read that as missing forcing,
with convection as the fix. That was wrong about the cause.

The forcing was fine; the sky had no room. An atmosphere pinned near saturation
cannot express a swing in saturation mass, because it is above the ceiling either
way. Once rain nucleated in the air and actually drained the sky (phase 1,
equilibrium 360k → 200k), the *same* forcing started producing weather.

Playtest, day 3000 / night 3000 with a 25 °C swing: a **dry night followed by
morning rain**, and total humidity swinging from ~95k at night to ~363k in
daylight — close to fourfold. That is a diurnal water cycle, not a rain rate.

The general lesson is worth keeping: when a driver looks too weak, check whether
the thing it drives is saturated before adding more driver.

## Then: convection

Not needed to fix constant rain any more — that is solved. Convection is now
about **spatial** structure: fronts, and moisture organised by terrain and
buoyancy rather than spread evenly. The remaining flatness is horizontal, not
temporal. All the
forcing exists — `day_amp_c`, `solar_heat_c`, `night_cool_c`, saturation mass
varying with temperature, a 1200-tick day — and it is swamped: measured **rain
fraction day 80% vs night 76%**.

Two findings bound the problem:

- The rain response used to clamp at saturation, which is exactly what the
  diurnal cycle and orographic lift move. Now allowed to climb to
  `SUPERSATURATION_HEADROOM`. Helped (equilibrium 386k → 360k, diurnal gap 4 → 6
  points) but did not fix it, so the clamp was secondary.
- The dominant term is the **ratio of the diurnal swing to the
  min-to-saturation span**, and 6 °C is small next to that span. The next lever
  is the span — a steeper saturation-versus-temperature curve — not the clamp.

Ground thermal inertia is the other half, and it is why longer days help more
than a bigger swing. Stone relaxes toward air temperature at
`sky_relax / (1 + capacity × inertia_scale)` ≈ 0.0146 per thermal step, and
temperature steps every ~20 ticks, giving a time constant near **1370 ticks**. A
600-tick daylight half therefore reaches only ~36% of the air's swing: the ground
never finishes warming before the sun sets. Day 3000 / night 3000 gives ~2.2 time
constants per half and ~89%.

Convection proper — buoyancy from the ground-versus-air difference — is cheap on
a tile field and is what would turn a steady rain rate into fronts.

## Snow falls gently (**landed**), wind drift still open

Snow nucleates in the air (`phase::deposit_snow_in_air`) and now descends at a
usable rate. The bug was that grain fall takes one step per pass and runs several
passes a tick — right for sand settling, and it made flakes appear in the sky and
arrive on the ground in the same breath.

Fixed by *odds* rather than a separate pass, which turned out much smaller than the
plan above assumed: an airborne flake rolls to hold position most passes
(`SNOWFALL_STEP_ODDS` 0.10), so one step is spread over many passes. The roll's
irregularity is itself snow-like. Same technique as the fractional seepage rates,
and it needs no new pass, so the hazard of "grain fall skips snow but the drift pass
does not run" never arises.

Gated on **airborne** snow only. Once a flake lands it is snowpack and behaves as
any other loose material, which is what lets drifts build and repose.

Ice is deliberately not gated and serves as the test control: if both slow down,
the gate is catching the wrong material.

**Still open: wind drift.** Flakes fall straight. Lateral movement is a real
addition, because grain fall only ever pulls a cell *straight down* — there is no
sideways step to bias. It wants either a dedicated drift pass or a lateral
displacement in the fall step, and it needs `Wind`, which the physics tick does not
currently receive.

**Note on the tick.** The roll is hashed on `world.tick`, so any loop calling
`apply_grain_fall` repeatedly without advancing the tick re-rolls the same answer
forever. Production advances it; a test did not and now does. Same footgun as the
fractional seepage gate.

## Open bug: weather reads a *static* surface map

`worldgen::continental_surface_y` recomputes the **original** procedural profile
from the seed. It is not the current terrain, so erosion, collapse, karst
dissolution and hand edits are all invisible to anything that asks it where the
ground is.

Consumers, in rough order of how much it matters:

| site | uses it for | consequence |
|---|---|---|
| `condensation.rs` `orographic_factors` | elevation/slope for orographic rain | rain keeps falling on a hill that has eroded away, and not on one that has grown |
| `wind.rs` (several) | upslope/downslope lift | lee and windward sides are fixed at worldgen |
| `phase.rs` | ground sample | frost/melt decisions on stale ground |
| `clouds.rs` `surface_y` | cloud floor *baseline* | least affected — `cloud_floor_y` then scans the real world upward from it |

Note the observed lee-side cloud formation is therefore keyed to the **worldgen**
hill, not the eroded one. It looks right today because the demo world's profile has
not moved much.

**Why it is not a quick fix.** The honest repair is a maintained live-surface cache
on `World` — topmost solid per column, invalidated on solidity change, which the
`competent_wake` plumbing already tracks — plus threading `&World` into
`orographic_factors` and the `Wind` slope queries, none of which currently take it.

**Do not fix it partially.** Making some consumers live and leaving others stale is
worse than consistent staleness: orographic rain keyed to live terrain while wind
lift is keyed to worldgen would put the rain and the lift on different hills.

`cloud_floor_y` is the pattern to copy for the scan itself — procedural value as a
starting hint, then walk the real column — which keeps the search short.

## Dead ends, recorded

Do not re-walk these:

- **Precipitation event cap.** 966 tiles competing for 48 slots looks like a 20×
  oversubscription and is not binding: sweeping `max_events_per_tick` 48 → 1024
  moved equilibrium humidity by *one unit*.
- **`max_prob_per_tick` as a tuning lever.** Non-monotonic (0.30 → 331k but
  0.60 → 472k) with eligible tiles swinging 425..1350 between runs. Evaporation,
  humidity and precipitation are mutually coupled and a 3000-tick run from a
  fresh stamp cannot separate them. Tune against a soak.

## Invariants

- **Humidity + world water is conserved exactly.** Measured holding at
  3,869,370 across 3000 ticks. `audit::sat_totals` plus `Humidity::total_mass`
  is the harness; use it on any change here.
- **Parcels must never hold real mass.** `mass` is always 0.0 and `vis_mass` is
  the drawn size. `release_parcels_into_humidity` would otherwise pour a copy
  back into the sky and mint vapour. That function also only releases parcels
  that *do* hold mass — taking the whole list clears the deck every tick, which
  silently made a first attempt at persistence a no-op.
- A conservation test only checks what you point it at. `ground_sat_sum` reported
  conserved mass as lost once rain started nucleating in the air; it needed to be
  a whole-world sum.

## Probes

- `tests/rain_probe.rs` — does it rain, and is it visible? Separates *water
  delivered* from *parcels flagged raining*, reports cloud coverage against
  eligible tiles, day/night rain fraction, and the dead ends above.
- `tests/perf_profile.rs` — frame budget, including the active-cell count that
  gates any per-cell atmospheric work.
