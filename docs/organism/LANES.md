# Passability lanes

*Frozen depth-lane vocabulary so plants and future animals coexist
in a side-view world without collapsing every niche to "monkey".
`passability-lanes` in the Organism Kernel plan.*

## Decision

Plants and trees are **not solid walls** in the animal movement
plane. A bone-and-muscle creature may pass in front of vegetation by
default, or choose to interact (browse, hide, climb, push through
thicket). Forced "scale every stem" is rejected — that collapses
niches to monkeys and insects and makes the side-view world
one-dimensional.

## Working default: 3 lanes

Depth into the screen, coarse:

| Lane | Job |
|------|-----|
| **Fore** | Default animal walk / run strip; drawn in front of trunks. |
| **Mid** | Plant structural lane. Trunks, canopies, crowns, epiphytes, fungi. Climb / interact happens here. |
| **Back** | Optional. Behind-trunk cover, deep thicket, ambush cover for later stages. |

`LaneId` is a `#[repr(u8)]` enum. Two lanes (Fore / Mid only) is
allowed for very early runs; three is the freeze so scenarios that
want cover can rely on Back existing.

## Rules of thumb

| Situation | Default | Opt-in interaction |
|-----------|---------|---------------------|
| Walking past a tree | Stay **Fore**, move in x; the trunk lives Mid (still shades, still exists). | Approach Mid: climb olive, rub, browse greens, break small stem. |
| Dense grass / carpet | Mostly Fore-passable; may slow (`tangle` cost). | Graze = interact with greens without leaving Fore. |
| Canopy travel | Not required. | Climb module + Mid/Back lane use (monkey / insect niche). |
| Rock / ground / water surface | Solid in the relevant plane. | — |

Collision / blocking rules of thumb:

- **Hard block:** rock, ground, water surface rules, thick fallen
  logs after topple.
- **Soft / lane-relative:** living `Stem` (olive) and `Photosystem`
  (green) block *within Mid* (climbers, epiphytes, stem integrity)
  but **do not** block Fore locomotion.
- **Interact affordance** on Mid plant cells: `Browse`, `Climb`,
  `Attach` (epiphyte, already), later `Push` / `Break`.
- Shade and light columns still use Mid greens — walking in Fore
  does not let you dodge photosynthesis physics; it only dodges
  *locomotion* jail (see [`LIGHT.md`](LIGHT.md)).

## Rendering order

Back → Mid → Fore. Same column `x` can hold a Mid stem *and* a Fore
creature without collision — the Fore creature draws over the trunk
so it reads as passing.

`wk-app` currently draws every column top-down with no lane awareness.
Phase 2 does not need lanes on screen; Phase 5 introduces plant
authoring at Mid; Phase 7 (animals) adds Fore drawn last with its own
horizontal position.

## Plant authoring rule (Set D onward)

Plants are authored with **Mid** as their default lane.

- Stems, greens, roots all live Mid. Stem occupancy is `Mid-solid`,
  not "fills the only cell".
- Epiphyte greens live Mid too; the pink `Holdfast` may layer on the
  host's Mid stem — this is the attach exception described in
  [`PLANTS.md`](PLANTS.md) and [`PALETTE.md`](PALETTE.md).
- Root cells living underground (below `surface_y`) do not need lane
  bookkeeping; they occupy the substrate cell and there is no Fore
  animal below ground yet.

The reason this rule matters *now*, before there is any animal code,
is that if plants are authored "column-solid" in Set D, the engine
learns "olive = universal solid" and gets it wrong when animals
arrive. Freezing lane occupancy in the plant authoring path prevents
that mistake.

## Fungi and lanes

Fungi are underground or in-litter and do not conflict with the Fore
walk strip. Cream hyphae inside standing-dead stems live in Mid with
their host olive — walking past a rotten trunk does not require a
climb interaction. Aboveground fungal fruiting bodies (later stage)
will be Mid.

## Animal-side interaction affordances (Phase 7 preview)

Later, an animal's `interact` verb targets a Mid pixel by column:

| Affordance | Effect |
|------------|--------|
| `Browse` | Consumes some `Photosystem` mass in that Mid cell (eats leaves). |
| `Climb` | Moves the animal from Fore into Mid (or Mid into Back), paying an energy cost. |
| `Attach` | Deposits a `Holdfast` (epiphyte, or a bird's nest later). |
| `Push` / `Break` | Applies structural damage to a Mid `Stem` (drops `integrity`). |

None of these need code in Phase 1 — they land in Phase 7. The
palette entries and lane bookkeeping just need to be forward-compatible
so we don't have to rewrite plants when the walkers arrive.

## What is deliberately not here

- Physical 2.5D depth (parallax layers, per-lane physics fields). One
  scalar `LaneId` per module cell is enough.
- Per-lane light or humidity. Fields stay column-based.
- Ambush hitboxes. Deferred.
- Rendering polish (sub-pixel Fore/Mid offsets). Later.
