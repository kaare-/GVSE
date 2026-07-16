# Evolution: species-selection loop

*Stage 11 design record. Implemented alongside this document.*

## Principle

There is **no** global evolution subsystem. Selection emerges from:

- substrate physics (temperature, moisture, burrows, karst niches)
- ecology (alive biomass as forage)
- creature behaviour (genome-driven grazers)

Agents that starve, desiccate, or freeze lose energy and despawn.
Agents that stay above a reproduction energy threshold fission into
an offspring with a **mutated** genome copy.

## Reproduction

On each agent tick, after forage / drink / dig / move cost:

1. Population `< MAX_AGENTS`, `repro_drive > 0`, and the agent is due
   (`tick % REPRO_PERIOD == entity_id % REPRO_PERIOD`)
2. A deterministic roll is `< repro_drive`
3. `energy ≥ REPRO_ENERGY_FRAC · max`
4. Parent pays `REPRO_COST_FRAC · current` energy
5. Offspring spawns nearby with that energy and `Genome::mutate(parent)`

Mutation is deterministic: `hash(world.seed, tick, parent_id, trait_i)`.
Trait noise is relative (±`MUTATION_SIGMA`) then clamped to sane ranges.

## Environmental stress

Beyond basal `metabolism`:

| Stress | Condition | Extra drain |
|--------|-----------|-------------|
| Desiccation | no surface/flowable water and no moisture | `DESICCATION_DRAIN` |
| Cold | column temperature `< 0 °C` | `COLD_DRAIN` |

These are soft pressures so selection can favour drink rate, efficiency,
or metabolism without a separate fitness function.

## Mass

Creature body mass stays out of `total_tracked` (same as stage 10).
Forage still books `biomass_eaten_total`.

## Scenario

**E17** — one grazer on a lush wet band reproduces; `births_total > 0`,
population grows, and at least one living genome differs from the founder.
