# Steam pipe (cell-resolution leftover)

**Status:** implemented beside the leftover field. Play default: pipe on,
leftover field off. Leftover tests keep the field (`SteamConfig` defaults).

## Where the code lives

| File | Holds |
|------|-------|
| `pipe.rs` | This motor: straws, the book, flash / pulse / wick, eruption, the `P` overlay. |
| `steam.rs` | What both motors share, and what only the pipe now uses: the sky probe, vessel classification (weather vs boiler), cavity humidity, haze, recondense, and the cadence. |
| `steam/leftover.rs` | The legacy pre-pipe motor — Dijkstra pressure field, chimney pinning, straw chains, route locking — behind `enable_leftover_field`, with its own tests. |

`steam.rs` was 10.9k lines with the two motors interleaved; the split is a
pure move, verified line by line against the commit before it.
**Crate:** `wk-voxel` (`pipe.rs`).

## Player surface

- **P** paints the pipe: dim claimed boiler, mid straw, bright mouth,
  brightest live puff. The leftover field owns P only when the pipe is off.
- HUD `steam=` is `Nc P=mains+feeders/cells sat=<mass> u=<live>` when the
  pipe is on; leftover field prints `L=zone/pin` instead.
- HUD `cave_h=` is sealed ambient cave cells, not the `steam=` field.
- F6 glossary pages 2 / 4 / 6 cover straw / feeder / mouth / sinter / HUD.
- Tab → Climate → Cavity humidity: pipe on/off, leftover field, sides,
  stroke, beat, erosion gain. Cadence sliders are inert while the pipe is on.
- Inspector prints `pipe=` (P band) on a pipe cell; `leftover=` only when
  the leftover field owns P.

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

## Surface

A mouth is Air the atmosphere can reach, and there are **two** ways to qualify
because each covers the other's blind spot:

- Above the column's **rock crest** — the open landscape.
- Below it, a cave whose void **reaches out past the crest** somewhere
  (`void_reaches_open_air`). A cave is under the terrain by definition, so the
  crest test alone rejected every one of them: the straw walked through the
  cave's air and kept climbing, which is where pipes through air came from.

Neither a local roof probe nor `air_void_open_to_sky` can stand in for this.
Both consult `void_is_confined`, which probes a fixed distance up for a ceiling,
so a cavern taller than the probe is declared open — `air_void_open_to_sky`
short-circuits on it before its own BFS ever runs. Reachability measured against
the crest cannot be fooled that way.

### Finding the crest

`live_surface_y` is the same descent humidity's surface map performs. What
matters is where it starts:

- The continental hint is only a guess, and a guess can land outside the loaded
  column (reported a crest of -35, below bedrock), inside the hill, or **inside
  a void within it**. That last one is the worst: the descent found the cavern
  floor and called it the surface — the roof-probe blind spot reached by another
  route. So the crest always comes down from the top of the loaded column.
- The mouth test reads the **rock** crest, not the waterline. A straw venting
  into its own spring pool lifts the skin above its own mouth, and testing the
  skin disqualified it, so the straw lost its discharge and banked every beat
  after. The aiming target still uses the skin, so a straw prefers to clear the
  pond.

Deliberately **not** cached: a per-column cache has to be invalidated on every
Air/solid flip and `sky_topo_gen` does not track them finely enough. A stale
crest is a straw routed against terrain that has changed, which is the class of
bug this rule exists to kill. The walk is only reached for Air candidates, since
rock fails on material first.

## Eruption

Every `PIPE_ERUPT_PERIOD` beats a main erupts; in between it simmers at
`stroke / period` so a charge accumulates.

On an eruption beat a sky mouth throws its charge up the open air, where it
renders (`erupt_jet`):

- **Water first, vapour second.** `PIPE_JET_LIQUID_EIGHTHS` of the charge
  goes in as liquid along the lower column, so it has weight and rains back
  down; the rest is cavity vapour over the whole height. Both are ordinary
  cell water and `World.steam`, so gravity and the water rules take over.
- **Tapered, not halved.** Halving put half the charge in the first cell and
  a quarter in the second, so every burst was one blip above the lip however
  much was behind it. Linear weights spend the charge over the whole height,
  densest at the base. Rounding remainder banks at the *base*; at the tip it
  inverted the taper.
- **Height buys reach.** A burst of `M` sat can be short and dense or tall
  and thin, not both. At `PIPE_JET_SAT_PER_CELL` the base stays around four
  units while height still grows with the charge, so force reads as reach.

It stops at rock rather than tunnelling, and only what will not fit falls
through to the sky humidity field, where it stops being visible.

## Solute and sediment

The chain from erosion to sinter, and what each species can actually do:

| Species | In the straw | At the mouth |
|---------|--------------|--------------|
| **Dissolved mineral** | Rides pore water: `deliver_liquid` carries it on every pump and wick hop. Boiling leaves it behind, which is why it concentrates at the flash front. | `precipitate_vent_mouth` builds sinter and an apron. |
| **Suspended silt** | Does **not** travel. `sediment::carry_with_water` refuses pore space by design — a grain bed filters fines. | Free water at the vent can hold it; the eruption jet's liquid can move it. |

### Where the root goes

A spring rises from the middle of its heat, so a root is the **heat-weighted
centre** of the body it drains (`hot_centre_of`), snapped to the nearest body
cell that holds water — a dry cell cannot flash. Candidates used to be taken in
tile-scan order, so the root was whichever wet hot cell the sweep met first: a
corner of the reservoir, often out where the rock is barely over boil. That is
the coolest part of the body and the longest way from anywhere, so the walk set
off across the hill instead of climbing out of it.

### Heat resolution

Heat lives on 4×4 tiles, and where that grid shows depends on what reads it:

- **Erosion** samples `sample_bilinear`. It carves the rock, so its gate is
  what the player ends up looking at — with tile heat, every cell in a tile
  yields or refuses together and the temperature grid prints itself into the
  stone as square zig-zags along the edge of the reservoir.
- **Claim membership** stays on tile heat. Sampling bilinearly there shrinks a
  small hot region to its interior and can split one body in two, and one hill
  wanting to be one spring matters more than a tidy claim edge.

### Erode → carry → deposit

**Flow erodes**, so erosion hangs off `hand_sat` — the one place every pipe
liquid transfer passes through, both the wick draining the reservoir and the
pump climbing the straw — with the water that actually moved as the
throughput. That is the signal seepage already uses, and it keeps erosion on
the paths that carry flow rather than everywhere the rock happens to be wet.
Keying it off lumen pressure instead only reached the few cells the pulse had
touched, and never the reservoir at all: a whole hill could drain through the
rock and leave it untouched. `mint_void` stays false, so a conduit stays rock.

**The load deposits where the water left it.** The flash front migrates up the
conduit as the cells under it dry, and solute stays behind when water boils
off, so the load piles up in dry rock near the top of the straw — nine
thousand units against three at the vent, when only the mouth was checked.
`deposit_along_straw` uses `precipitate_at`, whose own triggers are "the water
left" and "over the ceiling"; a dried cell holding a load is the first
exactly. Sinter lines the conduit and builds around the vent.

**The reservoir deposits too**, not only the straw. The wick erodes the body as
it drains it, so most of the freed carbonate is out in the reservoir — but
deposition only ever walked the straw, so a body cell that had dried still held
its load with nothing that would come for it. Load standing in *wet* rock is
dissolved in its pore water and belongs there; it comes out where the water
goes.

**A hot vent boils its puddle off and keeps the mineral** — the travertine
mechanism. Without it, arriving water pooled at the lip, which both refused
further delivery *and* held `carrying_capacity` (which scales with saturation)
far above the load, so there was never an excess to drop. The evaporated water
goes to the **sky**, not the cavity book: nothing drains cavity humidity at an
open cell while the pipe owns the loop, so routing it there saturated the cell
at 255 and stopped the turnover dead.

Erosion and deposition are a closed loop — flow opens the aperture, the
mineral it carries lines it again — so the pair settles instead of running
away.

Three units traps found wiring this up, each of which silently did nothing:

- `widen_aperture` reads `throughput` on the **sat** scale against a yield
  threshold. The pipe moves a few sat per hop, so the transfer needs
  `pipe_erode_gain` to clear it; converting lumen volume back to sat divides
  the expansion out and lands under the threshold, and nothing eroded at all.
- `mineral::carry_with_water` rounds its pro-rata share down. A one-sat
  transfer out of a 33-sat cell rounds to zero, and to zero again on every
  transfer after, so load never left the root. It now carries at least one
  unit, still strictly conservative.
- Deposition in rock **occludes pore** rather than minting Flowstone, so it is
  real but invisible. Only a full cell's excess in open Air becomes a visible
  deposit.

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
- A straw whose root has gone cold, been dug out, or turned to air is
  **retired** (`retire_cold_straws`). Nothing did this, so dead straws held
  their slots forever; with the book at `PIPE_MAX_MAINS + PIPE_MAX_FEEDERS`
  there is zero room, `collect_boiler_cands` returns nothing, and a
  genuinely full hot reservoir could never get a straw and never triggered.
  Temperature is the signal rather than wetness: a root is routinely dry for
  a beat right after it flashes.
- Candidates for a free slot are weighed by the **size of the body** each
  would drain, largest first. Bottom-left tile order meant whoever arrived
  first kept a slot regardless of what it was draining.
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

The sweep reaches `PIPE_WICK_REACH` **past** the claim, through any wet porous
rock. The claim is the *boiling* body, but a spring's drainage basin is not the
same thing as its hot vessel: groundwater under the root is cooler than boil, so
it was never claimed and the wick could not touch it — even though it sits under
the most head and is the obvious thing to draw on. Bounded, so a straw recharges
from the aquifer around it rather than siphoning the whole water table.

The sweep is seeded from **every** straw at once, and hands water inward at
BFS discovery rather than through a parent map — discovery order already is
nearest-first. Running it per path flooded the same reservoir once per
straw.

## Abandoned lumen

A main is rewalked every beat, so its route shifts as the hill changes.
`pulse_path` only lifts live steam from cells on its **current** path, and
nothing else touches `pipe_steam`, so units parked along a route that moved
used to stay there forever: the old straw kept painting beside the new one,
`u=` climbed each time a route moved, and water wicked out of the hill
disappeared into lumen it had no way to leave.

`reclaim_orphan_lumen` runs once routes have settled for the beat. An
abandoned conduit is rock again, so its vapour recondenses there: into the
cell if it has room, else parked as standing water, and only a sub-sat
remainder stays residual. The soak asserts no cell holds live steam off the
book.

## The claim is the vessel, not the water

`claim_wet_hot_body` spreads through hot **porous** rock whether or not a
cell currently holds water. Requiring water in every cell meant a draining
reservoir tore into fragments as its pores emptied, and each fragment then
read as its own spring — one well-behaved pipe becoming several needles side
by side. A patchy body claimed 2 cells instead of its whole extent.

The **seed** must still be wet, so a dry hot mass never invents a spring.

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
| `pipe_erode_gain` | 8 (Tab 0…64) | Throughput gain when flow erodes; 0 = off |
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

`PIPE_JOIN_MAX_REACH` caps the reach absolutely, because the bias is a
*ratio*: a deep spring under a tall hill has a `surface_dist` in the
hundreds and would otherwise adopt a main hundreds of cells away. Beyond the
reach, two hotspots are two springs.

A feeder is a **buried** conduit. `walk_toward` scores openness, and Air is
the highest rank there is, so a feeder heading sideways toward another straw
would surface and fly across the sky in a straight line. It now refuses cells
above the column's rock crest. `walk_pipe` can prefer air because it is aiming
at the sky and stops at a mouth; a feeder joins two springs underground.

One **main** straw walks to the free surface and is **rewalked every
beat**, so a carve / collapse / new waterline gets a new route. Feeders
pulse steam and
pump pore water toward the junction; the main pulses toward the mouth.
Shallow springs that are closer to the sky than to the main keep
their own vent. When the pipe is on, leftover's 28k field / Dijkstra
and steam cadence (boil / flood / assault) stay off. P paints the
straw, feeders, live puff, and claimed boilers.
With the pipe on it owns the boiling loop, so only `boil_point_c`,
`phase_expansion_drive`, `pipe_sides`, `pipe_stroke`, `pipe_beat` and
`pipe_erode_gain` do anything — the rest of that submenu drives the legacy
cadence and leftover field, which are inert. The Tab panel says so, because a
slider that does nothing is worse than no slider.

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
