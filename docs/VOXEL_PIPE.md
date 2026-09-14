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

1. Flash: pay sat, credit `sat × expand` live units at tile T.
2. Walk most-open neighbor that still goes toward surface
   (`live_surface_y` / unroofed Air). Openness: Air > snow/water/ice >
   loose > sand > gravel > stone > bedrock.
3. Mix **on the incoming face** (`heat / sides`, default 4) **before**
   displacement.
4. `mix_T < boil` → collapse. 1400 units → +1 sat; leftover `< expand`
   stays residual. Full seat → liquid overpressure along the path.
5. `mix_T ≥ boil` → steam lives. Spare seats: park beside liquid. Full
   cell: steam takes the seat, liquid continues.
6. Collapse ≠ boil. Boil is this cell’s own liquid flashing.
7. Seepage never moves steam. Solute rides liquid only.
8. 4×4 tiles only **ignite** and **pool residuals**. Ignite walks
   hot tiles on the beat, not the wet world.

## Tunables (`SteamConfig`)

| Knob | Default | Meaning |
|------|---------|---------|
| `enable_pipe` | play yes / tests no | Run this motor |
| `enable_leftover_field` | play no / tests yes | Old 28k leftover zone |
| `pipe_sides` | 4 | Face fraction `1/sides` |
| `pipe_stroke` | 1400 (Tab max 7000) | Units per beat |
| `pipe_beat` | `STEAM_EVERY` (5) | Pulse period |
| `phase_expansion_drive` | 96…1400 | Flash volume |

One **main** straw walks to the free surface and is **rewalked every
beat**, so a carve / collapse / new waterline gets a new route. A new
boiler that is closer to that straw than to its own surface becomes a
**feeder**: a shortest walk onto the main. Feeders pulse steam and
pump pore water toward the junction; the main pulses toward the mouth.
Shallow springs that are closer to the sky than to the main keep
their own vent. When the pipe is on, leftover's 28k field / Dijkstra
and steam cadence (boil / flood / assault) stay off. P paints the
straw, feeders, live puff, and claimed boilers.
Tab → Climate → cavity humidity tunes pipe on/off, leftover field,
sides, stroke, beat.

Open-sky mouth leaks **mass** only: hot → sky H, cool → distilled
liquid at the lip. Deposit is mouth / apron only — never the live
lumen.

## Locks

Mass-flat. `mint_void = false`. No sealed steam → sky H. Volume never
written into `sat`. Do not sinter the live lumen.
