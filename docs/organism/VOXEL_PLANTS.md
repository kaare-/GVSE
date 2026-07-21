# Voxel land plants — gene & feature roadmap

*Isolated `wk-voxel` port of Set D (and later Set E) from the column
kernel. Spec sources: [`PLANTS.md`](PLANTS.md), [`GENES.md`](GENES.md),
[`CORE_FEATURES.md`](CORE_FEATURES.md), [`LIGHT.md`](LIGHT.md),
[`SCENARIOS.md`](SCENARIOS.md). No `wk-agents` / `wk-world` imports.*

## Already landed (slice 0)

| Feature | Status |
|---------|--------|
| `Root` / `Stem` / `Photosystem` / `Nucleus` modules | Done |
| Fixed crown (no buoyancy) | Done |
| Pore-`sat` drink + drought stress | Done |
| Spawn on Air above porous solid | Done |
| Editor brushes + `T` minimal plant | Done |

## Roadmap

### D1 — Growth & allocation genes *(this PR)*

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

### D2 — Shade (canopy race)

| Gene | Role |
|------|------|
| `leaf_absorb` | How hard greens shade below |
| `shade_efficiency` | Dim-light harvest vs sun peak |

Features: per-column top-down module shade scan (`LIGHT.md`); photo
uses remaining light. Scenarios E36 / E37 spirit.

### D3 — Vegetative sprout

- Lateral rhizome tip → child plant on moist neighbour
- Genome mutate with `clone_fidelity`
- Soft pop cap shared with Atoms

### D4 — Drought banking

- Root count raises `energy_max` (storage), not photo floor
- Soft root:shoot budget; drought lifts root allowance
- Optional hibernate band (slow upkeep, no growth)

### E — Litter, epiphytes, topple *(later)*

| Gene | Role |
|------|------|
| `digest_rate` | Fungi |
| `attach_prefer` / `host_leave_fraction` | Epiphytes |

Features: stem `integrity`, topple, Holdfast, Hypha / Digest, organic
litter field. Needs D2 shade + dead stems.

## Explicitly out of Core for voxel

- Branching morphogenesis, seasonal leaf drop, wood rings
- Groundwater head field (use cell `sat` gradient only for now)
- Coarse `Ecology` LAI / ET bucket (column-only unless reintroduced)
- FEM / wind throw

## Isolation

Reimplement in `wk-voxel` (`plant.rs`, `organism.rs`, `blueprint.rs`).
Share palette hex + `.gvsecrt` shape only with column-GVSE.
