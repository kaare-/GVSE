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

Knobs on Tab `Genome` paint DTO → Nucleus / Root pixel traits (live
reads `BodyPlan`; not stored on `.gvsecrt`):

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
- `Blueprint::mutate_child` via chassis blueprint (`clone_fidelity`)
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

### E2 — Epiphytes + topple

| Gene | Role |
|------|------|
| `host_leave_fraction` | Epiphyte light left for host *(Wave Y)* |
| `attach_prefer` | Epiphyte seating / re-seek bias *(Wave AB)* |

**Wave U — Holdfast seating *(landed)*:**
- `ModuleId::Holdfast` (`0x0F`, `#FF3D9A`) appended after Bone (postcard-safe)
- `is_epiphyte` / `is_holdfast_anchored`; habitat spawn requires a host Stem
- Epiphyte tick: photo + upkeep; dies within ~8 ticks if unseated
- Editor: `0` Holdfast brush, `Y` minimal epiphyte template
- Scenario `e40_epiphyte_seat`

**Wave V — Standing-dead topple *(landed)*:**
- `Atom` / `Corpse.body_integrity` (empty ⇒ `1.0`); `SIM_SCHEMA_VERSION` 4
- Standing-dead Stem drain `DEAD_DECAY_PER_TICK`; fail ≤ `INTEGRITY_TOPPLE_THRESHOLD`
- `topple_stem_at`: break at lowest failing Stem, drop Organic in L/R ground band
- Epiphytes on fallen Stem cells force-unseat
- Scenario `e42_standing_dead_stem_topples`

**Wave W — Fungal stem rot *(landed)*:**
- Living Digest/Hypha sharing a standing-dead Stem world cell adds `FUNGAL_DECAY_PER_TICK`
- No new Atom fields / no schema bump (`collect_fungus_tissue_world_cells` each corpse tick)
- Scenario `e42b_fungal_rot_accelerates_topple` (abiotic control vs fungal treatment)

**Wave X — Living stem load / recharge *(landed)*:**
- Per-Stem weight = own Stem/Photosystem above + epiphyte modules on this stem or higher
- Excess over `STEM_FREE_LOAD` drains at `STEM_LOAD_DRAIN_PER_ABOVE`; recharge from energy
- Live topple via `topple_stem_at` (keeps `body_traits`); unseats riders
- Scenario `e45_live_stem_load_topple` (self-load holds vs epiphyte overload)

**Wave Y — HostLeaveFraction smother *(landed)*:**
- Photosystem `host_leave_fraction` (Tab / `BodyPlan`); default `0` = smotherer
- Same-column epiphytes above the host attenuate host light by `(1 − leave) × cast`
- `.gvsecrt` schema **3**; `SIM_SCHEMA_VERSION` **5** (schema-2 blueprints dual-load)
- Scenario `e43_host_leave_smother` (gentle host energy > smotherer)

**Wave Z — Stem wetness drink *(landed)*:**
- Land plants track `Atom.stem_wetness` toward root pore moisture (`STEM_WET_TRACK`)
- Epiphytes drink via Holdfast (`EPI_STEM_DRINK`); dry/unseated stress shares the U clock
- `SIM_SCHEMA_VERSION` **6**
- Scenario `e41_stem_wetness_drink` (moist keeps epi; drought kills epi, host lives)

**Wave AA — Hypha invade standing-dead *(landed)*:**
- Active fungi extend Hypha into orthogonally adjacent corpse Stem cells
- `try_grow_hypha_into_dead_stem` (`HYPHA_GROW_COST` / `HYPHA_GROW_PERIOD` / `MAX_HYPHA_MODULES`)
- Closes invade→rot→topple without manually seating Digest on the trunk
- Scenario `e42c_hypha_invades_dead_stem`

**Wave AB — AttachPrefer reseat *(landed)*:**
- Holdfast `attach_prefer` (Tab / `BodyPlan`); default `0` = no re-seek (E40 clock)
- Seek radius `1 + floor(prefer×4)` (max 5); `try_epiphyte_reseat` from tick + habitat/editor spawn
- `.gvsecrt` schema **4**; `SIM_SCHEMA_VERSION` **7** (schema-3 blueprints dual-load)
- Scenario `e40b_attach_prefer_reseat` (sticky re-seats; cling-free dies; near-miss spawn)

**Deferred (Wave AC+):** ghost roots, fallen-log animation, long-soak E43 lineages.

## Explicitly out of Core for voxel

- Branching morphogenesis, seasonal leaf drop, wood rings
- Groundwater head field (use cell `sat` gradient only for now)
- Coarse `Ecology` LAI / ET bucket (column-only unless reintroduced)
- FEM / wind throw

## Isolation

Reimplement in `wk-voxel` (`plant.rs`, `organism.rs`, `blueprint.rs`).
Share palette hex + `.gvsecrt` shape only with column-GVSE.
