# Scenarios

*Frozen falsification scenes as fill-in-the-blanks skeletons.
`falsify-scenarios` in the Organism Kernel plan.*

## Numbering

- **E1–E16** — existing hydrology / karst / ecology / burrow / grazer
  tests. See [`tests/scenarios/mod.rs`](../../tests/scenarios/mod.rs).
- **E17–E29** — reserved for evolution-loop follow-ups (E17 is the
  reproduction / mutation test on the `cursor/evolution-loop-fdf9`
  branch).
- **E30–E45** — organism kernel scenarios below.

Renumbering later is expensive (docs, test file names, expected
output). The block is locked.

## Skeleton conventions

Every scene has four fields. When Phase 2+ writes the actual Rust
test, it fills in the same four things:

- **World setup** — chunks, sea level, temperature, humidity, rain
  toggles, weather toggles. Match the minimal shape from
  `tests/scenarios/helpers.rs` where possible.
- **Blueprint** — modules + genome (see
  [`EDITOR.md`](EDITOR.md) for the on-disk shape).
- **Assertion** — what must be true after `run_ticks(...)`.
- **Expected read-out** — one line printed on success so the test
  logs read like a stubby lab notebook, matching e.g. `E13: dug 800
  kg sand, ...` today.

## Set A — alive in light

### E30 — Atom bloom

- **World.** One flat sand chunk, mid-latitude climate, rain off,
  weather off, day / night on. Existing `generate_flat_sand`.
- **Blueprint.** Atom = `Nucleus` + `Photosystem`. `MetabolicRate =
  MID`, `ReproduceAt = 0.7`, `CloneFidelity = 0.6`.
- **Assertion.** Alive Atom count grows through daylight and thins at
  night; mass audit balanced; drift ≤ 80 kg over 2000 ticks.
- **Read-out.** `E30: pop peak={} at day, {} at night, drift={}`.

### E31 — Messy boom

- **World.** Same as E30, longer soak.
- **Blueprint.** Two lineages: one low `ReproduceAt` + low
  `CloneFidelity` ("messy"), one high `ReproduceAt` + high
  `CloneFidelity` ("thrifty").
- **Assertion.** Messy lineage overshoots then crashes below thrifty
  by tick N; thrifty steady-state population > messy.
- **Read-out.** `E31: messy peak={}, thrifty peak={}, crossover
  tick={}`.

## Set B — chemical talk + nerves

### E32 — Scent flag

- **World.** Shallow water chunk (or moist column set) with chem
  fields enabled, 4 channels.
- **Blueprint.** One producer with `Nucleus`, `Photosystem`, high
  `MetabolicRate`, and a `ChemoEmitter` tuned to channel 0.
  Several observers with a `ChemoSensor` tuned to channel 0 driving
  a soma output.
- **Assertion.** `ChemField[0]` at observer cells rises after the
  producer feeds well; observer soma output correlates.
- **Read-out.** `E32: emit_total={} kg, mean_sensor_reading={}, lit
  observers={}/N`.

### E35 — Dialect split

- **World.** Two Petri patches (two chunks, one column between
  them) with chem fields enabled.
- **Blueprint.** Two lineages seeded to different `tuned_type`
  values (0 and 2). Emitter and sensor genes match within each
  lineage; mismatched across lineages.
- **Assertion.** Observers within a lineage react to their own
  producer's emissions; observers across lineages do not react
  above noise.
- **Read-out.** `E35: cross-lineage correlation={} (should be near
  zero)`.

## Set C — vertical niche + climate

### E33 — Day float / night sink

- **World.** Deep water chunk with light column enabled and thermal
  field enabled.
- **Blueprint.** Water Atom + `Buoyancy` + a soma wiring day / night
  phase to `depth_target`. Control lineage lacks the soma wiring.
- **Assertion.** Wired lineage tracks the lit band by day and sinks
  at night; control lineage held at fixed depth loses mass.
- **Read-out.** `E33: wired energy_max={}, control energy_max={}`.

### E34 — Heat edge

- **World.** Two neighbouring columns with a warm pocket driven by
  `ThermalField` source.
- **Blueprint.** Two lineages, narrow-`TempWidth` vs wide-`TempWidth`
  with matching `TempTolerance` module.
- **Assertion.** Narrow lineage dies out in the warm pocket; wide
  survives at a metabolic cost.
- **Read-out.** `E34: narrow_alive={}, wide_alive={}, wide_upkeep={}`.

## Set D — land plants + shade

### E36 — Canopy race

- **World.** Flat lush band with rain on and humidity field enabled.
- **Blueprint.** Two lineages: tall stem-heavy allocation vs
  leaf-carpet allocation. Same seeds; different `StemVsLeafVsRoot`.
- **Assertion.** Stem lineage overtops the carpet; carpet greens
  `light_remaining` drops sharply after canopy closure.
- **Read-out.** `E36: canopy height={}, carpet light={}, canopy
  light={}`.

### E37 — Understory persists

- **World.** Same as E36, after canopy closure.
- **Blueprint.** Third lineage seeded low with high `ShadeEfficiency`
  and low `LeafAbsorb`.
- **Assertion.** Understory alive biomass survives at low but
  non-zero level under closed canopy; naive carpet lineages die
  out.
- **Read-out.** `E37: understory alive={}, naive carpet
  alive={} (expect 0)`.

### E38 — Root drought

- **World.** Columns with wet subsurface but dry surface (heat +
  rain off). Groundwater head field enabled.
- **Blueprint.** Two lineages: shallow-root (`RootDepthBias = 0.1`)
  vs deep-root (`RootDepthBias = 0.9`).
- **Assertion.** Shallow lineage dies as surface dries; deep-root
  lineage reaches the water table (existing gw head field) and
  holds.
- **Read-out.** `E38: shallow alive={}, deep alive={}, mean deep
  root depth={} m`.

## Set E — fungi, epiphytes, toppling

### E39 — Litter bloom

- **World.** Set D end-state: canopy closes and shaded lineage
  dies, producing organic litter.
- **Blueprint.** Litter fungus (creature F). Seeded when
  `column.ecology.dead_biomass` crosses a threshold.
- **Assertion.** Fungal alive count spikes, `dead_biomass` drops,
  fungal count crashes as substrate is exhausted; drift within
  audit tolerance.
- **Read-out.** `E39: peak fungi={}, peak dead_biomass={}, final
  dead_biomass={}`.

### E40 — Epiphyte free-ride

- **World.** Established tall stems from a Set D pre-run.
- **Blueprint.** Epiphyte (creature E) with `AttachPrefer = 0.9`
  and a modest `LeafAbsorb`.
- **Assertion.** Epiphyte establishes on host olive; its greens
  harvest above understory light; host survives (gentle rider
  case).
- **Read-out.** `E40: epi alive={}, host alive={}, gap
  events={} (0)`.

### E41 — Allocation trap

- **World.** Same as E40 with rain toggled off (droughted).
- **Blueprint.** Same epiphyte. `stem_wetness` is the only drink
  source.
- **Assertion.** Epiphyte alive count drops as `stem_wetness`
  decays; host survives.
- **Read-out.** `E41: epi alive final={}, host alive final={}`.

### E42 — Smother → rot → topple

- **World.** Established tall stem, well fed.
- **Blueprint.** Smotherer epiphyte with high `LeafAbsorb` and
  `HostLeaveFraction = 0.0`.
- **Assertion.** Host `Energy.current` drops; host `Nucleus` dies;
  fungi (from seeded litter or pre-existing spores) invade
  standing-dead stems; `integrity` collapses; topple event fires;
  epiphyte is displaced to ground and usually dies.
- **Read-out.** `E42: host death tick={}, topple tick={}, epi
  landed alive={} (usually 0)`.

### E43 — Gentle wins long game

- **World.** Long soak on a stable tall-stem strip.
- **Blueprint.** Two epiphyte lineages: smotherer vs gentle rider
  (`HostLeaveFraction` differs).
- **Assertion.** Over the long run, the number of gentle rider
  lineages > smotherer lineages; smotherer lineages boom and
  collapse with their landlords.
- **Read-out.** `E43: gentle count={}, smother count={}, topple
  events={}`.

### E44 — Ghost roots

- **World.** Column with `SubstrateTag::Rock` on top of a wet
  water table. One "tree" seeded from a Set D run.
- **Blueprint.** Same tree genome for founder and follow-up sprout;
  fungi allowed to invade dead roots.
- **Assertion.** After the founder dies and its roots are digested,
  `SubstrateTag` under the trunk goes `Rock → Organic → Void →
  Loose` (or open `Void` if unfilled). A follow-up sprout on the
  same column reaches the water table in fewer ticks than the
  founder did.
- **Read-out.** `E44: founder ticks to table={}, follower ticks to
  table={} (expect <)`.

## Phase 7 preview

### E45 — Pass in front

- **World.** Established Set D forest with tall Mid trunks.
- **Blueprint.** Ground animal (Phase 7 module set — placeholder in
  the palette).
- **Assertion.** Fore-lane creature crosses a Mid trunk column
  without a `Climb` interaction; the world's shade rule is
  unaffected.
- **Read-out.** `E45: crossings={}, climb events={} (0 unless
  scripted)`.

Deferred to Phase 7. Written here so the number is reserved.

## Set A environment gates (implemented)

### E46a — Dry land kills plankton

- **World.** Dry flat sand, no standing water.
- **Blueprint.** Atom.
- **Assertion.** Organism count → 0 within a few ticks; corpse or litter left.
- **Read-out.** `E46a: dry land → organism_count=0`.

### E46b — Ice cap kills plankton

- **World.** Water under an ice cap (`top_ice_mass > 0`).
- **Blueprint.** Atom.
- **Assertion.** One organism tick kills the plankton (AgentStore path; full sim would melt ice above 0°C).
- **Read-out.** `E46b: ice cap → organism_count=0`.

### E46c — Bloom draws down dissolved CO₂

- **World.** Flooded flat sand, warm daylight, gas exchange on.
- **Blueprint.** Dense Atom band, repro on.
- **Assertion.** Mean dissolved CO₂ dips ≥ 0.10 below start; O₂ rises; mass non-negative.
- **Read-out.** `E46c: co2 …→min…  o2 …→…  living=…`.

### E46d — Cold blocks reproduction (unfrozen)

- **World.** 3°C water (above freeze) vs 22°C control; narrow `TempWidth`.
- **Blueprint.** Same Atom genome both worlds; energy topped each tick.
- **Assertion.** Cold births = 0, founder survives; warm control births > 0.
- **Read-out.** `E46d: cold births=0 warm births=N`.

### E47 — Ocean water budget + seeded water table

- **World.** Continental strip with humidity / wind / groundwater fields;
  weather on for the soak; flat sand ponds for the skin-evap check.
- **Assertion.** Gen-time aquifer under ocean beds (≥95% sat) and land
  base moisture; deep vs shallow ponds lose similar mass (skin, not
  depth); continental ocean loses <5% surface water over 3600 ticks
  while bed sat stays high and weather rain is non-trivial vs evap.
- **Read-out.** `E47a/b/c: …`.

### E48 — Air ↔ dissolved gas exchange

- **World.** Flooded flat sand, no organisms, weather/rain off.
- **Assertion.** Depleted dissolved CO₂/O₂ recharge from air; supersaturated
  water outgasses toward Henry equilibrium.
- **Read-out.** `E48a/b: co2 …→…`.

## Adding scenes

New scenes append after E48. Do not renumber existing entries even
if they never ship — the numbering is a contract with the docs.
