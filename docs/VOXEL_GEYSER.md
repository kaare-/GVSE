# Geyser landscape motor

**Status:** P0–P3 + three-store humidity model (sky H / sealed
`cave_humidity` / pressurized cavity humidity (`steam` wire)). Open hot water is accelerated
evap into sky H. **P4** episodic jet still waits on the FPS gate below.
**Crate:** `wk-voxel`. App: `wk-voxel-app`.
**Goal:** native **upward** landscape builder (hot springs → geysers →
sinter pipes/hills) that balances existing **downhill** erosion, without a
vapour CA or pressure PDE.

Read first: [`VOXEL_WATER.md`](VOXEL_WATER.md), [`VOXEL_WEATHER.md`](VOXEL_WEATHER.md)
(rejects world-wide vapour), [`VOXEL_GROUNDWATER_VEINS.md`](VOXEL_GROUNDWATER_VEINS.md),
[`VOXEL_THERMAL.md`](VOXEL_THERMAL.md) (heat loop before pressurized vapour),
[`archive/VOXEL_WEATHER_SOAK.md`](archive/VOXEL_WEATHER_SOAK.md).

---

## Why this exists

The world already carves downhill: gravity, repose, flow scour, karst dissolve.
Missing is a sidescroll stand-in for tectonics/volcanoes. Geysers / hot springs
fit the budget **only if sparse**: conduits, episodic work, reuse confined rise
+ dissolved mineral ledger.

```text
erosive:  gravity / repose / scour / karst
constructive: confined_rise → (steam) → mineral_deposit → sinter hills/pipes
karst opens conduits that feed confined rise
```

---

## Locked decisions

| Topic | Decision |
|-------|----------|
| Pore ice | Freeze pore `sat` **in place**. Host stays Sand/Stone/etc. **No frost heave** in v1. Blocks seepage / throughflow / confined walk while frozen; thaw restores liquid sat; mass-flat. Sparse map (`World.pore_ice`), not `MaterialId::Ice`. |
| Cavity humidity / pressure | **Two sealed modes + sky:** (1) sky [`Humidity`] for weather + open caves; (2) sparse `World.cave_humidity` for cool sealed ambient; (3) sparse `World.steam` (wire name) for **pressurized cavity humidity** in closed / semi-closed voids. Not a separate steam-gas species. Never dumps into H/rain **from a sealed pocket**. Hot water **carries heat** along reverse seep so channels warm through colder rock; leftover **volume** (`mass × expand` while T ≥ boil) is overpressure. |
| Weather vs boiler | Not “can I see the sky.” **Weather** = vapour leaves as fast as it is made (open hot rock, wide U). **Boiler** = leftover volume packs the vessel (sealed, or fat pocket + pinprick throat — including a 100-wide cave and a 1-wide chimney). Ambiguous cuts bias weather. |
| Pore pressure transport | **Marble tube:** surplus at the hot end displaces groundwater (one in, one out). Often a cold spring; heat is a dye on that column; gas at the mouth only when the throat cannot refill. Winner-take-most. Dry open pores may take **distilled** gas (no solute). Drive is leftover volume, not minted mass; conduits stay rock (`mint_void = false`). |
| Mouth leak | Choke rate emits **mass** (never volume). Mouth T ≥ live boil → sky H like evap. Cooler → distilled liquid at the lip. **No mineral rain.** Rejected H parks as water. |
| Pressure HUD | Inspector shows `pressure=` on hot saturated rock (pore flash) and on cavity vapour seats. Overlay **P** paints the pore/cavity pressure gradient (indigo → magenta → amber). Cavity fill is the connected-pocket mean (follows the cave / wet walls); not a 3×16 candle and not a continuum PDE. |
| Pore phase motor | Liquid→gas expansion is a **force budget** (`phase_expansion_drive`, default ~96×, Tab up to 192×), not minted mass. Heat above 100 °C further scales the pulse (~×3 by +80 °C). Expand is the primary knob (not `boiled×expand`, which saturated a u8 at tiny factors); still deliberately far below Clausius 1700×. Reverse seep **advects tile heat** with the water and follows **highest permeability** (path of least resistance); vents drop Flowstone. Existing cavity humidity keeps driving pore push + heat deposit beyond the hot zone. |
| Pressure | **No continuum PDE.** Sparse underground vapour density + episodic escape (widen / burst / reverse push). Reuse confined communicating-vessel head. |
| Pressure sinter | Buried hot saturated grains (`LooseRock` / `Gravel` / `Sand`) weld under pore-flash drive so the motor is not stuck on talus: carbonate cement when load ≥ threshold, otherwise Stone weld (no carbonate invent). Hydrothermal solute inject is **grain-only** and capped. |
| Soft-lid burst | Only when the soft column opens to **free atmosphere**. Buried grain lenses under competent rock do **not** burst into Air pipes. Ejecta relocates above / outside the chamber (or fluidizes to suspended) — never dumps into the pressure void. Assault / escape / reverse-seep widen competent rock as **high-aperture conduits** (`mint_void = false`); they do not mint cheap Air columns through rock. |
| Landscape build | Mineral rides water (`mineral.rs`); cool recondense + artesian outlets drop Flowstone sinter. |
| Sky path | **Hard split.** Geyser vapour must not write sky humidity / rain lottery. Do not retain cave mass in the weather H store. |

### Hard no’s

- No world-wide vapour or pressure grids.
- No second full-world confined/pressure BFS.
- **No writing sealed-cave boil / pressure steam into the sky Humidity store.**
  Open-sky hot water may evaporate into sky H (weather). A choked boiler may
  leak **mass** at the mouth into H only after a sky path exists. Sealed ambient
  air uses `cave_humidity`. Volume (`mass × expand`) never enters H.
- No frost heave (P5) until P1–P4 are proven and someone asks.
- Do not skip the condensation lottery to “make steam.”
- Do not apply the contact dry-pore skip on the deep seepage pass.

---

## Performance gate (blocker for P3 / P4)

Quiet / FPS-biased demo is already tight; wet soaks are physics-dominated
(confined wake + humidity/wind). Cadence pattern to copy: `SEEPAGE_EVERY` (= 5).

```bash
cargo test -p wk-voxel --test perf_profile --release -- --ignored --nocapture
```

| Condition | Limit |
|-----------|--------|
| New subsystem amortized idle | ≤ **0.5 ms/tick** |
| Erupt / flash ticks | ≤ **2 ms** wall for that subsystem |
| Active vents / steam cells | Hard-capped (hundreds, not world-wide) |
| Interactive wet FPS already &lt; ~30 | **Stop at P2**; cut water/confined cost first |

**Gate check (2026-09-06, post-P2 tip):** demo wall ~14 ms/tick headless;
soak ages to 150+ ms/tick. Rock bodies were ~3.4 ms on wet demo (water-halo
body reseed). **P3/P4 stay blocked** until wet wall is cut — next levers:
skip water-dirty body seeding, then stop beach-grain solidity from waking
ortho settled rock (support-above only).

**After body-halo skip (2026-09-07):** demo ~11.6 ms/tick headless; stress
~20.6 ms. Rock bodies still ~2–4 ms on wet soaks (grain churn). Still short
of a comfortable interactive wet budget for P3.

**After Moore-face seepage clamp (stacked on ice-phase):** dry hinterland
under wet seams no longer walks the full dirty rect — only the wet-facing
row/col until pores wet. Recheck `perf_profile` / `soak_age_inventory`
before promoting P3/P4.

P1 and P2 are allowed earlier: gates/bias on existing passes, not new world fields
(beyond the sparse pore-ice map).

---

## What already exists (hooks)

| Need | Location |
|------|----------|
| Free-surface Ice/Snow | `phase.rs` — **do not** teach it to freeze rock pores |
| Sky vapour | `humidity.rs`, `rules/evap.rs` — leave alone for steam |
| Confined rise / artesian | `rules/water_flow.rs` `wake_confined_head`, `apply_confined_upward_regions`, `transmits_pressure` (already includes full pores); calls `precipitate_artesian` |
| Rate “head” overlay | `water_head.rs` — 1.0–1.4 scale; P2 stacks geothermal warmth on this |
| Mineral load / deposits | `mineral.rs` — `carry_with_water`, `precipitate_artesian`, `DEPOSIT_MATERIAL = Flowstone`, sparse `World.dissolved` |
| Deep heat | `temperature.rs` — `at_cell`, geothermal helpers |
| Mass audits | `audit.rs` `sat_totals`, `mineral_total` |
| Perf harness | `tests/perf_profile.rs`, `tick_with_perf_profiled` |
| Cell flags | Low nibble full — pore ice / steam stay **sparse maps** |

Confined contract: [`VOXEL_WATER.md`](VOXEL_WATER.md) § confined upward head —
`CONFINED_HEAD_BFS_LIMIT`, equalized store on `World.confined`, wake visits
`has_solid && has_standing_air`.

---

## Phased implementation (molded to current tree)

### P0 — Docs / mental model — **done**

- [`docs/README.md`](README.md) indexes this doc.
- Short constructive-vs-erosive note in [`VOXEL_WATER.md`](VOXEL_WATER.md).
- Sync: saturated pores already transmit confined pressure (`transmits_pressure`);
  that is not a PDE.

**Done when:** a new agent can read Water + this file and understand upward
mineral build vs downhill carve.

### P1 — Pore ice (inert seal) — **done** (`pore_ice.rs`)

- Sparse `World.pore_ice: FxHashMap<(i32,i32), u8>` — frozen sat amount; cell
  `sat` stays put (mass stays in `sat_totals`).
- Freeze/thaw from temperature (`freeze_point_c`); cadence-gated; only
  `has_wet_pores` chunks; **zero volume change**.
- Gate: `transmits_pressure`, seepage peer transfers, throughflow through a
  frozen cell.
- Free-surface `phase` Ice/Snow path **unchanged**.
- Save schema bump (v16); `#[serde(default)]`.

**Acceptance:** freeze saturated Sand/Stone → no seepage/throughflow/confined
across cell; thaw → sat restored, `sat_totals` flat; lake ice path unchanged.

### P2 — Steady hot spring — **done** (bias on existing confined path)

- Stack a geothermal warmth multiplier on confined rise rate (deeper under
  regional table via `WaterHead::geothermal_warmth`) — still no new world scan.
- Warm artesian outlets precipitate more aggressively (`precipitate_artesian_warm`).
- Keep work on confined dirty / wake sets only.

**Acceptance:** warm confined shaft with dissolved load grows Flowstone lining /
mound faster than a cold control; `mineral_total` conserved.

### P3 — Sparse buoyant steam — **done** (void markers + escape)

Boil free **Air** sat and **pore** sat at/above 100 °C **under a roof** into
sparse `World.steam`. Unroofed hot free water is **accelerated evaporation**
into sky [`Humidity`] (evap owns it — not a steam flash). Sealed ambient
cave air uses sparse `World.cave_humidity` (not pressure). Hard cap
`MAX_STEAM_CELLS`.

| Setting | Behaviour |
|---------|-----------|
| Open surface (no roof) | Accelerated film evaporization into **Humidity** — steam module skips (hotter lake, not a special path) |
| Open cave / overhang / wide U | Weather if the mouth can dump volume as fast as it is made |
| Fat pocket + pinprick chimney | **Boiler** even if a bird can fly out — leftover volume is overpressure |
| Sealed cave under rock (any RH) | Ambient film → sparse **`cave_humidity`**; cool surplus drips to Air sat |
| Cave under solid roof, ≥ boil | Flash mass 1:1; volume = mass × expand; choke leak at the mouth |
| Hot wet rock | Surplus shoves groundwater (marble tube); dry competent pores may take distilled gas |

Save schema **v18** (`cave_humidity`). Cadence `STEAM_EVERY` (= 5).

**Acceptance:** open hot free water loses sat into Humidity with `steam==0`;
sealed cool film feeds `cave_humidity` not sky H; roofed ≥100 °C water mints
steam and pressurizes; cool steam recondenses.

### P4 — Episodic geyser jet — **next** (still FPS-aware)

Per-vent charge bag + surface jet + cooldown; rate-cap; timings bucket.
Escape tubes from P3 feed this.

### P5 — Frost heave — optional, later

---

## Implementation notes for agents

1. Prefer dirty chunks / confined store / geothermal columns only.
2. Occupancy remains source of truth — freeze/thaw must not leave weep/seam
   flags lying (same class of bug as sealed weep sticky unsat).
3. Do not rustfmt the whole crate.
4. Branch prefix `cursor/`, suffix `-fdf9`; PRs via ManagePullRequest.

## Suggested ship order for this PR track

1. **P0** docs (this file + links).
2. **P1** pore ice + tests.
3. **P2** hot-spring bias + warm-vs-cold mound tests.
4. Stop. Measure `perf_profile` before scheduling P3.
