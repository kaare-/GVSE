# Agents: ECS creature layer

*Stage 10 design record. Implemented alongside this document.*

## Choice: `hecs`

Small ECS, no engine lock-in. Agents live in a `hecs::World` owned by
`Simulation`, **outside** the column stack. They read column / void /
field / ecology state and call world APIs (`dig`, `eat_biomass`,
`drink_water`).

## Components

| Component | Role |
|-----------|------|
| `Pose` | `x` (world column, fractional), `y` (elevation m) |
| `Energy` | current / max; ≤0 → despawn |
| `Genome` | trait vector (speed, graze rate, dig drive, …) — no mutation yet |
| `Grazer` | marker for the scripted forage behaviour |

## Scripted grazer (first creature)

Each agent tick:

1. Keep host column hydrology-active (`agent_keep_awake`)
2. Drink if thirsty (surface water, else moisture)
3. Eat alive biomass if hungry
4. Optionally dig when energy is mid-low and `dig_drive` is high
5. Step toward the neighbour column with more alive biomass
6. Pay a basal energy cost; despawn if depleted

No reproduction / mutation — that is stage 11.

## Mass

Eating reduces `Ecology.alive_biomass` and books `biomass_eaten_total`
(audit sink, paired with grow like decay). Creature body mass is not
tracked in `total_tracked` for stage 10.

## Scenario

**E16** — grazer on a wet vegetated band reduces alive biomass and
remains alive with positive energy for N ticks.
