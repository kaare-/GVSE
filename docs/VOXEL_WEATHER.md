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

### Pinned: do not coarsen off-screen sim sky

Terrain already frustum-culls. `H` / `V` skip **paint** (resample,
floor walks, drop-top scans) for tiles / chunks that cannot touch the
viewport. Seat collection probes the camera tile box when it is
smaller than the vapour map, so a soaked sky does not walk every
humidity key to draw a shore strip. Drop-top scans also skip chunks
whose 64-high band sits entirely below the camera (shafts only go
down). That is leftover draw as `hum n` fills.

Coarsening **advect / wind / condensation / temperature** off-camera
would change weather: the world is a ring, vapour wraps, and panning
must find the same mass that ran at full rate while it was off-screen.
Parked. Draw leftover only.

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
  turns along the skin). Humidity caches that sample once per
  occupied seat before the two flux axes so a miss walks the
  world once, not twice (evap after rebuild is the usual new
  key). After blend, 6 Jacobi sweeps project
  `∇·v` on that key set only (not the sky): a valley feels the
  next wall before the last tile. Slip runs last.
- **Humidity** — donor-cell fractional flux through that heatmap, with a
  per-column free-air cache so buried seats lift over the crest without
  scanning the hill once per tile. Vertical flux clamps `|vy|` to 0.10
  so face-following / Jacobi climb cannot empty a tile in one hop
  (that vacuumed vapour below `min_mass_to_rain` and stopped C drops;
  the V overlay still shows the raw field). Convection stays
  `buoyant_rise_thermal` (per-row T mean, never `Temperature::mean()`
  in the loop). Condensation that samples a solid tile centre walks
  up to four cells for the first Air (rain) or empty Air (snow)
  before refusing. Rain pays 33 (visible drop + H shaft) and only
  lotteries when the tile is over the Clausius–Clapeyron hold
  (`saturation_mass_at_temp`). Below freeze the C pass never deposits
  water: it gathers neighbour humidity for a 255 flake, or holds.
  Snow does not carve the shaft.
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
   becomes rain **only above freeze**. At or below 0 °C it is a snowflake
   paid from the local 3×3 humidity parcel (a flake still costs 255 —
   thaw is a full water cell; snow does not carry sat yet), or a hold if
   the parcel cannot pay. It does not wait for the drizzle lottery, and
   a refused deposit leaves the vapour — never a clamp that deletes the
   mass, and never liquid rain at −20 °C.

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

Air lapse follows the tile's own height up to a **tropopause knee**
(`tropopause_elev_cells`, default 920 above sea 80 → **y = 1000**). One cell
is 0.25 m, so that weather column is only **~250 m** — coarse, but it keeps
peaks and climatic zones in the lapse instead of flattening halfway up.
Above the knee the profile is a weak stratospheric slope (default 0 —
isothermal). Default sky is 1064 (1000 + one 64-cell lid). Tile fields are
4×4, so height still costs CPU; drop the ceiling if a machine cannot hold
it. The old column-skin climate stamped `base − lapse × crest` onto every
air tile above a hill, which painted a cold cap over the mountain while the
same height over the sea stayed mild. Ground skin still uses crest elevation
(high land is colder). Buoyant rise stops at the knee so the lid stays dry.
`0` tropopause restores the linear profile. The old `sea + cloud_alt` deck
cap is still gone — that parked fog on ridges.

Convection proper — buoyancy from the ground-versus-air difference — is cheap on
a tile field and is what would turn a steady rain rate into fronts.

## Snow falls gently (**landed**), wind drift (**landed**, carefully)

Snow nucleates in the air (`phase::deposit_snow_in_air`) and now descends at a
usable rate. A flake costs a whole cell: do not let the cloudy remnant shave
that budget, and walk a few cells up from a solid tile centre (same as rain)
so a slope still snows. Rain carves the 1-wide H shaft; snow floats
through the wash and does not. The bug was that grain fall takes one step per pass and runs several
passes a tick — right for sand settling, and it made flakes appear in the sky and
arrive on the ground in the same breath.

Snow starts when a tile is **over** the Clausius–Clapeyron hold and the
local 3×3 parcel can pay a 255 flake. Inspector `humidity=` is that tile
mass: **−0.1 °C → ~206** (cell-sat picture ~21/255), **−3 °C → ~162**
(~16/255). Below freeze we never rain.

Fall is gated (`SNOWFALL_STEP_ODDS` 0.38) plus a cheap once-per-tick
`apply_airborne_snow_fall` on `has_snow` chunks. Flakes do **not** count
as unsupported grain — that used to force the ×64 deep settle every
tick and drop FPS to single digits. Surplus / C mint at most 8 flakes a
tick and skip a column that already has one nearby. The roll's
irregularity is itself snow-like. Same technique as the fractional
seepage rates, so the hazard of "grain fall skips snow but the drift
pass does not run" never arises.

Gated on **airborne** snow only. Once a flake lands it is snowpack and behaves as
any other loose material, which is what lets drifts build and repose.

A flake that falls into air **above freeze** melts to `Air+FULL` rain the
same tick (`thaw_airborne_snow`). Mid-air rain is gravity-only — surface
flow used to peel `drain_step_cap` (160) off that 255 cell and leave 95
beside it (two drops from one flake). The ordinary phase thaw only looks
at a band above the ground and only on `period_ticks`, so a cold-lid
shower used to ride the warm column as snow until it neared the surface.

Ice is deliberately not gated and serves as the test control: if both slow down,
the gate is catching the wrong material.

**Wind drift** is a second, once-per-tick pass (`apply_snow_wind_drift`), not a
bias inside grain fall. Grain fall only ever pulls straight down, and it can run
hundreds of passes on a deep settle — putting a sideways step in there would let
a flake cross the map in one tick. The drift pass:

- runs **once** after physics (same place rafts already take the wind)
- moves a flake **at most one cell** downwind
- fires with odds `|wind_vx| * tile_cols * 0.25`, clamped to 1 (default 0.05 × 4 × 0.25 = 0.05 — several **down for each sideways** against fall 0.38)
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

  After the leftover-cost cut (this host, 40 warm + 200 measure, 0 plants):
  short sky wall **35.2 ms/tick**, humidity.advect **3.4** + wind.rebuild **2.3**
  (was ~6.8 miss-path walks with an empty field). Tall 1064 wall **28.3**,
  humidity.advect **2.5** + rebuild **1.9**, seepage **5.5**, bodies apply
  **6.8**. Dry-halo / empty-sky skips do not change the wet-crust apply.

  After the wet-apply cut (wake tiles stay local, pond-interior seepage
  skip, FPS topology on interactive): short wall **31.7**, seepage **6.9**,
  bodies **4.6**. Tall 1064 wall **24.5**, seepage **5.1**, bodies **3.3**.

  After the confined / lake-bed occupancy skip (standing water next to
  rock only; rain-film sky and groundwater-only crust dropped): short
  wall **32.3**, seepage **6.9**, confined wake **2.2**. Tall 1064 wall
  **23.1**, physics **12.6**, seepage **5.0**, confined wake **2.1**,
  bodies **3.3**. The leftover confined ~2.1 ms is the real ocean/lake
  communicating-vessel walk, not drizzle columns.

  After the wet-crust seepage skip (perimeter-only weep on buried
  crust, lake-bed skips a full water table, both-full pore faces skip
  head math): short wall **31.0**, seepage **5.3**. Tall 1064 wall
  **22.2**, physics **11.1**, seepage **3.5**, confined wake **2.2**,
  bodies **3.3**. Split probe on demo: lake-bed 1.6, weep 1.6, deep
  1.7, seam couple 1.5 (was 2.0 / 2.4 / 3.3 / 3.4).

  After mid-ocean lake-bed peek, rain-sky evap skip, and uncased
  confined reject: short wall **30.6**. Tall 1064 wall **21.4**,
  evap→humidity **1.6**, seepage **3.5**, confined wake **2.2**.
  Lake-bed split 1.5 (was 1.6). Confined ~2.2 is still the ocean/lake
  communicating-vessel walk.

  After the humidity mix/lift clone skip and wind Jacobi ping-pong
  (this host, 40 warm + 200 measure, 0 plants): short wall **28.3**,
  humidity.advect **1.8** (was 3.4), wind.rebuild **2.4**. Tall 1064
  wall **20.1**, humidity.advect **1.3** (was 2.4), wind.rebuild
  **1.8**, evap→humidity **1.4**, seepage **3.5**, confined wake
  **2.1**. Flux / lift / mix math is unchanged. Confined leftover is
  still the ocean/lake communicating-vessel walk.

  After the confined standing-air y-band (plus one row for the rising
  film): short wall **28.1**, confined wake **1.5** (was 2.2). Tall
  1064 wall **20.0**, confined wake **1.4** (was 2.1), humidity.advect
  **1.3**, wind.rebuild **1.8**. Dry sky in shore chunks is skipped;
  wells / ocean equalise unchanged. Wind rebuild leftover is
  compose + project on occupied seats.

  After lake-bed standing-only y-band (dry sky skipped; unsat fronts
  keep the full rect): split probe lake-bed **1.3** (was 1.5). Sky-height
  seepage bucket stays **~3.6** (apply + weep + seam dominate). Tall
  1064 wall **20.7**, confined **1.4**, humidity.advect **1.3**,
  wind.rebuild **1.8**.

  After soak-age occupancy (clay / soluble / standing-air gates, plus
  condensation mass-before-floor): short wall **27.1**, tall 1064 wall
  **19.4**, humidity.advect **1.3**, wind.rebuild **1.9**, seepage
  **3.5**, confined **1.4**, condensation **0.48**, flow erosion
  **1.0**, suspension **0.06**, karst **0.04**. Fresh-stamp leftover
  is unchanged — these cuts bite as drizzle soaks land and humidity
  fills. `soak_age_inventory` (256 plants, climatic rain off, 8×400):
  wall **22.6 → 86.4**. `clay` stays **23**, `stand` stays **~28**
  (gates hold). Growers that are real work, not occupancy leaks:
  `hum n` **13k → 55k / 68k** (condensation **0.67 → 5.1**), dirty
  halo **7k → 33k**, `diss` **1.1k → 10.5k**, plant `mods` **2.1k →
  3.9k**, `susp` **6 → 286**. `loose` **34 → 74** and `buoy` **0 → 48**
  track litter, not a sticky-flag leak. Confined **2.6 → 9.1** then
  **6.8** with a flat stand count is leftover communicating-vessel
  work, not rain-wet land.

  After lottery-before-floor, dissolved-key snapshots, and confined
  chunk-local neighbour reads (this host, 40 warm + 200 measure, 0
  plants): short wall **26.6**, tall 1064 wall **19.5**, condensation
  **0.48**, seepage **3.6**, confined **1.5**, flow erosion **0.89**.
  Fresh-stamp is unchanged — these skips are leftover on filled /
  karst-aged worlds. Repeat soak (256 plants, 8×400): wall **22.9 →
  88.7**. Condensation **0.67 → 5.1** still tracks `hum n` **13k →
  55k** (over-sat tiles must lottery). Confined **2.7 → 8.3** then
  **7.0** is still equalized-shaft BFS, not neighbour HashMap probes.
  `clay` stays **23**. Do not skip the lottery; do not starve the
  confined wake.

  After rock-only confined casing (this host): fresh-stamp short wall
  **26.7**, tall **19.1**, confined **1.3 / 1.2**. Soak (256 plants,
  8×400): wall **23.2 → 90.2**. Confined **2.2 → 8.1** then **6.3**
  — same leftover as before. Plants / grains as walls was correct
  physics (films must not rise) but not the soak BFS. `clay` stays
  **23**.

  After skipping uncased BFS at the world’s highest standing-air row
  (this host, 40 warm + 200 measure, 0 plants): short wall **26.3**,
  tall **19.1**, confined **1.3 / 1.2**. Repeat soak: wall **23.2 →
  88.3**. Confined **2.2 → 8.2** then **6.3**. A high tarn keeps
  `max_stand` above the ocean, so coastal films still walk. Do not
  skip uncased higher-row rise — `confined_head_equalizes_across_large_deep_ocean`
  stalls (shaft top 36 vs sea 40). The leftover is equalized
  rock-cased shafts / connected ocean BFS below a higher standing
  row. Do not starve the wake; do not lower `CONFINED_HEAD_BFS_LIMIT`.

  After FxHash + reused BFS buffers (this host, 40 warm + 200 measure,
  0 plants): short wall **25.2**, tall **18.2**, confined **0.62 / 0.57**
  (was **1.3 / 1.2**). Soak (256 plants, 8×400): wall **21.8 → 86.3**.
  Confined **1.1 → 3.8** then **3.0** (was **2.2 → 8.2** then **6.3**).
  Same cells, same rise — SipHash and per-call alloc were leftover on
  the equalized walk. `clay` stays **23**. The remaining confined grower
  is still that walk, just cheaper. Do not starve the wake.

  After snapshotting humidity advect as a `Vec` (this host, 40 warm +
  200 measure, 0 plants): short wall **25.7**, tall **18.2**,
  humidity.advect **1.74 / 1.29** (same as the mix/lift skip). Repeat
  soak (256 plants, 8×400): wall **21.7 → 85.2**. Condensation
  **0.63 → 4.75** still tracks `hum n` **13k → 55k / 68k** (over-sat
  tiles must lottery). Flux math is unchanged — the SipHash `clone`
  was leftover on the snapshot, not the soak grower. `clay` stays
  **23**. Do not skip the lottery.

  After FxHash dissolved-load indexes and condensation `(key, mass)`
  snapshots (this host, 40 warm + 200 measure, 0 plants): short wall
  **25.4**, tall **18.1**, condensation **0.30 / 0.26** (was **0.49 /
  0.44** — leftover SipHash `at_tile` on the lottery walk). Repeat
  soak (256 plants, 8×400): wall **21.2 → 80.7** (was **21.7 →
  85.2**). Condensation **0.33 → 3.21** (was **0.63 → 4.75**). Flow
  **0.66 → 2.78** (was **0.74 → 3.82**). `hum n` still **13k → 55k**,
  `diss` still **1.1k → 10.3k**, `clay` stays **23**. Same cells, same
  mass — SipHash on load-index `contains` and a second humidity-map
  get were leftover as karst / sky fill. Lottery still walks every
  over-sat tile. Do not skip it.

  After skipping leftover flow/gravity on dry inland chunks (Moore
  neighbour still scanned so dry dest cells keep the +x equalise
  edge) and hashing temperature scratch / diffuse with FxHash (this
  host, 40 warm + 200 measure, 0 plants): short wall **24.4**, tall
  **17.5**. Temperature.step **4.5 / 10.8 ms/call** (was **6.6 /
  18.5** — leftover SipHash clone + sort). Repeat soak (256 plants,
  8×400): wall **20.3 → 80.5**. Flow **0.67 → 2.67**, gravity
  **0.52 → 2.26** — same leftover as before. Plants sit on rain-wet
  land, so the dry-inland skip does not move soak. `clay` stays
  **23**. Do not skip dry flow cells that own the +x equalise edge.

  After solving wind Jacobi pressure in `Vec`s (this host, 40 warm +
  200 measure, 0 plants): short wall **25.0**, tall **17.5**,
  wind.rebuild **2.42 / 1.86** (same as **2.32 / 1.87**). Repeat soak
  (256 plants, 8×400): wall **20.6 → 82.7**. Same six iterations,
  same slip — HashMap clone + per-iter insert were leftover hasher,
  not the compose / `face_blocked` cost. `clay` stays **23**. Do not
  retry the Jacobi solid-cache.

  After skipping unchanged far-sky temperature writes and running
  diffuse on a dense slab when the box is full: leftover hasher on
  1000-cell columns already on the lapse. Same couple / skip / pair
  stencil — tiles that did not move are not rewritten. Not view LOD;
  the ring still steps at full rate. Do not coarsen off-screen sim.

  After packing that slab once per temperature step (couple writes
  it; diffuse and row-means reuse it) and skipping far-sky / deep-crust
  `live_surface_at` on the props refresh (one seed-rock walk per
  column, same Air / Buried early-out; this host, 40 warm + 200
  measure, 0 plants): short wall **25.0**, tall **16.9**.
  Temperature.step **3.46 / 6.74 ms/call** (was **4.5 / 10.8** —
  leftover per-tile rock walk on Air already classified, plus a second
  HashMap pack for diffuse). Same couple / skip / pair stencil.
  Surface-band tiles still scan. Repeat soak (256 plants, 8×400):
  wall **20.1 → 80.7** — same leftover as before. Temperature is a
  period-20 hitch, not the soak grower. `Temperature::cells` stays
  `HashMap` for serde. Do not coarsen off-screen sim.
