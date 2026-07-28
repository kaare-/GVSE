//! E11 — cave roof collapses (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e11_cave_roof_collapses.rs`.
//! Product intent: a wide cavity under a weak (sand) roof collapses —
//! ceiling vacates and debris (LooseRock / seated sand) fills the void.
//!
//! Uses the full tick path (`tick_with_configs`) so roof failure runs
//! where the demo schedules it, not only the isolated rule helper.

use wk_material::MaterialId;
use wk_voxel::{
    tick_with_configs, Cell, ChunkCoord, FailureConfig, PerfConfig, World,
};

use crate::helpers::{count_material, lay_bedrock_floor};

#[test]
fn e11_sand_cave_roof_collapses() {
    let mut world = World::new(9011);
    world.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut world, 40);

    // Contiguous Air cavity under a sand ceiling (sand span limit = 0,
    // so even a short bridge must drop). Pillars at the ends.
    let x0 = 10;
    let x1 = 28;
    world.set_cell(x0 - 1, 1, Cell::solid(MaterialId::Sand));
    world.set_cell(x1 + 1, 1, Cell::solid(MaterialId::Sand));
    for x in x0..=x1 {
        world.set_cell(x, 1, Cell::air());
        world.set_cell(x, 2, Cell::solid(MaterialId::Sand)); // roof
    }

    let roof_sand_before = count_material(&world, MaterialId::Sand, x0, x1, 2, 2);
    let cavity_air_before = count_material(&world, MaterialId::Air, x0, x1, 1, 1);
    let loose_before = count_material(&world, MaterialId::LooseRock, x0, x1, 1, 2);
    assert_eq!(roof_sand_before, (x1 - x0 + 1) as usize);
    assert_eq!(cavity_air_before, (x1 - x0 + 1) as usize);
    assert_eq!(loose_before, 0);

    let fail = FailureConfig {
        enable_roof_collapse: true,
        max_roof_events: 64,
        ..FailureConfig::default()
    };
    let perf = PerfConfig {
        parallel_physics: false,
        ..PerfConfig::default()
    };

    // A few ticks: roof events + grain seating.
    for _ in 0..8 {
        tick_with_configs(&mut world, &perf, &fail);
    }

    let roof_sand_after = count_material(&world, MaterialId::Sand, x0, x1, 2, 2);
    let loose_after = count_material(&world, MaterialId::LooseRock, x0, x1, 1, 2);
    let debris_or_seated = count_material(&world, MaterialId::Sand, x0, x1, 1, 1)
        + count_material(&world, MaterialId::LooseRock, x0, x1, 1, 1);

    assert!(
        roof_sand_after < roof_sand_before,
        "sand ceiling should vacate (before={roof_sand_before} after={roof_sand_after})"
    );
    assert!(
        loose_after > 0 || debris_or_seated > 0,
        "collapse should yield debris in the cavity (loose={loose_after} seated/debris={debris_or_seated})"
    );
}
