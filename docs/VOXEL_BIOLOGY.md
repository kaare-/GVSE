# Voxel biology materials (Wave L)

Bone / Muscle / Skin are first-class world [`MaterialId`](../crates/wk-material/src/lib.rs)
variants as well as studio [`ModuleId`](organism/PALETTE.md) kinds.
Living pixels still use ModuleId; death paints the matching MaterialId
so soft tissue and skeleton decay on different clocks.

## Material table

| ID | Name | Hex | Physics class |
|----|------|-----|---------------|
| 12 | Bone | `#EFE7DA` | Solid, not a grain. High cohesion, low porosity/permeability, cliff-stable repose. Modest `roof_span_max_m` (arches can fail). |
| 13 | Muscle | `#C33C3C` | Soft grain (`is_grain` + repose). Flow-erodible bedload. High porosity. |
| 14 | Skin | `#FFDBAC` | Soft litter (`falls_through_empty_air` like Organic). Repose spreads; not flow-erodible. |

`MATERIAL_COUNT` is 15. Older saves that see ids 12–14 fail with
"unknown material" via `MaterialId::from_u8` (same pattern as unknown
blueprint modules).

## Corpse routing

[`module_death_material`](../crates/wk-voxel/src/biology.rs):

| ModuleId | MaterialId |
|----------|------------|
| Bone | Bone |
| Muscle | Muscle |
| Skin | Skin |
| Root / Stem / Photosystem / Nucleus / Digest / Hypha | Organic |

Call site: `dissolve_corpse_to_organic` (lingering corpse settle). Plant
roots still paint Organic via `leave_dead_roots_in_place`; shoot modules
(Stem / Nucleus / Photosystem) stay litter-only mid-air. Bone / Muscle /
Skin are **not** skipped — they leave kind-specific cells.

Per-pixel `PixelTraits` are dropped at death; the material cell carries
only registry props.

## Differential decay

[`apply_biological_decay`](../crates/wk-voxel/src/rules/decay.rs) —
opt-in pass (demo: **`B`** key, off by default).

| Transition | Approx half-life | Default per-tick prob |
|------------|------------------|------------------------|
| Muscle → Organic | ~100 ticks | `0.00693` |
| Skin → Organic | ~300 ticks | `0.00231` |
| Bone → Sand | ~5000 ticks | `0.000139` |

Deterministic hash: `(world.seed, gx, gy, tick, cfg.seed_salt)`.
Chunks without sticky `Chunk::has_biomaterial` are skipped; the flag
clears when a scan finds no Bone / Muscle / Skin left.

Fungi digest still targets Organic only. Muscle / Skin rot to Organic
first; Bone never enters the fungi path.

## Tick pipeline

Demo order (after condensation / karst):

1. Optional `apply_biological_decay` when `bio_decay_on`
2. Geotech map / organism roots / `tick_with_life` …

Tab → **Bio decay** exposes the three probabilities.

## Studio parity

Wave K made Bone / Muscle / Skin paintable ModuleIds with `PixelTraits`.
Wave L makes dead cells of those kinds persist as world materials.
See [`organism/PALETTE.md`](organism/PALETTE.md) and
[`organism/GENES.md`](organism/GENES.md).

## Bone fragility (Wave N)

Two paths turn Bone into Sand under load:

1. **Dead world Bone** — opt-in [`apply_bone_crush`](../crates/wk-voxel/src/failure.rs)
   (Tab → Geotech → **Bone crush**). When overburden σᵥ ≥
   `BONE_CRUSH_SIGMA_MIN` (or ≥ 6 solid cells above without a map),
   `MaterialId::Bone` → `Sand`. Chance + event caps match other geotech
   knobs. Off by default.
2. **Live `ModuleId::Bone`** — always on during `OrganismStore::step`.
   Column load = Σ (`mass × density`) of body modules in the same `dx`
   with higher `dy`. Capacity =
   `3.5 × stiffness × density × strength`. The lowest overloaded Bone
   pixel fractures (at most one / tick / organism), drops Sand into dry
   Air, and is removed from the body.

**F1 roof:** Bone ceilings participate (`roof_span_max_m = 4`). Debris is
**Sand** (Bone is not a grain — identity-keep would strand solids).

## Scenario

`e18_bone_persists_after_muscle_rots` — Nucleus + Bone + Muscle + Skin
creature dies; after dissolve, Muscle/Skin become Organic while Bone
lingers, then Bone eventually becomes Sand under elevated `bone_prob`.

`e19_bone_fragility` — Bone roof → Sand debris; dead Bone crush under
stack; live soft Bone fractures under self-stack.
