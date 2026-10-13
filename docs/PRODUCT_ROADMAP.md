# Product roadmap — pillars *(basis)*

**Status:** locked as planning basis (2026-10-13). Not a sprint backlog —
the long-horizon map so biology, client, studio, sharding, and ship work
do not thrash each other. Phase code backlogs
([`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md), [`ROADMAP.md`](ROADMAP.md))
stay the agent execution lists.

## Doctrine

| Rule | Meaning |
|------|---------|
| Host owns truth | Sim / mass / organisms only on world server |
| Player world allowlist | Live-server interactions are **few and explicit** |
| Studio = total freedom | Creature / blueprint design offline or in private space |
| Operator ≠ player | Fine-tune live worlds; full tools + settings |
| Region has a max | One instance = one capacity envelope; grow map by **sharding** |
| Soak stays macroquad | Park/TRACKED gates do not move to the ship client |

## Pillars

```text
┌─────────────────┐     allowlist cmds      ┌──────────────────┐
│ Design studio   │ ──────────────────────► │ World server(s)  │
│ (total freedom) │   validated blueprints  │ region = max size│
└─────────────────┘                         │ shard + halo later│
                                            └────────┬─────────┘
┌─────────────────┐     limited interact     │
│ Player client   │ ◄── ViewFrame / auth ────┤
│ (viewer+)       │                          │
└─────────────────┘                          │
┌─────────────────┐     full ops tools       │
│ Operator client │ ◄────────────────────────┘
│ (fine-tune)     │
└─────────────────┘

Ship surface: menus · brand · visual language · Steam / App Store
Ops / legal:  hosting · security · GDPR · accounts · support
```

### 1. Biology / sim depth

Continue ecology, organisms, climate, mass integrity. Park=0 / TRACKED
soaks remain the gate. Independent of ship UI polish.

### 2. Player client + world allowlist

Ship client is a **viewer** with a short, written allowlist of live
interactions (camera, present, maybe seed/drop later — **not** full
paint). Everything else is denied by host role policy.

**Contract owed:** one-page player allowlist before more world UI.

### 3. Design studio (creature space)

Full freedom to design creatures / blueprints. Sketch studio exists but
is **terribly out of date** — needs a rewrite or replacement, not a
paint-over.

**Contract owed:** studio ↔ server blueprint schema + versioning +
reject path on live spawn.

### 4. World server — load + regions

- One **region** = one sim instance with a **max size / load envelope**
  (today’s planning unit ≈ Compact 4096×tropopause + stream/park).
- To grow the **world**, **shard** (more regions) and join at edges
  with **halo + handoff** (design later; see
  [`HOSTING_COST.md`](HOSTING_COST.md)).
- More players in one place ≠ clone the sim; tighten interest / present
  first, then bigger box, then split the hot region.

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

### 8. Hosting continuum

| Track | Use |
|-------|-----|
| **Official** | AWS (or similar) for public / paid regions |
| **Scrap / homelab** | BC-250, old consoles, mining→game rigs — fine for **dev / community BYO host**, not the GDPR/Steam backbone unless productized |
| **Listen / friends** | Owner Mac / home host for small groups |

### 9. Stores

Steam and/or App Store: notarize (Apple pinned **2026-11-15**), depots,
store page, ratings, updates. MAS later unlock.

### 10. Security, data, compliance

Tokens → TLS; rate limits; no trusted client mass. Accounts when needed.
GDPR: privacy policy, retention, deletion path, where data lives.
Moderation / bans if public regions exist.

## Contracts between pillars *(highest leverage)*

Write these before large UI or AWS spend:

1. **Player world allowlist** — exact live cmds  
2. **Studio / blueprint schema** — version + validate  
3. **Region capacity number** — cols × height × target FPS × max peers  
4. **Identity** — anonymous / Steam / optional email  
5. **Money + who hosts** — who pays for a public Compact shard  

## Suggested dig order *(when choosing)*

| Priority | Topic | Why |
|----------|--------|-----|
| A | Player allowlist | Stops world UI sprawl |
| B | Studio / blueprint contract | Studio rewrite needs a target |
| C | Region max + cost realism | [`HOSTING_COST.md`](HOSTING_COST.md) |
| D | Biology depth | Parallel; soak-gated |
| E | Brand / menus | After A–B so screens have jobs |
| F | Halo / shard handoff | After region max locked |
| G | Official AWS + Steam | After compression + allowlist |

## Explicit non-goals *(until unlocked)*

- Seamless MMO before halo design note  
- Autoscale sim boxes on raw player count  
- Full editor powers for every live player  
- Treating scrap hardware as the compliance host  

## Related docs

- Execution: [`ROADMAP.md`](ROADMAP.md), [`PHASE8_BACKLOG.md`](PHASE8_BACKLOG.md)  
- Host / MP / ops: [`PHASE8_HOST_SHIP.md`](PHASE8_HOST_SHIP.md),
  [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md),
  [`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md)  
- Cost gaming: [`HOSTING_COST.md`](HOSTING_COST.md)  
