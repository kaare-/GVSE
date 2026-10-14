# Region capacity *(dig C contract)*

**Status:** design locked as defaults (2026-10-14). Dig order **C** from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Money sketch lives in
[`HOSTING_COST.md`](HOSTING_COST.md); this file is the **single process
envelope** hosts and planners must not silently exceed.

## Doctrine

| Rule | Meaning |
|------|---------|
| One region = one sim process | No autoscale of tick work with player count |
| Grow map by sharding | Halo / handoff ([`HALO_SHARD.md`](HALO_SHARD.md), dig F) |
| Lab may be smaller or larger | Player hardware; not the Wild SKU |
| Wild ships Compact envelope | Below numbers are the official unit |
| Egress before more boxes | Compress ViewFrame before N× AWS regions |

## Locked Wild / Shadow “ship unit”

| Dial | Value | Code / source |
|------|--------|---------------|
| Width | **4096** cols (64 × `CHUNK_CELLS_W`) | `compact_worldgen_params` |
| Height | Bedrock → sky = tropopause + stratosphere lid | `TROPOSPHERE_TOP_Y` + `STRATOSPHERE_CELLS` |
| Residency | **stream + park** (not full-resident) | Phase 4/8 |
| Idle stream headroom | ~**115** sim-FPS (~8.7 ms/tick) | Compact re-profile |
| Load target | ≥**30** sim-FPS under Presence load | planning floor |
| Product step | **20 Hz** local / Lab (`DEFAULT_STEP_PERIOD_MS` = 50) | `wk-voxel-client` |
| Remote present (WAN) | ≤**10 Hz** once compression lands | [`HOSTING_COST.md`](HOSTING_COST.md) |
| Default view strip | **720×384** RGBA | `COMPACT_VIEW_COLS` / `ROWS` |
| Soft peer cap | **4** | `DEFAULT_MAX_PEERS` |
| Hard peer cap | **8** | `MAX_PEERS_CAP` |
| Interest | Prefer overlapping cameras | Far union → tick→full-resident (~19 FPS) |

```text
Official region envelope (Wild default; Shadow recommended)

  cols      = 4096
  stream    = on
  park      = on
  peers     ≤ 4 soft / 8 hard
  present   = 20 Hz Lab/LAN; ≤10 Hz WAN after compress
  view      ≤ 720×384 (default)
  sim-FPS   ≥ 30 under load (gate for “add another peer / view”)
```

**5 km single process** is a measured stretch (heavier temp/rain) — not
the Wild default SKU. Lab may use it if the machine allows.

## What breaks the envelope

| Pressure | Symptom | Response |
|----------|---------|----------|
| Peers at opposite ring ends | Union residency → ~19 FPS | Cap far cameras / interest; shard |
| Raw RGBA @ 20 Hz WAN | ~170 Mbit/s/viewer | **Block** public Wild until compress |
| Full-resident Compact | ~53 ms/tick | Never ship path |
| “Just widen the ring” | Temps/rain/fields climb | New region + halo ([`HALO_SHARD.md`](HALO_SHARD.md)), not one fat process |
| 24/7 Presence AFK | Egress bill | Glance gear ([`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md)) |

## Tier mapping

| Tier | Envelope |
|------|----------|
| **Lab** | Any size the box can hold; defaults may match Compact for parity soaks |
| **Shadow** | Recommend Compact unit; owner may oversize at their power/uplink risk |
| **Wild** | **Exactly** this envelope per process (or a measured twin after soak) |

## Acceptance gates *(ops / soak)*

Before calling a build “Wild-ready region”:

- [ ] Compact stream park=0 / TRACKED soak still green (macroquad path)  
- [ ] Host RSS measured on a known box ([`HOSTING_COST.md`](HOSTING_COST.md) open list)  
- [ ] 4 overlapping peers keep ≥30 sim-FPS on target SKU  
- [x] ViewFrame zstd IPC path exists (`VIEW_SCHEMA` 10)  
- [x] WAN `--connect` present ≤10 Hz (Lab/loopback stay 20 Hz)  
- [x] Region envelope constants match this table (`REGION_*` in client)  

## Non-goals *(until unlocked)*

- Autoscale vCPU with concurrent Glance users  
- One process spanning continent-scale without shards  
- Treating Shadow scrap hosts as matching Wild GDPR residency  

## Related

- [`HOSTING_COST.md`](HOSTING_COST.md) — $ / egress / 10k casual  
- [`HALO_SHARD.md`](HALO_SHARD.md) — multi-region seam / handoff  
- [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) · [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md)  
- [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md) · [`VOXEL_PERF.md`](VOXEL_PERF.md)  
