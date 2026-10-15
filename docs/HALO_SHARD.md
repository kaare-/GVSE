# Halo / shard handoff *(dig F contract)*

**Status:** F0–F3 Lab seam path landed (2026-10-15). Product dig order **F**
from [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Region envelope:
[`REGION_CAPACITY.md`](REGION_CAPACITY.md). Cost: [`HOSTING_COST.md`](HOSTING_COST.md).

## Intent

Grow a **continuous map** by linking **Compact-sized regions** (one sim
process each) at shared edges. Reuse the in-process streaming vocabulary
(`HALO_CHUNKS`, resident freeze, `FrozenBacklog`, outbox exchange) across
**process** boundaries. Lab proves the seam; Shadow adds discovery; Wild
is last.

## Doctrine

| Rule | Meaning |
|------|---------|
| One region = one host truth | No dual-write of the same column |
| Halo is read replica | Neighbor edge strip is mirrored for CA / fields; owner process remains authoritative |
| Grow by shards, not fat rings | Official unit stays Compact ([`REGION_CAPACITY.md`](REGION_CAPACITY.md)) |
| Async backlog > hard lockstep | Missing / slow neighbor behaves like a frozen resident chunk |
| Mass audit stays honest | Seam transfers book like outboxes / frozen backlog, never silent mint/sink |
| Lab first | Two local hosts before any WAN / discovery / Wild mesh |
| ViewFrames stay peer→host | Halo traffic is edge state, not strip video ([`HOSTING_COST.md`](HOSTING_COST.md)) |

## What already exists *(reuse, do not reinvent)*

| Primitive | Where | Dig F use |
|-----------|--------|-----------|
| `HALO_CHUNKS` / resident / evict | `wk-voxel` streaming | Same widths for inter-process halo depth |
| `FrozenBacklog` + thaw paint | `frozen_backlog.rs` | Neighbor down / lag → absorb like frozen resident |
| Outbox / air-sat xfers | water flow commit path | Payload shape for seam water + sediment |
| World ring wrap | single process | **Not** a substitute for multi-region; Lab seams are open edges between two envelopes |
| `WorldTier` Lab/Shadow/Wild | host IPC | Policy only; F1 stays Lab |

In-process “halo” today means **camera residency**, not shard mesh. Dig F
extends the **edge bookkeeping** pattern to a second process.

## Geometry *(Lab default)*

```text
  Region A (host A)              Region B (host B)
  … | chunk … | edge chunk ||||| edge chunk | chunk … | …
                         ^ seam face
                         |  shared vertical face between
                         |  last owned col of A and first of B

  Each host:
    - owns its columns exclusively
    - keeps a read-only halo of the neighbor’s edge (`HALO_CHUNKS` deep)
    - never mutates halo cells; applies inbound transfers only on owned cols
```

- **Seam face** = one vertical interface (1D world for now; no 2D grid of
  regions until Lab 1D chain is boring).
- **Halo depth** = `HALO_CHUNKS` (default 3) of neighbor edge columns,
  mirrored each exchange.
- **Worldgen continuity** — Lab smoke may use mirrored / shared seed +
  abutting `world_x` ranges so terrain meets; artistic continuity is a
  later polish, not an F1 gate.

## Ownership & clock

| Topic | Lock |
|-------|------|
| Column owner | Exactly one host; stamped on region id + local `world_x` |
| Tick | Each host advances on its own product step; seam messages carry **source tick** |
| Apply rule | Inbound transfer applied on next local barrier/commit, like frozen backlog thaw |
| Stale halo | Keep last good mirror; mark edge “absorbing” if neighbor silent > N ticks (Lab default **60** product steps ≈ 3 s at 20 Hz) |
| Conflict | Impossible by construction — only owner writes owned cols; reject inbound that targets non-owned |

Hard lockstep across WAN is a **non-goal**. Lab may still run both hosts
on one machine at 20 Hz for soak readability.

## Payload layers *(ordered wedges)*

| Layer | Contents | Dig wedge |
|-------|----------|-----------|
| **P0 mass** | Water `sat` / Air xfers, suspended sediment, dissolved (pro-rata as today) | **F1** Lab smoke |
| **P1 fields** | Edge humidity / temp / wind skin samples needed for CA continuity | **F2** |
| **P2 life** | Organism / corpse / spore handoff (quarantine + habit caps) | **F3** |
| **P3 climate strips** | Coarser rain/cloud seam if P1 proves insufficient | optional |

Moisture backlog still open in-process; do not invent a richer moisture
protocol in F1 than `FrozenBacklog` already books.

### Organism handoff *(F3 sketch — not F1)*

- Atom crosses seam → serialize on owner, **delete locally**, deliver once
  with idempotency key; receiver `spawn` under existing gates
  (`SpawnFail::HabitCap`, Release quarantine rules where tier demands).
- No dual residency. If ACK lost, owner keeps a short “in flight” tombstone
  until ACK or timeout→re-spawn locally (mass-safe).
- Parked stream organisms: hand off only when the **column owner** changes;
  do not migrate the whole park set on every tick.

## Failure modes

| Neighbor state | Edge behaviour |
|----------------|----------------|
| Up to date | Exchange halo mirror + apply P0 transfers |
| Slow / partial | Apply what arrived; backlog remainder like frozen |
| Down / timeout | Absorbing edge — book outbound into local seam backlog; no reflective teleport |
| Rejoin | Replay backlog toward neighbor; refresh halo mirror from snapshot |

Mass audit: seam backlog counts in the same units as sat / sediment
totals (parallel to `frozen_backlog_total`).

## Transport *(Lab F1)*

| Dial | Lab F1 default |
|------|----------------|
| Topology | Two `wk-voxel-host` on loopback |
| Framing | New **seam** postcard channel (not ViewFrame, not player commands) |
| Auth | Shared local token / `--seam-token` (same spirit as peer Hello) |
| Rate | ≤ product step (20 Hz); coalesce if behind |
| Discovery | **None** — explicit `--seam-peer host:port` + side (`west`/`east`) |

Shadow discovery / hole-punch / directory = later dig (F4+). Do not hairpin
ViewFrames through a relay for seam state.

## Tier rollout

| Tier | When | Extra rules |
|------|------|-------------|
| **Lab** | F1–F3 | Operator may pause both; Viewer pause policy unchanged per host |
| **Shadow** | After Lab P0 green | Discovery + safe neighbor allowlist; scrap hosts may drop seams anytime |
| **Wild** | After Shadow + compress + allowlist | Official region graph; GDPR on official hosts only |

## Acceptance gates

### F0 — this note

- [x] Doctrine + Lab geometry + payload order written  
- [x] Linked from product / region / backlog docs  
- [ ] Owner skim OK (no silent rewrite of Compact envelope)

### F1 — Lab mass seam smoke

- [x] Seam module + face export / backlog (`wk_voxel::seam`)  
- [x] Postcard seam channel + host `--seam-*` CLI (`seam_ipc`, `SEAM_SCHEMA` 1)  
- [x] In-process + TCP unit tests: pair sat conserved; neighbor-down keeps outbound  
- [ ] Owner two-host loopback play smoke (`--dev --seam-side` / `--seam-listen` / `--seam-peer`)  
- [ ] Macroquad / headless park gate on **each** region still green in isolation  

**Landed API:** `attach_open_seam`, `export_seam_face`, `exchange_seam_p0`,
`Host::enable_lab_seam` / `drain_seam_outbound` / `apply_seam_inbound`.
Host flags: `--seam-side west|east`, `--seam-listen`, `--seam-peer`,
`--seam-token`.

### F2 — edge field mirror

- [x] `SeamFieldMirror` (hum / temp / wind_vx) on `SEAM_SCHEMA` 2 Exchange  
- [x] Soft blend owned edge toward peer mirror (`SEAM_FIELD_BLEND_ALPHA` 0.25)  
- [x] Unit test: edge temp moves toward peer sample  
- [ ] Owner two-host feel check (optional)

### F3 — organism / corpse handoff

- [x] `SeamLifeParcel` + in-flight tombstones / idempotent ACK (`SEAM_SCHEMA` 3)  
- [x] Face export when anchor `gx` is on the open edge; remap onto peer face  
- [x] Habit / pop refuse → no ACK; timeout re-spawns on owner  
- [x] Unit tests: cross once; habit-cap revive  
- [ ] Owner two-host creature walk (optional)  

## Explicit non-goals *(until unlocked)*

- Seamless continent-scale Wild MMO / interest management across dozens of regions  
- Autoscale vCPU by Glance count  
- 2D region grids, vertical shard stacks, or cross-tier Lab↔Wild live seams  
- Proxying Presence strips as “halo”  
- Replacing in-process stream/park with multi-process for a single camera  

## Proposed agent wedges

| Id | Topic | Depends |
|----|--------|---------|
| **F0** | This design note | — |
| **F1** | Seam IPC + P0 mass exchange (Lab two-host smoke) | F0 |
| **F2** | P1 edge field mirror | F1 |
| **F3** | P2 organism / corpse handoff | F1 (+ biology gates) |
| **F4** | Shadow discovery + neighbor allowlist | F1 |
| **F5** | Wild official region graph | F4 + hosting/compress |

## Related

- [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) · [`REGION_CAPACITY.md`](REGION_CAPACITY.md)  
- [`HOSTING_COST.md`](HOSTING_COST.md) · [`WORLDGEN.md`](WORLDGEN.md) (in-process halo)  
- [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md) · [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md)  
- [`BIOLOGY_DEPTH.md`](BIOLOGY_DEPTH.md) (habit caps / Release — F3)  
