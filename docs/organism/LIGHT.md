# Light competition

*Frozen column shade rule, `LeafAbsorb` / `ShadeEfficiency` genes,
and the post-barrier subsystem slot. `light-competition` in the
Organism Kernel plan.*

## The rule

Light stops being a smooth ambient bath and becomes a **column
resource**. In each world column, incoming sky light `L0(tick)` is
attenuated as it descends through pixels:

```
L(y_top)   = L0(tick) · sky_transmit(weather, cloud, humidity)
L(y-1)     = L(y)     · (1 − a · leaf(y) − s · stem(y))
harvest(y) = a · leaf(y) · L(y) · shade_eff(a)
```

Where:

- `leaf(y)` is 1 if the pixel at `(x, y)` is a `Photosystem` module,
  else 0.
- `stem(y)` is 1 if the pixel is an `Stem` module, else 0.
- `a` is the leaf's `LeafAbsorb` gene (0..1).
- `s` is a small fixed olive attenuation (working default `0.1`).
- `shade_eff(a)` reflects the `ShadeEfficiency` gene — see the
  tradeoff paragraph below.

Attenuation stacks multiplicatively — five stacked leaves each with
`a = 0.4` leave `(1 − 0.4)^5 ≈ 7.8 %` for whatever sits below.

## Genes involved

Defined fully in [`GENES.md`](GENES.md); the pair that lives *here*:

| Gene | Role |
|------|------|
| `LeafAbsorb` | High: strong shade cast + self-hunger if leaves are stacked. Low: passes light to modules below (understory friendly). |
| `ShadeEfficiency` | Harvest curve at low light. High: still produces energy in dim light but slower peak in full sun (understory specialist). Low: full sun specialist, useless in shade. |

The pairing is a classic canopy tradeoff:

- **Canopy thug** — high `LeafAbsorb`, low `ShadeEfficiency`. Wins
  the top of a stack; self-shades any own leaves below the tip;
  starves in dim light.
- **Understory persister** — low `LeafAbsorb`, high
  `ShadeEfficiency`. Bad at grabbing full sun; steady in scraps of
  light under a canopy.
- **Balanced sprawler** — mid values, wide leaf carpet with little
  vertical investment. Wins bare ground; loses to any taller
  neighbour.

## Subsystem slot

New post-barrier pass `run_shade`:

```
Simulation::step:
    // ... buffered subsystems ...
    barrier_commit(...);
    // Direct-mutation post-barrier passes:
    ...
    run_ecology(...);
    run_shade(...);        // new: writes per-column remaining-light array
    run_agents(...);       // module-driven photosystems sample the array
    ...
```

- Cadence: **every tick** for now. Cheap (a top-down scan per column
  active in the view). If profiling later demands, drop to every 2
  ticks; the visual read is unaffected.
- Reads: current column state (which cells are `Photosystem` / `Stem`
  / other modules), plus each cell's owning entity's `LeafAbsorb`.
- Writes: per-column `light_remaining: Vec<f32>` cached on the
  `Simulation` scratch (same shape and lifecycle as
  `per_column_flux` in
  [`OverlayData`](../../crates/legacy/wk-world/src/world.rs)). No new mass
  bucket — light is not tracked in the mass audit.

## Sky transmit

Weather, cloud shadow, and humidity already couple in the existing
climate + humidity fields. `sky_transmit` in Phase 5 is a lookup:

```
sky_transmit = climate.day_phase(tick)
             · cloud_shadow(x, tick)                // 0..1
             · (1 − 0.1 · humidity_rh(x, y_top))    // haze
```

The first factor mirrors what `Ecology::run` already samples for its
growth rate, so Phase 5 reuses that helper.

## Overlay

Cycle-`O` overlay adds a new mode `OverlayMode::LightRemaining`. Per
column, colour the top of the column by `light_remaining[top] / L0` —
dim red for shaded floor, bright white for exposed top. This is the
same shape as `TemperatureField` and `HumidityField` overlays already
in the archived column [`state.rs`](../../crates/legacy/wk-app/src/state.rs);
voxel overlays live in `wk-voxel-app`. It fits the
existing key.

## Interaction with column `Ecology` (archive)

On the **archived** column stack, coarse `Ecology.leaf_area` still
feeds leaf ET / infiltration / root erosion. The shade rule in this
spec operates on **module pixels**, not that LAI scalar. Voxel plants
use Set D modules + `wk-voxel` shade (`CanopyIndex`); they do not
read column `Ecology`.

## Interaction with water blobs

Floating water organisms (Set C) simply live at the module pixel they
sit at; `light_remaining` at that pixel is what their `Photosystem`
harvests. Depth changes light exposure exactly as required for
day-float / night-sink to matter (see
[`SCENARIOS.md`](SCENARIOS.md) E33).

## What is deliberately not here

- Refractive scattering, angle-of-incidence, or sun elevation
  variation. Sky comes straight down. Trees make columns of shade.
- Wavelength / colour of light. One scalar.
- Reflected / bounced light. If it wraps around modules, we cheat
  it later with a "ambient floor" gene, not a light-transport
  simulator.
- Photosynthesis biochemistry. Harvest is `a · leaf · L`.
