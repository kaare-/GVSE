# Archived weather leftover soak ledger

Cut-by-cut leftover from the climate-budget track. **Not current
product behaviour** — the live leftover and pins live in
[`VOXEL_WEATHER.md`](../VOXEL_WEATHER.md).

Soak shape unless a paragraph says otherwise: demo 1024×320, or
`soak_age_inventory` (256 plants, climatic-rain injector off, 8×400).
Do not treat older wall numbers as the present budget.

  Fresh-stamp demo (1024×320, 40 warm + 200 measure, 2026-08-27): wall
  **32.9 ms/tick**. Physics is 27.7 of that. Top three: confined wake
  7.7, seepage 7.4, rock bodies 6.3. Snow drift 0.001, suspension 0.09.
  The older 3 ms/tick figure is an *aged quiet* world, not this soak.

  After the leftover-cost cut (this host, 40 warm + 200 measure, 0 plants):
  short sky wall **35.2 ms/tick**, humidity.advect **3.4** + wind.rebuild **2.3**
  (was ~6.8 miss-path walks with an empty field). Tall 1064 wall **28.3**,
  humidity.advect **2.5** + rebuild **1.9**, seepage **5.5**, bodies apply
  **6.8**. Dry-halo / empty-sky skips do not change the wet-crust apply.

  After the wet-apply cut (wake tiles stay local, pond-interior seepage
  skip, FPS topology on interactive): short wall **31.7**, seepage **6.9**,
  bodies **4.6**. Tall 1064 wall **24.5**, seepage **5.1**, bodies **3.3**.

  After the confined / lake-bed occupancy skip (standing water next to
  rock only; rain-film sky and groundwater-only crust dropped): short
  wall **32.3**, seepage **6.9**, confined wake **2.2**. Tall 1064 wall
  **23.1**, physics **12.6**, seepage **5.0**, confined wake **2.1**,
  bodies **3.3**. The leftover confined ~2.1 ms is the real ocean/lake
  communicating-vessel walk, not drizzle columns.

  After the wet-crust seepage skip (perimeter-only weep on buried
  crust, lake-bed skips a full water table, both-full pore faces skip
  head math): short wall **31.0**, seepage **5.3**. Tall 1064 wall
  **22.2**, physics **11.1**, seepage **3.5**, confined wake **2.2**,
  bodies **3.3**. Split probe on demo: lake-bed 1.6, weep 1.6, deep
  1.7, seam couple 1.5 (was 2.0 / 2.4 / 3.3 / 3.4).

  After mid-ocean lake-bed peek, rain-sky evap skip, and uncased
  confined reject: short wall **30.6**. Tall 1064 wall **21.4**,
  evap→humidity **1.6**, seepage **3.5**, confined wake **2.2**.
  Lake-bed split 1.5 (was 1.6). Confined ~2.2 is still the ocean/lake
  communicating-vessel walk.

  After the humidity mix/lift clone skip and wind Jacobi ping-pong
  (this host, 40 warm + 200 measure, 0 plants): short wall **28.3**,
  humidity.advect **1.8** (was 3.4), wind.rebuild **2.4**. Tall 1064
  wall **20.1**, humidity.advect **1.3** (was 2.4), wind.rebuild
  **1.8**, evap→humidity **1.4**, seepage **3.5**, confined wake
  **2.1**. Flux / lift / mix math is unchanged. Confined leftover is
  still the ocean/lake communicating-vessel walk.

  After the confined standing-air y-band (plus one row for the rising
  film): short wall **28.1**, confined wake **1.5** (was 2.2). Tall
  1064 wall **20.0**, confined wake **1.4** (was 2.1), humidity.advect
  **1.3**, wind.rebuild **1.8**. Dry sky in shore chunks is skipped;
  wells / ocean equalise unchanged. Wind rebuild leftover is
  compose + project on occupied seats.

  After lake-bed standing-only y-band (dry sky skipped; unsat fronts
  keep the full rect): split probe lake-bed **1.3** (was 1.5). Sky-height
  seepage bucket stays **~3.6** (apply + weep + seam dominate). Tall
  1064 wall **20.7**, confined **1.4**, humidity.advect **1.3**,
  wind.rebuild **1.8**.

  After soak-age occupancy (clay / soluble / standing-air gates, plus
  condensation mass-before-floor): short wall **27.1**, tall 1064 wall
  **19.4**, humidity.advect **1.3**, wind.rebuild **1.9**, seepage
  **3.5**, confined **1.4**, condensation **0.48**, flow erosion
  **1.0**, suspension **0.06**, karst **0.04**. Fresh-stamp leftover
  is unchanged — these cuts bite as drizzle soaks land and humidity
  fills. `soak_age_inventory` (256 plants, climatic rain off, 8×400):
  wall **22.6 → 86.4**. `clay` stays **23**, `stand` stays **~28**
  (gates hold). Growers that are real work, not occupancy leaks:
  `hum n` **13k → 55k / 68k** (condensation **0.67 → 5.1**), dirty
  halo **7k → 33k**, `diss` **1.1k → 10.5k**, plant `mods` **2.1k →
  3.9k**, `susp` **6 → 286**. `loose` **34 → 74** and `buoy` **0 → 48**
  track litter, not a sticky-flag leak. Confined **2.6 → 9.1** then
  **6.8** with a flat stand count is leftover communicating-vessel
  work, not rain-wet land.

  After lottery-before-floor, dissolved-key snapshots, and confined
  chunk-local neighbour reads (this host, 40 warm + 200 measure, 0
  plants): short wall **26.6**, tall 1064 wall **19.5**, condensation
  **0.48**, seepage **3.6**, confined **1.5**, flow erosion **0.89**.
  Fresh-stamp is unchanged — these skips are leftover on filled /
  karst-aged worlds. Repeat soak (256 plants, 8×400): wall **22.9 →
  88.7**. Condensation **0.67 → 5.1** still tracks `hum n` **13k →
  55k** (over-sat tiles must lottery). Confined **2.7 → 8.3** then
  **7.0** is still equalized-shaft BFS, not neighbour HashMap probes.
  `clay` stays **23**. Do not skip the lottery; do not starve the
  confined wake.

  After rock-only confined casing (this host): fresh-stamp short wall
  **26.7**, tall **19.1**, confined **1.3 / 1.2**. Soak (256 plants,
  8×400): wall **23.2 → 90.2**. Confined **2.2 → 8.1** then **6.3**
  — same leftover as before. Plants / grains as walls was correct
  physics (films must not rise) but not the soak BFS. `clay` stays
  **23**.

  After skipping uncased BFS at the world’s highest standing-air row
  (this host, 40 warm + 200 measure, 0 plants): short wall **26.3**,
  tall **19.1**, confined **1.3 / 1.2**. Repeat soak: wall **23.2 →
  88.3**. Confined **2.2 → 8.2** then **6.3**. A high tarn keeps
  `max_stand` above the ocean, so coastal films still walk. Do not
  skip uncased higher-row rise — `confined_head_equalizes_across_large_deep_ocean`
  stalls (shaft top 36 vs sea 40). The leftover is equalized
  rock-cased shafts / connected ocean BFS below a higher standing
  row. Do not starve the wake; do not lower `CONFINED_HEAD_BFS_LIMIT`.

  After FxHash + reused BFS buffers (this host, 40 warm + 200 measure,
  0 plants): short wall **25.2**, tall **18.2**, confined **0.62 / 0.57**
  (was **1.3 / 1.2**). Soak (256 plants, 8×400): wall **21.8 → 86.3**.
  Confined **1.1 → 3.8** then **3.0** (was **2.2 → 8.2** then **6.3**).
  Same cells, same rise — SipHash and per-call alloc were leftover on
  the equalized walk. `clay` stays **23**. The remaining confined grower
  is still that walk, just cheaper. Do not starve the wake.

  After snapshotting humidity advect as a `Vec` (this host, 40 warm +
  200 measure, 0 plants): short wall **25.7**, tall **18.2**,
  humidity.advect **1.74 / 1.29** (same as the mix/lift skip). Repeat
  soak (256 plants, 8×400): wall **21.7 → 85.2**. Condensation
  **0.63 → 4.75** still tracks `hum n` **13k → 55k / 68k** (over-sat
  tiles must lottery). Flux math is unchanged — the SipHash `clone`
  was leftover on the snapshot, not the soak grower. `clay` stays
  **23**. Do not skip the lottery.

  After FxHash dissolved-load indexes and condensation `(key, mass)`
  snapshots (this host, 40 warm + 200 measure, 0 plants): short wall
  **25.4**, tall **18.1**, condensation **0.30 / 0.26** (was **0.49 /
  0.44** — leftover SipHash `at_tile` on the lottery walk). Repeat
  soak (256 plants, 8×400): wall **21.2 → 80.7** (was **21.7 →
  85.2**). Condensation **0.33 → 3.21** (was **0.63 → 4.75**). Flow
  **0.66 → 2.78** (was **0.74 → 3.82**). `hum n` still **13k → 55k**,
  `diss` still **1.1k → 10.3k**, `clay` stays **23**. Same cells, same
  mass — SipHash on load-index `contains` and a second humidity-map
  get were leftover as karst / sky fill. Lottery still walks every
  over-sat tile. Do not skip it.

  After skipping leftover flow/gravity on dry inland chunks (Moore
  neighbour still scanned so dry dest cells keep the +x equalise
  edge) and hashing temperature scratch / diffuse with FxHash (this
  host, 40 warm + 200 measure, 0 plants): short wall **24.4**, tall
  **17.5**. Temperature.step **4.5 / 10.8 ms/call** (was **6.6 /
  18.5** — leftover SipHash clone + sort). Repeat soak (256 plants,
  8×400): wall **20.3 → 80.5**. Flow **0.67 → 2.67**, gravity
  **0.52 → 2.26** — same leftover as before. Plants sit on rain-wet
  land, so the dry-inland skip does not move soak. `clay` stays
  **23**. Do not skip dry flow cells that own the +x equalise edge.

  After solving wind Jacobi pressure in `Vec`s (this host, 40 warm +
  200 measure, 0 plants): short wall **25.0**, tall **17.5**,
  wind.rebuild **2.42 / 1.86** (same as **2.32 / 1.87**). Repeat soak
  (256 plants, 8×400): wall **20.6 → 82.7**. Same six iterations,
  same slip — HashMap clone + per-iter insert were leftover hasher,
  not the compose / `face_blocked` cost. `clay` stays **23**. Do not
  retry the Jacobi solid-cache.

  After skipping unchanged far-sky temperature writes and running
  diffuse on a dense slab when the box is full: leftover hasher on
  1000-cell columns already on the lapse. Same couple / skip / pair
  stencil — tiles that did not move are not rewritten. Not view LOD;
  the ring still steps at full rate. Do not coarsen off-screen sim.

  After packing that slab once per temperature step (couple writes
  it; diffuse and row-means reuse it) and skipping far-sky / deep-crust
  `live_surface_at` on the props refresh (one seed-rock walk per
  column, same Air / Buried early-out; this host, 40 warm + 200
  measure, 0 plants): short wall **25.0**, tall **16.9**.
  Temperature.step **3.46 / 6.74 ms/call** (was **4.5 / 10.8** —
  leftover per-tile rock walk on Air already classified, plus a second
  HashMap pack for diffuse). Same couple / skip / pair stencil.
  Surface-band tiles still scan. Repeat soak (256 plants, 8×400):
  wall **20.1 → 80.7** — same leftover as before. Temperature is a
  period-20 hitch, not the soak grower. `Temperature::cells` stays
  `HashMap` for serde. Do not coarsen off-screen sim.

  After caching orographic `live_surface_at` per column (same factors
  for every over-sat tile in the column), sharing the lottery sat
  `at_tile` with the dew read, and draining winners from the snapshot
  mass (repeat soak, 256 plants, 8×400): condensation **0.35 → 2.80**
  (was **0.35 → 3.18**). Wall **20.7 → 83.3** — same leftover growers
  (halo, seep, confined). Lottery still visits every over-sat tile;
  `col_lo` still built for every key. Draw leftover: `T` / `U` / `M` /
  `G` now use the camera box (headless soak does not draw). Do not
  skip the lottery. Do not coarsen off-screen sim. Do not add a sky
  quadtree.

  After skipping dry contact-seepage pores that own no +x / +y wet-Air
  edge (infiltration from −x / −y is already owned by those cells),
  skipping dry/dry vertical seam wakes once any occupancy flag is set,
  and reading lake-bed / seam-band neighbours from the in-chunk cells
  (repeat soak, 256 plants, 8×400): seepage **3.83 → 5.95** (was
  **3.77 → 6.10**). Wall **20.8 → 83.7** — same leftover growers.
  Rain-wet halo sand that owns a wet edge, or already holds sat, still
  pays the contact walk; the skip does not move soak seep. Deep
  pore↔pore is unchanged. Do not apply the dry-pore skip on the deep
  pass. Do not skip a dry pore that owns the +x / +y wet-Air face.

  After planning gravity / flow / seepage from dilated dirty bits
  (each write +1 x / +2 y, not the bounding box; repeat soak, 256
  plants, 8×400): work-set **2295 / 4619** vs AABB **6861 / 29171**
  (old halo **7227 / 38144**). Gravity **0.36 → 1.00** (was **0.54 →
  2.24** — column walk is bits only). Seepage **2.35 → 4.45** (was
  **3.83 → 5.95**). Flow **0.98 → 3.25** (was **0.71 → 2.65**) — the
  flow scan still walks the box and bit-tests holes. Physics **10.0 →
  32.5** (was **10.7 → 33.3**). Wall **20.8 → 83.9** — same leftover
  growers. Legacy saves with a rect and empty bits keep the old
  inflate. Do not drop the +1 x / +2 y halo.

  After iterating flow / throughflow / seepage / confined on set bits
  (`for_each_cell` / `for_each_cell_in_y`, same tzcnt walk gravity
  already used; repeat soak, 256 plants, 8×400): flow **0.53 → 1.58**
  (was **0.98 → 3.25**). Seepage **2.03 → 3.95** (was **2.35 → 4.45**).
  Gravity **0.35 → 0.98** (unchanged). Physics **9.1 → 28.8** (was
  **10.0 → 32.5**). Wall **18.8 → 77.2** (was **20.8 → 83.9**). Confined
  **1.08 → 3.30** still walks the standing band / period-16 wake.
  Dry Air still owns the +x equalise edge. Flow early-out stays on
  the AABB, not the bit count. Do not skip the contact dry-pore
  check on the deep seepage pass.

  After walking filled-sky humidity diffusion on a dense slab (same
  +x/+y stencil; HashMap+sort stays until the box is half full or
  tiny; repeat soak, 256 plants, 8×400): wall **18.8 → 76.9** (was
  **18.8 → 77.2**). Diffuse is period-20 — the slab does not move
  soak wall. The new columns name the gap: humidity advect
  **1.84 → 13.45**, wind rebuild **2.67 → 19.73**, temperature
  **0.34 → 0.45** (T slab already holds). Those two growers track
  `hum n` **13k → 55k / 68k**. Not view LOD — every tile still
  steps. Do not coarsen off-screen advect. Do not retry the wind
  Jacobi solid-cache.

  After caching orographic speed/lift/descent per column for one
  rebuild, and packing Jacobi face-blocked + neighbour indices for
  that rebuild only (repeat soak, 256 plants, 8×400): wind
  **1.13 → 7.20** (was **2.67 → 19.73**). Wall **17.2 → 62.3**
  (was **18.8 → 76.9**). Humidity advect **1.85 → 13.38**
  (unchanged — still tracks `hum n`). Temperature **0.35 → 0.45**.
  Same compose stencil and slip. Do not cache solids across
  rebuilds. Do not coarsen off-screen wind.

  After walking filled-sky humidity flux on a dense slab, sampling
  wind into a Vec parallel to the snapshot, and caching free-air /
  orographic lift per occupied column (repeat soak, 256 plants,
  8×400): humidity advect **1.63 → 9.30** (was **1.85 → 13.38**).
  Wall **16.9 → 60.9** (was **17.2 → 62.3**). Wind **1.10 → 7.42**
  (unchanged). Same fractional flux and wrap. Do not coarsen
  off-screen advect. Lottery still visits every over-sat tile.

  After packing flux, buried-lift, wind-mix, and oro onto that same
  slab (write-back only tiles that moved; HashMap path stays until
  the box is half full; repeat soak, 256 plants, 8×400): humidity
  advect **1.63 → 3.71** (was **1.63 → 9.30**). Wall **17.2 → 53.9**
  (was **16.9 → 60.9**). Wind **1.09 → 7.29** (unchanged). Early
  soak stays sparse (`hum n` **13k / 68k**). Same stencil and
  valley test. Do not coarsen off-screen advect. Lottery still
  visits every over-sat tile.

  After packing wind compose / spatial blend / Jacobi on that same
  filled-sky box (solids packed for this rebuild only; thermal ∇T
  reads the existing temperature slab; repeat soak, 256 plants,
  8×400): wind **0.89 → 4.11** (was **1.09 → 7.29**). Wall
  **16.6 → 52.7** (was **17.2 → 53.9**). Humidity advect
  **1.62 → 3.91**. Same drivers, same six Jacobi sweeps, same slip.
  Do not cache solids across rebuilds. Do not coarsen off-screen
  wind. Lottery still visits every over-sat tile.
