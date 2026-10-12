# wk-voxel roadmap (next ~1–2 months)

Owner-facing plan. Dates are not estimates — phases are ordered gates.
Mass budget confidence is the entry ticket for everything after Phase 0.

Companion docs: [`VOXEL_WATER.md`](VOXEL_WATER.md) (B overlay / leftover),
[`VOXEL_BUDGET_SOAK.md`](VOXEL_BUDGET_SOAK.md) (headless / Xvfb soaks),
[`VOXEL_PARALLEL.md`](VOXEL_PARALLEL.md) (threading), [`VOXEL_PERF.md`](VOXEL_PERF.md)
(Phase 1 profile baselines), [`WORLDGEN.md`](WORLDGEN.md)
(world size / streaming), organism docs under [`organism/`](organism/).

## Operating loop (already running)

- Daily **09:00 Oslo**: decision list in the Cursor agent chat (only
  blockers / forks that need Kaare).
- Prefer **headless BudgetSnap** soaks (`./scripts/budget_soak.sh`) via
  subagents; use **Xvfb** (`./scripts/xvfb_app.sh`) when eyes are needed.
- Stacked draft PRs → CI green → ready → **Enable auto-merge** on the PR
  (repo setting alone is not enough). Bases are often not `main`, so
  filter the PR list by head branch if a number “disappears”.

---

## Phase 0 — Mass budgets hold *(done 2026-10-07)*

**Goal:** Overnight / long headless soaks show TRACKED and min.tot rates
near flat with `park=0`, `clip=0`, `clamp=0`. Short soaks are necessary
but not sufficient (50k-tick soak can reopen a −3/t water term after a
flat 120-tick run).

**Done**

- Park leftover banked; karst convert credited.
- Overlay body / crush dest salvage (#348).
- Headless BudgetSnap harness + Xvfb scripts (#349).
- Sandstone/Conglomerate cement emit on crush/shatter (`write_debris_cell`).
- Wet-film snow/frost credit; lateral ice/snow relocate; no vapour-sat
  park on Ice/Snow; alpine film-park skip (#351).

**Exit (met)** — headless 50k on tip `0d7b6a6` / `#351`:

| Signal | Result |
|--------|--------|
| `\|TRACKED\|/t` | **0.03** (d=−1465) |
| min.tot `/t` | **+0.12** (d=+5827) |
| `park` | **0** |
| wall | ~66 min |

**Reconfirm (2026-10-09)** — 50k after roof-collapse Snow mint fix, tip
`7838473`: TRACKED **−0.00/t** (d=−6), min.tot **+0.07/t** (d=+3405),
`park=0`. Gate still holds with snow precip on.

---

## Phase 1 — Performance for larger worlds & long soaks *(done 2026-10-07)*

**Goal:** Tick cost stays playable as map size and soak age grow.
Buildup (dissolved maps, steam/pipe ledgers, dirty sets, landscape
bodies) must not produce the “fresh world fine → morning at 5 FPS” shape.

**Done**

- Settle sticky-loose dirty (#353); rock-body strata flood sleep (#353).
- Seam seepage full↔full skip (#354).
- Grain settle Air-destination trim (#355); stress **~37 FPS** on 2048×1064.
- Baselines + diminishing-returns notes in [`VOXEL_PERF.md`](VOXEL_PERF.md).

**Exit (met)** — owner closed Phase 1 (2026-10-07): ≥30 FPS on stress stamp;
no further CA dirty trims without weather coarsen. Stretch 5 km+ and field
shell left for later / Phase 4.

---

## Phase 2 — Geysers / hydrothermal tighten *(done 2026-10-07)*

**Goal:** Feel local and readable; less glitchy route churn.

**Done**

- Sticky conduit + local mouth seek (`LEFTOVER_PLAN_MAX_HORIZ=48`) (#357).
- Leftover lake UW / seafloor mouths (#358).
- Mild reverse-seep vertical soften (rock×32 / vent×48) (#359).
- Short budget soak near vents: `park=0`.
- Xvfb soak (~tick 1122): stable pipe straws on vent hill; UW plumes
  not clear in that pan — owner closed Phase 2 anyway (2026-10-07).

**Scope guard held:** mass-flat mouth / park / sinter. Cooling-too-fast
in sub-zero air deferred to Phase 3 (water-T).

---

## Phase 3 — Ice revisit *(done 2026-10-09)*

**Goal:** Ice is an old material with quirks. Treat as a concentrated
pass, not drive-by tweaks. **Design discussion first** — do not pre-commit
a water-T rewrite.

**Owner notes** (2026-10-07)

- Direction: toward a **brittle solid**, not powder-throughflow as the
  default look.
- Biggest suspect: **water particles don’t carry temperature**, so
  standing water on ice can’t really melt it. May need a larger rewrite.
- Related: geyser discharge cooling too fast in mostly sub-zero climates
  (same missing water-T bookkeeping).

**Design locked** (2026-10-07 owner): **option 2 — free water carries
temperature.** Sparse ledger (not `Cell` widen). Also addresses geyser
discharge cooling in sub-zero air. See [`VOXEL_WATER.md`](VOXEL_WATER.md) § Phase 3.

**Done (option 2 slices A–C)**

- Sparse `World.water_temp` + mix/clear (gravity + mouth).
- Phase film-on-ice / contact melt reads free-water T.
- Mouth stamps hot water_temp; soft cool after thermal step.

**Xvfb glance** (2026-10-08): cold alpine + ice lid / water paint —
standing film sits on the lid; after soak a wet band under the pale cap
(not instant vanish). No obvious powder cascading through solid ice.
Pipe net live nearby; no clear mouth-on-ice money shot.
Artifacts: `/opt/cursor/artifacts/phase3-ice-13-water-on-ice.png`,
`phase3-ice-15-lid-soak.png`.

**Brittle solid locked** (2026-10-08 owner)

- Mimic ocean ice: floes float on water; **thin sheets fragile**, thicker
  sections more solid / load-bearing.
- Kill powder look (Ice should not soft-pack fall / hillside powder peel
  when thick).
- With `water_temp` melt: **drop / greatly relax** Ice+Snow column hardcap
  so packs can thicken for real.

**Landed (partial-sat freeze / frozen condensate)**
- `min_sat_to_freeze` default **64**; Ice/Snow bank thaw yield on `Cell.sat`
  (legacy `sat==0` ⇒ 255). Near-full wet-air pockets under/around lids
  freeze mass-flat instead of pulsing as free Air through ice.

**Landed (lake ice lid pulse)**
- Freeze gates on `water_temp_at` (not tile alone); Ice buoyancy does not
  pop through the free-surface film **or** under-pack gaps.
- Freeze/thaw **hysteresis** (`thaw_hysteresis_c` default 0.75 °C).
- Cold film on ice **seals** into the lid; cascade will not peel lid film;
  thin hillside ice does not peel at a waterline.

**Landed (brittle solid pass)**

- Thick Ice (`ice_column_thickness ≥ ice_carry_thickness`) refuses haze
  soft-fall and hillside cold-peel; thin glaze still peels / drops.
- Ice floes on full lake + surface film over full water; rise kept.
- Default `max_ice_cells_per_column=64`, `enable_cull=false`.

**Xvfb acceptance pass** (2026-10-08, tip of pulse + lake-heat stack)

- Alpine-cold (T≈−0.8 °C) + F3 water/Ice paint; ~50 s unpaused soak.
- Stills: `/opt/cursor/artifacts/phase3-ice-accept-03-lid-fresh.png` …
  `07-soak-53s.png`, `10-final.png`.
- Agent read: phase=on under subzero; no powder cascade in stills.
  Painted lid is hard to resolve at this framing — **owner playtest**
  remains the acceptance gate (pulse / deep-lake heat / “reads as ice”).
- Glossary Phase line updated for partial-sat freeze + hysteresis.

**Landed (alpine-cold cull + lid chill)** (2026-10-08 playtest)

- `alpine-cold` preset still had `enable_cull=true` / `max_ice=12` —
  thickening lids relocated laterally (“pulling hard”). Now matches
  brittle defaults (`cull=false`, `max_ice=64`).
- Ice lids report `free_water≈0`, so buoyancy never sank skin cold into
  the column; deep lake stayed still near −5 under −30 °C air. Skin
  couple now quenches free-water tiles under the lid; buried free-water
  climate relax slightly stronger.

**Landed (ice sheet lock)** (2026-10-08 playtest)

- Under-lid freeze may not deepen more than one cell past a wet
  neighbour’s ice bottom (empty air / shore do not block). Heat quench
  retuned milder after owner “dumping heat” note.

**Xvfb sheet tip** (2026-10-08, `#370` tip)

- Stills: `/opt/cursor/artifacts/phase3-sheet-03-lid-fresh.png` …
  `09-final.png`. Agent framing again weak for lid geometry; owner
  playtest remains the gate.

**Headless 5k soak** (tip `#370`, `short_budget_soak`)

- `park=0`, mineral ~flat (`+0.06/t`). Test passed.
- TRACKED `+136/t` with UNEXPL-W/M flags still lighting — not a Phase 0
  flat gate. Isolation (2026-10-08): killed by `enable_snow_precip=false`
  (`GVSE_SOAK_OFF=snow` → TRACKED `−0.01/t`); flake H-debit itself is
  clean (`under=0`). Dual-axis H flux over-donate fixed (real, unit-tested)
  but **5k still +131/t** after that fix — not the soak mint. Soak
  `hum_adv`≈0 → not advect; diffuse / orphan / evap add=debit ruled out
  (5k OFF table in VOXEL_BUDGET_SOAK). Descent gate: `OFF=snowfall` →
  **+15.66/t** (baseline +131); `snowwet`/`slush` still +135/+140 — not
  haze swap or slush. `snowraft`/`snowsurf` still +159/+141 — not lake
  float lid or weather crest. Condensation liquid stale-`take_around`
  underpay fixed (unit mint; live clamp) but 5k still **+130.86/t**
  (`dep_add=debit=534`) — not the soak leftover. Free writers beside
  Snow (`OFF=flow` **+196**, `seep` **+145**, `park` **+133**; units
  TRACKED-flat) ruled out — not flow/seep/park. **A vs B split:**
  `OFF=phase` still **+111/t** (`paid≈Δsnow`, bare≈0) = residual **A**
  post-descent; phase ON only **~+20/t** more (**B**). Thaw units
  TRACKED-flat; baseline free↑ ≈ snow leave — thaw credits OK.
  `paid−phase`~+640k ≈ total mint, not B (coincidence with A). Residual
  **A** remeasure: `phase` vs `phase,snowfall` late triad isolates
  **~+91/t** landed-pack; `phase,cond` still **+93**, `phase,steam`
  **+102**, `phase,gravity` **+109** — not lottery/steam/gravity.
  `free_other+evap≈+80k` ≪ free_in — mint not in scoped World::set_cell
  Air writers. Chunk miss probes: `par_air≈0`, `grav_air` sink only.
  `OFF=settle` → **+43/t** (cuts ~⅔ of A); `TRACKED≈swap_snow−paid`.
  Call-site: fall/drift/nucleate paired; **failure** other net ≈ TRACKED.
  **Fixed:** Ice/Snow excluded from roof collapse (thaw-sat dump mint).
  5k after: baseline **−0.01/t**, `phase` **−0.01**, `phase,settle`
  **−0.01**, `phase,snowfall` **−0.04**. Residual A closed (full soak
  flat). Kill: `OFF=snow`.
  **50k Phase 0 gate** tip `7838473`: TRACKED **−0.00/t**, min
  **+0.07/t**, `park=0` — rates still under 0.5/t with snow on.

**Exit (met)** — owner closed Phase 3 (2026-10-09): move to Phase 4
re-profile + larger worlds. Ice model (option 2 A–C + brittle + sheet
lock + lid chill) + TRACKED flat after roof-collapse Snow fix are the
hard gates; remaining Xvfb framing weakness is not a Phase-3 blocker.

**Merge hygiene (done 2026-10-11)** — catch-up `#375` landed `#366`–`#373`
on `park-bank`; `#374` Phase 4 tip merged after.

---

## Phase 4 — Optimisation again + larger worlds *(closed)*

**Goal:** After ice/geyser churn, re-profile and push map scale.
May include ring width / ceiling / streaming follow-ups from
[`WORLDGEN.md`](WORLDGEN.md).

**Entry**

1. Close Phase 3 in this doc (done 2026-10-09).
2. Re-profile tip after Phase 3 ice/heat/sheet + mint stack on the same
   harness as Phase 1 (`perf_profile_*`, warm 40 / measure 200) — land
   numbers in [`VOXEL_PERF.md`](VOXEL_PERF.md) § Phase 4 baseline.
3. Compare hotspots to Phase 1 settle2 table (seepage / bodies / advect).
   Prefer **width stretch** before another CA dirty trim unless a clear
   ≥1 ms surgical win appears.

**Acceptance (met)**

- Phase 4 baseline table landed (short sky / demo / stress, 0 plants).
- Scale lever: stretch **5.12 km @ ~1064 h** under stream+evict
  (view=320) at **~74** sim-FPS after temp+rain gates (`#399`–`#400`).
- Compact stretch stream+evict **~115** sim-FPS; full-resident Compact
  gate stays ~19 (owner-accepted).
- Streaming residency + organism park + disk/F5/prune closed (`#388`–
  `#395`); body/seep stream gates (`#397`–`#398`).

**Out of scope (still deferred)**

- Phase 5 organism rewrite.
- Enabling rayon by default.
- Weather coarsen / condensation lottery skip as a first lever.
- Optional 5 km leftover trims (wind.rebuild ~1.5 ms, rock bodies
  ~3.7 ms) — not required for the ≥30 gate.

**Landed (summary)**

- Owner accepted Compact ~19 FPS full-resident; scale via streaming.
- Streaming stack: CA window → field shell → steam/leftover →
  `ChunkStore` + remint → side maps → `FrozenBacklog` → mycelium/spore
  → organism park by body span → disk / F5 / prune.
- Compact stretch stream+evict ~19 → **~67** → (body/seep) **~91** →
  (temp/rain) **~115** sim-FPS.
- 5 km stream+evict: residency chunks **459** / store **4981**; wall
  **~74** sim-FPS (≥30 **pass**).

**Exit (met)** — decision-list default (2026-10-11): park Phase 4 /
open Phase 5 planning. Width + streaming goals closed; leftover FPS
trims are optional follow-ups, not Phase-4 blockers.

---

## Phase 5 — Biological system upgrade *(E2 agent-parked 2026-10-12)*

**Goal:** Big organism/ecology pass. Lots of **fundamental changes** to
the system we have — not a thin skin.

Headless soaks for this phase turn **organisms on** (Phase 0 gate stays
organisms-off).

**Wedge Set E2** — see [`organism/PHASE5_SCOPE.md`](organism/PHASE5_SCOPE.md).
Slices 1–7 + organisms-on soak/park gates landed (TRACKED −0.01/t,
park=0). Owner visual for tipped grey logs remains open (not a code
blocker). Daily 09:00 Oslo lists continue.

---

## Phase 6 — Optimise the biological loop *(in progress)*

**Goal:** After the E2 upgrade lands, cost and buildup of the life pass
under long soak / larger maps. See
[`organism/PHASE6_SCOPE.md`](organism/PHASE6_SCOPE.md).

**Slice 1 landed (2026-10-12):** Compact organisms-on baseline —
256 plants ≈ **1.3 ms** org / **55 ms** wall (~2.4%); physics-bound.
Inside org: land/metab hottest (ring-wide seed).

**Slice 2 landed (2026-10-12):** Compact stream+evict + 256 plants in
focus resident band — wall **53 → 12.8** ms; org **~0.6** ms both
modes (living=256, parked=0). Stream org splits: post/pose lead.

**Slice 3 landed (2026-10-12):** Hotspot cut #1 — skip plant-only
contacts (+ epi/soft-pile gates). Stream org **0.59 → 0.29** ms
(post ≈0); Compact 256 **1.30 → 0.83** ms; soak park=0.

**Slice 4 deferred (2026-10-12):** land/metab tip×count / stem cadence
regressed river growth unit test; reverted.

**Slice 5 landed (2026-10-12):** PreferentialRootPath soft-cap (4096) +
rock stale drop; soak park=0 / preferential=0.

**Slice 6 landed (2026-10-12):** Soft litter + root_residue soft-caps
(4096) + stale Organic prune.

**Agent park (2026-10-12):** Phase 6 measured slices closed for agent
(baseline, stream+plants, hotspot #1, overlay GC; #2 deferred). Optional
5k organisms soak evidence; default roadmap next Phase 7 or E2 visual.

---

## Phase 7 — Surface spice (atmosphere & light) *(closed for agent)*

**Goal:** Hopefully simple but important presentation:

- Colors; sunrise / sunset
- Moon + moon cycle
- Seasons
- Stars that move over time and seasons
- Shadows / shading from sundown
- Moonlight glitter on water

**Exit criteria:** visual acceptance on Xvfb/playtest;
no sim-mass regressions from lighting-only work.

**Closed (2026-10-12):** wedges A–D landed ([`PHASE7_SCOPE.md`](PHASE7_SCOPE.md))
with Xvfb + 3k soak park=0 each. Optional E2 visual remains owner-only.
Default roadmap next: Phase 8 scoping, or owner pick.

---

## Phase 8 — Server / client *(scope draft)*

**Goal:** Split sim presentation from ownership. Review alternatives;
prefer a **production-compatible client** that can pass **Apple and/or
Steam** inspection. Heavy work and testing; own architecture note
before coding.

**Scope (2026-10-12):** [`PHASE8_SCOPE.md`](PHASE8_SCOPE.md) —
ownership split. **Stack pick:** ship = **A egui+wgpu/winit**; soak/dev
= **macroquad** ([`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md)).
**Contract:** [`PHASE8_CONTRACT.md`](PHASE8_CONTRACT.md) (`ViewFrame` +
`ClientCommand`). **Viewer client:** `wk-voxel-client` (egui+wgpu; strip + sky + palette;
host `step_world`, `VIEW_SCHEMA` 4). **Owner:** client = viewer / camera /
limited commands; host = sim. **Next:** Compact host, packaging, or
out-of-process. Soak stays macroquad.

---

## Decision log

| When | Topic | Decision |
|------|--------|----------|
| 2026-10-07 | Phase 0 rate eps | Tight: `\|rate W\|` and `\|rate M\|` &lt; **0.5/t** on 50k+ headless |
| 2026-10-07 | Phase 0 organisms | **Off** until Phase 5 bio rewrite |
| 2026-10-07 | Phase 0 closed | 50k headless: TRACKED **0.03/t**, min **+0.12/t**, park=0 (#351) |
| 2026-10-09 | Phase 0 reconfirm | 50k after roof-collapse Snow fix (`7838473`): TRACKED **−0.00/t**, min **+0.07/t**, park=0 |
| 2026-10-07 | Phase 1 FPS / size | ≥**30 FPS**; grow size until diminishing returns; stretch **5 km+ @ ~1064 h** |
| 2026-10-07 | Phase 1 stress gate | Met after settle Air-dest: stress **~37 FPS** (27.3 ms); no clear ≥1 ms CA win left — discuss seepage wakes / bodies / fields ([`VOXEL_PERF.md`](VOXEL_PERF.md)) |
| 2026-10-07 | Phase 1 closed | Owner: close Phase 1; move to Phase 2 geysers (stretch width / leftover CA deferred) |
| 2026-10-07 | Phase 2 code | Sticky/local (#357), UW mouths (#358), mild vert (#359) |
| 2026-10-08 | Phase 3 brittle | Ocean-like float; thin fragile / thick solid; relax column hardcap (water_temp melt) |
| 2026-10-08 | Phase 3 Xvfb | Ice lid + water: film/wet band holds; no powder thrash in stills |
| 2026-10-07 | Phase 3 ice option | **2 — water carries T** (sparse free-water ledger; melt + geyser cool) |
| 2026-10-09 | Phase 3 closed | Owner: move to Phase 4; ice + TRACKED flat after roof Snow fix |
| 2026-10-09 | Phase 4 start | Re-profile post-ice tip; then width stretch / ≥1 ms lever ([`VOXEL_PERF.md`](VOXEL_PERF.md)) |
| 2026-10-09 | Phase 4 seam wake | Wet seam lowers + skip full↔full band; Compact wall **62→55** ms (~18 FPS) |
| 2026-10-11 | Compact FPS gate | Accept **~19 sim-FPS** full-resident Compact; scale via streaming |
| 2026-10-11 | Stream active CA | Camera window filters `plan_active`; RAM still full-resident |
| 2026-10-11 | Stream field shell | Phase columns + humidity advect skip outside window |
| 2026-10-11 | Stream temp/wind/evap | Window temp step, wind seats, evap chunks; Compact ~43 sim-FPS |
| 2026-10-11 | Stream steam/leftover | Steam cadence + leftover cands windowed; Compact ~45 sim-FPS |
| 2026-10-11 | ChunkStore (opt-in) | In-memory postcard eviction API; play sync not wired; FrozenBacklog open |
| 2026-10-11 | Stream remint gate | Far set_cell skips blank mint; sync_residency in play; FrozenBacklog open |
| 2026-10-11 | Stream side maps | Park steam/dissolved/pipe/litter with ChunkStore; FrozenBacklog open |
| 2026-10-11 | FrozenBacklog water | Missing-dest flow books sat; thaw on reload; sat_totals counts it |
| 2026-10-11 | Stream+evict A/B | Compact stretch wall 52→15 ms (~67 sim-FPS); chunks 1088→459 |
| 2026-10-11 | FrozenBacklog loads | Suspended + dissolved ride absorbed water; audits count them |
| 2026-10-11 | Stream myc park | Mycelium strains/energy/lineage stamps park with SideMapStore |
| 2026-10-11 | Stream spore park | SporeBank cells park with SideMapStore under sync_residency |
| 2026-10-11 | Stream residency close | CA→evict→backlog→side/myc/spore parked; ~67 sim-FPS; organism ECS open |
| 2026-10-11 | Stream org park | Living atoms park by anchor gx; pop caps include parked; corpses open |
| 2026-10-11 | Stream corpse park | Corpses park by anchor gx with atoms; corpse_total fills cap |
| 2026-10-11 | Stream body span | Park when all body cols outside resident; restore on any tip |
| 2026-10-11 | ChunkStore disk | Opt-in directory backend (`enable_disk`); play wire still open |
| 2026-10-11 | Stream disk play | Tab toggle; `saves/stream_chunks/{seed}` when streaming |
| 2026-10-11 | F5 stream flush | to_snapshot restores ChunkStore/side/organism parks first |
| 2026-10-11 | Stream chunk prune | Drop other seed dirs under saves/stream_chunks on reseed |
| 2026-10-11 | Stream stack closed | Residency+disk+F5+prune done; next = re-profile / ≥1 ms / width |
| 2026-10-11 | Body stream gate | Competent float wake + wake regions honor active StreamWindow |
| 2026-10-11 | Seep stream gate | Lake-bed/seam/weep wakes + seam couple honor StreamWindow |
| 2026-10-11 | 5 km stream probe | 20480×1064 stream+evict ~16 sim-FPS (fail ≥30); temp 32 + rain 21 ms |
| 2026-10-11 | Temp+rain stream | 5 km ~16→**~74** sim-FPS (pass ≥30); Compact stream ~91→**~115** |
| 2026-10-11 | Phase 4 closed | Width+streaming goals met; park Phase 4 / open Phase 5 planning |
| 2026-10-11 | Phase 5 scope draft | [`organism/PHASE5_SCOPE.md`](organism/PHASE5_SCOPE.md) decision picks (no code) |
| 2026-10-12 | Phase 5 E2 wedge | Accept Set E2; stem integrity + rot topple on woody corpses |
| 2026-10-12 | E2 live integrity | Living stems recharge + load tip; completes PLANTS.md integrity table |
| 2026-10-12 | E2 Holdfast scaffold | Holdfast Mid-lane share + attach genes + minimal_epiphyte |
| 2026-10-12 | E2 epi seating | Habitat spawn on host Stem; unseated death |
| 2026-10-12 | E2 HostLeave | Same-column epiphyte smother shade via host_leave |
| 2026-10-12 | E2 attach_prefer | Sticky Holdfast reseat on nearby Stem |
| 2026-10-12 | E2 ghost-root | Organic residue→Void→Loose + preferential re-root (E44) |
| 2026-10-12 | E2 org soak gate | 600t / 48 plants: TRACKED −0.01/t, park=0, living=50 |
| 2026-10-12 | E2 agent-parked | Slices 1–7 + soak/park gates done; Phase 6 default next |
| 2026-10-12 | Phase 6 stream+plants | Compact A/B: wall 53→12.8 ms; org ~0.6 ms live-in-view; next = hotspot cut |
| 2026-10-12 | Phase 6 hotspot #1 | Plant-only contact skip; stream org 0.59→0.29; Compact 1.30→0.83; soak park=0 |
| 2026-10-12 | Phase 6 hotspot #2 | Deferred: tip×count/stem cadence regress river growth; next = buildup/GC |
| 2026-10-12 | Phase 7 scope draft | Celestial polish (A) default; owner pick |
| 2026-10-12 | Phase 7 A code | Dawn/dusk palette + moon cycle crescent + drifting stars |
| 2026-10-12 | Phase 7 A Xvfb | Dusk amber (~315), midnight crescent+stars (~626), dawn rose (~887) |
| 2026-10-12 | Phase 7 A closed | 3k soak park=0 TRACKED −0.02/t; next default wedge B |
| 2026-10-12 | Phase 7 B locked | Sundown cast / terrain lee (agent default) |
| 2026-10-12 | Phase 7 B closed | Low-sun cast + terrain lee + warm key; 3k soak park=0; next C |
| 2026-10-12 | Phase 7 C locked | Season dial (sky-only day length + tint) |
| 2026-10-12 | Phase 7 C closed | Summer/winter Xvfb + 3k soak park=0; next D |
| 2026-10-12 | Phase 7 D locked | Water glitter on lit free-water tops |
| 2026-10-12 | Phase 7 closed | A–D done; 3k soak park=0; next Phase 8 or owner pick |
| 2026-10-12 | Phase 8 scope draft | Host/client split + stack shortlist; owner pick before code |
| 2026-10-12 | Phase 8 stack compare | A–D written ([`PHASE8_CLIENT_STACKS.md`](PHASE8_CLIENT_STACKS.md)); agent lean A; owner pick |
| 2026-10-12 | Phase 8 stack locked | Ship **A** egui+wgpu; macroquad = soak/dev; next = contract sketch |
| 2026-10-12 | Phase 8 contract sketch | ViewFrame ≠ SimSnapshot; ClientCommand queue; next = A spike |
| 2026-10-12 | Phase 8 A spike | `wk-voxel-client` egui+wgpu empty window + Host tick; next = strip/sky |
| 2026-10-12 | Phase 8 terrain strip | ViewFrame v2 RGBA strip + stamped world tick; next = sky/palette |
| 2026-10-12 | Phase 8 sky present | ViewFrame v3 sky fields + air fill + sun/moon marker; next = palette |
| 2026-10-12 | Phase 8 palette/glitter | App-aligned cell colours + water glitter on A client |
| 2026-10-12 | Phase 8 A parked | Present path landed #425–#430; owner pick next wedge |
| 2026-10-12 | Phase 8 viewer+host | Client = viewer; host `step_world`; VIEW_SCHEMA 4 |
| 2026-10-09 | Phase 4 bodies | Climatic probe: Compact bodies ~9 ms = real falls; streaming next vs CA |
| 2026-10-07 | Phase 2 closed | Owner: close Phase 2; Phase 3 ice (design discussion first) |
| 2026-10-07 | Phase 2 Xvfb | Stable pipe straws on vent hill @tick~1122; UW plumes not clear in pan — owner close vs re-pan |
| 2026-10-07 | Geyser locality | Local pressure/pathfinding (+ hard seek limits); tune in play |
| 2026-10-07 | UW / ocean vents | Lake underwater springs **and** seafloor ocean-column vents |
| 2026-10-07 | Ice direction | Toward **brittle solid**; water-T rewrite likely; discuss at Phase 3 |
| 2026-10-07 | Phase 5 bio | Fundamental look + fundamental changes; own major planning session |
| 2026-10-07 | Phase 8 client | Review alts; production path for Apple/Steam inspection |

## Clarifications later

- Phase-1 stretch width / field-shell cuts: **closed with Phase 4** (5 km stream-on).
- Phase 3 ice option 2 slices A–C + brittle/sheet/lid: closed with Phase 3.
- Optional Phase-4 leftovers (5 km wind.rebuild / rock bodies): only if owner wants more FPS.
- Phase-5 E2 owner visual: tipped grey logs at 1× zoom
  ([`organism/PHASE5_SCOPE.md`](organism/PHASE5_SCOPE.md)).
- Phase-8 shortlist: see [`PHASE8_SCOPE.md`](PHASE8_SCOPE.md); owner pick.
