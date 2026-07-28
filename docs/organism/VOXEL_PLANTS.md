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
| Editor brushes + `T` minimal plant | Done |

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
`LIGHT.md`); plant photo uses `effective_photo_light`. E36/E37 spirit.

### D3 — Vegetative sprout *(landed)*

- Lateral rhizome tip → child plant on moist neighbour
- `Genome::mutate` with `clone_fidelity`
- Soft pop cap shared with Atoms
- Root elongation biases sideways when banking for a sprout

### D4 — Drought banking *(landed)*

- Root count raises `energy_max` (starch storage); photo / growth floors stay on `energy_base_max`
- Soft root:shoot budget; stressed moisture lifts root allowance
- Hibernate band (`DROUGHT_DORMANT_FRAC`): slow upkeep, no photo/drink/growth; die after max dormant ticks

### E1 — Litter + fungi *(landed)*

| Gene | Role |
|------|------|
| `digest_rate` | Fungi digest speed |

Features:

- Soft litter field (`World::soft_litter`)
- `Digest` / `Hypha` modules; digest soft litter first, then peel Organic → Air
- Starve / dry hibernate (mirror plant drought); spore burst to neighbour litter
- Editor: `F` fungus template, brushes `5` Digest / `6` Hypha

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
