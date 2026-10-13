# Hosting cost — gaming numbers from known Compact data

**Status:** planning sketch (2026-10-13). Order-of-magnitude only —
not a quote. Uses measured / coded constants from Phase 4–8. Pillars:
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Region/shard doctrine:
one instance = one max envelope; grow map by sharding.

Prices below are **rough AWS us-east-ish on-demand list** as of late
2025/2026 thinking (~±30%). Spot / reserved / egress tiers move the
decimal; they do not change the **egress cliff**.

## What we already know

| Fact | Value | Source |
|------|--------|--------|
| Region shape (ship unit) | Compact **4096×tropopause**, stream+park | Phase 4/8 |
| Sim headroom (stream) | **~115 sim-FPS** (~**8.7 ms**/tick) | Compact re-profile |
| Full-resident Compact | ~**19 sim-FPS** (~53 ms) — **not** the ship path | Phase 4 gate |
| Ship present cadence | **20 Hz** (`DEFAULT_STEP_PERIOD_MS` = 50) | `wk-voxel-client` |
| Default view strip | **720×384** RGBA | `COMPACT_VIEW_COLS` / `COMPACT_VIEW_ROWS` |
| Strip payload | **720×384×4 ≈ 1.05 MiB**/frame | IPC comment |
| Resident chunks (stream) | **~459** loaded (was 1088 full) | `VOXEL_PERF` A/B |
| 5 km single process | ~**74** sim-FPS after temp/rain stream — possible but heavier | Phase 4 |
| Max peers (wire) | default **4**, hard **8** | `PHASE8_MULTIPLAYER` |
| Encoding today | postcard + **raw RGBA** (no compression) | `ipc.rs` |

### CPU read

At **20 Hz** product step we need ≤50 ms/tick. Compact stream at
~8.7 ms leaves **~5× headroom** on the profile machine (single-thread
hot path). So **one modern 2–4 vCPU box** can own **one Compact region**
for a handful of overlapping cameras **if** residency stays local.

Far-apart cameras → union residency → tick climbs toward full-resident
(~19 FPS). That is a **gameplay / interest** limit, not an AWS SKU fix.

### Bandwidth read (the scary one)

Per viewer, uncompressed, 20 Hz:

```text
1.05 MiB × 20 Hz ≈ 21 MiB/s ≈ 170 Mbit/s
≈ 76 GiB/hour ≈ 55 TiB/month (24/7 one viewer)
```

AWS egress often ~**$0.08–0.09 / GB** after free tier →  
**one always-on uncompressed viewer ≈ $4k–5k / month** in data alone.
That is **not realistic**. Official hosting is unrealistic **until
ViewFrame is compressed / delta’d / rate-capped for remote**.

## Scenario table *(after fixing egress)*

Assume we ship at least one of: **zstd on strip**, **dirty tiles /
delta**, **remote present 10 Hz**, **narrower default view** — and that
combined we hit ~**10×** smaller wire than raw RGBA (planning target,
not measured). Then ~**17 Mbit/s** → ~**5.5 TiB/month**/viewer 24/7,
or ~**$400–500**/mo egress per glued-on viewer — still harsh.

Tighter **target for “friends on one official region”**:

| Knob | Target |
|------|--------|
| Remote present | **10 Hz** |
| Compression + delta | **≥20×** vs raw (goal) |
| View | ≤720 wide; encourage overlap |
| Concurrent peers | **4** (current default) |

Wire then ~**4 Mbit/s**/viewer → ~**1.3 TiB/month**/viewer 24/7 →
~**$100–120**/mo egress **per always-on spectator**.  
Part-time play (e.g. 4 h/day) cuts that ~6× → **~$15–20**/viewer/mo.

### Compute (one Compact region)

| SKU (illustrative) | Role | Ballpark $/mo on-demand |
|--------------------|------|-------------------------|
| `c7i.large` 2 vCPU / 4 GiB | Dev / 1–2 peers | ~$60 |
| `c7i.xlarge` 4 vCPU / 8 GiB | Comfortable 4 peers | ~$120 |
| `c7i.2xlarge` 8 vCPU / 16 GiB | Headroom / noisy neighbors | ~$240 |
| + 50–100 GB SSD | saves / logs | ~$5–10 |

Reserved / 1-year save ~40–50%. Spot is fine for soak CI, risky for
live public.

**RAM:** stream Compact is hundreds of MB–few GB class (459 chunks +
fields + orgs). Prefer **≥8 GiB** for official so OS + peak paint/spawn
do not OOM.

### Bundled “official” sketches

| Scenario | Compute | Egress* | Total ballpark |
|----------|---------|---------|----------------|
| **A. Friends only** — 1 region, ≤4 peers, ~4 h/day each, compressed remote | ~$60–120 | ~$20–80 | **~$80–200 / mo** |
| **B. One public always-on demo** — 1 region, avg 2 viewers glued 24/7, compressed | ~$120 | ~$200–400 | **~$300–500 / mo** |
| **C. Raw RGBA 20 Hz public** | ~$120 | **$k+** | **Unrealistic — do not** |
| **D. N regions (shards)** | N × compute | N × local viewers | Linear in regions; halo traffic small vs strips |

\*Egress dominates until compression lands. Numbers assume the
**≥20× + 10 Hz** target above.

### Homelab / scrap

Power + residential uplink, not AWS list. A BC-250 / mini-PC can run
**one Compact** for LAN/friends if uplink is the limit (~20–50 Mbit
upload → **must** compress; often only 1 remote peer). Fine for
**dev / community BYO**; weak for GDPR, Steam-trust, and 24/7 public.

## How it gets realistic

Ordered levers (do before scaling instances):

1. **Compress ViewFrame** (zstd on RGBA or tile codec) — mandatory for WAN  
2. **Delta / dirty rectangles** — strip rarely fully changes  
3. **Remote present ≤10 Hz**; keep 20 Hz for local/LAN  
4. **Interest** — overlapping cameras; avoid union→full-resident  
5. **Region max = Compact** (or measured twin); shard for map size  
6. **Session length economics** — bill or design for play sessions, not 24/7 AFK viewers  
7. **CDN later** only helps static/marketing; **sim strips stay peer→host**

## Region capacity (working definition)

Until a soak locks better numbers, **official region** =

```text
Compact 4096 × tropopause
stream + park
target ≥30 sim-FPS under load (headroom from ~115 idle stream)
max_peers = 4 (soft), 8 (hard)
remote present ≤10 Hz once WAN path exists
```

To increase **world** size → **more regions + halo/handoff**, not a
bigger single process (5 km single-box already showed temp/rain cost).

## Open measurements *(before trusting $)*

- [ ] zstd ratio on real Compact strips (day/night, busy shore)  
- [ ] RSS / peak RAM of `wk-voxel-host --` Compact on a known box  
- [ ] Tick ms with 4 peers at opposite ends of the ring  
- [ ] Actual Mbps after compression at 10 Hz  

## Related

- [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) — pillars  
- [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md) — peers / auth  
- [`VOXEL_PERF.md`](VOXEL_PERF.md) — stream FPS history  
