//! E8 — snapshot round-trip then continue ticking (voxel port).
//!
//! Legacy oracle: `tests/scenarios/e8_save_load.rs` (column stack via wk-io).
//! Product intent: save → load preserves state; sim can continue.

use wk_voxel::{
    tick, CarbonBudget, Cell, CloudStore, Humidity, OrganismStore, SimSnapshot, Temperature, Wind,
    World, WorldgenParams, CHUNK_CELLS_W,
};

use crate::helpers::{sat_sum, setup_hill_world};

fn climate_for(params: &WorldgenParams) -> (Humidity, Wind, Temperature) {
    let humidity = Humidity::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
    );
    let wind = Wind::climate(
        4,
        0.05,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.bedrock_floor_y,
        params.sky_ceiling_y,
        params.wrap_x,
    );
    let temperature = Temperature::with_world_bounds(
        4,
        0,
        params.bedrock_floor_y,
        params.width_cols,
        params.sky_ceiling_y,
        params.seed,
        params.width_cols,
        params.sea_level_y,
        params.wrap_x,
    );
    (humidity, wind, temperature)
}

#[test]
fn e8_save_load_continuation() {
    let params = WorldgenParams {
        seed: 99999,
        width_cols: CHUNK_CELLS_W as i32,
        bedrock_floor_y: 0,
        sea_level_y: 2,
        sky_ceiling_y: 32,
        wrap_x: false,
        ..WorldgenParams::default()
    };

    let mut world = setup_hill_world(params.seed, params.width_cols, 32, 6);
    // Seed a crest film so there is non-trivial state to serialize.
    for x in 30..35 {
        world.set_cell(x, 8, Cell::water());
    }
    for _ in 0..20 {
        tick(&mut world);
    }
    let tick_before = world.tick;
    let sat_before = sat_sum(&world, 0, 63, 0, 16);

    let (humidity, wind, temperature) = climate_for(&params);
    let snap = SimSnapshot::new(
        params,
        world,
        humidity,
        wind,
        temperature,
        CloudStore::new(),
        OrganismStore::new(),
        CarbonBudget::default(),
    );
    let bytes = snap.to_bytes().expect("serialize");
    let loaded = SimSnapshot::from_bytes(&bytes).expect("deserialize");

    assert_eq!(loaded.world.tick, tick_before);
    assert_eq!(
        sat_sum(&loaded.world, 0, 63, 0, 16),
        sat_before,
        "sat inventory must round-trip"
    );
    // Spot-check a few cells for bit-identity after postcard.
    for x in [0, 32, 63] {
        for y in [0, 4, 8] {
            assert_eq!(
                snap.world.get_cell(x, y),
                loaded.world.get_cell(x, y),
                "cell mismatch at ({x},{y})"
            );
        }
    }

    let mut world2: World = loaded.world;
    for _ in 0..20 {
        tick(&mut world2);
    }
    assert!(world2.tick > tick_before);
    // Continuation should keep a finite, non-negative water inventory.
    let sat_after = sat_sum(&world2, 0, 63, 0, 16);
    assert!(sat_after >= 0);
    assert!(
        sat_after > 0,
        "continued sim should still hold water (sat={sat_after})"
    );
}
