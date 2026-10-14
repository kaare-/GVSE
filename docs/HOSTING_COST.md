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
| Encoding today | postcard + **zstd RGBA** on IPC Frame (`StripCodec`, VIEW_SCHEMA 10) | `ipc.rs` |

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

### Homelab / scrap → **Shadow** tier

Power + residential uplink, not AWS list. A BC-250 / mini-PC can run
**one Compact** for LAN/friends if uplink is the limit (~20–50 Mbit
upload → **must** compress; often only 1–2 remote peers).

In the product model this is **first-class Shadow**: player-hosted
shards that can **dynamically link** when online. Our cost is mostly
**relay/directory** (cheap) if peers mesh P2P; full relay of strips
re-creates the egress cliff. Prefer: hole-punch / local LAN / optional
small TURN for signaling — not proxying every ViewFrame through AWS.

**Lab** (offline local) is $0 hosting. **Wild** (official sub) is where
the AWS numbers in this doc apply. See
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) § Lab · Shadow · Wild.

## Money takeaway

| Tier | Who pays | Recurring product fee? |
|------|----------|----------------------|
| Lab | Player electricity | No |
| Shadow | Player / friends hardware + uplink | No (optional tip jar / sub for directory niceties) |
| Wild | Us (AWS) → **player subscription** | **Yes — that’s the point** |

Wild without a sub only works as a tiny loss-leader demo, not 10k
casual Presence.

## How it gets realistic

Ordered levers (do before scaling instances):

1. **Compress ViewFrame** (zstd on RGBA or tile codec) — mandatory for WAN  
2. **Delta / dirty rectangles** — strip rarely fully changes  
3. **Remote present ≤10 Hz** — viewer `--connect` to non-loopback uses
   100 ms Pull cadence; keep 20 Hz for local/LAN / Lab / `--spawn-host`  
4. **Interest** — overlapping cameras; avoid union→full-resident  
5. **Region max = Compact** (or measured twin); shard for map size  
6. **Session length economics** — Glance vs Presence; no 24/7 AFK strips  
7. **Shadow P2P** — don’t hairpin BYO mesh video through official egress  
8. **CDN later** only helps static/marketing; **sim strips stay peer→host**

## Region capacity

**Contract:** [`REGION_CAPACITY.md`](REGION_CAPACITY.md) (dig C).
Official region = Compact 4096 × tropopause, stream+park, ≥30 sim-FPS
under load, peers 4/8, WAN present ≤10 Hz after compress.

To increase **world** size → **more regions + halo/handoff**
([`HALO_SHARD.md`](HALO_SHARD.md)), not a bigger single process (5 km
single-box already showed temp/rain cost).

## Fun scale: ~10k players (mostly casual)

**10k accounts ≠ 10k ViewFrames.** Mix assumed:

| Cohort | Habit | Wire product |
|--------|--------|----------------|
| **Bus / kill-time** | Open 30–90 s, check creature / season / “is my fungus alive?” | **Lite status** — small JSON / tiny thumbnail / last snapshot (KB, not MiB×Hz) |
| **Session play** | Sit 10–40 min, pan a bit | Compressed ViewFrame, **5–10 Hz**, one camera |
| **TV lean-back** | World as ambient on a big screen | One long stream, **5–10 Hz**, maybe wider strip; still compressed |
| **Glued** | Few power users / streamers | Full viewer; cap concurrent per region |

### Concurrent load sketch (illustrative)

For **10k registered**, casual games often see roughly:

```text
DAU           ~2k–4k   (20–40%)
Peak online   ~200–600 (sessions open — many lite)
Heavy strip   ~20–80   (real ViewFrame pullers at once)
TV / glued    ~5–20    (long-lived, hungry)
```

If **every** peak-online user got today’s raw 20 Hz strip, egress alone
is fantasy money. If **lite-first**:

| Path | Peak count | Payload class | Egress order (compressed targets) |
|------|------------|---------------|-----------------------------------|
| Lite check-in | 300 | ~1–20 KB / open | **Negligible** (GB/day, not TiB) |
| Session strip | 40 @ 10 Hz, ~20× smaller than raw | ~0.5–2 Mbit/s each | ~**$150–600 / mo** if those 40 were 24/7; real sessions ≪ that |
| TV / glued | 10 @ 10 Hz | ~1–4 Mbit/s each | **~$80–300 / mo** if 24/7; evenings-only cheaper |

**Compute:** creatures live in **regions**. 10k players’ organisms do not
fit one Compact forever — you shard by map and/or by “world instance.”

Rough region count if each Compact comfortably holds sim for a slice of
life (order-of-magnitude, not sim math):

```text
Suppose ~50–200 “active homes” of load per region
→ tens to low hundreds of region processes at 10k players
→ @ ~$80–120/mo reserved-ish each → $4k–20k/mo compute if all hot
```

That upper band is **if every region is always-on and full**. Reality
for casual:

- Cold regions **hibernate** (park to disk, wake on check-in)  
- Many players share a **public** region; private gardens are rarer  
- Lite check-in can hit a **read replica / snapshot service**, not wake
  full CA if the design allows “stale by minutes”

**Ballpark official stack at 10k casual** (after compression + lite API):

| Layer | Role | Crude $/mo |
|-------|------|------------|
| Always-on public regions | A few “main worlds” / showcases | $300–1.5k |
| Wake-on-demand shards | Player gardens / quiet zones | $200–2k (bursty) |
| Lite API + auth + DB | Bus check-ins, creature cards | $50–300 |
| Egress (strips + lite) | Dominated by TV/session, not bus | $200–1.5k |
| **Total (order)** | | **~$1k–5k / mo** early;  
| | | **tens of k$** only if many hot always-on regions + fat TV |

Without **lite check-in**, you accidentally build a 10k-seat video CDN
and the bill follows that industry, not a voxel toy.

### Product implication

The player client needs **two gears** (see
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md)):

1. **Glance** — bus/TV thumbnail / creature sheet / “world postcard”  
2. **Presence** — real ViewFrame session (paid in CPU + egress)

Studio stays offline/freedom. Operator still tunes live regions.

## Open measurements *(before trusting $)*

- [x] zstd on IPC Frame path (`StripCodec::ZstdRgba`, level 1)  
- [ ] zstd ratio on **real** Compact strips (day/night, busy shore) vs flat-sky unit ≥10×  
- [ ] RSS / peak RAM of `wk-voxel-host --` Compact on a known box  
- [ ] Tick ms with 4 peers at opposite ends of the ring  
- [ ] Actual Mbps after compression at 10 Hz  
- [ ] Lite snapshot size (creature card / 64×36 postcard)  

## Related

- [`REGION_CAPACITY.md`](REGION_CAPACITY.md) — locked envelope  
- [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) — pillars  
- [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md) — peers / auth  
- [`VOXEL_PERF.md`](VOXEL_PERF.md) — stream FPS history  
