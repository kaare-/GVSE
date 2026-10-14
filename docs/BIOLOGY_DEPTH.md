# Biology depth *(dig D)*

**Status:** D3b in progress (2026-10-14). Product dig order **D** from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Soak-gated; no ship-UI
work under this dig. Dig D2–D5 merged to park-bank.

## Intent

Deepen living worlds (ecology / organism cost / climate×life) without
reopening TRACKED / park. Lab may feel richest; Wild stays allowlisted.

## D1 — Hotspot #2 harvest-neutral *(done)*

Phase 6 deferred land/metab cut
([`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md)): tip×count-always
and stem-integrity cadence both starved river tip growth
(`plant_size_is_limited_by_cost_not_by_the_ceiling`).

**Approach:** For grounded upright multi-leaf plants, keep a **full
posed-leaf light sum** every 4 ticks; on skip ticks scale that cache by
**tip-sky ratio** (not tip×count). Absolute harvest stays near the last
true canopy walk; day/cloud still track via tip sky. Fallen / stemless /
single-leaf paths unchanged.

### Acceptance

- [x] `plant_size_is_limited_by_cost_not_by_the_ceiling` green  
- [x] Compact organisms-on land/metab ↓ vs prior ~0.34 → **~0.26** ms
      at 256 (`phase6-d1-organisms-compact-baseline.log`)  
- [x] Short organisms-on soak: park=0, TRACKED −0.01/t, living=50  
- [x] Linked from Phase 6 scope + product roadmap  

## D2 — Release ecology path *(done)*

Studio → world entry under [`SpawnGate::Release`](../crates/wk-voxel/src/blueprint.rs)
([`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)). Postcard `.gvsecrt`
bytes with host quarantine, cooldown, and a soft living budget.

### Acceptance

- [x] `ReleaseBlueprint` (`COMMAND_SCHEMA` 8); Operator allow / Viewer deny  
- [x] Quarantine / cooldown / soft-cap / oversize rejects  

## D3 — Population / litter ecology *(done)*

Living habit soft-caps inside `MAX_ATOMS` (plants 180 / fungi 48 /
atoms 128). Parked stream-side life counts.

### Acceptance

- [x] `habit_soft_cap_blocks_fungus_not_plant`  
- [x] `SpawnFail::HabitCap` on host + editor  

## D3b — Corpse settle → soft-litter drip *(this wedge)*

Phase 6 soft-capped litter **maps**. Dig D3b improves litter
**turnover**: resting corpses drip soft litter during settle (half of
`death_litter_budget`), and dissolve banks only the remainder. Instant
death still leaves zero litter (grey linger first).

| Dial | Default |
|------|---------|
| Period | `CORPSE_SETTLE_LITTER_PERIOD` = 60 settled ticks |
| Drip | `CORPSE_SETTLE_LITTER_DRIP` = 1 unit / pulse |
| Cap | half of `death_litter_budget` during settle |
| Track | `Corpse::litter_dripped` (serde default 0) |
| Dissolve | `dissolve_corpse_to_organic_with_drip` |

### Acceptance

- [x] Contract in this file  
- [x] `death_leaves_grey_corpse_then_organic` — drip mid-settle; dissolve keeps litter  
- [x] Instant death still litter=0  
- [ ] CI voxel stack green  

## D4 — E2 tipped grey-log *(owner-only; skipped)*

Pose + `corpse_rgb` already land; accept is 1× studio eyeball.

## D5 — Climate×life on existing genes *(done)*

Drought-stressed grow-pulse `alloc_root` lean (`apply_drought_alloc_stress`).

### Acceptance

- [x] `drought_alloc_boost_gene_scaled_stressed_only`  

## Later Dig D options *(not this PR)*

| Id | Topic |
|----|--------|
| D5b | Cold↔`metabolic_rate` Q10 throttle (E46d-safe; no cold death) |

## Related

- [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md)  
- [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md) · [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)  
- [`organism/GENES.md`](organism/GENES.md) · [`organism/VOXEL_PLANTS.md`](organism/VOXEL_PLANTS.md)  
