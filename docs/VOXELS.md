# Voxels, columns, and fields

*Author: design memo, mid-2026. This document answers the question
"would this whole simulation work as a voxel grid with heatmap-based
physics for everything?" and records the recommended architecture:
keep columns for material identity, add scalar/vector fields
("heatmaps") for smooth physics, extend voids for cavities. Full
voxelisation deferred with reasons.*

## The question, restated

We're considering: what if the world weren't 5632 columns of up-to-8
layers, but a 2D voxel grid — every 0.25 m × 0.25 m cell has a
material, mass fraction, connectivity, temperature, moisture, solute
concentration? And the physics — heat flow, water flow, dissolution,
weathering, air movement — was expressed as computations on
heatmap-style fields defined over the grid?

The ambition list, in the order the user raised it:

- Per-material physical properties: mass, compressive strength, shear
  strength, permeability, solubility (chemical + mechanical),
  temperature.
- Block connectivity: connected blocks form massive rock; disconnected
  ones are loose and subject to gravity, and can be pushed by sand or
  water if the driving force is large enough.
- Water flow through permeable media by material permeability;
  mineral pickup by chemical + mechanical dissolution.
- Cave formation as an emergent consequence of dissolution in weak
  soluble rock.
- Deep underground modelled with geothermal heat from a bottom
  boundary.
- Heatmaps everywhere: temperature, wind, underground flow, mineral
  content, air pressure, humidity, block temperature, sub-surface heat
  penetration and storage.
- Above-ground: evaporation, humidity, wind, air temperature (via
  heatmap), block temperature.
- Heat transfer through the ground, storage during the day, radiation
  at night.
- Everything else on top, including evolving creatures.

The intuition the user is developing: **"basically anything becomes a
computation between two or more heatmaps."**

That intuition is *exactly right* for a large class of the physics
listed. It's also incomplete — some processes are genuinely discrete
and want per-cell state, not fields. This memo unpacks which is which.

## What the user got right

The phrase "computation between heatmaps" is, unwittingly, the
foundational abstraction of computational continuum physics. Every
diffusion process, every advection process, every gradient-driven
transport process — heat flow, groundwater flow, atmospheric
circulation, solute transport, radiation transfer — is a differential
equation whose numerical discretisation is exactly:

- Represent each field as a 2D array of samples on a regular grid.
- Update each cell as a function of itself and its neighbours (a
  "stencil operation").
- Repeat every tick.

Textbook form: `∂φ/∂t = ∇·(K ∇φ) + S`. The left side is "how field φ
changes over time." The right side is "gradient flow driven by
transport coefficient field K, plus source field S." Everything is a
field. Everything is defined on a grid. Everything is a stencil.

- **Heat flow**: `∂T/∂t = α ∇²T + Q`, where T = temperature heatmap,
  α = thermal diffusivity (per-material), Q = heat source (from sun
  during day, from radiation cooling at night, from geothermal at
  bottom boundary). Three heatmaps → one output heatmap. Per user's
  intuition, exactly.
- **Groundwater flow (Darcy's law)**: `∂h/∂t = ∇·(K ∇h) + R`, where
  h = hydraulic head heatmap, K = permeability (per-material), R =
  recharge (rain infiltration reaching the water table). Three
  heatmaps → one heatmap. Same shape.
- **Wind from pressure**: `v = −(1/ρ) ∇p`, where p = air pressure
  heatmap, ρ = air density. Wind field is the gradient of a pressure
  field. One heatmap → one vector field.
- **Mineral advection**: `∂c/∂t + v·∇c = k · sol · v · A`, where
  c = dissolved concentration heatmap, v = flow velocity field,
  sol = solubility of contact material, A = contact surface. Multiple
  heatmaps → one heatmap.

The user's mental model is the right mental model. Where the model
breaks down is when the phenomenon is not smooth — when the underlying
physics has *identity* per cell rather than a continuous field value.

## What voxels/columns are actually needed for

Some phenomena are genuinely discrete. You can't smoothly interpolate
"is this cell sandstone or is it cave-air" — it's binary. That is
material identity, and it wants per-cell storage regardless of
whether the smoothing physics operates on a coarser field.

Discrete phenomena in the ambition list:

| Process | Why discrete |
|---------|--------------|
| Material identity per location | Sandstone or granite or air — no in-between. Field can't represent this. |
| Block connectivity (loose vs. massive) | Union-find of same-material adjacent voxels. Graph, not field. |
| Rock fracture / failure | A block breaks or doesn't. Discrete event. |
| Water surface geometry | Free surface is a sharp interface, not a smooth field. |
| Sediment deposits | Particles land at specific cells. |
| Cave void volumes | A cell is void or filled. |
| Creature positions | Discrete entities in space. |

Notice these are all *identity/inventory* phenomena, not
*transport/gradient* phenomena. That's the split.

## The hybrid architecture

The right shape for this whole ambition is:

- One **material grid** at the voxel/column resolution (0.25 m) storing
  what each cell *is*. Material ID, mass fraction 0–1, connectivity
  cluster ID, a few flags.
- Multiple **field grids** ("heatmaps") at their own resolutions, each
  storing a scalar or vector at each field cell. Temperature at 0.5 m,
  groundwater head at 1 m, air pressure at 2 m, dissolved mineral
  concentration at 0.5 m, humidity at 2 m, etc.
- **Coupling** between fields and material: material properties feed
  into field coefficients (permeability, thermal diffusivity, albedo);
  field values act on material (dissolution rate driven by contact
  with high-concentration flow; freeze/thaw driven by local T).

This is a standard architecture in scientific computing. Weather
models, groundwater models, ocean models, engineering FEA — none of
them are pure voxel or pure field, all of them are hybrid.

### Why fields can be coarser than voxels

Thermal diffusion smooths temperature over its diffusion length, which
for rock at reasonable time scales is metres, not centimetres. A 0.5 m
temperature field is *exactly as accurate* as a 0.25 m one because
temperature genuinely doesn't vary faster than that. A 2 m humidity
field over air is likewise correct because air mixes on second-scale
timescales at metre resolutions.

Fine resolution buys nothing when the physics doesn't produce
sub-cell-scale variation. That's the numerical-physics reason
multigrid works.

### How fields couple to material

The coupling is one of two shapes:

- **Material → field coefficient**: "the thermal diffusivity at field
  cell (i,j) is the area-weighted mean of the thermal diffusivities of
  the voxels underneath it." Done once per material change.
- **Field → material rate**: "at voxel (i,j), if the concentration at
  the covering field cell is > threshold and the material is soluble,
  reduce its mass by rate proportional to (concentration ×
  solubility × contact area) this tick."

Both are simple lookups. Neither requires per-voxel field values.

## Budget analysis: is this real-time?

Take the shipped active window: ~30 chunks × 64 columns = 1920
columns. Vertically, if we model 100 m of subsurface plus 30 m of air,
that's 130 m at 0.25 m = 520 voxels tall.

**Total voxels**: 1920 × 520 ≈ 1.0 M voxels.

**Per-tick voxel work** for the discrete physics (identity,
connectivity, dissolution, gravity on loose rocks):

- **Connectivity graph**: only updated when the material grid changes.
  Union-find is amortised near-constant per event. Not per-tick. Cost
  ≈ 0.
- **Compressive/shear stress**: only computed on load-change events
  (mass added, mass removed, roof span exceeded). Global stress
  propagation is a linear system solve; running it every tick is
  intractable, running it *per event* on the local cluster is fine.
  Cost per tick ≈ 0 in steady state; O(cluster_size) at each event.
- **Dissolution**: only runs on voxels adjacent to flowing water AND
  above a dissolution rate threshold. Active-set size for a lively
  cave system is ~1000–10000 voxels (the wetted perimeter of all cave
  passages), not a million.
- **Loose-rock gravity**: only on voxels marked loose. In a stable
  world, near zero. After a collapse event, briefly O(collapse_size).
- **Water content**: this is either a field (smooth) or a per-voxel
  scalar (identity + amount). Most of the water in a cave is smooth;
  the surface layer where the interface is sharp needs per-voxel
  bookkeeping. Active-set ≈ voxels at the water surface, O(sqrt(N))
  for a rough estimate.

**Realistic per-tick voxel work: O(10 k) active cells at O(50 ns)
each = 500 μs per tick.** Comfortably within budget.

**Per-tick field work**:

At 0.5 m field resolution, the field grid is 4× fewer cells than the
material grid: 250 k field cells. A 5-point Laplacian stencil (heat
diffusion) is ~10 flops per cell = 2.5 Mflops per field per tick.
Five active fields (temperature, humidity, pressure, groundwater
head, dissolved minerals) = 12.5 Mflops per tick.

At 60 tps: 750 Mflops/s for all field work. A modern desktop core is
~100 Gflops single-thread. This is ~1% of one core.

**Total hybrid budget**: ~15–20 Mflops per tick (fields) + 500 μs of
active-set voxel work (discrete). Comfortably real-time on one core
for a much larger world than currently shipped.

**Compare against naive full-voxel physics** — every voxel updated for
every process every tick:

- 1 M voxels × 5 processes × 100 flops = 500 Mflops per tick
- × 60 tps = 30 Gflops/s
- ≈ 30% of one core just for baseline physics

Doable, but not comfortable. And that's assuming stable physics at
0.25 m — realistic Darcy flow at that resolution would need implicit
time integration to stay stable, which triples the cost. The naïve
full-voxel path is on the edge; the hybrid is easy.

## Applying the hybrid to the user's ambition list

Walk through each phenomenon the user listed and place it in the
hybrid:

| Phenomenon | Where it lives |
|------------|----------------|
| Mass, permeability, solubility (property table) | Material grid + `MaterialProps` lookup. No physics. |
| Compressive / shear strength | Material property. Consulted at fracture / roof-collapse events, not per tick. |
| Block connectivity, loose vs. massive | Voxel-level union-find. Updated on material change. |
| Loose-rock gravity, push-by-water | Voxel-level, active-set only (the loose set). |
| Water flow through permeable rock (Darcy) | Field: hydraulic head. Coefficient: material permeability. Standard PDE solve. |
| Free-surface water (rivers, lakes, cave rivers) | Hybrid: field for pressure, voxel-level for the interface. |
| Mineral pickup: chemical dissolution | Field: dissolved concentration. Rate depends on flow × solubility × contact. Reduces voxel mass. |
| Mineral pickup: mechanical erosion | Same shape, driven by shear stress ∝ flow gradient. |
| Cave formation | Emergent from dissolution reducing voxel mass to zero over time. |
| Deep underground | Just more voxels below the current bedrock line. |
| Geothermal from bottom | Boundary condition on temperature field: `T[y = bottom_row] = geothermal_target`. |
| Air temperature | Temperature field, above-ground half. Boundary at top: sun/night. Boundary at ground: soil temperature. |
| Wind | Vector field derived as `−∇p / ρ` from a pressure field. Pressure field driven by temperature differentials (convection) and boundary conditions. |
| Humidity | Field, advected by wind, sourced by evaporation, sunk by precipitation. |
| Evaporation | Rate field: at each surface cell, evaporation rate ∝ (saturation_deficit × available_water × temperature). Reads water grid, writes humidity. |
| Heat transfer between blocks / into ground / storage / night radiation | All one PDE: `∂T/∂t = α ∇²T + Q`. Q includes solar during day, radiation loss at night, geothermal at bottom. Nothing special; it just works. |
| Events modify heatmaps | A fire ignites → local `Q` spike in the temperature field. Water evaporates → local sink in the humidity field. Exactly the user's intuition. |

Where the user asked "is it better to modify a heatmap on world
events than to try to calculate this on a block to block basis?" —
**yes, generally, and by a large margin.** Heatmap physics is faster,
more numerically stable, more amenable to SIMD and parallelisation,
and has a mature literature of stable-time-step recipes to draw from.
Block-to-block heat propagation as an explicit graph is what you'd
write in an undergraduate exercise; nobody does it professionally.

The one exception, and it matters: when the underlying material is
highly heterogeneous at sub-cell scales (a thin insulating layer, a
narrow high-conductivity vein), a coarser field cell "averages away"
the important structure. The solution is standard: the field
coefficient at cell (i,j) is not a per-material value but an
*effective* coefficient computed as a harmonic or arithmetic mean of
the underlying voxels' properties, depending on whether the field
flows across or along the heterogeneity. Well-studied problem, known
formulas.

## Where the column model still falls short

The current column model *could* host all the fields above with no
structural change — a field grid is a separate 2D array that doesn't
care whether the material grid is columns-of-layers or voxels. So
adding fields is orthogonal to voxelisation and can happen first,
independently.

But the material grid itself has real limitations:

1. **Overhangs and surface caves.** Layer stacks can't represent
   "there is rock at y=8 but nothing at y=6." The `Void` annotation
   from `UNDERGROUND.md` handles subsurface caves. For *surface*
   overhangs (cliffs you can walk under) it needs a small extension:
   allow a void to breach the top of the column, so the layer stack
   above the void doesn't count toward `surface_y`.

2. **Individual boulder simulation.** A layer is a mass of a material,
   not a *thing*. Rolling a specific rock down a slope isn't
   expressible. Either a separate boulder-entity list (small, cheap)
   or a voxelised region.

3. **Sharp geological structures that don't align with layers.** A
   near-vertical dike of granite intruding through sandstone doesn't
   fit "layers stacked bottom-up." Terrain generation can approximate
   with narrow-column material transitions, but you lose the ability
   to represent a dike that spans a vertical extent inside a layer.

4. **Fully 3D-ish local physics.** Column geometry is 1D vertically.
   Fields are the right fix for smooth phenomena (heat, pressure, flow)
   — they're 2D over the whole slab and don't care about column
   structure. But for discrete phenomena that want vertical spatial
   detail (a hollow ceiling with a stalactite hanging into empty
   space), you're back to voxels.

Of these, (1) is addressed by the void extension in stage 7;
(2) is addressed by a small entity list in stage 10; (3) is a real
limitation but doesn't block the vision; (4) is why voxels are still on
the table as a possible future option.

## Full voxel rewrite — the honest cost

If we did go for a full voxel rewrite, replacing the column model
entirely:

- Rewrite `wk-material`, `wk-world`, `wk-sim`. Roughly the whole
  substrate. Only `wk-io` and `wk-app` survive with modifications.
- 12 subsystems become ~15 subsystems (add: connectivity, stress
  propagation, per-voxel gravity, cluster gravity). Each has to be
  re-designed for voxel semantics.
- Every scenario test has to be rewritten. The mass-audit invariant
  has to be re-proven for the new representation.
- Save format v2 needed. Migration from v1 possible but non-trivial.
- Stable-timestep tuning has to be redone from scratch: implicit
  Darcy solve for groundwater; CFL-limited advection for solute
  transport; parallelisation strategy revised.

Optimistic scope: many months to first parity with the current
substrate. Realistic scope: longer, plus real risk of never reaching
real-time at the full ambition on target hardware.

Meanwhile, the roadmap in `PLAN.md` reaches "creatures + evolution"
in a similar timeframe *while* keeping the currently-working
substrate. That's the trade.

## Recommendation

Do the hybrid:

1. **Add the field layer as an additive stage.** Fields are new 2D
   arrays; they don't disturb columns. Introduce them one at a time
   (temperature first, then humidity, then subsurface heat with
   geothermal boundary, then pressure/wind). Each field wires into
   existing subsystems as a lookup replacing a hardcoded constant.
   Landing this stage is a strict upgrade: the sim gets more physical
   and no less performant.

2. **Extend the void model for surface overhangs.** Small addition to
   `UNDERGROUND.md`: allow a void whose top exceeds `surface_y − ε`
   to count as a surface overhang. Renderer and pathfinding for
   creatures get a two-line update. Zero cost when no overhangs exist.

3. **Small entity list for genuine "things" that aren't in a layer.**
   Boulders that roll, logs floating on water, ice floes. Entities
   have position + velocity + material + mass and interact with the
   column stack via well-defined hooks. This is where creatures
   eventually live too.

4. **Bounded voxel regions only if a specific feature demands it.**
   For instance: if late-game cave interior gameplay wants detailed
   3D-ish cave geometry, voxelise *inside the void volumes*, not the
   whole world. This is standard "adaptive mesh refinement" — coarse
   representation everywhere, fine representation where needed.

5. **Reserve the full voxel rewrite** as an option that exists but
   would only be exercised if the hybrid genuinely fails a
   requirement of the target game. So far nothing in the ambition
   list actually requires it.

The user's core insight — heatmaps as first-class simulation
elements — becomes stage 6 (field layer) and it's a big win. The
part that would require a full voxel rewrite (per-voxel material
identity everywhere) is not what makes the heatmap idea powerful, and
in fact the current column model is a perfectly good "voxel-ish"
material grid for the smoother-physics purposes. The columns *are*
1D voxels for stratigraphic identity; they just happen to be
compressed into layers.

## Risks and cautions

If we do voxelise, even in a bounded region:

- **Time-step stability is a research problem, not an engineering
  problem.** Explicit schemes have CFL limits proportional to grid
  spacing squared for diffusion, so a 4× finer voxel grid needs a 16×
  smaller time step for the same stability, or an implicit solver.
  Implicit solvers are correct but harder to write and slower.
- **Mass conservation with fields is trickier than with layers.**
  A layer has an exact integer kg mass; a field has a floating-point
  sample per cell, and the total is a sum of samples times cell
  volume, which accumulates rounding error. The audit invariant has
  to be rewritten with a tolerance that scales with number of field
  cells, not the current fixed 100 kg over 100k ticks.
- **Numerical dispersion in advection.** A cheap upwind advection
  scheme spreads a sharp concentration front over time, so a "pulse"
  of dissolved mineral gets smeared. There are better schemes
  (MUSCL, WENO) but they're substantially more code.
- **Anisotropic materials** (a permeability that's high horizontally
  and low vertically — bedded sandstone is exactly this) need a
  tensor coefficient, not a scalar. Standard, but another degree of
  freedom to get right.

None of these are dealbreakers. All of them are things a scientific
computing textbook has a chapter on. They just aren't zero-cost.

## Summary

- The user's intuition — most physics is heatmap operations — is
  correct and matches how professional simulations are actually
  structured.
- The current column model is a working material grid. Voxelisation
  is not necessary to unlock the heatmap paradigm.
- Recommend: hybrid architecture. Fields alongside columns (stage 6),
  void extension for surface overhangs (stage 7 extension), entity
  list for real "things" (stage 10). Full voxel rewrite reserved for
  a hypothetical future need.
- With the hybrid, the ambition list is achievable inside real-time
  budget on one core, and the work builds on top of the current
  substrate instead of replacing it.
- If any specific gameplay requirement later *demands* voxels (cave
  interior detail is the most likely candidate), a bounded voxelised
  region is the answer, not a global rewrite.
