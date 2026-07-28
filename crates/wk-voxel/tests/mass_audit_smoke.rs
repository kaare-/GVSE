//! Closed-basin cell-sat conservation over a long dirty tick run.
//!
//! Physics [`tick`] moves free / pore water but does not mint or delete
//! sat (rain / bare evap / ice cull live outside this path). A multi-chunk
//! fixture with standing water + grain should keep [`sat_totals`] flat.
//!
//! ```bash
//! cargo test -p wk-voxel --test mass_audit_smoke --release
//! ```

use wk_material::MaterialId;
use wk_voxel::{
    sat_totals, set_mass_audit_enabled, tick_with_configs, Cell, FailureConfig, PerfConfig, World,
    CELL_SAT_TICK_TOLERANCE,
};

/// Three chunks wide (~192 cells), bedrock bowl, sand bed, free water,
/// and a sand tower so gravity / flow / grain / repose all dirty.
fn wide_dirty_basin() -> World {
    let mut w = World::new(0xA55_A001);
    let width = 64 * 3;
    // Bedrock floor + walls so water cannot leave the loaded strip.
    for x in 0..width {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=20 {
        w.set_cell(0, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(width - 1, y, Cell::solid(MaterialId::Bedrock));
    }
    // Sand bed with a shallow pool.
    for x in 1..width - 1 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        if (40..120).contains(&x) {
            w.set_cell(x, 2, Cell::water());
            w.set_cell(x, 3, Cell::water());
        }
    }
    // Grain tower above one pool edge — settles and dirties neighbours.
    for y in 4..=12 {
        w.set_cell(80, y, Cell::solid(MaterialId::Sand));
        w.set_cell(81, y, Cell::solid(MaterialId::Sand));
    }
    w
}

#[test]
fn wide_basin_conserves_cell_sat_over_2000_ticks() {
    // Exercise the in-tick debug assert as well as the end-to-end sum.
    set_mass_audit_enabled(true);

    let mut w = wide_dirty_basin();
    let start = sat_totals(&w);
    assert!(
        start.cell_total > 10_000,
        "fixture should hold a meaningful water inventory (got {})",
        start.cell_total
    );

    // Default failure is fine on this bowl (no limestone roofs). Keep
    // parallel off so the run is deterministic across hosts.
    let perf = PerfConfig {
        parallel_physics: false,
        ..PerfConfig::default()
    };
    let failure = FailureConfig::default();

    for _ in 0..2000 {
        tick_with_configs(&mut w, &perf, &failure);
    }

    let end = sat_totals(&w);
    let delta = end.cell_total - start.cell_total;
    assert!(
        delta.abs() <= CELL_SAT_TICK_TOLERANCE,
        "cell sat drifted over 2000 ticks: start={} end={} Δ={} (free {}→{}, pore {}→{})",
        start.cell_total,
        end.cell_total,
        delta,
        start.free_air,
        end.free_air,
        start.pore,
        end.pore,
    );
    assert_eq!(w.tick, 2000);

    set_mass_audit_enabled(false);
}
