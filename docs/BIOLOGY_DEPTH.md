# Biology depth *(dig D)*

**Status:** D5b in progress (2026-10-14). Product dig order **D** from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Soak-gated; no ship-UI.
Dig D2–D5 + D3b merged to park-bank.

## Intent

Deepen living worlds (ecology / organism cost / climate×life) without
reopening TRACKED / park. Lab may feel richest; Wild stays allowlisted.

## Landed

| Id | Topic |
|----|--------|
| D1 | Tip-sky–scaled land/metab light cache |
| D2 | `ReleaseBlueprint` ecology path |
| D3 | Habit soft-caps (plant/fungus/atom) |
| D3b | Corpse settle → soft-litter drip |
| D4 | E2 grey-log — **owner-only** (skipped) |
| D5 | Drought↔`alloc_root` grow-pulse lean |

## D5b — Cold↔`metabolic_rate` Q10 *(this wedge)*

E46d already sterilises fission below `optimum − width` without killing.
Dig D5b adds a **quiet winter** activity scale: harvest and upkeep drop
together below the cold edge; high `metabolic_rate` throttles harder;
thrifty lineages stay relatively lively. Growth blocks when activity ≤
`COLD_BLOCKS_GROWTH`. No cold energy drain (founder survives).

| Dial | Default |
|------|---------|
| Slack | `cold_slack` — 0 at edge, 1 one `temp_width` colder |
| Activity | `cold_activity` — floor `COLD_ACTIVITY_FLOOR` = 0.35 |
| Severity | × `metabolic_rate/2` (clamped) |
| Growth | blocked when activity ≤ `COLD_BLOCKS_GROWTH` = 0.55 |
| Surfaces | land plants (coldest body cell) + Set A atoms (seat cell) |

### Acceptance

- [x] Contract in this file  
- [x] `cold_activity_throttles_hot_metab_harder`  
- [x] E46d `cold_water_blocks_atom_fission_founder_survives` still green  
- [ ] CI voxel stack green  

## Related

- [`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md)  
- [`organism/GENES.md`](organism/GENES.md) · [`organism/SCENARIOS.md`](organism/SCENARIOS.md) E46d  
