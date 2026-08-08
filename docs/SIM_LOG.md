# Simulation event log

Headless-friendly event + sample stream for soak tests and bug hunts.
Lives in `wk-voxel` (`event_log.rs`) — no GUI dependency.

## What it records

**Events** (discrete): births / deaths by habit, plant tips, spores,
emergent fruiting, spore-bank wakes, geotech roof/shear/compaction,
harness notes.

**Samples** (every N ticks): `p/f/a`, corpses, fallen plants, spore bank,
sat free/pore, mycelium cream/sugar, carbon buckets, optional mean T.

## Usage

```bash
# Short CI harness
cargo test -p wk-voxel --test sim_log_soak --release short_logged_life_run -- --nocapture

# Long soak (writes NDJSON when GVSE_SIM_LOG is set)
GVSE_SIM_LOG=/tmp/gvse-soak.ndjson GVSE_SOAK_TICKS=50000 GVSE_SIM_LOG_PERIOD=120 \
  cargo test -p wk-voxel --test sim_log_soak --release long_sim_log_soak \
  -- --ignored --nocapture
```

Format is **NDJSON** (`type: event|sample` per line). Summary also prints
to stderr for cloud agent logs.

## Hooks

- `tick_with_life` → `FailureStats` (geotech counts)
- `OrganismStore::step_with_carbon` → `OrganismStepOutcome { spores, stats }`
- Harness / app can call `SimLog::record_*` / `maybe_sample` each frame
