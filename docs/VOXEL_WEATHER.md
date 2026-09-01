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

The N cartoon-bank overlay is **deleted**. `CloudStore` only dumps leftover
save-file parcel mass into humidity and runs buoyant rise. Shade, sky cover,
and the vapour look come from the humidity field (`H`). Use `hum` (total
humidity mass) as the sky-water dial.

### Pinned: wind streak overlay needs a real visual

`V` draws a coarse lattice of short world-space arrows from the local
wind heatmap (every 2nd tile, every 3rd when zoomed out). Direction is
unit-length; length/alpha encode speed with a floor so the Tab default
(~0.05 tiles/tick, weaker near the ground) still reads. The sim field
is unchanged. Default off. Humidity haze no longer uses a global
`sea_level + 4` cut — the per-column live floor clips buried cells.

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

## Climate loop (wind × T × humidity) — budgeted

The coupled weather stack is:

- **Wind heatmap** — `Wind::rebuild_field` every `WIND_FIELD_PERIOD` (4)
  ticks, only on wet seats + a 1-tile halo + a thin near-surface band.
  Misses in `vector_at` return the climate mean after the same
  surface-slip as the heatmap (no into-rock residual; a descent
  turns along the skin).
- **Humidity** — donor-cell fractional flux through that heatmap, with a
  per-column free-air cache so buried seats lift over the crest without
  scanning the hill once per tile. Convection stays `buoyant_rise_thermal`
  (per-row T mean, never `Temperature::mean()` in the loop).
- **Temperature** — still period 20. Night humidity blanket, wind mix,
  and near-surface air↔ground couple live *inside* that step. Do not
  run a full-field `advect_air_with_wind` every tick.

Do **not** resurrect N lobe-mask cloud banks, H per-cell haze as the
default draw, or a full-sky wind rebuild every frame. Those were the
FPS cliff that forced the earlier revert.

### Fog sheet vs lofted cloud (no hardcoded Y pump)

Playtest on the climate-budget branch parked almost all visible vapour
as a film on the live crest, with the sky above washed to a thin equal
haze. Two mechanics did that, and neither is "add a pump to sea+N":

1. **`buoyant_rise_thermal` was capped at `sea + cloud_alt`.** That is
   the Tab “Vapor rise deck above ground” lever. On a mountain the
   surface tile already sits at or above that deck, so the rise no-ops
   and `lift_buried_to_free_air` dumps every buried seat onto the crest.
   The slider is gone; the cap is the humidity `hy_max` / sky box.

   A second leftover Y pump survived that cut: horizontal advection
   snapped dest `hy` to the neighbour's free-air crest, and lift-buried
   hoisted any seat below that crest. Pond vapour that spread one tile
   onto either bank teleported onto **both shores**, then humid-heat
   and drizzle locked two hot rainy columns there. Horizontal flux now
   stays at the same `hy`. Lift-buried only hoists seats deeper than
   this column *and* both neighbours (truly inside a hill). Climate /
   couple sit on the water skin (`live_skin_y`), not the excavated bed
   — an inland pond is not a fake ocean hole.
   Unstable lapse (warm under cold, plus the per-row T anomaly) is what
   organises loft — same walk, no teleport. `cloud_alt_above_sea` stays
   on the struct for save compat only.
The draft itself is ground-heated air. The sun hits the ground; the
ground always radiates. Night is the sun being off, not a second
cool pulse, and there is no noon/midnight skin swing (`day_amp_c` is
retired). Humidity in the column reflects incoming sun (daytime shade)
and blankets the leak. Air only couples to that skin. Warm humid air
under the colder lapse rises (`buoyant_rise_thermal`) and carries heat
with it — vapor's specific heat is ~1.9× dry air
(`humid_heat_scale`, Tab “Humid air heat capacity”). Wet tiles relax
toward climate more slowly, and each lift mixes source T into the
tile above. Surplus condenses aloft and falls. That is the pipe —
not a deck slider.

Keep the loft cheap: row means live on the period-20 thermal step
(never `width × wet-rows` lookups per tick), rise runs every other
tick, and the condensation film floor walks down from the wet tile
(not up from y=0 through the mountain). A lofted sky has more wet
seats — do not pay a full-field scan for each of them.

2. **Condensation wiped the film.** At a few degrees below zero the
   Clausius–Clapeyron sat is ~100–120 mass; `mass_per_droplet` is 255,
   so the first free-air tile rained itself empty every event and never
   fed a column. Rain now leaves `~0.82 × sat` in the tile and **skips
   the lowest free-air row + one above it**, so the source layer can
   lift. Aloft surplus still rains.

   Cold air holding less vapour is a **separate** path
   (`precipitate_thermal_surplus`). Surplus above `saturation_mass_at_temp`
   becomes water (or a snowflake) in that tile. It does not wait for the
   drizzle lottery, and a refused deposit leaves the vapour — never a
   clamp that deletes the mass.

Do not add a sea-level humidity floor or a fixed "cloud row" teleport
to get this look back. If the sky goes sheet-fog again, check those two
gates first.

## Landed: no cloud animation — humidity is the vapour look

**Design call (playtest, 2026-08-26), deleted 2026-09-01.** Drop cloud parcels
entirely rather than replacing them with a derived deck. Tune the *vapour
field's own rendering* instead. The N overlay is gone and is not coming back.

This is "a cloud is just an air block with saturation" taken to its conclusion:
if the field is the cloud, there is no reason to derive an intermediate object
from it. What went:

- `CloudStore`'s visual half — parcels, persistence, drift, dissipation
- `draw_clouds` and the three `draw_depth_cloud_layer` call sites
- `deck_from_field`, `pick_spread_across_x`, `nimbus` / `echo` HUD tags
- the `N` hotkey and Tab Clouds (N visual echo) tree

What stayed: leftover save-file parcel mass dumps into humidity; buoyant rise
still lifts vapour; `cloud_floor_y` still clips the `H` haze against terrain
(not damp air). Shade and sky cover read humidity (`cloud_sky_transmit`,
`precip_cover_fraction`). `H` is the vapour look.

## Superseded plan: a derived deck instead of parcels

Not taken. A humidity-derived deck would still have been a second
representation. The landed call deletes the animation and draws the
field (`H`) instead.

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
forcing exists — `solar_heat_c`, `night_cool_c` (continuous radiate), saturation mass
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

Sun/radiate used to add raw °C. Water's low albedo plus a 0.15 leak made the
ocean a sun magnet while dry sand could net-cool at noon. Forcing is now
` (solar − radiate) / (1 + capacity × inertia × force_inertia) `, water radiates
near land (`water_night_cool_scale` 0.70), and only 12 water cells add stack
capacity. Lakes stay a buffer. Land skins lead the day.

Air lapse follows the tile's own height. The old column-skin climate stamped
`base − lapse × crest` onto every air tile above a hill, which painted a cold
cap over the mountain while the same height over the sea stayed mild. Ground
skin still uses crest elevation (high land is colder). The sky does not.

Convection proper — buoyancy from the ground-versus-air difference — is cheap on
a tile field and is what would turn a steady rain rate into fronts.

## Snow falls gently (**landed**), wind drift (**landed**, carefully)

Snow nucleates in the air (`phase::deposit_snow_in_air`) and now descends at a
usable rate. The bug was that grain fall takes one step per pass and runs several
passes a tick — right for sand settling, and it made flakes appear in the sky and
arrive on the ground in the same breath.

Fixed by *odds* rather than a separate pass, which turned out much smaller than the
plan above assumed: an airborne flake rolls to hold position most passes
(`SNOWFALL_STEP_ODDS` 0.15), so one step is spread over many passes. The roll's
irregularity is itself snow-like. Same technique as the fractional seepage rates,
and it needs no new pass, so the hazard of "grain fall skips snow but the drift pass
does not run" never arises.

Gated on **airborne** snow only. Once a flake lands it is snowpack and behaves as
any other loose material, which is what lets drifts build and repose.

Ice is deliberately not gated and serves as the test control: if both slow down,
the gate is catching the wrong material.

**Wind drift** is a second, once-per-tick pass (`apply_snow_wind_drift`), not a
bias inside grain fall. Grain fall only ever pulls straight down, and it can run
hundreds of passes on a deep settle — putting a sideways step in there would let
a flake cross the map in one tick. The drift pass:

- runs **once** after physics (same place rafts already take the wind)
- moves a flake **at most one cell** downwind
- fires with odds `|wind_vx| * tile_cols * 0.25`, clamped to 1 (default 0.05 × 4 × 0.25 = 0.05 — about **three down for each sideways** against fall 0.15)
- ignores ice (control) and landed snowpack (repose owns piles)
- refuses a solid or occupied destination
- no-ops at zero wind, so every existing fall test stays honest

A flake that falls and drifts therefore walks a stair: three down for each
sideways at default climate wind, never a teleport. The first playtest scale
(`|wind| × tile_cols` with no 0.25) slid more than it dropped. Strong wind
still caps at one cell across per tick.

**Note on the tick.** The fall roll is hashed on `world.tick`, so any loop calling
`apply_grain_fall` repeatedly without advancing the tick re-rolls the same answer
forever. Production advances it; a test did not and now does. Same footgun as the
fractional seepage gate. The drift roll uses the same clock.

## Weather reads the *live* surface (**landed**)

`worldgen::continental_surface_y` still recomputes the original procedural
profile — worldgen needs that. Every weather consumer that asks where the ground
*is* now goes through `live_surface_at`: same value as a hint, then a short walk
of the live column (`LIVE_SURFACE_SEARCH` = 64). Unloaded columns keep the hint,
so tests and HUD that have no grid yet degrade to today's behaviour. A falling
flake is a solid, so the walk used to stop on it — orographic dump, cloud floor
and frost sat on needles. Airborne snow / ice / organic is now peeled; seated
pack stays, because that *is* the surface.

All five consumers flipped together. Partial would have been worse than stale:
orographic rain on the live hill and wind lift on the seed hill would put rain
and lift on different mountains.

| site | uses it for |
|---|---|
| `condensation.rs` `orographic_factors` | elevation/slope for orographic rain |
| `wind.rs` (`orographic_lift`, `ascent_cells`, `is_tall_terrain`) | upslope/downslope lift |
| `phase.rs` `column_may_phase` | where to start the frost/melt scan |
| `clouds.rs` `surface_y` | cloud floor baseline (the walk *up* from it already existed; `.max(rock)` was pinning the floor to a peak that had eroded away) |
| `temperature.rs` skin / thermal-props window | lapse, land/sea bias, and the cheap scan band |

A maintained per-column cache was the other honest option. It needs invalidation
on every solidity change, and `set_cell` is hot enough that the extra write was
not worth it: terrain moves locally, the procedural hint stays close, and the
walk is a few cells.

**Humidity advection follows the live hill.** `Humidity::advect` is still the
uniform `(vx, vy)` residual. The app then spends `Wind::orographic_lift` on
the vapour field (`advect_with_surface`) so mass rises on columns that climb
the *current* surface, not the seed profile. Flatten the downwind face and
the loft stops.

**The 4×4 field and the 1-wide path.** Humidity is a tile store. `H`
paints occupied tiles plus a one-tile neighbour halo (so an emptied
nucleating tile does not become a 4-wide hole). A drop nucleates in
the centre column; that column is left open from the drop downward.
Cells that remain bilinear-sample the store (wrapped on the ring) so
the y=127/128 tile edge is a wash, not a clamp. Draw uses wrapped
world-x so the ring seam is not stacked. Per-column cloud floor so
the clip does not step 4-wide.

## Dead ends, recorded

Do not re-walk these:

- **Occupied-only column rects after a drain.** Painting only keys that
  still have mass leaves a 4-wide hole when a droplet empties its tile.
  Shaft-first resample needs that punched seat (from a neighbour) or
  rain is both 1-wide and 4-wide.
- **12% live-max floor on resampled cells.** An emptied tile samples
  toward zero; the floor ate the sibling columns and widened the hole.
  Soft 2% floor on the wash; keep 12% only for the coarse helper.
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
- **Parcels must never hold live mass.** `CloudStore` is save-compat only: the
  next step dumps leftover parcel mass into humidity and clears the list. Do
  not rebuild cartoon banks from the field.
- A conservation test only checks what you point it at. `ground_sat_sum` reported
  conserved mass as lost once rain started nucleating in the air; it needed to be
  a whole-world sum.

## Probes

- `tests/rain_probe.rs` — does humidity actually drain into ground water?
  Reports tiles wanting rain, day/night rain fraction, and the dead ends above.
  There is no parcel-animation half anymore.
- `tests/perf_profile.rs` — frame budget, including the active-cell count that
  gates any per-cell atmospheric work. Includes snow drift and clay
  suspension (those live outside `tick_with_perf` in the app).

  Fresh-stamp demo (1024×320, 40 warm + 200 measure, 2026-08-27): wall
  **32.9 ms/tick**. Physics is 27.7 of that. Top three: confined wake
  7.7, seepage 7.4, rock bodies 6.3. Snow drift 0.001, suspension 0.09.
  The older 3 ms/tick figure is an *aged quiet* world, not this soak.
