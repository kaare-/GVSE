# Player world allowlist *(contract)*

**Status:** design locked as defaults (2026-10-14). Dig order A from
[`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md). Implement role/tier gates
only after this note; do not grow player UI past this table without an
owner unlock.

Tiers: **Lab** · **Shadow** · **Wild** ([`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md)).
Roles today: Viewer / Operator ([`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md)).

## Intent

Players watch and lightly touch living worlds. **Studio** is where they
build. **Operator** is where Kaare (or a world owner) tunes. Mixing those
into the player path blows cost, trust, and product clarity.

## Gears

| Gear | Wire | When |
|------|------|------|
| **Glance** | Lite status / postcard / creature card (KB; not yet shipped) | Bus check-in, AFK TV thumbnail, Wild list |
| **Presence** | `ViewFrame` Pull (compressed later; today raw RGBA) | Sitting with the world, pan/watch |

Glance is **not** a `ClientCommand` flood — it is a future read API.
Presence uses today’s Hello / Cmd / Pull.

## Roles vs tiers

| Who | Lab | Shadow | Wild |
|-----|-----|--------|------|
| **Player (Viewer)** | Local host → treat as Operator for that Lab process *or* split UI later; see note | Viewer on someone else’s shard; Operator only if they own the host token | Viewer only (sub account) |
| **World owner** | Same machine | `--ops-token` / host owner | Official ops / Kaare |
| **Operator client** | Optional | Optional on owned shard | Official fine-tune |

**Lab note:** Today in-process / `--spawn-host` is Operator-equivalent
(trusted local). Product UI may still *present* as “player in my lab”
while the process allows paint/spawn — that is Lab privilege, not Wild.

## Command matrix *(Presence)*

`✓` allow · `✗` deny · `owner` = Operator role / host owner only ·
`later` = not shipped; needs unlock + schema bump.

| Command / action | Player Wild | Player Shadow (guest) | Lab (local owner) | Operator |
|------------------|-------------|------------------------|-------------------|----------|
| Hello + auth | ✓ (sub/token) | ✓ (token) | ✓ (loopback) | ✓ |
| Glance (lite read) | ✓ later | ✓ later | ✓ later | ✓ later |
| `SetCamera` | ✓ | ✓ | ✓ | ✓ |
| `SetPresentHints` | ✓ | ✓ | ✓ | ✓ |
| `SetPaused` / `StepOnce` | ✗ Wild* | ✗ guest | ✓ | ✓ |
| `Quit` (leave session) | ✓ | ✓ | ✓ | ✓ |
| `SetSimSettings` | ✗ | ✗ | ✓ | ✓ |
| `Save` / `Load` / `ResetWorld` | ✗ | ✗ | ✓ | ✓ |
| `PaintTerrain` | ✗ | ✗ | ✓ | ✓ |
| `SpawnOrganism` (live paint-in) | ✗ | ✗ | ✓ | ✓ |
| **Release blueprint to world** | later† | later† | n/a (local) | later |
| **Emote / pin camera share** | later | later | later | — |
| Tear down host process | ✗ | ✗ | ✓ (local) | owner |

\*Wild pause would stop everyone’s weather — deny for players.  
†**Release** = validated studio blueprint → host places under strict
rules (cooldown, cap, quarantine). Not free `SpawnOrganism` / paint.
Own design note when unlocked.

## Defaults locked here

1. Wild players are **Presence viewers** (+ future Glance). No pause,
   no sim dials, no paint, no free spawn.
2. Shadow **guests** match Wild player powers; shard **owner** keeps
   Operator on that host.
3. Lab is the sandbox — full tools on the local process.
4. Studio remains the only place for total creature-design freedom.
5. Anything not in the table is **deny** until listed.

## Implementation debt *(when coding)*

Today Viewer may still `SetPaused` / `StepOnce` ([`Host::command_allowed`](../crates/wk-voxel-client/src/lib.rs)).
Tighten to this matrix:

- Add tier (or “public wild”) flag on `ServeConfig` / HelloAck  
- Wild/Shadow-guest: deny pause/step  
- Lab/Operator: unchanged  

Glance API and Release blueprint are **out of this wedge**.

## Acceptance for the tighten PR

- [x] This file linked from product roadmap + Phase 8 backlog  
- [x] Wild/guest cannot pause, paint, spawn, save, reset, set sim  
- [x] Lab / Operator still can  
- [x] Tests: role × tier matrix smoke  
- [x] Player HUD copy matches (no F5/F3 affordances in Wild player UI)

## Related

- [`PRODUCT_ROADMAP.md`](PRODUCT_ROADMAP.md) · [`HOSTING_COST.md`](HOSTING_COST.md)  
- [`STUDIO_BLUEPRINT.md`](STUDIO_BLUEPRINT.md) · Release path  
- [`PHASE8_MULTIPLAYER.md`](PHASE8_MULTIPLAYER.md) · [`PHASE8_OPERATOR.md`](PHASE8_OPERATOR.md)  
- [`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md)  
