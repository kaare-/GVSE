# Voxel land plants — gene & feature roadmap

*Isolated `wk-voxel` port of Set D (and later Set E) from the column
kernel. Spec sources: [`PLANTS.md`](PLANTS.md), [`GENES.md`](GENES.md),
[`CORE_FEATURES.md`](CORE_FEATURES.md), [`LIGHT.md`](LIGHT.md),
[`SCENARIOS.md`](SCENARIOS.md). No `crates/legacy/` column-crate imports.*

## Already landed (slice 0)

| Feature | Status |
|---------|--------|
| `Root` / `Stem` / `Photosystem` / `Nucleus` modules | Done |
| Fixed crown (no buoyancy) | Done |
| Pore-`sat` drink + drought stress | Done |
| Spawn on Air above porous solid | Done |
| Editor brushes + `T` minimal plant + `W` seaweed | Done |

## Roadmap

### D1 — Growth & allocation genes *(landed)*

Genes on `Genome` (mutated later on sprouts):

| Gene | Role |
|------|------|
| `alloc_stem` / `alloc_leaf` / `alloc_root` | `StemVsLeafVsRoot` surplus split |
| `root_depth_bias` | Deep dive vs shallow sprawl |

Features:

- Root elongation toward moist / down-biased neighbours
- Stem upward + leaf place from surplus
- Soft module caps (readable 1× bodies)
- Growth energy floor (bank before spend)

Falsifiers: root grows into wetter sand; stem-heavy alloc grows taller.

### D2 — Shade (canopy race) *(landed)*

| Gene | Role |
|------|------|
| `leaf_absorb` | How hard greens shade neighbours / self-stack |
| `shade_efficiency` | Dim-light harvest vs sun peak |

Features: sparse canopy index + neighbour cast (`shade.rs`, lite
`LIGHT.md`); plant photo uses `effective_photo_light`. Standing water
attenuates sky light with depth (`column_sky_light`) — deep seats go
dark, so submerged stemmed plants stem-race toward the surface, while
stemless seaweed elongates its Photosystem ribbon, or they fail the
cost/benefit. E36/E37 spirit.

### Seaweed template *(landed)*

- Editor `W` → `Blueprint::minimal_seaweed`: Nucleus + one Root holdfast +
  vertical Photosystem string (no Stem).
- Shoot growth keeps stemless habits stemless and stacks leaves upward
  from the frond tip.
- Mutation will not invent a trunk on a stemless parent.
- Spawn under water on a moist bed; Tab plant knobs do not overwrite the
  ribbon’s leaf-heavy alloc.
- **Leaf drink (emergent):** Photosystems in *standing water* sip free-column
  sat. Dry-land / film Air does not count — shore leaves never drink.
  When leaves bathe, soft root budget collapses to one holdfast (no
  seaweed flag).
- **Soft leaves (draw + growth):** stemless ribbons are floppy — they
  elongate *up* into standing water, lay on the waterline when emerged, and
  dry-mat onto terrain (piling when cells collide). Photosystems on a
  `Stem` / branch stay in the canopy (wood holds them up; long tips may
  nod a little past `LEAF_SUPPORT_WOODY` but never flatten to the ground).
  Underwater tips lean with climate wind **or** local water-sat shear.
  Woody `Stem` stays upright on land.
- **Light competition:** equal-height neighbours shade each other (dense
  meadows). Canopy index + photo sample use posed draw cells, so flopped
  piles cast and receive shade where the greens sit. Photosystem pixels
  tint bright `#2ECC40` → dim olive by effective light (easy to read which
  leaves are working).

### D3 — Vegetative sprout *(landed)*

- Lateral rhizome tip → child plant on moist neighbour
- `Genome::mutate` + `mutate_body` with `clone_fidelity` (genes and
  module add/swap/delete; habit stays plant)
- Soft pop cap shared with Atoms
- Root elongation biases sideways when banking for a sprout
- **Anti-flood:** long sprout period (~0.6 demo day), higher energy /
  root gates, soft local density (≤8 crowns in ±4 columns), and **one
  living crown per column** (sprouts skip occupied seats; stacked
  saves reseat younger crowns on the next tick).

### D3b — Wind spores / ferns *(landed)*

- Paint [`ReproSpore`](PALETTE.md) (`7` in the editor) on a land plant
- Rare wind-biased dispersal farther than rhizome reach (`try_plant_wind_spore`)
- Child is a juvenile plant that keeps a sorus so ferns can keep spreading
- Gene + blueprint mutation on the same `clone_fidelity` knob
- App draws lilac spore puffs drifting on climate wind (`SporeFx`)
- Rhizome sprout still works without spore modules (local clone only)

### D4 — Drought banking *(landed)*

- Root count raises `energy_max` (starch storage); photo / growth floors stay on `energy_base_max`
- Soft root:shoot budget; stressed moisture lifts root allowance
- Hibernate band (`DROUGHT_DORMANT_FRAC`): slow upkeep, no photo/drink/growth; die after max dormant ticks

### E1 — Litter + fungi *(landed; fruiting body + mycelium field)*

| Gene | Role |
|------|------|
| `digest_rate` | Fruiting-body forage / local seed cadence |

Features:

- **Studio designs the fruiting body** (`F` template: Nucleus / Digest / Hypha
  pixels). Plant it on Organic; it seeds the ground network.
- **Mycelium is a ground field** (`Cell::_pad` on Organic), not something you
  paint in the creature editor. `step_mycelium_field` thickens / spreads on
  moist Organic even after the fruiting body dies; threads prefer climbing
  toward free Air.
- Networks that **breach the surface from below** can emerge a **surface
  stalk**. Buried bodies rhizomorph-hop locally; stalks wind-disperse spores
  far (`ReproSpore`).
- Soft litter — bonus energy sip; Organic forage scales with field intensity
- Established moist networks support fruiting bodies (no energy-starve)
- Standing rain counts as moisture; **never** flash Organic → Sand
- Long colonization may compost Organic → `MaterialId::Soil` (sat preserved)
- Editor: `F` fruiting body; brushes `5` Digest / `6` Hypha / `7` ReproSpore;
  F3 Soil brush

### E1b — Lingering corpses → Organic *(landed)*

- Death keeps a grey corpse drawable (land pinned / plankton sinks)
- Land plants: root stencil becomes Organic in the ground immediately (sat preserved)
- After settle ticks, remaining body → Organic + soft litter
- Fungi feed on that residue (not on instant despawn)

### Plant tune *(this PR)*

- Root drink only touches pore sat (never free Air water); slow sip + return to humidity
- Stronger moisture tropism for root elongation
- Softer upkeep / longer plant life; Tab → Plants/fungi gene knobs

### E2 — Epiphytes + topple *(later)*

| Gene | Role |
|------|------|
| `attach_prefer` / `host_leave_fraction` | Epiphytes |

Features: stem `integrity`, topple, Holdfast. Needs dead stems.

## Explicitly out of Core for voxel

- Branching morphogenesis, seasonal leaf drop, wood rings
- Groundwater head field (use cell `sat` gradient only for now)
- Coarse `Ecology` LAI / ET bucket (column-only unless reintroduced)
- FEM / wind throw

## Isolation

Reimplement in `wk-voxel` (`plant.rs`, `organism.rs`, `blueprint.rs`).
Share palette hex + `.gvsecrt` shape only with column-GVSE.
