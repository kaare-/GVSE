# Agents: ECS creature layer

*Column-stack archive. Stages 10–11 design record for the scripted
grazer in [`crates/legacy/wk-agents`](../crates/legacy/wk-agents).
Active life is module-pixel `OrganismStore` in `wk-voxel` / studio in
`wk-voxel-app` — see [`organism/`](organism/). Do not extend this
path.*

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
| `Genome` | trait vector (speed, graze, dig, repro, …) |
| `Grazer` | marker for the scripted forage behaviour |

## Scripted grazer (first creature)

Each agent tick:

1. Keep host column hydrology-active (`agent_keep_awake`)
2. Eat alive biomass when energy is below max (forage is the primary drive)
3. Drink if thirsty (surface water, else moisture)
4. Optionally dig when energy is mid-low and `dig_drive` is high
5. Step toward the neighbour column with more alive biomass
6. Pay basal metabolism plus dry/cold stress; despawn if depleted
7. If well-fed and `repro_drive` allows, fission with mutated genome

Reproduction and mutation details: `docs/EVOLUTION.md`.

## Mass

Eating reduces `Ecology.alive_biomass` and books `biomass_eaten_total`
(audit sink, paired with grow like decay). Creature body mass is not
tracked in `total_tracked`.

## Scenarios

**E16** — grazer on a wet vegetated band reduces alive biomass and
remains alive with positive energy for N ticks (`repro_drive = 0`).

**E17** — founder reproduces; population grows and offspring genomes
differ from the parent (stage 11).
