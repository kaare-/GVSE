# Biology depth *(dig D)*

**Status:** D3 in progress (2026-10-14). Product dig order **D** from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Soak-gated; no ship-UI
work under this dig.

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
([`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)). Not free
`SpawnOrganism` / paint — postcard `.gvsecrt` bytes with host
quarantine, cooldown, and a soft living budget.

| Dial | Default (Operator-first) |
|------|--------------------------|
| Who | Operator only this wedge (Wild player Release stays allowlist-later) |
| Wire | `ClientCommand::ReleaseBlueprint { col, y, bytes }` (`COMMAND_SCHEMA` 8) |
| Size | ≤ `MAX_RELEASE_BLUEPRINT_BYTES` (64 KiB) |
| Modules | ≤ `MAX_RELEASE_MODULES` (256) after Mid-only quarantine |
| Gate | `Blueprint::quarantine_for_release` → `validate_for_spawn(Release)` |
| Cooldown | `RELEASE_COOLDOWN_TICKS` (30) between successes; cleared on `ResetWorld` |
| Cap | `RELEASE_LIVE_SOFT_CAP` (32) living + existing atom pop cap |
| Quarantine | Mid lane only; genome clamped to mutation ranges; disk file untouched |

### Acceptance

- [x] Contract in this file + studio / allowlist crumbs  
- [x] `ReleaseBlueprint` on host; Operator allow / Viewer deny  
- [x] Invalid habit / oversize / cooldown / soft-cap reject with ACK detail  
- [x] Built-in atom blueprint Release places living organism  
- [ ] Wild player Release (identity + per-account dials) — later  

## D3 — Population / litter ecology *(this wedge)*

Phase 6 already soft-capped litter / residue **maps**. Dig D3 soft-caps
**living habits** so one bloom (esp. fungi) cannot wallpaper the global
`MAX_ATOMS` bucket. HUD already shows `p=/f=/a=` — soft-caps match that
split. Parked stream-side life counts (same fairness as pop cap).

| Dial | Default |
|------|---------|
| Plants | `HABIT_SOFT_CAP_PLANTS` = 180 |
| Fungi | `HABIT_SOFT_CAP_FUNGI` = 48 |
| Atoms (floaters + epiphytes) | `HABIT_SOFT_CAP_ATOMS` = 128 |
| Global | existing `MAX_ATOMS` / `atom_cap` still binds |
| Gates | sprout / spore birth / fission / emergent fruiting / plant spore-bank wake / editor+Release spawn |
| Litter maps | unchanged (Phase 6 GC) |

### Acceptance

- [x] Contract in this file  
- [x] `habit_soft_cap_blocks_fungus_not_plant` — fungus soft-cap binds; plant still spawns  
- [x] Parked fungi count toward habit soft-cap  
- [x] Host / macroquad editor surface `SpawnFail::HabitCap`  
- [ ] Short organisms-on soak still park=0 / TRACKED flat (CI stack)  

## Later Dig D options *(not this PR)*

| Id | Topic |
|----|--------|
| D4 | E2 tipped grey-log owner visual |
| D5 | Climate×life stress on existing genes |
| D3b | Corpse settle → soft-litter drip (litter turnover follow-on) |

## Related

- [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md)  
- [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md) · [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)  
