# Biology depth *(dig D)*

**Status:** D1 in progress (2026-10-14). Product dig order **D** from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Soak-gated; no ship-UI
work under this dig.

## Intent

Deepen living worlds (ecology / organism cost / climate×life) without
reopening TRACKED / park. Lab may feel richest; Wild stays allowlisted.

## D1 — Hotspot #2 harvest-neutral *(this wedge)*

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

## Later Dig D options *(not this PR)*

| Id | Topic |
|----|--------|
| D2 | Release ecology path (`SpawnGate::Release`) |
| D3 | Population / litter ecology tuning |
| D4 | E2 tipped grey-log owner visual |
| D5 | Climate×life stress on existing genes |

## Related

- [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md)  
- [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md) · [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)  
