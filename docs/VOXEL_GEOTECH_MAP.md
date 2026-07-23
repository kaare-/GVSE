# Geotech stress maps — shear demand, wetness, overburden

*Plan for slow derived overlays that sweep contacts / columns and
modulate failure. Complements [`VOXEL_FAILURE.md`](VOXEL_FAILURE.md)
(F1–F3 CA writes) and [`VOXEL_FIELDS.md`](VOXEL_FIELDS.md) (fields
modulate; cells own mass).*

## Goal

Replace “rediscover geometry inside every failure tick” with a
**general stress mapping** rebuilt on a slow cadence:

1. Sweep solid↔Air (and later solid↔solid) **contacts**.
2. Derive scalar fields: shear demand, pore wetness, overburden σᵥ,
   optional lateral hydro load.
3. HUD (`G`) visualises; CA failure **reads** the map instead of
   re-walking every face every tick.
4. Not every physics substep — period ~20 like Temperature.

```
cells ──period-20 sweep──► GeotechMap (shear / wet / σᵥ / hydro)
                              │
                              ▼
                    F2 shear / F3 compact / HUD
```

## Why not every tick

Full contact sweeps are O(loaded faces). Stress geometry changes
slowly vs flow×12. Dirty-halo-only scans **miss** static wet cliffs
(see F2 full-chunk fix). Period rebuild of all loaded chunks is the
honest default; later: dirty-tile invalidation.

## Map contents

| Channel | Unit (v1) | Source |
|---------|-----------|--------|
| **Shear demand** | 0..~4 score | Face relief (1–2) + optional hydro column bonus |
| **Wetness** | 0..1 | `sat/capacity` on solid faces (and tile avg later) |
| **Overburden σᵥ** | relative | Σ density above cell (column walk) |
| **Hydro load** | cells | Contiguous wet-Air column height beside face |

v1 stores a **sparse** `HashMap<(gx,gy), FaceStress>` for solid cells
with any open face (demand ≥ 1). Buried rock omitted.

```text
FaceStress {
  shear_demand: f32,   // geometry + hydro proxy
  wetness: f32,        // pore fill 0..1
  overburden: f32,     // relative σᵥ (optional v1)
  hydro_load: u16,     // wet Air column height (cells)
}
```

### Shear score (v1)

```
base = face_shear_demand(gx, gy)          // 0, 1, or 2
hydro = wet_air_column_beside(gx, gy)     // 0..H_cap (e.g. 32)
score = base + k_hydro * (hydro / H_cap)  // k_hydro ~ 2.0
```

A 1-cell stone dam holding a tall reservoir lights up high on `G`
even when dry-stone `c_eff` would currently refuse demand-1 in F2b.
That gap is intentional: **map first, gate CA second**.

### Wet Air column

From a side-Air neighbour, walk **up** while cells are Air with
`sat ≥ FILM` (e.g. 200). Cap height. This is a **proxy** for lateral
hydrostatic load — not a full pressure solver.

## Cadence / tick placement

```
tick_with_configs   // water → grain → F1/F2 CA (still local for now)
apply_flow_erosion
★ geotech_map.rebuild(world)   if geotech_map_due(tick)   // period 20, phase 7
humidity.diffuse if due
temperature.step if due
…
```

- Rebuild **after** CA so the map matches post-tick geometry.
- F2b may keep using live `face_shear_demand` until **S3** wires
  map-gated failure (avoid double semantics mid-migration).

## HUD

| Key | Overlay |
|-----|---------|
| `G` | Geotech shear-demand (cool → hot on face cells) |

Optional later: cycle `G` modes (shear / wet / σᵥ) or Tab checkbox.

Inspector: when `G` on or always, show `shear` / `hydro` / `wet` for
the hovered solid if present in the map.

## Phases (PR chain)

| Phase | Deliverable |
|-------|-------------|
| **S0** | ✅ This plan + index in `docs/README.md` / failure F4 pointer |
| **S1** | ✅ `GeotechMap` rebuild (shear + wetness + hydro) + `G` overlay + tests |
| **S2** | ✅ Overburden σᵥ channel + `G` cycles shear / σᵥ / wet |
| **S3** | ✅ F2b gates on map shear score (thin wet dams) |
| **S4** | F3 compaction gates on σᵥ map |
| **S5** | Perf: dirty-tile rebuild; optional rayon sweep |

### Backlog (investigation — not this PR)

| Item | Note |
|------|------|
| Waterline not equalising | Tall wet column vs low basin — flow/head, not geotech |
| Thin dam intuition | ✅ S1 lights on `G`; S3 map-gated shear can break it |
| Full Mohr–Coulomb | Out of scope |

## Config

```text
Tab → Geotech
  [x] Rebuild stress map (default on)
  Map period (ticks) — advanced, default 20
```

Map itself is **not** saved in `SimSnapshot` (derived). Rebuild on
load / first due tick.

## Tests (S1)

| Test | Expect |
|------|--------|
| `dry_buried_stone_absent` | No map entry |
| `vertical_cliff_face_recorded` | demand ≥ 1 |
| `tall_wet_column_raises_hydro_and_score` | Dam face score > dry cliff face |
| `rebuild_is_deterministic` | Same world → same map |
| `geotech_map_due_phase` | Period/phase gate |

## Acceptance

- `G` shows hot edges on cliffs and especially on thin walls against
  deep water.
- Rebuild cost stays off the flow×12 path; period 20 by default.
- F1/F2 behaviour unchanged until S3 explicitly gates on the map.

## Done when

1. Plan checked in and linked from failure/fields docs.
2. S1 map + overlay playable.
3. S3 makes the “paper dam” case fail under high hydro score without
   melting dry inland mountains (chance + caps remain).
