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

### Open: droplets fall too fast

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
what distinguishes a droplet from a column.

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

## Next: clouds drawn from the field, parcels deleted

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

## Then: convection

The remaining reason it rains constantly rather than having weather. All the
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
