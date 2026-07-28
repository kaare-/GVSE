# Ecology: per-column plant bucket

*Stage 8 design record. Implemented alongside this document.*

## Motivation

Creatures and evolution need a living substrate, not bare geology.
A cheap per-column plant state gives:

- **Erosion resistance** via roots (dams and hillsides hold)
- **Evapotranspiration** via leaf area (couples to the humidity field)
- **Infiltration boost** via root channels
- **Selection gradients** later (biomass, nutrient, shade) for grazers
  and for cave vs surface niches once burrows exist

## Data model

```rust
pub struct Ecology {
    pub root_density: f32,   // 0..1
    pub leaf_area: f32,      // 0..1 LAI proxy
    pub dead_biomass: i64,   // kg
    pub alive_biomass: i64,  // kg
    pub nutrient: f32,       // 0..1
}
```

Stored on `Column` with `#[serde(default)]`. Empty/default ecology is a
no-op for all feedback paths so pre-ecology saves keep working.

Biomass is **not** an `Organic` layer. Density settling would float
organic litter incorrectly relative to rooted plants. Biomass lives in
the ecology bucket and in `MassAudit::biomass_total`.

## Growth model (`run_ecology`)

Each due tick, for active columns above sea level:

```
light     = surface exposure (snow/ice/deep water reduce it)
water     = moisture / moisture_cap
temp_f    = unimodal comfort around ~18 °C
growth    = light * water * temp_f * nutrient * GROWTH_COEFF
death     = drought/cold/heat stress * DEATH_COEFF * alive

alive    += growth - death
dead     += death
nutrient -= growth * USE + dead_decay * RECYCLE
leaf/root asymptote toward f(alive)
```

Mass closure:

- Growth books `biomass_grow_total` (atmospheric C + incorporated water)
  and may pull a small moisture delta.
- Decay of `dead_biomass` books `biomass_decay_total` (return to air /
  mineralisation). Recycled fraction becomes `nutrient`.

`total_tracked` includes `biomass_total = Σ(alive + dead)`.

## Feedback into existing subsystems

| Subsystem | Coupling |
|-----------|----------|
| `run_sediment` | effective erosion resistance × `(1 + ROOT_EROSION_SCALE * root_density)` |
| `run_evaporation` | extra factor `(1 + LEAF_ET_SCALE * leaf_area)` on moisture/surface evap |
| `run_infiltration` | permeability × `(1 + ROOT_INFIL_SCALE * root_density)` |

Constants stay game-tuned and small so barren scenarios (E1–E8) remain
in the same qualitative regime when ecology starts near zero.

## Seeding

At chunk generation, seed a low alive biomass and nutrient from biome:

- Ocean / deep shelf → 0
- Coast / plains → modest grass
- Mountain → sparse

Wetness at gen time can bump the seed; rain then grows it further.

## Scenarios

- **E14** — wet warm plains accumulate alive biomass; dry/cold does not
- **E15** — rooted column resists erosion vs bare twin under the same flow
  (column: `Ecology.root_density`; voxel port: living `ModuleId::Root`
  cells bind grain repose / bedload — see
  `crates/wk-voxel/tests/scenarios/e15_roots_reduce_erosion.rs`)

## Non-goals (later stages)

- Species / genotypes (stage 10–11)
- Cave chemoautotrophs (needs void light + dissolved coupling)
- Explicit shade competition between columns
- Harvesting APIs for creatures (`eat`) — stage 10
