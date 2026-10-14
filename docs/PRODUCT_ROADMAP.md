# Product roadmap — pillars *(basis)*

**Status:** locked as planning basis (2026-10-13). Not a sprint backlog —
the long-horizon map so biology, client, studio, sharding, and ship work
do not thrash each other. Phase code backlogs
([`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md), [`ROADMAP.md`](ROADMAP.md))
stay the agent execution lists.

## Doctrine

| Rule | Meaning |
|------|---------|
| Host owns truth | Sim / mass / organisms only on whichever host runs that world |
| Three world tiers | **Lab** (offline) · **Shadow** (BYO mesh) · **Wild** (official sub) |
| Player world allowlist | Live interactions are **few and explicit** (stricter in Wild) |
| Studio = total freedom | Creature / blueprint design; not the live Wild editor |
| Operator ≠ player | Fine-tune official / owned hosts; full tools + settings |
| Region has a max | One instance = one capacity envelope; grow map by **sharding** |
| Soak stays macroquad | Park/TRACKED gates do not move to the ship client |

## Pillars

```text
                    ┌─────────────────────────────────────────┐
  blueprints        │  Lab — offline local universe             │
 ──────────────►    │  size = what your machine allows          │
 Design studio      └─────────────────────────────────────────┘
 (total freedom)                      │ optional export / migrate
                    ┌─────────────────▼───────────────────────┐
                    │  Shadow — player-hosted shards            │
                    │  dynamically linked at edges (halo)       │
                    │  your power / uplink; we help connect     │
                    └─────────────────┬───────────────────────┘
                                      │ optional "release to wild"
                    ┌─────────────────▼───────────────────────┐
                    │  Wild — official world server (sub)       │
                    │  see what creatures do in the real mesh   │
                    └─────────────────────────────────────────┘

Player client: Glance / Presence · allowlist by tier
Operator: fine-tune Wild (and owned Lab/Shadow hosts)
Ship / legal: menus · brand · Steam · GDPR on official path
```

### 1. Biology / sim depth

Continue ecology, organisms, climate, mass integrity. Park=0 / TRACKED
soaks remain the gate. Independent of ship UI polish.

**Contract:** [`BIOLOGY_DEPTH.md`](BIOLOGY_DEPTH.md) — Dig D agent-closed
on park-bank (D4 grey-log owner-only leftover).

### 2. Player client + world allowlist

Ship client is a **viewer** with a short, written allowlist of live
interactions (camera, present, maybe seed/drop later — **not** full
paint). Everything else is denied by host role policy.

At casual scale (bus check-ins, TV lean-back), the client needs **two
gears**: **Glance** (lite status / postcard — cheap) vs **Presence**
(real ViewFrame — expensive). See [`HOSTING_COST.md`](HOSTING_COST.md)
§ “10k players”.

**Contract:** [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md) — Lab / Shadow /
Wild × Viewer / Operator command matrix. Tighten `Host::command_allowed`
before more world UI.

### 3. Design studio (creature space)

Full freedom to design creatures / blueprints. Sketch studio exists but
is **terribly out of date** — needs a rewrite or replacement, not a
paint-over.

**Contract:** [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md) — `.gvsecrt`
schema 1, validate/reject, Lab spawn vs future Release (not free Wild
`SpawnOrganism`).

### 4. Worlds — Lab · Shadow · Wild

One **region** = one sim process with a **max load envelope** (planning
unit ≈ Compact 4096×tropopause + stream/park — locked in
[`REGION_CAPACITY.md`](REGION_CAPACITY.md)). Grow a continuous map by
**sharding** + **halo / handoff** — Lab-first contract in
[`HALO_SHARD.md`](HALO_SHARD.md).

| Tier | Who pays compute | What it is |
|------|------------------|------------|
| **Lab** | Player’s machine | Offline (or local-only) laboratory — as big as the box allows; full experimentation; no AWS bill |
| **Shadow** | Player / friends | Self-hosted shards, **dynamically connected** into a mesh when peers are online; scrap/homelab welcome; our software does discovery + seam, not the power bill |
| **Wild** | Subscription | Official always-on regions — the “real” wilderness where creatures prove themselves; Glance/Presence economics apply ([`HOSTING_COST.md`](HOSTING_COST.md)) |

**Money model:** client + Lab free (or Steam one-time); **Wild is the
sub**; Shadow is optional social/infra without forcing everyone onto
the paid mesh. Creatures can stay Lab forever, roam Shadow when hosts
are up, or be released into Wild when the player wants stakes.

### 5. Operator client

Fine-tuning live worlds: sim dials, save/load/reset, paint, spawn
presets (Phase 8 wedges landed). Broader dials / blueprint editor later.
[`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md).

### 6. Ship surface — menus, brand, visual style

General menus, logos, and a coherent visual language across studio,
player, and operator. One design system pass beats per-screen invent.

### 7. Server ↔ client productization

Auth, reconnect, schema UX, LAN/remote, packaging. Phase 8 host ship +
multiplayer/auth are the start; TLS, compression, and public exposure
harden next ([`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md),
[`HOSTING_COST.md`](HOSTING_COST.md)).

### 8. Hosting continuum *(maps to tiers)*

| Track | Tier | Use |
|-------|------|-----|
| **Official AWS** | Wild | Paid public / always-on regions; GDPR backbone |
| **Shadow mesh** | Shadow | BC-250, old consoles, home NAS — first-class, not a shame path |
| **Lab local** | Lab | Single-machine universe; default for “just play / science” |
| **Listen / friends** | Shadow | Small group without going Wild |

### 9. Stores

Steam and/or App Store: notarize (Apple pinned **2026-11-15**), depots,
store page, ratings, updates. MAS later unlock.

### 10. Security, data, compliance

Tokens → TLS; rate limits; no trusted client mass. Accounts when needed.
GDPR: privacy policy, retention, deletion path, where data lives.
Moderation / bans if public regions exist.

## Contracts between pillars *(highest leverage)*

Write these before large UI or AWS spend:

1. **Player allowlist per tier** — [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md)
2. **Studio / blueprint schema** — [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)
3. **Region capacity number** — [`REGION_CAPACITY.md`](REGION_CAPACITY.md)
4. **Identity** — anonymous / Steam / optional email (Wild needs this)  
5. **Money** — Wild = subscription; Lab/Shadow = player hardware  
6. **Shadow discovery + halo** — [`HALO_SHARD.md`](HALO_SHARD.md) (Lab
   seams first; Shadow discovery later)  
7. **Migrate Lab → Shadow → Wild** — export / quarantine / release rules  

## Suggested dig order *(when choosing)*

| Priority | Topic | Why |
|----------|--------|-----|
| A | Player allowlist | [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md) — stops world UI sprawl |
| B | Studio / blueprint contract | [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md) — studio rewrite target |
| C | Region max + cost realism | [`REGION_CAPACITY.md`](REGION_CAPACITY.md) + [`HOSTING_COST.md`](HOSTING_COST.md) |
| D | Biology depth | [`BIOLOGY_DEPTH.md`](BIOLOGY_DEPTH.md) — D1–D3 + D5 (D4 owner visual) |
| E | Brand / menus | After A–B so screens have jobs |
| F | Halo / shard handoff | [`HALO_SHARD.md`](HALO_SHARD.md) — Lab seams first; Shadow; Wild last |
| G | Wild AWS + Steam sub | After compression + allowlist + Lab solid |

## Explicit non-goals *(until unlocked)*

- Seamless Wild MMO before Lab seam smoke ([`HALO_SHARD.md`](HALO_SHARD.md) F1)  
- Autoscale official sim boxes on raw player count  
- Full editor powers for every Wild player  
- Forcing all play through subscription (Lab/Shadow remain)  
- Treating Shadow scrap hosts as the GDPR system of record  

## Related docs

- Execution: [`ROADMAP.md`](ROADMAP.md), [`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md)  
- Player gates: [`PLAYER_ALLOWLIST.md`](PLAYER_ALLOWLIST.md)  
- Studio / blueprints: [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md)  
- Region envelope: [`REGION_CAPACITY.md`](REGION_CAPACITY.md)  
- Halo / shards: [`HALO_SHARD.md`](HALO_SHARD.md)  
- Host / MP / ops: [`PHASE8_HOST_SHIP.md`](PHASE8_HOST_SHIP.md),
  [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md),
  [`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md)  
- Cost gaming: [`HOSTING_COST.md`](HOSTING_COST.md)  
