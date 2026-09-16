# Steam pipe (cell-resolution leftover)

**Status:** implemented beside the leftover field. Play default: pipe on,
leftover field off. Leftover tests keep the field (`SteamConfig` defaults).
**Crate:** `wk-voxel` (`pipe.rs`).

Temperature only **ignites**. Steam, pressure, and the overlay live on the
**water / seepage grid**. Seepage still moves liquid. The leftover zone
flood is not this motor.

Read first: [`VOXEL_GEYSER.md`](VOXEL_GEYSER.md) (mass / mouth / mint
locks), [`VOXEL_GROUNDWATER_VEINS.md`](VOXEL_GROUNDWATER_VEINS.md).

## Book

- 1 sat = `expand` steam units (1400 at Tab max).
- Occupancy (fill / overlay) = `liquid + live units`.
- Mass = `liquid + (live + residual) / expand`.
- Residual after collapse does **not** occupy seats.
- `World.steam` (`u8`) remains pressurized **cavity humidity**. Pipe
  units are `World.pipe_steam` / `pipe_res` / `pipe_steam_t`.

## Pulse

1. Flash: pay sat, credit `sat × expand` live units at tile T. Intake is
   **capped to one stroke's worth of sat** per beat (`pipe_flash_capped`).
   An uncapped flash mints far more volume than a stroke can carry, so the
   surplus banked in the lumen forever.
2. Walk toward the column's **sky-open vent** (`sky_open_y`, not
   `live_surface_y` — the latter stops at the first non-solid cell, so an
   enclosed cavity read as a surface and the straw dead-ended inside rock).
   Score, in descending weight:
   - `openness_rank`: Air > snow/water/ice > loose > sand > gravel > stone >
     bedrock. Dominant, so Air always beats rock.
   - **permeability**, which separates beds that `openness_rank` lumps
     together — the tight and fractured parts of one rock type. Without it
     the walk was a dead-straight vertical bore, ignoring a fractured seam
     one column over.
   - distance to the vent, then a climb bonus. Dipping back down is
     penalised or the straw wobbles into a bed it has already crossed.

   A step may drift `WALK_DETOUR_SLACK` past the **best** distance reached
   so far, which lets the straw track along a bed to an easier crossing.
   Measuring against the best rather than the current cell is what stops
   drift compounding into a wander.
3. The pulse is a **conveyor**: at every hop it lifts live steam parked by
   earlier beats and carries it along (bounded by `PIPE_SWEEP_STROKES`).
   Without the sweep, `Park` / `Displace` stranded units permanently.
4. Mix **on the incoming face** (`heat / sides`, default 4) **before**
   displacement.
5. `mix_T < boil` → collapse. 1400 units → +1 sat; leftover `< expand`
   stays residual. Full seat → liquid overpressure along the path.
6. `mix_T ≥ boil` → steam lives. Spare seats: park beside liquid. Full
   cell: steam takes the seat, liquid continues.
7. Collapse ≠ boil. Boil is this cell’s own liquid flashing.
8. Seepage never moves steam. Solute rides liquid only.
9. 4×4 tiles only **ignite** and **pool residuals**. Ignite walks
   hot tiles on the beat, not the wet world.

## Book invariants

Enforced by `rewalk_network` every beat:

- Mains are rewalked from their roots, and one that no longer routes is
  dropped.
- A feeder that cannot land on a main is **dropped**, never kept. Keeping
  it meant it was never rewalked again — a straw frozen over terrain that
  had changed under it, which is what put routes in mid-air, and its stale
  cells still drove `rebuild_claimed`.
- With no main on the book, orphans are tried **until one walks**. Spending
  a single attempt on `feeders[0]` left the whole book frozen when that one
  root was unroutable: no main, every feeder stale, and the stale claim
  starving `collect_boiler_cands` so no main could ever be rebuilt. That
  deadlock read `P=0+24/21132` on the HUD.
- **One straw per root.** A duplicated root is one spring drawn twice: it
  doubles intake and paints a second needle beside the first.
- A cell flashes **once per beat** however many straws cross it. Feeders
  share their main's cells, so flashing per path multiplied intake by the
  overlap and defeated the per-beat cap.

## Intake

Each beat, before flashing, `wick_reservoir` draws the **claimed body**
toward the straw that drains it: multi-source BFS out from the straw's
cells, then every claimed cell hands one stroke's sat to the neighbour one
hop closer in. Nearest cells move first, so the whole body advances a hop
per beat instead of stalling behind full cells.

Without the wick the claimed set was only paint. Intake was whatever sat
happened to stand in the straw's own cells, refilled purely by seepage
through its walls, so a thousand-cell reservoir fed a one-cell straw at
sipping rate no matter how much hot water stood behind it.

The body is allowed to be gridlocked: when the straw is at capacity there
is nowhere for the water to go and nothing moves. Flash is what makes room.

The sweep is seeded from **every** straw at once, and hands water inward at
BFS discovery rather than through a parent map — discovery order already is
nearest-first. Running it per path flooded the same reservoir once per
straw.

## Cost

`apply_pipe_motor` on a 31k-cell reservoir, measured by
`tests/pipe_overlay_profile.rs`: **12.5 ms → 5.8 ms** per beat.

- `rebuild_claimed` floods the body, so it is skipped on the second pass
  unless `attach_new_boilers` actually changed the book — which is nearly
  every beat.
- `claim_wet_hot_body` claims on **push**, not pop. Popping made a cell
  discovered by several neighbours pay a chunk lookup and a temperature
  lookup once per discovering edge, up to eight times per cell.
- The `P` overlay: no leftover flood while the pipe owns `P`, and vertical
  runs merged (31.6k quads → 535).

## Mass

Every hop is conservative, and two paths used to break that:

- `pool_residuals` discarded `add_sat`'s shortfall. A full park cell
  refuses the minted sat, and dropping the return value deleted it — a
  saturated reservoir leaked hundreds of sat per beat. The mint now spreads
  across the tile and whatever still will not fit stays residual.
- The carried condensate packet was a `u8`. A pulse accumulates liquid at
  every hop, so on a long lumen `saturating_add` truncated it. It is now
  `u32`, handed over in 255-sat chunks by `deliver_liquid_bulk`.

`long_soak_of_apply_pipe_motor_is_mass_flat` asserts exact conservation
over 1000 beats: opening sat + recharge == cells + humidity.

## Tunables (`SteamConfig`)

| Knob | Default | Meaning |
|------|---------|---------|
| `enable_pipe` | play yes / tests no | Run this motor |
| `enable_leftover_field` | play no / tests yes | Old 28k leftover zone |
| `pipe_sides` | 4 | Face fraction `1/sides` |
| `pipe_stroke` | 1400 (Tab max 7000) | Units per beat |
| `pipe_beat` | `STEAM_EVERY` (5) | Pulse period |
| `phase_expansion_drive` | 96…1400 | Flash volume |

## Joining

A boiler becomes a **feeder** when reaching an existing straw is cheaper
than boring its own bore to the surface:

```text
dist_to_path(boiler, main) < surface_dist(boiler) × PIPE_JOIN_BIAS
```

The bias exists because the two sides are not alike. `dist_to_path` is
measured through whatever rock is in the way; `surface_dist` is a plain
vertical count that ignores how hard the climb would be. Without it a
spring ten cells under a broad hill always preferred its own bore to a
trunk forty cells sideways, and a soak grew parallel mains straight to the
surface (`P=8+24`) instead of one mainline. Reaching a conduit that already
exists is worth several times its distance in fresh rock.

`PIPE_MAX_MAINS` is a backstop, not a target: a soak that reaches it is
drawing needles rather than a network.

One **main** straw walks to the free surface and is **rewalked every
beat**, so a carve / collapse / new waterline gets a new route. Feeders
pulse steam and
pump pore water toward the junction; the main pulses toward the mouth.
Shallow springs that are closer to the sky than to the main keep
their own vent. When the pipe is on, leftover's 28k field / Dijkstra
and steam cadence (boil / flood / assault) stay off. P paints the
straw, feeders, live puff, and claimed boilers.
Tab → Climate → cavity humidity tunes pipe on/off, leftover field,
sides, stroke, beat.

Open-sky mouth leaks **mass** only: hot → sky H, cool → distilled
liquid at the lip. **Sealed-void mouth** (walker terminates in a
roofed cavity) deposits units into `World.steam` cavity humidity
instead — sub-`expand` remainder banks in `pipe_res` at the mouth
so successive pulses can mint 1 sat. Deposit is mouth / apron only
— never the live lumen. While the pipe is on, cadence is off, but
[`recondense_cool`](../crates/wk-voxel/src/steam.rs) still runs on
the STEAM_EVERY beat so stale cavity vapour cannot strand when a
boiler dies.

**Eruption rhythm.** Feeders simmer every beat (steady water pump
+ steam trickle toward the main). The main itself follows a
`PIPE_ERUPT_PERIOD = 8` cycle: seven simmer beats at `stroke / 8`
so most of each beat's flash stays banked as live at the root, then
one erupt beat that unleashes everything live has accumulated. A
geyser plays as a real periodic jet, not a constant thin puff.

## Locks

Mass-flat. `mint_void = false`. No sealed steam → sky H. Volume never
written into `sat`. Do not sinter the live lumen.
