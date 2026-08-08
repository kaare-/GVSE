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
| **Sym water support** | `sym_water_*`, `plants_dry_sym_recv`, drought, root moist, Organic depth |
| **Plant evolution** | mean alloc / depth-bias / fidelity, body module means, treaties |
| **Habit cohorts** | `woody_plants`, `stemless_wet` (bathing seaweed), **`stemless_dry`** (stranded seaweed on drying land), per-cohort mean roots / moist / drought, `mean_depth_bias_stemless_dry`, `mean_org_depth_woody` |

`stemless_dry` + `mean_roots_stemless_dry` capture the common failure mode where
seaweed ends up on dry land and sprouts long roots through dry periods —
distinct from woody plants losing their stem count in the pooled means.

## Usage

```bash
# Short CI harness
cargo test -p wk-voxel --test sim_log_soak --release short_logged_life_run -- --nocapture

# Long soak (writes NDJSON when GVSE_SIM_LOG is set)
GVSE_SIM_LOG=/tmp/gvse-soak.ndjson GVSE_SOAK_TICKS=1000000 GVSE_SIM_LOG_PERIOD=2000 \
  cargo test -p wk-voxel --test sim_log_soak --release long_sim_log_soak \
  -- --ignored --nocapture
```

Fixture: moist beach grove + deep lake, woody plants (Symbiont), seaweed,
mycelium lineage inoculum. Climate loop mirrors Tab: evap (period 5) →
humidity → clouds (`coag_rate=0.12`) → condensation → physics → carbon →
organisms. Checkpoint notes include woody/wet/dry cohorts.

To replay the Tab-side knobs from a successful soak locally: open Settings
(Tab) → **Named presets** → load `soak-survival` (built-in, also shipped as
`presets/soak-survival.json`). Save your own setups the same way.

Format is **NDJSON** (`type: event|sample` per line).
