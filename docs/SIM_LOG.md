# Simulation event log

Headless-friendly event + sample stream for soak tests and bug hunts.
Lives in `wk-voxel` (`event_log.rs`) — no GUI dependency.

## What it records

**Events** (discrete): births / deaths by habit, plant tips, spores,
emergent fruiting, spore-bank wakes, geotech roof/shear/compaction,
harness notes.

**Samples** (every N ticks):

| Group | Fields |
|-------|--------|
| Population | `p/f/a`, corpses, fallen plants, spore bank |
| Water mass | sat free / pore / total |
| Mycelium | cream cells/sum, sugar, strain cells |
| Carbon | atmosphere, dissolved (+ optional mean T) |
| **Sym water support** | `sym_water_recv/sent_tick`, sugar paid/recv, `plants_sym_linked`, `plants_drought`, **`plants_dry_sym_recv`** (drought plants still getting cream water), `plants_with_symbiont`, `mean_root_moist`, `mean/max_organic_depth` |
| **Plant evolution** | stemless count, mean body/root/stem/photo modules, mean `alloc_*`, `root_depth_bias`, `clone_fidelity`, leaf/shade knobs, mean `sym_water` / `sym_energy` treaties |

`plants_dry_sym_recv` is the desert-support signal: plants whose bed is
dry / drought-stressed but still received pore water from the mycelium
network that tick (Symbiont Supply path, including long-haul
`pull_mycelium_cargo_to`).

`mean_organic_depth` tracks cream/litter stacks under crowns — useful when
Organic buildup lifts roots off water-rich mineral strata and lakes dry into
the litter sponge.

## Usage

```bash
# Short CI harness
cargo test -p wk-voxel --test sim_log_soak --release short_logged_life_run -- --nocapture

# Long soak (writes NDJSON when GVSE_SIM_LOG is set)
GVSE_SIM_LOG=/tmp/gvse-soak.ndjson GVSE_SOAK_TICKS=1000000 GVSE_SIM_LOG_PERIOD=2000 \
  cargo test -p wk-voxel --test sim_log_soak --release long_sim_log_soak \
  -- --ignored --nocapture
```

Fixture seeds a moist beach grove + deep lake with **land plants**
(Symbiont painted), **seaweed**, and **mycelium inoculum** (designed
fruiting body + Symbiont stamped as lineage — no living stalk until the
network emerges). The harness also runs the water→humidity→cloud loop with
raised `coag_rate` (0.12) and higher cloud deck knobs for the fixture sky.

Format is **NDJSON** (`type: event|sample` per line). Summary also prints
to stderr for cloud agent logs (includes `dry_sym`, moist, org depth, and
evolution means).

## Hooks

- `tick_with_life` → `FailureStats` (geotech counts)
- `OrganismStore::step_with_carbon` → `OrganismStepOutcome { spores, stats }`
- Sample after the organism step so `sym_*_last` still holds
- Harness / app can call `SimLog::record_*` / `maybe_sample` each frame
