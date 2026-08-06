# Voxel land plants — gene & feature roadmap

*Isolated `wk-voxel` port of Set D (and later Set E) from the column
kernel. Spec sources: [`PLANTS.md`](PLANTS.md), [`GENES.md`](GENES.md),
[`CORE_FEATURES.md`](CORE_FEATURES.md), [`LIGHT.md`](LIGHT.md),
[`SCENARIOS.md`](SCENARIOS.md). No `crates/legacy/` column-crate imports.*

## Already landed (slice 0)

| Feature | Status |
|---------|--------|
| `Root` / `Stem` / `Photosystem` / `Nucleus` modules | Done |
| Fixed crown on purchase; free-float tipped when unanchored over water | Done |
| Woody tip rigid-bakes body (stem+root rotate together; raft / free-float / sand undercut) | Done |
| Uprooted woody: short wet keel; no mineral tunnel / nucleus→bed pipe | Done |
| Floating land/plankton corpses drift with wind + local water current | Done |
| Sand-rooted crowns never hoist on organic mats / water; no shore sail | Done |
| Raft tip resists with root keel (more dangling roots → harder tip) | Done |
| Upright draw ranks ignore shed cells; stemless never marks upright | Done |
| Shoots need Air; roots crack Stone→LooseRock; death skips Stone/dry-Air | Done |
| Woody leaves do not pile-lift; pick uses tipped draw pose | Done |
| Woody leaves Moore-adjacent to Stem (no midair flecks) | Done |
| Raft/sail spans use body-local dx (no ring wrap explosion) | Done |
| Tipped plants stay tipped after re-root; new Stem/Photo grow upright | Done |
| Upright mast re-tips when tippy; draw-space sail counts new shoots | Done |
| Tip bakes into body (new stems grow up); floaters elongate roots | Done |
| Tipped waterline logs capped; re-tip folds Stem only; no terrain pierce | Done |
| Vegetative / spore juvenile = pruned parent clone (not template) | Done |
| Substrate-rooted plants stay pinned when flooded (raft organic floats) | Done |
| Seaweed stays on bed holdfast; floats only if holdfast lost/rafting | Done |
| Crown holdfast ignores seepage; reseats if displaced; no stream tip | Done |
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

Features: column Beer–Lambert through posed Photosystem / Stem cells
(`shade.rs`, per `LIGHT.md`); plant photo sums per-leaf
`effective_photo_light`. Standing water attenuates sky light with depth
(`column_sky_light`) — deep seats go dark, so submerged stemmed plants
stem-race toward the surface, while stemless seaweed elongates its
Photosystem ribbon, or they fail the cost/benefit. E36/E37 spirit.

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
  dry-mat onto terrain (piling when cells collide). Woody canopies grow
  **short petioles** beside the trunk/branch (`WOODY_LEAF_MAX_CANT`) — not
  seaweed-length tip chains — and stay in the canopy in draw (wood holds
  them up; tips may nod a little past `LEAF_SUPPORT_WOODY`). New woody
  leaves also obey competition: Moore gap from *foreign* live Photosystems
  (same spirit as root spacing) and a minimum effective light
  (`WOODY_LEAF_MIN_LIGHT`) so dim / crowded sites stay bare. Underwater
  tips lean with climate wind **or** local water-sat shear. Woody `Stem`
  stays upright on land.
- **Light competition:** sky attenuates top-down through leaf/stem stacks
  (self-shade + taller neighbours). Equal-height peers still compete via
  lateral bleed. Cast / harvest / tint use posed draw cells, so flopped
  piles shade where the greens sit. Photosystem pixels tint bright
  `#2ECC40` → dim olive from **raw** sky × column transmit — understory
  genes don't wash out the read.
- **Woody leaf abscission:** stemmed plants drop Photosystems that stay
  below `WOODY_LEAF_STARVE_LIGHT` for `WOODY_LEAF_STARVE_TICKS` (sky ×
  shade, ignoring the day clock so night alone never strips a canopy).
  Litter paints as Organic in dry Air. Stemless seaweed ribbons never
  shed this way; at least one leaf is always kept.

### D3 — Vegetative sprout *(landed)*

- Lateral rhizome tip → child plant on moist neighbour
- `Genome::mutate` + `mutate_body` with `clone_fidelity` (genes and
  module add/swap/delete; habit stays plant)
- Soft pop cap shared with Atoms
- Root elongation biases sideways when banking for a sprout
- **Anti-flood / spacing:** long sprout period (~0.6 demo day), higher
  energy / root gates, soft local density (≤5 crowns in ±4 columns), and
  **crown clearance** (no neighbour within 2 columns — keeps T-canopies
  readable). Crowded / stacked saves reseat younger crowns outward.

### D3b — Wind spores / ferns *(landed)*

- Paint [`ReproSpore`](PALETTE.md) (`7` in the editor) on a land plant
- Rare wind-biased dispersal farther than rhizome reach (`try_plant_wind_spore`)
- Child is a juvenile plant that keeps a sorus so ferns can keep spreading
- Gene + blueprint mutation on the same `clone_fidelity` knob
- App draws lilac spore puffs drifting on climate wind (`SporeFx`)
- Rhizome sprout still works without spore modules (local clone only)

### Floating / tipped woody castaways

- An **unseated** woody plant (`fallen`) is one rigid body: it rides the
  free surface or rests on the beach. Proximal-root scrapes on bed, shore,
  or neighbour substrate must **not** teleport the nucleus to `solid_y+1`
  (that used to plant floating trees on the lake floor).
- **Rooted vs uprooted (woody):** roots are always a solid rigid body with
  the trunk (tip-bake), never soft terrain goo.
  - **Uprooted** (open water / wet Air under the nucleus): short wet keel
    only (`UPROOTED_ROOT_KEEL_MAX`); no mineral tunnels, no nucleus→bed
    pipes. Existing pierces past the keel are pruned each tick.
  - **Rooted purchase while tipped:** shore tips with mineral under the
    nucleus may elongate into the beach; the chassis stays where it rests.
  Stemless seaweed may still snap back to a short holdfast.
- Shore re-root stays tipped; only `upright_growth` shoots stand up.
- Shore-tipped logs resting on mineral do **not** ride a flickering runoff
  waterline (`gy = top`) — that pumped landslide regrowth ±1px when water
  ran past. Open-water castaways (wet Air under the nucleus) still float.

### Stemless ribbons vs floating Organic

- **Draw / collision:** stemless Photosystem pile-up only stacks in free Air.
  Tips that land in floating Organic drop under the lid instead of climbing
  out the top (that looked like the tip “pumping” through litter while the
  holdfast stayed put).
- **Seating:** free-surface ignores full-sat water sealed under Organic; stemless
  rescue skips Air-on-Organic. Holdfast may see mineral through a thin compost
  lid but never seats *on* Organic.

### D4 — Drought banking *(landed)*

- Root count raises `energy_max` (starch storage); photo / growth floors stay on `energy_base_max`
- Soft root:shoot budget; stressed moisture lifts root allowance
- Hibernate band (`DROUGHT_DORMANT_FRAC`): slow upkeep, no photo/drink/growth; die after max dormant ticks
- **Respiration:** upkeep uses tissue-weighted load (Photosystem ≫ Stem/Root) and a
  lower night floor than plankton — full module-count upkeep emptied river plants
  on the first night once they grew a trunk. Elongation / submerged stem-urge
  only run while `day ≥ PLANT_GROW_MIN_DAY`.

### E1 — Litter + fungi *(landed; fruiting body + mycelium field)*

| Gene | Role |
|------|------|
| `digest_rate` | Fruiting-body forage / local seed cadence |

Features:

- **Studio designs the fruiting body** (`F` template: Nucleus / Digest / Hypha
  pixels). Plant stamps cream **and** a lineage so later stalks match the
  painted mushroom (mutated on spore release).
- **Mycelium is a ground field** (`Cell::_pad` on Organic + mineral
  corridors: moist Soil/Sand easy, rock hard). Goal-seeks Organic and the
  free surface; dry fade disconnects, remoisten reconnects; compost leaves
  residual cream on Soil (square Soil patches = per-cell humify).
  Press **`M`** for a bright per-strain mycelium overlay.
- Networks that **breach the surface from below** emerge a **surface stalk**
  from the nearest lineage (else `minimal_fungus`). Stalks wind-disperse
  mutated spores far; rhizomorph hops stay local.
- Soft litter — bonus energy sip; Organic forage scales with field intensity
- Established moist networks support fruiting bodies (no energy-starve)
- Standing rain counts as moisture; **never** flash Organic → Sand
- Mycelium compost Organic → `MaterialId::Soil` (sat preserved; Tab knobs)
- Mycelium cream (0..=255) sticks Organic (repose/scour) and toughens rafts
- Crude CO₂ buckets (atm + dissolved): litter oxidation, algae draw dissolved,
  land photo lightly pulls atm; buckets persist in saves
- **Spore bank:** wind spores that land dry / crowded / cold hibernate on the
  landing cell and may germinate much later when moisture, space, or warmth
  return (Tab → Life → Spore bank; HUD `spores=`)
- Editor: `F` fruiting body; brushes `5` Digest / `6` Hypha / `7` ReproSpore;
  F3 Soil brush; Tab → Life pages for compost + carbon + spore bank

### E1b — Lingering corpses → Organic *(landed)*

- Death keeps a grey corpse drawable (land pinned / plankton sinks)
- Land plants: root stencil becomes Organic in the ground immediately (sat preserved)
- After settle ticks, remaining body → Organic + soft litter
- Fungi feed on that residue (not on instant despawn)

### Plant tune *(this PR)*

- Root drink only touches pore sat (never free Air water); slow sip + return to humidity.
  Land roots stop sipping once pore fill is above `ROOT_DRINK_COMFORT_FRAC`
  so growing plants do not flash-dry moist sand into drought dormancy.
- Stronger moisture tropism for root elongation
- Softer upkeep / longer plant life; Tab → Plants/fungi gene knobs

### E2 — Epiphytes + topple *(later)*

| Gene | Role |
|------|------|
| `attach_prefer` / `host_leave_fraction` | Epiphytes |

Features: stem `integrity`, topple, Holdfast. Needs dead stems.

## Explicitly out of Core for voxel

- Branching morphogenesis, seasonal (calendar) leaf drop, wood rings
  (productivity abscission for woody leaves is landed — see above)
- Groundwater head field (use cell `sat` gradient only for now)
- Coarse `Ecology` LAI / ET bucket (column-only unless reintroduced)
- FEM / wind throw

## Isolation

Reimplement in `wk-voxel` (`plant.rs`, `organism.rs`, `blueprint.rs`).
Share palette hex + `.gvsecrt` shape only with column-GVSE.
