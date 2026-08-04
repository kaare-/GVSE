//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Unit tests for cellular-automaton rules.

use super::*;
use crate::active::{clear_all_dirty, plan_active};
use crate::cell::{water_capacity, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;
use crate::humidity::Humidity;
use crate::phase::PhaseConfig;
use crate::temperature::Temperature;
use wk_material::{HydroOverrides, MaterialId};

use super::head::{hydraulic_head, seepage_rate_with};

fn setup_column_world() -> World {
    // One chunk. Row y=0 is a solid Bedrock floor; every other
    // cell is Air (empty).
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..(CHUNK_CELLS_W as i32) {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w
}

#[test]
fn droplet_falls_one_cell_per_pass() {
    let mut w = setup_column_world();
    w.set_cell(4, 10, Cell::water());
    assert!(w.get_cell(4, 10).unwrap().sat.is_full());
    assert!(w.get_cell(4, 9).unwrap().sat.is_empty());

    apply_gravity_fall(&mut w);
    assert!(w.get_cell(4, 10).unwrap().sat.is_empty());
    assert!(w.get_cell(4, 9).unwrap().sat.is_full());
}

#[test]
fn droplet_stops_on_bedrock() {
    let mut w = setup_column_world();
    w.set_cell(2, 1, Cell::water());
    apply_gravity_fall(&mut w);
    // Bedrock capacity is 0 — no move.
    assert!(w.get_cell(2, 1).unwrap().sat.is_full());
    assert!(w
        .get_cell(2, 0)
        .unwrap()
        .sat
        .is_empty());
}

#[test]
fn resting_column_does_not_compress() {
    let mut w = setup_column_world();
    // Water in y=1..4 (four cells), solid bedrock at y=0.
    for y in 1..=4 {
        w.set_cell(2, y, Cell::water());
    }
    apply_gravity_fall(&mut w);
    // All four cells should still be full — each already sits on
    // full water or bedrock and has nowhere to go.
    for y in 1..=4 {
        assert!(
            w.get_cell(2, y).unwrap().sat.is_full(),
            "y={y} lost water"
        );
    }
}

#[test]
fn lake_bed_sand_wets_clay_and_stone_below_via_tick() {
    // Lake water on sand over clay over stone. Downward pore soak
    // must reach every porous layer (gravity + seepage), not stop
    // at the sand cap.
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    // Contain laterally so free water can't run off the column.
    for x in 3..=5 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=6 {
        w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(5, y, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(4, 2, Cell::solid(MaterialId::Clay));
    w.set_cell(4, 3, Cell::solid(MaterialId::Sand));
    w.set_cell(4, 4, Cell::water());
    w.set_cell(4, 5, Cell::water());

    for _ in 0..30 {
        tick(&mut w);
    }

    let sand = w.get_cell(4, 3).unwrap();
    let clay = w.get_cell(4, 2).unwrap();
    let stone = w.get_cell(4, 1).unwrap();
    let sand_cap = water_capacity(MaterialId::Sand);
    let clay_cap = water_capacity(MaterialId::Clay);
    let stone_cap = water_capacity(MaterialId::Stone);
    assert_eq!(sand.sat.0, sand_cap, "sand should saturate");
    assert_eq!(clay.sat.0, clay_cap, "clay under sand must saturate");
    assert_eq!(stone.sat.0, stone_cap, "stone under clay must saturate");
}

#[test]
fn deep_stone_stack_keeps_wetting_after_surface_quiesces() {
    // Reproduce the lake-bed report: sand saturates quickly, then
    // deeper porous stone must keep taking water over many ticks
    // even after the free-surface looks settled.
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 3..=5 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=20 {
        w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(5, y, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=16 {
        w.set_cell(4, y, Cell::solid(MaterialId::Stone));
    }
    w.set_cell(4, 17, Cell::solid(MaterialId::Sand));
    w.set_cell(4, 18, Cell::solid(MaterialId::Sand));
    // Deep lake column above the bed.
    for y in 19..=22 {
        w.set_cell(4, y, Cell::water());
    }

    // After a few ticks the sand cap is wet; the deep stone must
    // not be left dry because dirty planning went quiet.
    for _ in 0..8 {
        tick(&mut w);
    }
    let sand = w.get_cell(4, 18).unwrap().sat.0;
    let sand_cap = water_capacity(MaterialId::Sand);
    assert_eq!(sand, sand_cap, "sand cap should be saturated early");

    for _ in 0..40 {
        tick(&mut w);
    }
    let stone_cap = water_capacity(MaterialId::Stone);
    let deep = w.get_cell(4, 1).unwrap().sat.0;
    let mid = w.get_cell(4, 8).unwrap().sat.0;
    assert_eq!(
        mid, stone_cap,
        "mid-stack stone should saturate (mid={mid})"
    );
    assert_eq!(
        deep, stone_cap,
        "deep stone under the lake bed should saturate (deep={deep})"
    );
}

#[test]
fn water_saturates_porous_solid_up_to_capacity() {
    let mut w = setup_column_world();
    // Sand cell sits above bedrock at y=1; water above at y=2.
    w.set_cell(3, 1, Cell::solid(MaterialId::Sand));
    w.set_cell(3, 2, Cell::water());

    // One pass: as much as fits transfers into the sand up to its
    // porosity capacity.
    apply_gravity_fall(&mut w);
    let sand = w.get_cell(3, 1).unwrap();
    let above = w.get_cell(3, 2).unwrap();
    let sand_cap = water_capacity(MaterialId::Sand);
    assert_eq!(sand.sat.0, sand_cap);
    assert_eq!(above.sat.0, u8::MAX - sand_cap);

    // A second pass: sand is at capacity → no more water moves in.
    apply_gravity_fall(&mut w);
    let sand2 = w.get_cell(3, 1).unwrap();
    let above2 = w.get_cell(3, 2).unwrap();
    assert_eq!(sand2.sat.0, sand_cap);
    assert_eq!(above2.sat.0, u8::MAX - sand_cap);
}

#[test]
fn does_not_leak_through_stone() {
    // Stone porosity is small but > 0. Ensure the pass never over-fills
    // and no water disappears.
    let mut w = World::new(2);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..(CHUNK_CELLS_W as i32) {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::water());
    let cap = water_capacity(MaterialId::Stone);
    let start_mass: i32 =
        w.get_cell(5, 2).unwrap().sat.0 as i32 + w.get_cell(5, 1).unwrap().sat.0 as i32;

    apply_gravity_fall(&mut w);

    let stone = w.get_cell(5, 1).unwrap();
    let above = w.get_cell(5, 2).unwrap();
    assert_eq!(stone.sat.0, cap);
    assert_eq!(above.sat.0 as i32 + stone.sat.0 as i32, start_mass);
}

#[test]
fn droplet_falls_across_chunk_boundary() {
    // Chunk (0, 1) at y=64..127; chunk (0, 0) at y=0..63.
    // Drop a water cell at gy=64 (bottom row of chunk (0,1)),
    // expect it in gy=63 (top row of chunk (0,0)) after one pass.
    let mut w = World::new(3);
    // Instantiate both chunks so `get_cell` returns Some for both
    // sides of the seam.
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    w.set_cell(7, 64, Cell::water());
    assert!(w.get_cell(7, 64).unwrap().sat.is_full());
    assert!(w.get_cell(7, 63).unwrap().sat.is_empty());

    apply_gravity_fall(&mut w);

    assert!(w.get_cell(7, 64).unwrap().sat.is_empty());
    assert!(w.get_cell(7, 63).unwrap().sat.is_full());
}

#[test]
fn droplet_falls_one_step_across_even_to_odd_seam() {
    // Even-cy above odd-cy (cy=2 → cy=1): pull + checkerboard must
    // still move exactly one cell, not double-step.
    let mut w = World::new(3);
    w.ensure_chunk(ChunkCoord::new(0, 1));
    w.ensure_chunk(ChunkCoord::new(0, 2));
    let seam = 2 * CHUNK_CELLS_H as i32; // 128
    w.set_cell(7, seam, Cell::water());
    apply_gravity_fall(&mut w);
    assert!(w.get_cell(7, seam).unwrap().sat.is_empty());
    assert!(w.get_cell(7, seam - 1).unwrap().sat.is_full());
    assert!(
        w.get_cell(7, seam - 2).unwrap().sat.is_empty(),
        "must not fall two cells in one checkerboard rule"
    );
}

#[test]
fn missing_below_chunk_stops_fall() {
    // Chunk (0, 0) exists; chunk (0, -1) does not. A water cell
    // at gy=0 (bottom of chunk 0,0) has no below chunk — it must
    // stay put rather than pour into the void.
    let mut w = World::new(4);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.set_cell(1, 0, Cell::water());
    apply_gravity_fall(&mut w);
    assert!(w.get_cell(1, 0).unwrap().sat.is_full());
    // Below chunk still doesn't exist.
    assert_eq!(w.get_cell(1, -1), None);
}

// ------------ lateral spill ------------

fn setup_air_row(width: i32) -> World {
    // Bedrock floor at y=0, everything above y=0 is Air, in one
    // 64-wide chunk. `width` is how many columns to make bedrock.
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..width {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w
}

fn total_sat(world: &World, xs: std::ops::Range<i32>, y: i32) -> i64 {
    xs.map(|x| world.get_cell(x, y).map(|c| c.sat.0 as i64).unwrap_or(0))
        .sum()
}

#[test]
fn spill_equalizes_isolated_pair() {
    // Bedrock walls (cap 0) so the water cell only has one Air
    // neighbour — isolates a single pair. Stone is porous and
    // would participate in seepage, not in spill.
    let mut w = setup_air_row(64);
    w.set_cell(9, 5, Cell::solid(MaterialId::Bedrock));
    w.set_cell(10, 5, Cell::water());
    // (11, 5) starts as default Air with sat 0.
    let start_mass = w.get_cell(10, 5).unwrap().sat.0 as i32;

    apply_lateral_spill(&mut w);

    let l = w.get_cell(10, 5).unwrap().sat.0 as i32;
    let r = w.get_cell(11, 5).unwrap().sat.0 as i32;
    // Head equalisation on equal-cap Air ≡ half the sat gap.
    assert_eq!(l, 255 - 127);
    assert_eq!(r, 127);
    assert_eq!(l + r, start_mass, "mass conserved");
}

#[test]
fn spill_is_symmetric_across_a_single_pass() {
    // Water at gx=10 with dry air on both sides. Rule must feed
    // both neighbours equally — the pair is symmetric.
    let mut w = setup_air_row(64);
    w.set_cell(10, 5, Cell::water());
    apply_lateral_spill(&mut w);
    let left = w.get_cell(9, 5).unwrap().sat.0;
    let right = w.get_cell(11, 5).unwrap().sat.0;
    assert_eq!(left, right, "L/R must be equal");
    assert!(left > 0);
    // Mass conserved across the three cells.
    let total = w.get_cell(9, 5).unwrap().sat.0 as i32
        + w.get_cell(10, 5).unwrap().sat.0 as i32
        + w.get_cell(11, 5).unwrap().sat.0 as i32;
    assert_eq!(total, 255);
}

#[test]
fn spill_conserves_mass_over_a_long_chain() {
    let mut w = setup_air_row(64);
    // Puddle at columns 20..25 (5 water cells), rest dry.
    for x in 20..25 {
        w.set_cell(x, 3, Cell::water());
    }
    let start_mass = total_sat(&w, 0..64, 3);
    for _ in 0..30 {
        apply_lateral_spill(&mut w);
    }
    let end_mass = total_sat(&w, 0..64, 3);
    assert_eq!(start_mass, end_mass, "mass must be preserved");
}

#[test]
fn spill_stops_at_a_solid_wall() {
    let mut w = setup_air_row(64);
    // Impermeable Bedrock wall — spill is Air–Air only.
    w.set_cell(5, 5, Cell::solid(MaterialId::Bedrock));
    w.set_cell(4, 5, Cell::water());
    apply_lateral_spill(&mut w);
    assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Bedrock);
    assert_eq!(w.get_cell(5, 5).unwrap().sat.0, 0);
    assert_eq!(w.get_cell(3, 5).unwrap().sat.0, 127);
    assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 255 - 127);
}

#[test]
fn spill_propagates_one_cell_per_tick() {
    // Full-water cell at x=32 in an otherwise dry row. After N
    // ticks, non-zero sat should reach at least x=32-N and x=32+N.
    let mut w = setup_air_row(64);
    w.set_cell(32, 3, Cell::water());

    for tick_i in 1..=4 {
        apply_lateral_spill(&mut w);
        // Both sides should have some water by tick_i.
        let left = w.get_cell(32 - tick_i, 3).unwrap();
        let right = w.get_cell(32 + tick_i, 3).unwrap();
        assert!(
            left.sat.0 > 0,
            "tick={tick_i} left cell x={} should have water",
            32 - tick_i
        );
        assert!(
            right.sat.0 > 0,
            "tick={tick_i} right cell x={} should have water",
            32 + tick_i
        );
        // The frontier is exactly `tick_i` cells: no water yet
        // one further out.
        if 32 - tick_i - 1 >= 0 {
            assert_eq!(
                w.get_cell(32 - tick_i - 1, 3).unwrap().sat.0,
                0,
                "tick={tick_i} frontier at x={}",
                32 - tick_i - 1
            );
        }
        if 32 + tick_i + 1 < 64 {
            assert_eq!(
                w.get_cell(32 + tick_i + 1, 3).unwrap().sat.0,
                0,
                "tick={tick_i} frontier at x={}",
                32 + tick_i + 1
            );
        }
    }
}

// ------------ seepage ------------

#[test]
fn seepage_wets_adjacent_sand_from_air_water() {
    let mut w = World::new(42);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..8 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Dry sand beside a full water cell.
    w.set_cell(3, 1, Cell::water());
    w.set_cell(4, 1, Cell::solid(MaterialId::Sand));
    let before = w.get_cell(3, 1).unwrap().sat.0 as i32
        + w.get_cell(4, 1).unwrap().sat.0 as i32;
    apply_seepage(&mut w);
    let sand = w.get_cell(4, 1).unwrap();
    let air = w.get_cell(3, 1).unwrap();
    assert!(sand.sat.0 > 0, "sand should take on pore water");
    assert!(air.sat.0 < 255, "air should lose sat to the sand");
    assert_eq!(
        air.sat.0 as i32 + sand.sat.0 as i32,
        before,
        "mass conserved"
    );
    // Rate-limited: one tick can't dump the whole lake into sand.
    let rate = seepage_rate_with(MaterialId::Sand, &HydroOverrides::default());
    assert!(sand.sat.0 as i32 <= rate);
}

#[test]
fn seepage_skips_impermeable_bedrock() {
    let mut w = World::new(43);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.set_cell(2, 1, Cell::water());
    w.set_cell(3, 1, Cell::solid(MaterialId::Bedrock));
    apply_seepage(&mut w);
    assert_eq!(w.get_cell(2, 1).unwrap().sat.0, 255);
    assert_eq!(w.get_cell(3, 1).unwrap().sat.0, 0);
}

#[test]
fn seepage_prefers_lower_head() {
    // Two sand cells on bedrock: left full pores, right dry.
    let mut w = World::new(44);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..16 {
        for y in 0..4 {
            w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
        }
    }
    let cap = water_capacity(MaterialId::Sand);
    w.set_cell(5, 2, Cell {
        material: MaterialId::Sand,
        sat: Sat(cap),
        ..Cell::default()
    });
    w.set_cell(6, 2, Cell::solid(MaterialId::Sand));
    apply_seepage(&mut w);
    let l = w.get_cell(5, 2).unwrap().sat.0;
    let r = w.get_cell(6, 2).unwrap().sat.0;
    assert!(r > 0);
    assert!(l < cap);
    assert_eq!(l as i32 + r as i32, cap as i32);
}


#[test]
fn saturated_stone_weeps_into_side_air() {
    // Cliff face: full stone column beside open Air.
    let mut w = World::new(45);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..8 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let cap = water_capacity(MaterialId::Stone);
    for y in 1..=4 {
        w.set_cell(
            3,
            y,
            Cell {
                material: MaterialId::Stone,
                sat: Sat(cap),
                ..Cell::default()
            },
        );
        w.set_cell(4, y, Cell::air()); // open cliff face
    }
    let stone_before: i32 = (1..=4)
        .map(|y| w.get_cell(3, y).unwrap().sat.0 as i32)
        .sum();
    for _ in 0..30 {
        apply_seepage(&mut w);
        apply_gravity_fall(&mut w);
    }
    let stone_after: i32 = (1..=4)
        .map(|y| w.get_cell(3, y).unwrap().sat.0 as i32)
        .sum();
    assert!(
        stone_after < stone_before,
        "stone pores must weep into the cliff (before={stone_before} after={stone_after})"
    );
}

#[test]
fn throughflow_exits_side_face_before_deep_toe() {
    // Terrace pool over saturated stone with an open cliff mid-face.
    // Pressed surface water should spring out the side, not only the toe.
    let mut w = World::new(46);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..10 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let cap = water_capacity(MaterialId::Stone);
    // Saturated stone pillar at x=3, y=1..4; cliff Air at x=4, y=2..4.
    // y=1 stays stone beside bedrock so the only mid opening is the side.
    for y in 1..=4 {
        w.set_cell(
            3,
            y,
            Cell {
                material: MaterialId::Stone,
                sat: Sat(cap),
                ..Cell::default()
            },
        );
    }
    // Seal the toe: stone continues under the cliff so bottom exit
    // is blocked; only side Air at (4,3) is open.
    w.set_cell(
        4,
        1,
        Cell {
            material: MaterialId::Stone,
            sat: Sat(cap),
            ..Cell::default()
        },
    );
    w.set_cell(4, 2, Cell::air());
    w.set_cell(4, 3, Cell::air());
    w.set_cell(4, 4, Cell::air());
    // Surface pool pressing on the pillar.
    w.set_cell(3, 5, Cell::water());
    let side_before = w.get_cell(4, 3).unwrap().sat.0
        + w.get_cell(4, 2).unwrap().sat.0
        + w.get_cell(4, 4).unwrap().sat.0;
    apply_water_flow(&mut w);
    let side_after = w.get_cell(4, 3).unwrap().sat.0
        + w.get_cell(4, 2).unwrap().sat.0
        + w.get_cell(4, 4).unwrap().sat.0;
    assert!(
        side_after > side_before,
        "throughflow must vent into the cliff face (before={side_before} after={side_after})"
    );
    // Pool should have lost some sat to the spring.
    assert!(w.get_cell(3, 5).unwrap().sat.0 < 255);
}

#[test]
fn hydraulic_head_ranks_full_air_above_dry_sand() {
    let ha = hydraulic_head(10, Sat::FULL, water_capacity(MaterialId::Air));
    let hs = hydraulic_head(10, Sat::EMPTY, water_capacity(MaterialId::Sand));
    assert!(ha > hs);
}

#[test]
fn world_hydro_porosity_caps_seepage_into_sand() {
    // Default sand soaks a neighbouring full water cell. Zero porosity
    // via World.hydro must block that soak with no install step.
    let mut w = setup_column_world();
    w.set_cell(4, 1, Cell::solid(MaterialId::Sand));
    w.set_cell(5, 1, Cell::water());
    apply_seepage(&mut w);
    let soaked = w.get_cell(4, 1).unwrap().sat.0;
    assert!(soaked > 0, "default sand should take pore water");

    let mut sealed = setup_column_world();
    sealed.hydro.set_porosity(MaterialId::Sand, 0);
    sealed.hydro.set_permeability(MaterialId::Sand, 0);
    sealed.set_cell(4, 1, Cell::solid(MaterialId::Sand));
    sealed.set_cell(5, 1, Cell::water());
    apply_seepage(&mut sealed);
    assert_eq!(
        sealed.get_cell(4, 1).unwrap().sat.0,
        0,
        "zero-porosity sand via World.hydro must not take pore water"
    );
    assert_eq!(sealed.water_capacity(MaterialId::Sand), 0);
}

#[test]
fn spill_crosses_chunk_boundary() {
    // Full water cell at gx=63 in chunk (0, 0); empty air at
    // gx=64 in chunk (1, 0). Stone wall at gx=62 so only the
    // cross-boundary pair contributes.
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(1, 0));
    for x in 0..128 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(62, 5, Cell::solid(MaterialId::Bedrock));
    w.set_cell(63, 5, Cell::water());
    apply_lateral_spill(&mut w);
    assert_eq!(w.get_cell(63, 5).unwrap().sat.0, 255 - 127);
    assert_eq!(w.get_cell(64, 5).unwrap().sat.0, 127);
    // Mass conserved across the boundary.
    assert_eq!(
        w.get_cell(63, 5).unwrap().sat.0 as i32
            + w.get_cell(64, 5).unwrap().sat.0 as i32,
        255
    );
}

#[test]
fn gravity_only_drains_droplet_over_passes() {
    // Verified without lateral spill so we can assert exact
    // per-tick positions of a single droplet.
    let mut w = setup_column_world();
    w.set_cell(6, 5, Cell::water());

    apply_gravity_fall(&mut w);
    assert!(w.get_cell(6, 4).unwrap().sat.is_full());
    assert!(w.get_cell(6, 5).unwrap().sat.is_empty());
    apply_gravity_fall(&mut w);
    assert!(w.get_cell(6, 3).unwrap().sat.is_full());
    apply_gravity_fall(&mut w);
    apply_gravity_fall(&mut w);
    assert!(
        w.get_cell(6, 1).unwrap().sat.is_full(),
        "water should be resting on bedrock"
    );
    apply_gravity_fall(&mut w);
    assert!(w.get_cell(6, 1).unwrap().sat.is_full());
    assert!(w.get_cell(6, 0).unwrap().sat.is_empty()); // bedrock sat stays 0
}

// ------------ grain fall ------------

#[test]
fn ice_falls_through_empty_air_but_floats_on_water() {
    let mut w = setup_column_world();
    w.set_cell(3, 4, Cell::solid(MaterialId::Ice));
    apply_grain_fall(&mut w);
    assert_eq!(w.get_cell(3, 3).unwrap().material, MaterialId::Ice);
    assert_eq!(w.get_cell(3, 4).unwrap().material, MaterialId::Air);

    // Float on standing water — lake lids must not sink.
    let mut w2 = setup_column_world();
    w2.set_cell(3, 1, Cell::water());
    w2.set_cell(3, 2, Cell::solid(MaterialId::Ice));
    apply_grain_fall(&mut w2);
    assert_eq!(w2.get_cell(3, 2).unwrap().material, MaterialId::Ice);
    assert_eq!(w2.get_cell(3, 1).unwrap().material, MaterialId::Air);
    assert!(w2.get_cell(3, 1).unwrap().sat.is_full());

    // Haze is not a float seat — drop through (closes the ice-pump dead-band).
    let mut w3 = setup_column_world();
    w3.set_cell(
        3,
        1,
        Cell {
            material: MaterialId::Air,
            sat: Sat(128),
            flags: Default::default(),
            _pad: 0,
        },
    );
    w3.set_cell(3, 2, Cell::solid(MaterialId::Ice));
    apply_grain_fall(&mut w3);
    assert_eq!(
        w3.get_cell(3, 1).unwrap().material,
        MaterialId::Ice,
        "ice must fall through partial-sat haze"
    );
    assert_eq!(w3.get_cell(3, 2).unwrap().material, MaterialId::Air);
    assert_eq!(w3.get_cell(3, 2).unwrap().sat.0, 128);
}

#[test]
fn hanging_snow_and_ice_settle_onto_bedrock() {
    let mut w = setup_column_world();
    w.set_cell(2, 6, Cell::solid(MaterialId::Snow));
    w.set_cell(3, 6, Cell::solid(MaterialId::Ice));
    for _ in 0..10 {
        apply_grain_fall(&mut w);
    }
    assert_eq!(w.get_cell(2, 1).unwrap().material, MaterialId::Snow);
    assert_eq!(w.get_cell(3, 1).unwrap().material, MaterialId::Ice);
}

#[test]
fn grain_falls_through_empty_air() {
    let mut w = setup_column_world();
    // Sand at y=5, everything below is empty Air, bedrock at y=0.
    w.set_cell(4, 5, Cell::solid(MaterialId::Sand));
    apply_grain_fall(&mut w);
    assert_eq!(
        w.get_cell(4, 4).map(|c| c.material),
        Some(MaterialId::Sand)
    );
    assert_eq!(
        w.get_cell(4, 5).map(|c| c.material),
        Some(MaterialId::Air)
    );
}

#[test]
fn grain_stops_on_competent_rock() {
    let mut w = setup_column_world();
    w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
    w.set_cell(4, 3, Cell::solid(MaterialId::Sand));
    apply_grain_fall(&mut w);
    // Below Stone is not Air → no swap.
    assert_eq!(w.get_cell(4, 3).unwrap().material, MaterialId::Sand);
    assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Stone);
}

#[test]
fn grain_stops_on_another_grain() {
    let mut w = setup_column_world();
    w.set_cell(4, 1, Cell::solid(MaterialId::Sand));
    w.set_cell(4, 2, Cell::solid(MaterialId::Gravel));
    apply_grain_fall(&mut w);
    // y=1 is Sand (not Air); Gravel at y=2 has nowhere to swap.
    // Sand at y=1 has bedrock at y=0 (not Air), also stays.
    assert_eq!(w.get_cell(4, 1).unwrap().material, MaterialId::Sand);
    assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Gravel);
}

#[test]
fn grain_sinks_through_water_swap_conserves_mass() {
    // Water column at y=1..=4 (all Air with sat=full); sand at
    // y=5. After one grain pass, sand moves to y=4 and the water
    // that was at y=4 rises into y=5.
    let mut w = setup_column_world();
    for y in 1..=4 {
        w.set_cell(4, y, Cell::water());
    }
    w.set_cell(4, 5, Cell::solid(MaterialId::Sand));
    let start_water: i32 = (1..=5)
        .map(|y| w.get_cell(4, y).unwrap().sat.0 as i32)
        .sum();

    apply_grain_fall(&mut w);

    let end_water: i32 = (1..=5)
        .map(|y| w.get_cell(4, y).unwrap().sat.0 as i32)
        .sum();
    assert_eq!(end_water, start_water, "water sat is conserved by swap");
    assert_eq!(w.get_cell(4, 4).unwrap().material, MaterialId::Sand);
    // Sand carries its own sat (0) up... wait, the Air cell's
    // water rises. The newly-vacated cell at y=5 receives the
    // sat that was in the old below-cell (y=4 water full).
    assert_eq!(w.get_cell(4, 5).unwrap().material, MaterialId::Air);
    assert!(w.get_cell(4, 5).unwrap().sat.is_full());
}

#[test]
fn grain_falls_across_chunk_boundary() {
    // Sand at gy=64 (chunk (0,1) local (7,0)); Air at gy=63
    // (chunk (0,0) local (7,63)). Sand should end at gy=63.
    let mut w = World::new(5);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    w.set_cell(7, 64, Cell::solid(MaterialId::Sand));
    assert_eq!(
        w.get_cell(7, 64).unwrap().material,
        MaterialId::Sand
    );
    apply_grain_fall(&mut w);
    assert_eq!(
        w.get_cell(7, 63).unwrap().material,
        MaterialId::Sand,
        "grain should have crossed the seam"
    );
    assert_eq!(
        w.get_cell(7, 64).unwrap().material,
        MaterialId::Air,
        "vacated cell above must be Air"
    );
}

#[test]
fn grain_falls_one_cell_per_pass_through_empty_column() {
    // Multi-pass check that grain fall obeys the 1 cell / pass rule.
    let mut w = setup_column_world();
    w.set_cell(20, 10, Cell::solid(MaterialId::Sand));
    for expected in (1..=9).rev() {
        apply_grain_fall(&mut w);
        assert_eq!(
            w.get_cell(20, expected).map(|c| c.material),
            Some(MaterialId::Sand),
            "grain should be at y={expected}"
        );
        assert_eq!(
            w.get_cell(20, expected + 1).map(|c| c.material),
            Some(MaterialId::Air)
        );
    }
    // One more pass: bedrock below at y=0, no swap.
    apply_grain_fall(&mut w);
    assert_eq!(
        w.get_cell(20, 1).unwrap().material,
        MaterialId::Sand
    );
}

// ------------ grain repose ------------

#[test]
fn sand_cliff_slides_diagonally() {
    let mut w = setup_column_world();
    // Pillar on bedrock: top sand has empty diagonal-down seats.
    w.set_cell(5, 1, Cell::solid(MaterialId::Sand));
    w.set_cell(5, 2, Cell::solid(MaterialId::Sand));
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::Air,
        "sand must not hold a 1-cell cliff"
    );
    let left = w.get_cell(4, 1).map(|c| c.material) == Some(MaterialId::Sand);
    let right = w.get_cell(6, 1).map(|c| c.material) == Some(MaterialId::Sand);
    assert!(left || right, "sand slides into a diagonal-down seat");
}

#[test]
fn organic_litter_slides_diagonally() {
    // Dead-leaf towers used to fall straight down only — no repose.
    let mut w = setup_column_world();
    w.set_cell(5, 1, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 2, Cell::solid(MaterialId::Organic));
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::Air,
        "Organic must not stack as a 1-cell cliff"
    );
    let left = w.get_cell(4, 1).map(|c| c.material) == Some(MaterialId::Organic);
    let right = w.get_cell(6, 1).map(|c| c.material) == Some(MaterialId::Organic);
    assert!(left || right, "Organic litter should sprawl diagonally");
}

#[test]
fn organic_cliff_slides_into_humid_air_film() {
    // Long-soak bug: Organic was routed through the ice-seat check, so
    // humid/film Air next to a litter cliff froze the face in place.
    // Fall + repose matches the real tick (mid slide can leave an overhang).
    let mut w = setup_column_world();
    w.set_cell(5, 1, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 2, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 3, Cell::solid(MaterialId::Organic));
    for x in [4, 6] {
        for y in 1..=3 {
            let mut haze = Cell::air();
            haze.sat = Sat(24);
            w.set_cell(x, y, haze);
        }
    }
    for _ in 0..12 {
        apply_grain_fall(&mut w);
        apply_grain_repose(&mut w);
    }
    assert_eq!(
        w.get_cell(5, 3).unwrap().material,
        MaterialId::Air,
        "Organic tip must not persist as a humid cliff"
    );
    let height = (1..=3)
        .filter(|&y| w.get_cell(5, y).map(|c| c.material) == Some(MaterialId::Organic))
        .count();
    assert!(
        height <= 1,
        "Organic column should sprawl under haze (height={height})"
    );
}

#[test]
fn soil_cliff_slides_into_humid_air_film() {
    let mut w = setup_column_world();
    w.set_cell(5, 1, Cell::solid(MaterialId::Soil));
    w.set_cell(5, 2, Cell::solid(MaterialId::Soil));
    w.set_cell(5, 3, Cell::solid(MaterialId::Soil));
    for x in [4, 6] {
        for y in 1..=3 {
            let mut haze = Cell::air();
            haze.sat = Sat(24);
            w.set_cell(x, y, haze);
        }
    }
    for _ in 0..12 {
        apply_grain_fall(&mut w);
        apply_grain_repose(&mut w);
    }
    assert_eq!(
        w.get_cell(5, 3).unwrap().material,
        MaterialId::Air,
        "Soil tip must not persist as a humid cliff"
    );
}

#[test]
fn organic_does_not_repose_into_standing_water() {
    let mut w = setup_column_world();
    w.set_cell(5, 1, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 2, Cell::solid(MaterialId::Organic));
    for x in [4, 6] {
        for y in 1..=2 {
            w.set_cell(x, y, Cell::water());
        }
    }
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::Organic,
        "Organic must float on full water, not slide into the lake"
    );
}

#[test]
fn painted_organic_platform_falls_under_tick() {
    // Wide F3 brush stroke high in empty sky — must not hang as a floating island.
    let mut w = setup_column_world();
    for x in 2..=12 {
        for y in 20..=22 {
            w.set_cell(x, y, Cell::solid(MaterialId::Organic));
        }
    }
    // One tick must seat the platform (multi-pass grain settle).
    tick(&mut w);
    let floating = (2..=12)
        .flat_map(|x| (15..=22).map(move |y| (x, y)))
        .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
        .count();
    assert_eq!(
        floating, 0,
        "organic platform must leave the sky ({floating} cells left high)"
    );
    let seated = (1..=14)
        .flat_map(|x| (1..=6).map(move |y| (x, y)))
        .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
        .count();
    assert!(
        seated >= 20,
        "most litter should seat near bed (seated={seated})"
    );
}

#[test]
fn painted_midair_organic_falls_under_tick() {
    // F3 terrain paint marks dirty, but water substeps clear it and write
    // nothing — grain fall used to see an empty plan and leave litter
    // floating until shear/erosion re-dirtied the column.
    let mut w = setup_column_world();
    w.set_cell(5, 8, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 9, Cell::solid(MaterialId::Organic));
    tick(&mut w);
    assert_eq!(
        w.get_cell(5, 9).unwrap().material,
        MaterialId::Air,
        "mid-air Organic tip must fall under tick gravity"
    );
    assert_eq!(
        w.get_cell(5, 8).unwrap().material,
        MaterialId::Air,
        "mid-air Organic must leave the paint height"
    );
    let on_bed = (1..=3).any(|y| {
        w.get_cell(5, y).map(|c| c.material) == Some(MaterialId::Organic)
            || w.get_cell(4, y).map(|c| c.material) == Some(MaterialId::Organic)
            || w.get_cell(6, y).map(|c| c.material) == Some(MaterialId::Organic)
    });
    assert!(on_bed, "Organic should seat near bedrock (fall + repose)");
}

#[test]
fn settle_loose_grains_drops_organic_without_full_tick() {
    // After F3 unpause, settle (via tick or directly) must drop litter
    // that was painted mid-air while the editor held the sim paused.
    let mut w = setup_column_world();
    w.set_cell(5, 40, Cell::solid(MaterialId::Organic));
    settle_loose_grains(&mut w, None, GRAIN_SETTLE_PASSES);
    assert_eq!(
        w.get_cell(5, 40).unwrap().material,
        MaterialId::Air,
        "settle_loose_grains must drop mid-air Organic"
    );
    assert_eq!(
        w.get_cell(5, 1).unwrap().material,
        MaterialId::Organic,
        "Organic should seat on bedrock"
    );
}

#[test]
fn underwater_sand_repose_does_not_leave_dry_air() {
    // Sand on an underwater ledge slides into an empty pocket beside
    // standing water. Vacated cell must become water (not sky-flash Air).
    let mut w = setup_column_world();
    for x in 3..=7 {
        for y in 1..=3 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Sand));
    // Empty bubble seat diagonal-down from the sand.
    w.set_cell(4, 1, Cell::air());
    let sat_before: u32 = (0..16)
        .flat_map(|x| (0..8).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y))
        .map(|c| c.sat.0 as u32)
        .sum();
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(4, 1).unwrap().material,
        MaterialId::Sand,
        "sand should occupy the underwater seat"
    );
    let vacated = w.get_cell(5, 2).unwrap();
    assert_eq!(vacated.material, MaterialId::Air);
    assert!(
        vacated.sat.0 >= 200,
        "vacated underwater cell must be standing water, not dry/film air (sat={})",
        vacated.sat.0
    );
    let sat_after: u32 = (0..16)
        .flat_map(|x| (0..8).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y))
        .map(|c| c.sat.0 as u32)
        .sum();
    assert_eq!(
        sat_after, sat_before,
        "underwater repose must steal neighbour water, not mint sat"
    );
}

#[test]
fn repose_does_not_slide_sand_into_standing_water() {
    // Supported sand with only full-water diagonal seats must stay put.
    // Vertical sink through water is grain-fall's job, not repose.
    let mut w = setup_column_world();
    for x in 3..=7 {
        for y in 1..=4 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Sand));
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::Sand,
        "sand must not swim sideways into the lake via repose"
    );
    assert_ne!(w.get_cell(4, 1).unwrap().material, MaterialId::Sand);
    assert_ne!(w.get_cell(6, 1).unwrap().material, MaterialId::Sand);
}

#[test]
fn repose_does_not_slide_sand_into_wet_film() {
    // Shoreline film (sat 1..199) was the remaining fleck cycle: sand
    // slid in, stole lake water, flow refilled film, repeat.
    let mut w = setup_column_world();
    for x in 3..=7 {
        for y in 1..=3 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Sand));
    let mut film = Cell::air();
    film.sat = Sat(80);
    w.set_cell(4, 1, film);
    w.set_cell(6, 1, film);
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::Sand,
        "sand must not avalanche into waterline film"
    );
    assert_ne!(w.get_cell(4, 1).unwrap().material, MaterialId::Sand);
    assert_ne!(w.get_cell(6, 1).unwrap().material, MaterialId::Sand);
}

#[test]
fn underwater_sand_mound_reaches_quiescence() {
    // Submerged sand mound under a water column should stop rearranging.
    let mut w = setup_column_world();
    for x in 2..=10 {
        for y in 1..=6 {
            w.set_cell(x, y, Cell::water());
        }
    }
    // Small mound on bedrock.
    for x in 4..=7 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
    }
    w.set_cell(5, 2, Cell::solid(MaterialId::Sand));
    w.set_cell(6, 2, Cell::solid(MaterialId::Sand));
    w.set_cell(5, 3, Cell::solid(MaterialId::Sand));
    let fingerprint = |w: &World| -> Vec<(i32, i32)> {
        let mut cells = Vec::new();
        for x in 0..16 {
            for y in 0..10 {
                if w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand) {
                    cells.push((x, y));
                }
            }
        }
        cells
    };
    // Settle for a while, then assert two windows match.
    for _ in 0..40 {
        apply_grain_fall(&mut w);
        apply_grain_repose(&mut w);
        apply_gravity_fall(&mut w);
    }
    let a = fingerprint(&w);
    for _ in 0..20 {
        apply_grain_fall(&mut w);
        apply_grain_repose(&mut w);
        apply_gravity_fall(&mut w);
    }
    let b = fingerprint(&w);
    assert_eq!(
        a, b,
        "underwater sand mound must reach a resting configuration"
    );
    // No dry Air bubbles trapped beside sand below the water surface.
    for x in 2..=10 {
        for y in 1..=5 {
            let Some(c) = w.get_cell(x, y) else { continue };
            if c.material != MaterialId::Air {
                continue;
            }
            let next_to_sand = [(-1, 0), (1, 0), (0, -1), (0, 1)].iter().any(|&(dx, dy)| {
                w.get_cell(x + dx, y + dy).map(|n| n.material) == Some(MaterialId::Sand)
            });
            if next_to_sand {
                assert!(
                    c.sat.0 >= 200,
                    "dry/film bubble at ({x},{y}) next to sand — cycling residue"
                );
            }
        }
    }
}

#[test]
fn loose_rock_holds_single_step() {
    let mut w = setup_column_world();
    // Neighbor floor at same height → drop of 1; LooseRock max_step≥1 holds.
    w.set_cell(4, 1, Cell::solid(MaterialId::LooseRock));
    w.set_cell(5, 1, Cell::solid(MaterialId::LooseRock));
    w.set_cell(5, 2, Cell::solid(MaterialId::LooseRock));
    // Empty diagonal seat would be (4,1) or (6,1); (4,1) is occupied.
    // Open (6,1): drop from y=2 is 1 Air cell onto bedrock — not > 1.
    apply_grain_repose(&mut w);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::LooseRock,
        "loose rock can hold a short stair (repose step ≥ 1)"
    );
}

#[test]
fn snow_avalanches_off_cliff_but_not_into_water() {
    let mut w = setup_column_world();
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Snow));
    apply_grain_repose(&mut w);
    assert_eq!(w.get_cell(5, 2).unwrap().material, MaterialId::Air);
    let left = w.get_cell(4, 1).map(|c| c.material) == Some(MaterialId::Snow);
    let right = w.get_cell(6, 1).map(|c| c.material) == Some(MaterialId::Snow);
    assert!(left || right, "snow avalanches into empty diagonal-down air");

    // Snow next to a water seat must stay (float / slush).
    let mut w2 = setup_column_world();
    w2.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w2.set_cell(5, 2, Cell::solid(MaterialId::Snow));
    w2.set_cell(4, 1, Cell::water());
    w2.set_cell(6, 1, Cell::water());
    apply_grain_repose(&mut w2);
    assert_eq!(
        w2.get_cell(5, 2).unwrap().material,
        MaterialId::Snow,
        "snow must not slide into standing water"
    );
}

fn cold_field(temp_c: f32) -> crate::temperature::Temperature {
    let mut t = crate::temperature::Temperature::with_world_bounds(
        4, 0, 0, 64, 64, 1, 64, 32, false,
    );
    t.config.base_temp_c = temp_c;
    for v in t.cells.values_mut() {
        *v = temp_c;
    }
    t
}

#[test]
fn cold_snow_spills_onto_wet_film_on_ice() {
    let mut w = setup_column_world();
    // Lake ice under a thin wet film; snow on the shore. Wall the
    // open side so snow cannot diagonal-escape into empty Air.
    w.set_cell(4, 1, Cell::solid(MaterialId::Ice));
    w.set_cell(4, 2, Cell::water()); // wet film on ice
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Snow));
    w.set_cell(6, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(6, 2, Cell::solid(MaterialId::Stone));
    let temp = cold_field(-6.0);
    apply_cold_avalanche(&mut w, &temp, 0.0);
    assert_eq!(
        w.get_cell(4, 2).unwrap().material,
        MaterialId::Snow,
        "cold avalanche may seat snow on a wet film over ice"
    );
    assert_eq!(w.get_cell(5, 2).unwrap().material, MaterialId::Air);
}

#[test]
fn cold_snow_still_refuses_open_water() {
    let mut w = setup_column_world();
    // Water on both diagonal seats — no empty-Air escape.
    w.set_cell(4, 1, Cell::water());
    w.set_cell(6, 1, Cell::water());
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Snow));
    let temp = cold_field(-8.0);
    apply_cold_avalanche(&mut w, &temp, 0.0);
    assert_eq!(
        w.get_cell(5, 2).unwrap().material,
        MaterialId::Snow,
        "open water (no ice) must still refuse snow"
    );
}

#[test]
fn cold_wet_sand_smears_onto_ice_lid() {
    let mut w = setup_column_world();
    // Ice ledge; wet sand beside the empty seat above it. Wall +x.
    w.set_cell(4, 1, Cell::solid(MaterialId::Ice));
    w.set_cell(4, 2, Cell::air());
    let mut sand = Cell::solid(MaterialId::Sand);
    sand.sat = Sat(80);
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, sand);
    w.set_cell(6, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(6, 2, Cell::solid(MaterialId::Stone));
    let temp = cold_field(-4.0);
    apply_cold_avalanche(&mut w, &temp, 0.0);
    assert_eq!(
        w.get_cell(4, 2).unwrap().material,
        MaterialId::Sand,
        "cold wet sand should smear onto the ice lid"
    );
}

#[test]
fn hillside_ice_slides_in_cold_avalanche() {
    let mut w = setup_column_world();
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 2, Cell::solid(MaterialId::Ice)); // glaze on rock
    let temp = cold_field(-10.0);
    apply_cold_avalanche(&mut w, &temp, 0.0);
    assert_eq!(w.get_cell(5, 2).unwrap().material, MaterialId::Air);
    let left = w.get_cell(4, 1).map(|c| c.material) == Some(MaterialId::Ice);
    let right = w.get_cell(6, 1).map(|c| c.material) == Some(MaterialId::Ice);
    assert!(left || right, "hillside ice peels into a diagonal seat");
}

#[test]
fn sand_pile_flattens_over_ticks() {
    let mut w = setup_column_world();
    // Steep sand column on bedrock.
    for y in 1..=6 {
        w.set_cell(8, y, Cell::solid(MaterialId::Sand));
    }
    for _ in 0..40 {
        apply_grain_fall(&mut w);
        apply_grain_repose(&mut w);
    }
    let mut max_h = 0;
    for x in 5..12 {
        for y in (1..=8).rev() {
            if w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand) {
                max_h = max_h.max(y);
                break;
            }
        }
    }
    assert!(
        max_h <= 3,
        "sand tower should flatten under repose (max_h={max_h})"
    );
}

#[test]
fn live_roots_bind_sand_repose() {
    use std::collections::HashSet;
    use super::grain::ROOT_REPOSE_STEP_BONUS;

    let mut bare = setup_column_world();
    let mut rooted = setup_column_world();
    for y in 1..=5 {
        bare.set_cell(8, y, Cell::solid(MaterialId::Sand));
        rooted.set_cell(8, y, Cell::solid(MaterialId::Sand));
    }
    let mut roots = HashSet::new();
    for y in 1..=5 {
        roots.insert((8, y));
    }
    assert!(ROOT_REPOSE_STEP_BONUS >= 1);

    for _ in 0..40 {
        apply_grain_fall(&mut bare);
        apply_grain_repose(&mut bare);
        apply_grain_fall(&mut rooted);
        apply_grain_repose_bound(&mut rooted, Some(&roots));
    }

    let height = |w: &World| {
        (1..=8)
            .rev()
            .find(|&y| w.get_cell(8, y).map(|c| c.material) == Some(MaterialId::Sand))
            .unwrap_or(0)
    };
    let h_bare = height(&bare);
    let h_rooted = height(&rooted);
    assert!(
        h_rooted > h_bare,
        "rooted column should hold a taller pile (bare={h_bare} rooted={h_rooted})"
    );
}

// ------------ flow erosion ------------

fn cascade_shelf_world(bed: MaterialId) -> World {
    // Sand/gravel shelf (x=3..6 at y=1) with water on top and a
    // cascade lip into empty Air at x=7 — classic bedload setup.
    let mut w = setup_column_world();
    for x in 3..=6 {
        w.set_cell(x, 1, Cell::solid(bed));
        w.set_cell(x, 2, Cell::water());
    }
    // Lip: empty column at x=7 so water has cascade bias +x.
    w.set_cell(7, 1, Cell::air());
    w.set_cell(7, 2, Cell::air());
    w
}

#[test]
fn flowing_water_scours_sand_bed_downhill() {
    let mut w = cascade_shelf_world(MaterialId::Sand);
    let cfg = GrainConfig {
        erosion_rate: 1.0, // force picks under flow bias
        max_events_per_tick: 32,
        ..GrainConfig::default()
    };
    let sand_before = (3..=8)
        .filter(|&x| w.get_cell(x, 1).map(|c| c.material) == Some(MaterialId::Sand))
        .count();
    for t in 0..30 {
        w.tick = t;
        apply_flow_erosion(&mut w, &cfg);
        apply_grain_fall(&mut w);
        apply_grain_repose(&mut w);
    }
    let sand_at_lip = w.get_cell(7, 1).map(|c| c.material) == Some(MaterialId::Sand)
        || w.get_cell(8, 1).map(|c| c.material) == Some(MaterialId::Sand);
    let bed_hole = (3..=6).any(|x| {
        w.get_cell(x, 1).map(|c| c.material) != Some(MaterialId::Sand)
    });
    assert!(
        sand_at_lip || bed_hole,
        "cascade should move sand off the shelf (before={sand_before}, lip={sand_at_lip}, hole={bed_hole})"
    );
}

#[test]
fn still_lake_does_not_erode_sand_bed() {
    let mut w = setup_column_world();
    // Closed basin: sand floor, water, stone walls — no cascade.
    for x in 4..=8 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(x, 2, Cell::water());
    }
    w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
    w.set_cell(9, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(9, 2, Cell::solid(MaterialId::Stone));
    let cfg = GrainConfig {
        erosion_rate: 1.0,
        ..GrainConfig::default()
    };
    for t in 0..40 {
        w.tick = t;
        apply_flow_erosion(&mut w, &cfg);
    }
    for x in 4..=8 {
        assert_eq!(
            w.get_cell(x, 1).unwrap().material,
            MaterialId::Sand,
            "still lake must not scour bed at x={x}"
        );
    }
}

#[test]
fn deep_still_shore_does_not_erode_sand_bed() {
    // Sand shelf into deep still water (both columns full). Previously
    // `flow_bias` treated sat-full Air below a neighbour as a cascade
    // lip and scoured forever. Wall the open sides so only the
    // deep-water interface remains.
    let mut w = setup_column_world();
    for x in 0..12 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=3 {
        w.set_cell(1, y, Cell::solid(MaterialId::Stone));
        w.set_cell(11, y, Cell::solid(MaterialId::Stone));
    }
    for x in 2..=5 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(x, 2, Cell::water());
        w.set_cell(x, 3, Cell::water());
    }
    for x in 6..=10 {
        for y in 1..=3 {
            w.set_cell(x, y, Cell::water());
        }
    }
    let cfg = GrainConfig {
        erosion_rate: 1.0,
        max_events_per_tick: 32,
        ..GrainConfig::default()
    };
    for t in 0..50 {
        w.tick = t;
        apply_flow_erosion(&mut w, &cfg);
    }
    for x in 2..=5 {
        assert_eq!(
            w.get_cell(x, 1).unwrap().material,
            MaterialId::Sand,
            "deep still shore must not scour sand at x={x}"
        );
    }
}

#[test]
fn flat_lake_near_cascade_lip_settles_surface() {
    // Upper lake drains into a catch basin. Once the basin fills,
    // the free surface must stop thrashing (old cascade-pull dumped
    // all sat and fought equalize → endless 1-cell spikes).
    let mut w = setup_column_world();
    for x in 0..14 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
    }
    for x in 1..=10 {
        w.set_cell(x, 2, Cell::water());
        w.set_cell(x, 3, Cell::water());
    }
    w.set_cell(0, 2, Cell::solid(MaterialId::Stone));
    w.set_cell(0, 3, Cell::solid(MaterialId::Stone));
    // Catch column: room at y=3 over a filling y=2 cell.
    w.set_cell(11, 2, Cell::air());
    w.set_cell(11, 3, Cell::air());
    w.set_cell(12, 2, Cell::solid(MaterialId::Stone));
    w.set_cell(12, 3, Cell::solid(MaterialId::Stone));

    for _ in 0..120 {
        tick(&mut w);
    }
    // Catch should be full (or nearly) — no ongoing drain.
    let catch = w.get_cell(11, 2).unwrap().sat.0;
    assert!(
        catch >= 200,
        "catch basin should fill so the lake can quiesce (sat={catch})"
    );
    let fingerprint = |w: &World| -> Vec<u8> {
        (1..=11)
            .flat_map(|x| {
                (2..=3).map(move |y| w.get_cell(x, y).map(|c| c.sat.0).unwrap_or(0))
            })
            .collect()
    };
    let a = fingerprint(&w);
    for _ in 0..30 {
        tick(&mut w);
    }
    let b = fingerprint(&w);
    assert_eq!(
        a, b,
        "lake surface must stop rearranging after the catch fills"
    );
}

#[test]
fn ice_bank_is_not_flow_eroded() {
    let mut w = cascade_shelf_world(MaterialId::Ice);
    // Put ice as the bank next to cascading water.
    w.set_cell(6, 2, Cell::solid(MaterialId::Ice));
    w.set_cell(6, 1, Cell::solid(MaterialId::Ice));
    let cfg = GrainConfig {
        erosion_rate: 1.0,
        ..GrainConfig::default()
    };
    for t in 0..20 {
        w.tick = t;
        apply_flow_erosion(&mut w, &cfg);
    }
    assert_eq!(w.get_cell(6, 2).unwrap().material, MaterialId::Ice);
    assert_eq!(w.get_cell(6, 1).unwrap().material, MaterialId::Ice);
}

#[test]
fn flow_erosion_conserves_free_water_sat() {
    // Underwater deposit used to overwrite Air+sat with dry sand and
    // mint Cell::water() at the scour hole — net lake water vanished.
    let mut w = cascade_shelf_world(MaterialId::Sand);
    // Deepen the water column so a deposit seat can be wet Air.
    for x in 3..=6 {
        w.set_cell(x, 3, Cell::water());
    }
    let sat_before: u32 = (0..16)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y))
        .map(|c| c.sat.0 as u32)
        .sum();
    let cfg = GrainConfig {
        erosion_rate: 1.0,
        max_events_per_tick: 32,
        ..GrainConfig::default()
    };
    for t in 0..40 {
        w.tick = t;
        apply_flow_erosion(&mut w, &cfg);
        apply_grain_fall(&mut w);
        apply_gravity_fall(&mut w);
    }
    let sat_after: u32 = (0..16)
        .flat_map(|x| (0..16).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y))
        .map(|c| c.sat.0 as u32)
        .sum();
    assert_eq!(
        sat_after, sat_before,
        "erosion/deposit must conserve free+pore water sat (before={sat_before} after={sat_after})"
    );
}

// ------------ rain ------------

fn setup_sky_row(y: i32) -> World {
    // Chunk with bedrock floor so climatic rain lands on the surface
    // (y=1), scanning down from the sky row.
    let mut w = World::new(11);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    assert!((0..CHUNK_CELLS_H as i32).contains(&y));
    for x in 0..CHUNK_CELLS_W as i32 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w
}

#[test]
fn rain_is_deterministic_for_seed_and_tick() {
    let mut a = setup_sky_row(30);
    let mut b = setup_sky_row(30);
    let cfg = RainConfig {
        top_y: 30,
        x_range: (0, 63),
        prob_per_col_per_tick: 0.5,
        droplet_sat: 32,
        seed_salt: 0xF00,
        closed_loop: false, // unit fixture: open faucet
        ..RainConfig::default()
    };
    apply_rain(&mut a, &cfg);
    apply_rain(&mut b, &cfg);
    for x in 0..64 {
        assert_eq!(
            a.get_cell(x, 1).map(|c| c.sat.0),
            b.get_cell(x, 1).map(|c| c.sat.0),
            "identical worlds must produce identical rain (x={x})"
        );
    }
}

#[test]
fn rain_respects_x_range() {
    let mut w = setup_sky_row(30);
    let cfg = RainConfig {
        top_y: 30,
        x_range: (5, 20),
        prob_per_col_per_tick: 1.0, // always
        droplet_sat: 40,
        seed_salt: 1,
        closed_loop: false,
        ..RainConfig::default()
    };
    apply_rain(&mut w, &cfg);
    for x in 0..64 {
        let sat = w.get_cell(x, 1).unwrap().sat.0;
        let sky = w.get_cell(x, 30).unwrap().sat.0;
        assert_eq!(sky, 0, "rain must not hang in the sky at x={x}");
        if (5..=20).contains(&x) {
            assert!(sat > 0, "x={x} in range should have rain on the ground");
        } else {
            assert_eq!(sat, 0, "x={x} outside range should stay dry");
        }
    }
}

#[test]
fn rain_droplet_saturates_at_full() {
    let mut w = setup_sky_row(30);
    w.set_cell(3, 1, Cell::water()); // surface already full
    let cfg = RainConfig {
        top_y: 30,
        x_range: (3, 3),
        prob_per_col_per_tick: 1.0,
        droplet_sat: 40,
        seed_salt: 2,
        closed_loop: false,
        ..RainConfig::default()
    };
    apply_rain(&mut w, &cfg);
    // Full film on bare rock does not stack into a wedge.
    assert_eq!(w.get_cell(3, 1).unwrap().sat.0, u8::MAX);
    assert_eq!(w.get_cell(3, 2).unwrap().sat.0, 0);
}

#[test]
fn rain_skips_non_air_cells() {
    let mut w = setup_sky_row(30);
    // Buried column of stone — no free air above a solid landing.
    for y in 1..=30 {
        w.set_cell(10, y, Cell::solid(MaterialId::Stone));
    }
    let cfg = RainConfig {
        top_y: 30,
        x_range: (10, 10),
        prob_per_col_per_tick: 1.0,
        droplet_sat: 40,
        seed_salt: 3,
        closed_loop: false,
        ..RainConfig::default()
    };
    apply_rain(&mut w, &cfg);
    assert_eq!(w.get_cell(10, 30).unwrap().material, MaterialId::Stone);
    assert_eq!(w.get_cell(10, 30).unwrap().sat.0, 0);
}

#[test]
fn closed_loop_rain_drains_humidity_and_does_not_mint() {
    let mut w = setup_sky_row(30);
    let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 32);
    // One column of vapor paying for the droplet.
    h.add(7, 30, 80.0);
    let hum_before = h.total_mass();
    let ground_before = w.get_cell(7, 1).unwrap().sat.0 as f32;
    let cfg = RainConfig {
        top_y: 30,
        x_range: (7, 7),
        prob_per_col_per_tick: 1.0,
        droplet_sat: 40,
        seed_salt: 9,
        closed_loop: true,
        sea_level_y: 1,
        max_flood_above_sea: 12,
        ..RainConfig::default()
    };
    apply_rain_with_temp(&mut w, &cfg, None, None, Some(&mut h));
    let ground_after = w.get_cell(7, 1).unwrap().sat.0 as f32;
    let landed = ground_after - ground_before;
    assert!(landed > 0.0, "closed-loop rain should land when humidity pays");
    let drained = hum_before - h.total_mass();
    assert!(
        (drained - landed).abs() < 1.5,
        "humidity drain must match landed mass (landed={landed}, drained={drained})"
    );
}

#[test]
fn closed_loop_rain_without_humidity_mints_nothing() {
    let mut w = setup_sky_row(30);
    let cfg = RainConfig {
        top_y: 30,
        x_range: (0, 63),
        prob_per_col_per_tick: 1.0,
        droplet_sat: 64,
        seed_salt: 11,
        closed_loop: true,
        ..RainConfig::default()
    };
    apply_rain(&mut w, &cfg);
    for x in 0..64 {
        assert_eq!(
            w.get_cell(x, 1).unwrap().sat.0,
            0,
            "closed loop with no atmosphere must not mint at x={x}"
        );
    }
}

#[test]
fn flood_guard_blocks_climatic_rain_on_tall_water() {
    let mut w = setup_sky_row(30);
    // Stack standing water well above sea level.
    for y in 1..=20 {
        w.set_cell(4, y, Cell::water());
    }
    let mut h = crate::humidity::Humidity::with_world_bounds(4, 0, 0, 64, 32);
    h.add(4, 30, 500.0);
    let cfg = RainConfig {
        top_y: 30,
        x_range: (4, 4),
        prob_per_col_per_tick: 1.0,
        droplet_sat: 64,
        seed_salt: 13,
        closed_loop: true,
        sea_level_y: 4,
        max_flood_above_sea: 8, // surface at y=20 >> 4+8
        ..RainConfig::default()
    };
    let before = w.get_cell(4, 21).map(|c| c.sat.0).unwrap_or(0);
    apply_rain_with_temp(&mut w, &cfg, None, None, Some(&mut h));
    let after = w.get_cell(4, 21).map(|c| c.sat.0).unwrap_or(0);
    assert_eq!(before, after, "flood guard must refuse further deepening");
    assert_eq!(h.total_mass(), 500.0, "guard should not drain humidity");
}

#[test]
fn surface_flow_drains_hill_film_diagonally() {
    let mut w = World::new(7);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..8 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(3, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(3, 2, Cell::solid(MaterialId::Stone));
    w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(4, 2, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(4, 3, Cell::water());
    apply_water_flow(&mut w);
    assert!(
        w.get_cell(4, 3).unwrap().sat.0 < u8::MAX,
        "hill film should drain by head"
    );
    assert!(
        w.get_cell(5, 2).unwrap().sat.0 > 0,
        "water should move diagonally downhill"
    );
}

#[test]
fn surface_flow_levels_diagonal_slope_wedge() {
    // Packed staircase wedge — the "gaffa tape" failure mode.
    // Head equalisation across diagonals must flatten it downhill.
    let mut w = World::new(21);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..20 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Rising slope solid under a diagonal water wedge.
    for x in 2..10 {
        for y in 1..=(x - 1) {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
        w.set_cell(x, x, Cell::water());
    }
    let high_before: i32 = (6..10)
        .map(|x| w.get_cell(x, x).unwrap().sat.0 as i32)
        .sum();
    assert!(high_before > 500);
    for _ in 0..40 {
        tick(&mut w);
    }
    let high_after: i32 = (6..10)
        .map(|x| w.get_cell(x, x).map(|c| c.sat.0 as i32).unwrap_or(0))
        .sum();
    assert!(
        high_after < high_before / 4,
        "wedge crest should empty (before={high_before} after={high_after})"
    );
    let pool: i32 = (0..6)
        .flat_map(|x| (1..=4).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    assert!(pool > 400, "water should pool at the foot (got {pool})");
}

#[test]
fn beach_film_drains_into_ocean_not_inland() {
    let mut w = World::new(19);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..20 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 0..6 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(x, 2, Cell::water());
    }
    for x in 6..12 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
        w.set_cell(x, 2, Cell::solid(MaterialId::Sand));
    }
    for x in 8..12 {
        w.set_cell(x, 3, Cell::solid(MaterialId::Sand));
    }
    w.set_cell(6, 3, Cell::water());
    for _ in 0..8 {
        tick(&mut w);
    }
    assert_eq!(
        w.get_cell(6, 3).unwrap().sat.0,
        0,
        "beach film should leave the sand"
    );
    assert_eq!(
        w.get_cell(7, 3).unwrap().sat.0,
        0,
        "must not climb inland up the beach"
    );
    // Film may sit one cell seaward or soak into sand — either is
    // fine; the failure mode was climbing inland.
    let inland_high: i32 = (8..12)
        .map(|x| w.get_cell(x, 4).map(|c| c.sat.0 as i32).unwrap_or(0))
        .sum();
    assert_eq!(inland_high, 0, "no water above the inland sand terrace");
}

// NOTE: A previous test forced physics into a quiescent state via
// `clear_all_dirty` and expected water to still drain. That was
// testing the retired `remount_unbalanced_surface_water` bandaid.
// In practice, physics only quiesces when no cell has moved for a
// full tick; any new write (rain, cloud downpour, editor spawn)
// rebuilds the dirty rect and re-wakes flow. The artificial
// clear-then-idle case is intentionally not supported.

#[test]
fn beach_slope_rain_does_not_perch_on_shelves() {
    // A monotonic sand slope descending to open Air on the left.
    // Rain deposits water at every column. Every wet cell has sand
    // below and diagonal-down sand — old flow trapped water as
    // staircase perched pools. Diagonal throughflow lets it seep
    // down the slope until it reaches open Air / ocean.
    let mut w = World::new(31);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..30 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Slope rises 1 cell per column from x=8..=22 (crest at 22, y=15).
    // Left of x=8 is open Air over bedrock (the "sea").
    for x in 8..=22 {
        let top = x - 7; // 1..=15
        for y in 1..=top {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    for x in 23..30 {
        for y in 1..=15 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    // Saturate all sand so throughflow is the only drain path.
    let cap_sand = crate::cell::water_capacity(MaterialId::Sand);
    for x in 8..30 {
        for y in 1..=15 {
            if let Some(c) = w.get_cell(x, y) {
                if c.material == MaterialId::Sand {
                    w.set_cell(x, y, Cell { sat: Sat(cap_sand), ..c });
                }
            }
        }
    }
    // Rain deposit: full sat on every shelf surface cell along the slope.
    for x in 8..=22 {
        let top = x - 7;
        w.set_cell(x, top + 1, Cell::water());
    }
    for _ in 0..80 {
        tick(&mut w);
    }
    // Water on the slope should be nearly gone.
    let mut perched = 0;
    for x in 8..=22 {
        for y in 1..=15 {
            let Some(c) = w.get_cell(x, y) else { continue };
            if c.material == MaterialId::Air && c.sat.0 >= 128 {
                perched += 1;
            }
        }
    }
    // Sea pool at x=0..=7, y=1..=3 should have caught the drained mass.
    let sea: i32 = (0..=7)
        .flat_map(|x| (1..=3).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    assert!(
        perched <= 3,
        "slope should drain (perched={perched}, sea_sat={sea})"
    );
    assert!(sea > 500, "sea should catch drainage (sea_sat={sea})");
}

#[test]
fn user_scenario_water_equilibrates_across_flat_shelf_and_cascades() {
    // The user's mental model:
    // - Rain drops water on an Air cell above an impermeable block.
    // - Immediate neighbours also sit above impermeable → water
    //   spreads (averaged) across them.
    // - One neighbour is an "air-above-air" cascade edge → water
    //   there falls, opening space for more sideways flow.
    //
    // World layout (y-up):
    //   x:  8 9 10 11 12 13
    //   y=51 . . .  .  .  .   (Air, dry)
    //   y=50 . . W  .  .  .   (Air; W = rain drop)
    //   y=49 # # #  #  .  .   (Bedrock shelf ending at 11; 12+ is Air)
    //   y=48 . . .  .  .  .   (Air below the shelf edge)
    //   y=0  # # #  #  #  #   (Bedrock floor)
    let mut w = World::new(2);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..32 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 8..=11 {
        w.set_cell(x, 49, Cell::solid(MaterialId::Bedrock));
    }
    // Rain drop at (10, 50).
    w.set_cell(10, 50, Cell::water());

    // One tick's water flow should already start cascading right
    // (x=11,50 has Air below at y=49? no, x=11,49 is bedrock. So
    // cascade edge is at x=12,50 whose below x=12,49 is Air).
    // Water at x=10,50 first goes right one cell per substep, then
    // falls off the shelf.
    for _ in 0..20 {
        tick(&mut w);
    }

    // No water should have climbed left onto more bedrock shelf.
    assert!(
        w.get_cell(10, 50).unwrap().sat.0 < 8,
        "source cell must nearly empty (got sat={})",
        w.get_cell(10, 50).unwrap().sat.0
    );
    // Water should have cascaded off the shelf. Some sits on the
    // shelf (equilibrated across x=6..12), some falls into the
    // right chasm at x=12+. Whichever way it goes, it must not
    // climb inland uphill (there's no uphill to climb here — just
    // the flat bedrock shelf x=8..=11 and the left/right chasms).
    let landed_right: i32 = (12..32)
        .flat_map(|x| (0..=48).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    let landed_left: i32 = (0..8)
        .flat_map(|x| (0..=48).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    assert!(
        landed_right + landed_left >= 150,
        "water should cascade off the shelf edge (right={landed_right} left={landed_left})"
    );
}

#[test]
fn rain_on_descending_shore_cascades_into_ocean_pool() {
    // Mimics the visible image: shore descends left, ocean at bottom
    // left. Rain drops water at shelf-top cells. Water must cascade
    // diagonally down the shore into the ocean pool in a few ticks —
    // not accumulate as terraced puddles.
    let mut w = World::new(4);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(1, 0));
    for x in 0..100 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Slope: shore top rises from y=1 at x=5 to y=15 at x=20 (rise=1/col).
    for x in 5..=20 {
        let top = x - 4; // 1..=16
        for y in 1..=top {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    // High plateau for x=21..40 (top=17).
    for x in 21..40 {
        for y in 1..=17 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    // Ocean pool below sea level at x=0..4 (deep).
    for x in 0..5 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    // Rain falls at three shelf-top cells along the slope.
    w.set_cell(10, 7, Cell::water());   // shelf top at x=10 is y=6
    w.set_cell(15, 12, Cell::water());  // shelf top at x=15 is y=11
    w.set_cell(20, 17, Cell::water());  // shelf top at x=20 is y=16

    // Run enough ticks for cascade to reach ocean.
    for _ in 0..30 {
        tick(&mut w);
    }

    // No water should remain on the sand shelves at the deposit
    // heights (perched pool test).
    let perched_10 = w.get_cell(10, 7).unwrap().sat.0;
    let perched_15 = w.get_cell(15, 12).unwrap().sat.0;
    let perched_20 = w.get_cell(20, 17).unwrap().sat.0;
    assert!(
        perched_10 < 32 && perched_15 < 32 && perched_20 < 32,
        "shelf cells should drain (got {perched_10}, {perched_15}, {perched_20})"
    );

    // Ocean gained some, sand absorbed some (seepage), and total
    // mass is conserved.
    let mut total = 0i64;
    for x in 0..40 {
        for y in 0..30 {
            if let Some(c) = w.get_cell(x, y) {
                total += c.sat.0 as i64;
            }
        }
    }
    // Baseline sand had 0 sat; ocean had 5*5*255=6375; rain added 3*255=765.
    // Sand can absorb up to 15*15*180 ≈ 40k, so mass may sit in sand.
    assert!(total >= 6375 + 765 - 50, "mass roughly conserved (total={total})");
}

#[test]
fn continuous_rain_on_flat_shelf_drains_via_cascade_edge() {
    // Flat sand shelf 8 cells wide. Left of shelf is a cliff (Air).
    // Right of shelf is inland (more sand higher).
    // Rain sat=8 falls on every shelf cell every tick.
    // With cascade at the left edge, water shouldn't stack up.
    let mut w = World::new(5);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..40 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Cliff: at x=0..=9 shore top is Y=1 (Air above). At x=10..=17
    // shore top is y=10 (flat shelf 8 cells wide). At x=18+ higher.
    for x in 10..=17 {
        for y in 1..=10 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    for x in 18..30 {
        for y in 1..=15 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }

    // Track max sat seen on the shelf over 40 ticks of rain.
    let mut max_shelf_sat: u8 = 0;
    for _t in 0..40 {
        // Rain deposit: 8 sat on each shelf cell (10..=17) at y=11.
        for x in 10..=17 {
            let cell = w.get_cell(x, 11).unwrap();
            if cell.material == MaterialId::Air {
                let new_sat = (cell.sat.0 as i32 + 8).min(255) as u8;
                w.set_cell(x, 11, Cell { sat: Sat(new_sat), ..cell });
            }
        }
        tick(&mut w);
        for x in 10..=17 {
            let c = w.get_cell(x, 11).unwrap();
            if c.sat.0 > max_shelf_sat {
                max_shelf_sat = c.sat.0;
            }
        }
    }
    // At steady state, water on the shelf should be low because
    // cascade at x=10 dumps it off the cliff on each substep.
    assert!(
        max_shelf_sat < 200,
        "shelf water should drain via cascade edge (max seen: {max_shelf_sat})"
    );
}

#[test]
fn continuous_rain_on_stepped_shore_does_not_pool_on_shelves() {
    // Realistic shore: descending in 2-cell-wide steps (like a
    // staircase). Rain falls on every shelf. Water must cascade
    // down step by step, not accumulate as terrace pools.
    //
    // Terrain (top view of tops):
    //   x=  8 9 10 11 12 13 14 15 16 17
    //   top y=1 1  3  3  5  5  7  7  9  9
    //
    // Ocean at x < 8. Each shelf is 2 cells wide, drop of 2y.
    let mut w = World::new(6);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..30 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let tops = [(8, 1), (9, 1), (10, 3), (11, 3), (12, 5), (13, 5), (14, 7), (15, 7), (16, 9), (17, 9)];
    for &(x, top) in &tops {
        for y in 1..=top {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    // Ocean pool at x=0..=7, up to y=1.
    for x in 0..=7 {
        w.set_cell(x, 1, Cell::water());
    }

    let mut max_shelf: u8 = 0;
    for _t in 0..40 {
        // Rain 6 sat per tick on each shelf-top-air cell.
        for &(x, top) in &tops {
            let y = top + 1;
            let cell = w.get_cell(x, y).unwrap();
            if cell.material == MaterialId::Air {
                let new_sat = (cell.sat.0 as i32 + 6).min(255) as u8;
                w.set_cell(x, y, Cell { sat: Sat(new_sat), ..cell });
            }
        }
        tick(&mut w);
        for &(x, top) in &tops {
            let y = top + 1;
            let c = w.get_cell(x, y).unwrap();
            if c.sat.0 > max_shelf {
                max_shelf = c.sat.0;
            }
        }
    }
    // Steady-state shelf sat should stay low.
    assert!(
        max_shelf < 128,
        "stepped-shore shelves should keep draining (max shelf sat: {max_shelf})"
    );
}


#[test]
fn same_y_equalize_flattens_stepped_lake_surface() {
    // Free-surface terrace inside a closed basin (solid shores):
    //
    //   y=3: # W W . . . . #
    //   y=2: # W W W W W W #
    //   y=1: # # # # # # # #
    //
    // Same-Y equalise should spread the step across the row so the
    // surface is no longer a hard cliff of full cells.
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..10 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
    }
    // Basin walls.
    w.set_cell(0, 2, Cell::solid(MaterialId::Bedrock));
    w.set_cell(0, 3, Cell::solid(MaterialId::Bedrock));
    w.set_cell(9, 2, Cell::solid(MaterialId::Bedrock));
    w.set_cell(9, 3, Cell::solid(MaterialId::Bedrock));
    for x in 1..9 {
        w.set_cell(x, 2, Cell::water());
    }
    w.set_cell(2, 3, Cell::water());
    w.set_cell(3, 3, Cell::water());

    for _ in 0..30 {
        tick(&mut w);
    }

    let row: Vec<u8> = (1..9).map(|x| w.get_cell(x, 3).unwrap().sat.0).collect();
    let max = *row.iter().max().unwrap();
    let min = *row.iter().min().unwrap();
    let sum: i32 = row.iter().map(|&s| s as i32).sum();
    assert_eq!(sum, 510, "mass on the free surface must be conserved: {row:?}");
    assert!(
        max < 220,
        "terrace must thin out across the lake (row={row:?})"
    );
    assert!(
        min > 20,
        "dry gaps on the free surface must fill in (row={row:?})"
    );
    assert!(
        (max as i32) - (min as i32) < 80,
        "same-Y surface should be close to level (row={row:?})"
    );
}

#[test]
fn flow_every_other_substep_does_not_stall_packed_surface_equalize() {
    // Packed free-surface terrace — gravity is a no-op, so the old
    // odd-only every-other path cleared dirty on step 0, skipped
    // flow, then broke on empty plan at step 1. Even-step flow fixes it.
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..10 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(0, 2, Cell::solid(MaterialId::Bedrock));
    w.set_cell(0, 3, Cell::solid(MaterialId::Bedrock));
    w.set_cell(9, 2, Cell::solid(MaterialId::Bedrock));
    w.set_cell(9, 3, Cell::solid(MaterialId::Bedrock));
    for x in 1..9 {
        w.set_cell(x, 2, Cell::water());
    }
    w.set_cell(2, 3, Cell::water());
    w.set_cell(3, 3, Cell::water());

    let perf = PerfConfig {
        flow_every_other_substep: true,
        parallel_physics: false,
        ..PerfConfig::default()
    };
    let fail = crate::failure::FailureConfig {
        enable_roof_collapse: false,
        enable_shear_weaken: false,
        enable_compaction: false,
        ..crate::failure::FailureConfig::default()
    };
    for _ in 0..60 {
        tick_with_configs(&mut w, &perf, &fail);
    }
    let row: Vec<u8> = (1..9).map(|x| w.get_cell(x, 3).unwrap().sat.0).collect();
    let max = *row.iter().max().unwrap();
    let wet_cols = row.iter().filter(|&&s| s > 20).count();
    assert!(
        max < 240,
        "every-other must thin the packed terrace, not stall at FULL (row={row:?})"
    );
    assert!(
        wet_cols >= 4,
        "every-other must spread surface water across the basin (row={row:?})"
    );
}

#[test]
fn solid_staircase_film_drains_left_into_lower_pool() {
    // Geometry from the user's first image (impermeable sand):
    //
    //   y=3:  .  .  .  w  .     <- thin film on higher step (THE STUCK PIXEL)
    //   y=2:  d  P  P  #  .     <- drop | pool2(2) | pool3(3)? wait
    //
    // Simpler staircase matching the description:
    //   y=3: . . . W .
    //   y=2: D # P P #
    //   y=1: # # # # #
    //   y=0: ###########
    //
    // W at (3,3) on sand (3,2). Diagonal-down left is sand (2,2).
    // Same-row left (2,3) is Air above sand (2,2) — corner cell.
    // From (2,3), diagonal-down left (1,2) is pool Air P.
    // Drop D at (0,2) is lower basin.
    //
    // Expected: W → (2,3) → dump into P → eventually into D.
    // Must NOT sit for 100 ticks.
    let mut w = World::new(11);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..16 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        w.set_cell(x, 1, Cell::solid(MaterialId::Bedrock));
    }
    // Sand step face + upper terrace.
    w.set_cell(2, 2, Cell::solid(MaterialId::Bedrock)); // step face
    w.set_cell(3, 2, Cell::solid(MaterialId::Bedrock)); // under W
    w.set_cell(4, 2, Cell::solid(MaterialId::Bedrock));
    // Lower terrace floor under pool/drop.
    w.set_cell(0, 1, Cell::solid(MaterialId::Bedrock));
    w.set_cell(1, 1, Cell::solid(MaterialId::Bedrock));
    // (1,2) and (0,2) are Air — the lower pool/drop level.
    // Seed a little water in the pool so it's "occupied" like the image.
    w.set_cell(1, 2, Cell { material: MaterialId::Air, sat: Sat(200), flags: Default::default(), _pad: 0 });
    // The stuck higher film.
    w.set_cell(3, 3, Cell::water());

    for _ in 0..30 {
        tick(&mut w);
    }

    let stuck = w.get_cell(3, 3).unwrap().sat.0;
    let corner = w.get_cell(2, 3).unwrap().sat.0;
    let pool = w.get_cell(1, 2).unwrap().sat.0;
    let drop = w.get_cell(0, 2).unwrap().sat.0;
    assert!(
        stuck < 8,
        "higher-step film must drain (stuck={stuck} corner={corner} pool={pool} drop={drop})"
    );
    assert!(
        (pool as i32) + (drop as i32) >= 200,
        "water must reach lower level (pool={pool} drop={drop})"
    );
}

#[test]
fn impermeable_shore_cascades_off_within_seconds() {
    // Simulates user's setup: sand set to impermeable (no seepage
    // or throughflow). Uses Bedrock terrain to model this without
    // touching global overrides (which race with other tests).
    //
    // Shore descends left, has a 6-cell flat plateau at top, then
    // rises again. Rain hits every cell along the shore surface.
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..60 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 8..=13 {
        for y in 1..=(x - 7) {
            w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
        }
    }
    for x in 14..=19 {
        for y in 1..=6 {
            w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
        }
    }
    for x in 20..=25 {
        let top = 6 + (x - 19);
        for y in 1..=top {
            w.set_cell(x, y, Cell::solid(MaterialId::Bedrock));
        }
    }
    let surface_cells: Vec<(i32, i32)> = (8..=25)
        .map(|x| {
            let mut top_y = 0;
            for y in 1..30 {
                if let Some(c) = w.get_cell(x, y) {
                    if c.material == MaterialId::Bedrock {
                        top_y = y;
                    }
                }
            }
            (x, top_y + 1)
        })
        .collect();

    let mut max_plateau: u8 = 0;
    for _t in 0..60 {
        for &(x, y) in &surface_cells {
            let cell = w.get_cell(x, y).unwrap();
            if cell.material == MaterialId::Air {
                let new_sat = (cell.sat.0 as i32 + 5).min(255) as u8;
                w.set_cell(x, y, Cell { sat: Sat(new_sat), ..cell });
            }
        }
        tick(&mut w);
        // Only assert plateau cells drain. Beach edge pools by design.
        for x in 14..=19 {
            let c = w.get_cell(x, 7).unwrap();
            if c.sat.0 > max_plateau {
                max_plateau = c.sat.0;
            }
        }
    }
    assert!(
        max_plateau < 128,
        "impermeable plateau must keep draining (max sat: {max_plateau})"
    );
}

#[test]
fn tick_drains_hill_mound_instead_of_stalling() {
    let mut w = World::new(11);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..16 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 0..8 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
        w.set_cell(x, 2, Cell::solid(MaterialId::Stone));
    }
    for x in 8..16 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
    }
    for y in 3..=6 {
        for x in 3..6 {
            w.set_cell(x, y, Cell::water());
        }
    }
    let mass_high = |w: &World| -> i32 {
        (3..6)
            .flat_map(|x| (3..=6).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
            .sum()
    };
    let before = mass_high(&w);
    assert!(before > 1000);
    for _ in 0..12 {
        tick(&mut w);
    }
    let after = mass_high(&w);
    assert!(
        after < before / 2,
        "mound should mostly leave the high step (before={before} after={after})"
    );
    let low: i32 = (8..16)
        .flat_map(|x| (1..=5).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    assert!(low > 200, "water should pool on the lower step (got {low})");
}

#[test]
fn lone_ridge_pixel_drains_via_throughflow_or_evap() {
    // A single wet Air on a sand crest with sand on both flanks
    // (no Air neighbours). Historic sticky-water case: gravity +
    // seepage stopped at sand porosity, leaving the pixel forever.
    let mut w = World::new(13);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..12 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Sand pyramid, crest at (6, 3).
    for x in 3..10 {
        for y in 1..=3 {
            if (x as i32 - 6).abs() <= (3 - y) {
                w.set_cell(x, y, Cell::solid(MaterialId::Sand));
            }
        }
    }
    // Saturate the pyramid sand fully so gravity + seepage would stop.
    let cap_sand = crate::cell::water_capacity(MaterialId::Sand);
    for x in 3..10 {
        for y in 1..=3 {
            if let Some(c) = w.get_cell(x, y) {
                if c.material == MaterialId::Sand {
                    w.set_cell(x, y, Cell {
                        sat: Sat(cap_sand),
                        ..c
                    });
                }
            }
        }
    }
    w.set_cell(6, 4, Cell::water()); // lone wet Air on crest
    let cfg = EvapConfig {
        rate_per_tick: 1,
        dry_above_max: 200,
        period_ticks: 1,
    };
    for _ in 0..200 {
        tick(&mut w);
        apply_evaporation(&mut w, &cfg);
    }
    let stuck = w.get_cell(6, 4).unwrap().sat.0;
    assert!(
        stuck < 8,
        "lone ridge pixel should drain (throughflow + orphan evap), got sat={stuck}"
    );
}

#[test]
fn surface_flow_moves_single_sat_droplet_off_ridge() {
    // Force-1 trickle: sat=1 with drier Air neighbours must move —
    // the head equalizer's floor truncated 0.5 to zero and left
    // droplets stuck. Prefer downhill; mass is preserved.
    let mut w = World::new(3);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..8 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let mut c = Cell::air();
    c.sat = Sat(1);
    w.set_cell(4, 3, c);
    apply_water_flow(&mut w);
    let src = w.get_cell(4, 3).unwrap().sat.0 as i32;
    let mass: i32 = (0..8)
        .flat_map(|x| (1..=4).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i32))
        .sum();
    assert_eq!(src, 0, "lone droplet must leave the source cell");
    assert_eq!(mass, 1, "mass must be preserved (got {mass})");
}

// ------------ evaporation ------------

#[test]
fn evap_removes_from_surface_water_only() {
    // Water column at gy=1..=5, dry air above. Only the topmost
    // wet cell (gy=5) has dry Air above and should lose sat.
    let mut w = setup_column_world();
    for y in 1..=5 {
        w.set_cell(4, y, Cell::water());
    }
    let cfg = EvapConfig::default();
    apply_evaporation(&mut w, &cfg);
    for y in 1..=4 {
        assert!(
            w.get_cell(4, y).unwrap().sat.is_full(),
            "sub-surface cell y={y} should not evaporate"
        );
    }
    // Top wet cell lost a tiny bit.
    let top = w.get_cell(4, 5).unwrap().sat.0;
    assert!(top < u8::MAX);
    assert!(top >= u8::MAX - cfg.rate_per_tick);
}

#[test]
fn evap_drains_a_droplet_to_zero_over_time() {
    // Surface film on bedrock should tick down to zero over many passes.
    let mut w = setup_column_world();
    let mut c = Cell::air();
    c.sat = Sat(20);
    w.set_cell(4, 1, c);
    let cfg = EvapConfig {
        rate_per_tick: 5,
        dry_above_max: 200,
        period_ticks: 1,
    };
    for _ in 0..10 {
        apply_evaporation(&mut w, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }
    assert_eq!(w.get_cell(4, 1).unwrap().sat.0, 0);
}

#[test]
fn evap_skips_airborne_rain_droplets() {
    // Wet Air with empty sky below is falling rain — must not
    // re-evaporate before gravity can land it.
    let mut w = setup_column_world();
    let mut c = Cell::air();
    c.sat = Sat(80);
    w.set_cell(4, 10, c);
    let cfg = EvapConfig::default();
    apply_evaporation(&mut w, &cfg);
    assert_eq!(
        w.get_cell(4, 10).unwrap().sat.0,
        80,
        "airborne rain must survive evaporation"
    );
}

#[test]
fn evap_leaves_dry_cells_alone() {
    let mut w = setup_column_world();
    // Air at y=5 with sat=0. No writes should occur.
    let cfg = EvapConfig::default();
    apply_evaporation(&mut w, &cfg);
    assert_eq!(w.get_cell(4, 5).unwrap().sat.0, 0);
}

#[test]
fn evap_into_humidity_conserves_mass() {
    // Sat leaving cells lands as humidity mass. Sum should stay
    // constant across a single evap pass.
    use crate::humidity::Humidity;
    let mut w = setup_column_world();
    for y in 1..=5 {
        w.set_cell(4, y, Cell::water());
    }
    let mut h = Humidity::new(4);
    let cfg = EvapConfig {
        rate_per_tick: 3,
        dry_above_max: 200,
        period_ticks: 1,
    };
    let cell_sat_before: i64 = (1..=5)
        .map(|y| w.get_cell(4, y).unwrap().sat.0 as i64)
        .sum();
    let hum_before = h.total_mass();

    apply_evaporation_into_humidity(&mut w, &mut h, &cfg);

    let cell_sat_after: i64 = (1..=5)
        .map(|y| w.get_cell(4, y).unwrap().sat.0 as i64)
        .sum();
    let hum_after = h.total_mass();
    assert!(cell_sat_after < cell_sat_before, "some water must have left");
    let removed = (cell_sat_before - cell_sat_after) as f32;
    let gained = hum_after - hum_before;
    assert!(
        (removed - gained).abs() < 1e-3,
        "removed sat ({removed}) should equal humidity gain ({gained})"
    );
}

#[test]
fn evap_into_humidity_matches_bare_evap_cell_state() {
    // The cell-side effect must be identical to `apply_evaporation`
    // — humidity routing is purely an additive record of the
    // removed mass, not a different eligibility rule.
    use crate::humidity::Humidity;
    let mut w_bare = setup_column_world();
    let mut w_hum = setup_column_world();
    for y in 1..=5 {
        w_bare.set_cell(4, y, Cell::water());
        w_hum.set_cell(4, y, Cell::water());
    }
    let mut h = Humidity::new(4);
    let cfg = EvapConfig::default();
    apply_evaporation(&mut w_bare, &cfg);
    apply_evaporation_into_humidity(&mut w_hum, &mut h, &cfg);
    for y in 1..=5 {
        assert_eq!(
            w_bare.get_cell(4, y).map(|c| c.sat.0),
            w_hum.get_cell(4, y).map(|c| c.sat.0),
            "cell y={y} should evaporate identically"
        );
    }
}

// ------------ karst dissolution ------------

fn setup_limestone_world() -> World {
    // Chunk (0, 0). Solid Limestone at y=1..=10, Bedrock at y=0,
    // Air above.
    let mut w = World::new(999);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..CHUNK_CELLS_W as i32 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=10 {
            w.set_cell(x, y, Cell::solid(MaterialId::Limestone));
        }
    }
    w
}

#[test]
fn dry_limestone_never_dissolves() {
    let mut w = setup_limestone_world();
    // No wet neighbours anywhere — just dry Air above.
    let cfg = KarstConfig {
        prob_per_wet_neighbour: 1.0,
        min_wet_neighbour_sat: 200,
        seed_salt: 1,
    };
    for _ in 0..50 {
        apply_karst_dissolution(&mut w, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }
    // No cell converted.
    for x in 0..(CHUNK_CELLS_W as i32) {
        for y in 1..=10 {
            assert_eq!(
                w.get_cell(x, y).unwrap().material,
                MaterialId::Limestone,
                "dry limestone at ({x},{y}) must not dissolve"
            );
        }
    }
}

#[test]
fn wet_limestone_eventually_dissolves() {
    let mut w = setup_limestone_world();
    // Put water full on top of a specific limestone cell.
    w.set_cell(10, 11, Cell::water());
    let cfg = KarstConfig {
        prob_per_wet_neighbour: 1.0,
        min_wet_neighbour_sat: 200,
        seed_salt: 42,
    };
    // With prob 1.0 the top-most limestone under the puddle
    // should convert on the first tick.
    apply_karst_dissolution(&mut w, &cfg);
    let after = w.get_cell(10, 10).unwrap();
    assert_eq!(after.material, MaterialId::Air, "wet limestone must dissolve");
}

#[test]
fn karst_is_deterministic_for_seed_and_tick() {
    let mut a = setup_limestone_world();
    let mut b = setup_limestone_world();
    // Same puddle placement on both.
    for x in 5..=15 {
        a.set_cell(x, 11, Cell::water());
        b.set_cell(x, 11, Cell::water());
    }
    let cfg = KarstConfig {
        prob_per_wet_neighbour: 0.5,
        min_wet_neighbour_sat: 200,
        seed_salt: 7,
    };
    for _ in 0..10 {
        apply_karst_dissolution(&mut a, &cfg);
        apply_karst_dissolution(&mut b, &cfg);
        a.tick = a.tick.wrapping_add(1);
        b.tick = b.tick.wrapping_add(1);
    }
    for x in 0..(CHUNK_CELLS_W as i32) {
        for y in 1..=10 {
            assert_eq!(
                a.get_cell(x, y).map(|c| c.material),
                b.get_cell(x, y).map(|c| c.material),
                "seed-determinism failed at ({x},{y})"
            );
        }
    }
}

#[test]
fn karst_ignores_non_limestone_solids() {
    // Stone cell adjacent to water — should never dissolve.
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
    w.set_cell(5, 6, Cell::water());
    let cfg = KarstConfig {
        prob_per_wet_neighbour: 1.0,
        min_wet_neighbour_sat: 200,
        seed_salt: 3,
    };
    for _ in 0..20 {
        apply_karst_dissolution(&mut w, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }
    assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Stone);
}

// ------------ condensation rain ------------

fn setup_cloud_world() -> (World, crate::humidity::Humidity) {
    let mut w = World::new(21);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..CHUNK_CELLS_W as i32 {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let h = crate::humidity::Humidity::new(4);
    (w, h)
}

fn ground_sat_sum(w: &World) -> i64 {
    (0..CHUNK_CELLS_W as i32)
        .map(|x| w.get_cell(x, 1).map(|c| c.sat.0 as i64).unwrap_or(0))
        .sum()
}

#[test]
fn condensation_never_rains_from_a_dry_tile() {
    let (mut w, mut h) = setup_cloud_world();
    // No humidity anywhere. Rain must not appear.
    let cfg = CondensationConfig {
        top_y: 30,
        ..CondensationConfig::default()
    };
    for _ in 0..20 {
        apply_condensation_rain(&mut w, &mut h, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }
    for x in 0..CHUNK_CELLS_W as i32 {
        assert_eq!(w.get_cell(x, 1).unwrap().sat.0, 0);
        assert_eq!(w.get_cell(x, 30).unwrap().sat.0, 0);
    }
}

#[test]
fn condensation_rains_when_tile_is_wet() {
    let (mut w, mut h) = setup_cloud_world();
    // Humidity over tile covering gx centre 2; rain lands on ground.
    h.add(1, 30, 1000.0);
    let cfg = CondensationConfig {
        top_y: 30,
        max_prob_per_tick: 1.0, // guaranteed to rain
        ..CondensationConfig::default()
    };
    apply_condensation_rain(&mut w, &mut h, &cfg);
    let landed = w.get_cell(2, 1).unwrap();
    assert!(
        landed.sat.0 > 0,
        "cloud with 1000 mass should have rained on the ground (got sat={})",
        landed.sat.0
    );
    assert_eq!(w.get_cell(2, 30).unwrap().sat.0, 0, "sky row stays dry");
}

#[test]
fn condensation_frosts_thin_ice_not_snow_towers_when_cold() {
    // Cold humidity drizzle may glaze rock with frost, but must not
    // mint Snow stacks or grow ice pillars under clear sky.
    let (mut w, mut h) = setup_cloud_world();
    w.set_cell(2, 1, Cell::solid(MaterialId::Sand));
    h.add(1, 30, 2000.0);
    let hum_before = h.total_mass();
    let mut temp = crate::temperature::Temperature::with_world_bounds(
        4, 0, 0, 64, 64, 1, 64, 32, false,
    );
    temp.config.base_temp_c = -12.0;
    for v in temp.cells.values_mut() {
        *v = -12.0;
    }
    // Demo uses small drizzle droplets; frost still pays a full cell
    // via the humidity-tile retry when mass ≥ 255.
    let cfg = CondensationConfig {
        top_y: 30,
        max_prob_per_tick: 1.0,
        mass_per_droplet: 40.0,
        ..CondensationConfig::default()
    };
    let phase = crate::phase::PhaseConfig::default();
    for _ in 0..8 {
        apply_condensation_rain_phased(
            &mut w,
            &mut h,
            &cfg,
            None,
            Some(&temp),
            Some(&phase),
        );
        w.tick = w.tick.wrapping_add(1);
    }
    for y in 1..16 {
        assert_ne!(
            w.get_cell(2, y).map(|c| c.material),
            Some(MaterialId::Snow),
            "condensation must not place snow at y={y}"
        );
    }
    assert_eq!(
        w.get_cell(2, 2).map(|c| c.material),
        Some(MaterialId::Ice),
        "cold condensate should leave a thin ice glaze on ground"
    );
    for y in 3..16 {
        assert_ne!(
            w.get_cell(2, y).map(|c| c.material),
            Some(MaterialId::Ice),
            "frost must stay a thin coat — no ice tower at y={y}"
        );
    }
    let drained = hum_before - h.total_mass();
    // Lateral frost coat may seat several columns; each seat costs 255.
    assert!(
        drained >= 255.0 - 1e-3,
        "frost must drain at least one full cell (got {drained})"
    );
    assert!(
        (drained / 255.0 - (drained / 255.0).round()).abs() < 1e-3,
        "frost drains must be whole cells (got {drained})"
    );
}

#[test]
fn condensation_is_mass_conservative() {
    let (mut w, mut h) = setup_cloud_world();
    // Spread humidity across a few tiles.
    h.add(1, 30, 400.0);
    h.add(6, 30, 300.0);
    h.add(11, 30, 250.0);
    let total_before = h.total_mass();
    let world_sat_before = ground_sat_sum(&w);

    let cfg = CondensationConfig {
        top_y: 30,
        max_prob_per_tick: 1.0,
        ..CondensationConfig::default()
    };
    for _ in 0..5 {
        apply_condensation_rain(&mut w, &mut h, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }

    let total_after = h.total_mass();
    let world_sat_after = ground_sat_sum(&w);
    let humidity_lost = total_before - total_after;
    let world_gained = (world_sat_after - world_sat_before) as f32;
    assert!(
        (humidity_lost - world_gained).abs() < 1.5,
        "humidity_lost={humidity_lost}, world_gained={world_gained} — mass must balance"
    );
}

#[test]
fn condensation_is_deterministic_for_seed_and_tick() {
    let (mut w1, mut h1) = setup_cloud_world();
    let (mut w2, mut h2) = setup_cloud_world();
    for tile in [(1, 30), (6, 30), (11, 30)] {
        h1.add(tile.0, tile.1, 400.0);
        h2.add(tile.0, tile.1, 400.0);
    }
    let cfg = CondensationConfig {
        top_y: 30,
        max_prob_per_tick: 0.7,
        seed_salt: 12345,
        ..CondensationConfig::default()
    };
    for _ in 0..10 {
        apply_condensation_rain(&mut w1, &mut h1, &cfg);
        apply_condensation_rain(&mut w2, &mut h2, &cfg);
        w1.tick = w1.tick.wrapping_add(1);
        w2.tick = w2.tick.wrapping_add(1);
    }
    for x in 0..CHUNK_CELLS_W as i32 {
        assert_eq!(
            w1.get_cell(x, 1).map(|c| c.sat.0),
            w2.get_cell(x, 1).map(|c| c.sat.0),
            "world state must be deterministic at x={x}"
        );
    }
    assert_eq!(h1.total_mass(), h2.total_mass());
}

#[test]
fn condensation_skips_non_air_landing_cell() {
    let (mut w, mut h) = setup_cloud_world();
    h.add(1, 30, 1000.0);
    // Solid column — nowhere for surface rain to land.
    for y in 1..=30 {
        w.set_cell(2, y, Cell::solid(MaterialId::Stone));
    }
    let mass_before = h.total_mass();
    let cfg = CondensationConfig {
        top_y: 30,
        max_prob_per_tick: 1.0,
        ..CondensationConfig::default()
    };
    apply_condensation_rain(&mut w, &mut h, &cfg);
    assert_eq!(w.get_cell(2, 30).unwrap().material, MaterialId::Stone);
    assert_eq!(h.total_mass(), mass_before);
}

#[test]
fn orographic_boost_rains_thinner_clouds_over_tall_land() {
    use crate::worldgen::WorldgenParams;
    let p = WorldgenParams::default();
    // Find a tile whose centre is well above sea (mountain belt).
    let mut tall_hx = None;
    let tc = 4;
    for hx in 0..(p.width_cols / tc) {
        let gx = hx * tc + tc / 2;
        let s = crate::worldgen::continental_surface_y(
            p.seed,
            gx,
            p.sea_level_y,
            p.width_cols,
        );
        if s >= p.sea_level_y + 22 {
            tall_hx = Some(hx);
            break;
        }
    }
    let tall_hx = tall_hx.expect("worldgen should have tall land");
    // Landing column is the tile centre (matches condensation deposit).
    let centre_gx = tall_hx * tc + tc / 2;
    let surface = crate::worldgen::continental_surface_y(
        p.seed,
        centre_gx,
        p.sea_level_y,
        p.width_cols,
    );
    let mut w = World::new(p.seed);
    // Terrain under the mountain column so rain can land.
    for y in [surface, surface + 1, 40] {
        w.ensure_chunk(ChunkCoord::new(
            centre_gx.div_euclid(CHUNK_CELLS_W as i32),
            y.div_euclid(CHUNK_CELLS_H as i32),
        ));
    }
    w.set_cell(centre_gx, surface, Cell::solid(MaterialId::Stone));
    for y in (surface + 1)..=40 {
        w.set_cell(centre_gx, y, Cell::air());
    }
    let sky = surface + 12;
    for y in (surface + 1)..=sky {
        w.ensure_chunk(ChunkCoord::new(
            centre_gx.div_euclid(CHUNK_CELLS_W as i32),
            y.div_euclid(CHUNK_CELLS_H as i32),
        ));
        w.set_cell(centre_gx, y, Cell::air());
    }
    let mut h = crate::humidity::Humidity::new(tc);
    // Thin cloud — below default min_mass_to_rain (64) but above
    // orographic-reduced threshold on tall peaks.
    // Add at tile centre so humidity key matches landing column.
    h.add(centre_gx, sky, 50.0);
    let cfg = CondensationConfig {
        top_y: sky,
        min_mass_to_rain: 64.0,
        max_prob_per_tick: 1.0,
        full_mass: 120.0,
        mass_per_droplet: 24.0,
        ..CondensationConfig::default()
    };
    let oro = OrographicConfig {
        seed: p.seed,
        width_cols: p.width_cols,
        sea_level_y: p.sea_level_y,
        tall_above_sea: 22,
        wind_sign: 1,
        ..OrographicConfig::default()
    };
    let before = h.total_mass();
    // Without oro: should not rain (mass 50 < 64).
    for _ in 0..40 {
        apply_condensation_rain(&mut w, &mut h, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }
    assert_eq!(h.total_mass(), before, "thin cloud should not rain flat");
    // With oro over tall land: should dump within a few dozen ticks.
    for _ in 0..40 {
        apply_condensation_rain_with_orographic(&mut w, &mut h, &cfg, Some(&oro));
        w.tick = w.tick.wrapping_add(1);
        if h.total_mass() < before {
            break;
        }
    }
    assert!(
        h.total_mass() < before,
        "orographic rain should drain thin mountain clouds (mass {} → {})",
        before,
        h.total_mass()
    );
}

#[test]
fn karst_low_sat_neighbour_does_not_dissolve() {
    // Air cell above limestone has sat below threshold → no
    // dissolution.
    let mut w = setup_limestone_world();
    let mut wet_ish = Cell::air();
    wet_ish.sat = Sat(50); // below threshold 200
    w.set_cell(10, 11, wet_ish);
    let cfg = KarstConfig {
        prob_per_wet_neighbour: 1.0,
        min_wet_neighbour_sat: 200,
        seed_salt: 4,
    };
    for _ in 0..10 {
        apply_karst_dissolution(&mut w, &cfg);
        w.tick = w.tick.wrapping_add(1);
    }
    assert_eq!(
        w.get_cell(10, 10).unwrap().material,
        MaterialId::Limestone,
        "damp-but-not-wet neighbour must not dissolve karst"
    );
}

#[test]
fn settled_column_goes_quiescent() {
    // Droplet falls down a one-cell-wide bedrock shaft so lateral
    // spill can't keep the row alive forever. Once it rests, the
    // dirty plan empties and physics early-outs.
    let mut w = setup_column_world();
    for y in 1..16 {
        w.set_cell(3, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(5, y, Cell::solid(MaterialId::Bedrock));
    }
    w.set_cell(4, 8, Cell::water());
    for _ in 0..20 {
        tick(&mut w);
    }
    // Consume any residual dirty from the last write.
    tick(&mut w);
    assert!(
        plan_active(&w).is_empty(),
        "settled world must plan no active chunks"
    );
    // Find where the droplet rested (flow substeps can leave a
    // thin film split across a couple of floor cells).
    let mut rested = None;
    for y in 1..8 {
        if let Some(c) = w.get_cell(4, y) {
            if c.sat.0 > 0 {
                rested = Some((4, y, c.sat.0));
                break;
            }
        }
    }
    let (rx, ry, sat_before) = rested.expect("droplet should rest in the shaft");
    tick(&mut w);
    assert_eq!(w.get_cell(rx, ry).unwrap().sat.0, sat_before);
    assert!(plan_active(&w).is_empty());
}

#[test]
fn tick_runs_gravity_then_spill_and_conserves_mass() {
    // Full tick pass: flow substeps drop the droplet several rows
    // and spread it sideways. Total sat is unchanged.
    let mut w = setup_column_world();
    w.set_cell(30, 5, Cell::water());
    let start_mass = 255i64;

    tick(&mut w);
    let after_mass: i64 = (0..64i32)
        .flat_map(|x| (0..64i32).map(move |y| (x, y)))
        .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as i64))
        .sum();
    assert_eq!(after_mass, start_mass, "tick must conserve total sat");

    // With FLOW_SUBSTEPS gravity passes, the droplet leaves y=5.
    assert!(w.get_cell(30, 5).unwrap().sat.is_empty());
    // Some water should exist on the floor or just above it, spread
    // across neighbouring columns.
    let wet_near_floor: i32 = (28..=32)
        .flat_map(|x| (1..=4).map(move |y| (x, y)))
        .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.sat.0 > 0).unwrap_or(false))
        .count() as i32;
    assert!(
        wet_near_floor >= 2,
        "droplet should fall and spread (wet cells={wet_near_floor})"
    );
}

#[test]
fn quiescent_lake_still_evaporates() {
    // Dirty-rect physics can go idle while a lake remains. Evap
    // must keep bleeding surface water via the wet-air occupancy
    // flag — not only when the chunk is dirty.
    let mut w = setup_column_world();
    w.set_cell(4, 1, Cell::water());
    clear_all_dirty(&mut w);
    assert!(plan_active(&w).is_empty());
    assert!(w.chunks[&ChunkCoord::new(0, 0)].has_wet_air);

    let mut h = crate::humidity::Humidity::new(4);
    let cfg = EvapConfig {
        rate_per_tick: 5,
        dry_above_max: 200,
        period_ticks: 1,
    };
    apply_evaporation_into_humidity(&mut w, &mut h, &cfg);
    assert!(
        w.get_cell(4, 1).unwrap().sat.0 < u8::MAX,
        "surface water must evaporate even when physics is quiescent"
    );
    assert!(h.total_mass() > 0.0);
}

#[test]
fn karst_skips_chunks_without_limestone_flag() {
    let mut w = setup_column_world();
    // Wet air only — no limestone. Flag stays false; pass is a no-op.
    w.set_cell(4, 2, Cell::water());
    assert!(!w.chunks[&ChunkCoord::new(0, 0)].has_limestone);
    let cfg = KarstConfig {
        prob_per_wet_neighbour: 1.0,
        min_wet_neighbour_sat: 1,
        seed_salt: 1,
    };
    apply_karst_dissolution(&mut w, &cfg);
    assert_eq!(w.get_cell(4, 2).unwrap().material, MaterialId::Air);
}

#[test]
fn parallel_tick_matches_serial_on_multi_chunk_fixture() {
    // Two-by-two chunk slab with water + sand so gravity, spill,
    // seepage, and grain all fire across several colours.
    fn build() -> World {
        let mut w = World::new(42);
        for cx in 0..2 {
            for cy in 0..2 {
                w.ensure_chunk(ChunkCoord::new(cx, cy));
            }
        }
        for x in 0..128 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(10, 40, Cell::water());
        w.set_cell(70, 90, Cell::water());
        w.set_cell(20, 50, Cell::solid(MaterialId::Sand));
        w.set_cell(90, 100, Cell::solid(MaterialId::Sand));
        w.set_cell(11, 1, Cell::solid(MaterialId::Sand));
        w
    }

    crate::parallel::set_parallel_enabled(false);
    let mut serial = build();
    for _ in 0..30 {
        tick(&mut serial);
    }

    crate::parallel::set_parallel_enabled(true);
    let mut parallel = build();
    for _ in 0..30 {
        tick(&mut parallel);
    }

    for cx in 0..2 {
        for cy in 0..2 {
            let coord = ChunkCoord::new(cx, cy);
            let a = serial.chunks.get(&coord).expect("serial chunk");
            let b = parallel.chunks.get(&coord).expect("parallel chunk");
            assert_eq!(
                a.cells, b.cells,
                "parallel tick diverged from serial at {coord:?}"
            );
        }
    }
    // Leave the process default (parallel on) for later tests.
    crate::parallel::set_parallel_enabled(true);
}
#[test]
fn stamped_world_midair_sand_falls_after_quiet_then_paint() {
    // App path: world runs quiet (dirty cleared), F3 paints sand in sky
    // while paused, then unpause → one tick must seat the sand.
    use crate::worldgen::{stamp_world, WorldgenParams};
    let mut w = World::new(7);
    let p = WorldgenParams {
        seed: 1,
        width_cols: CHUNK_CELLS_W as i32 * 2,
        bedrock_floor_y: 0,
        sea_level_y: 40,
        sky_ceiling_y: CHUNK_CELLS_H as i32 * 3,
        bedrock_thickness: 4,
        stone_thickness: 8,
        sand_cap_thickness: 2,
        limestone_in_shelf_and_coast: false,
        wrap_x: true,
    };
    stamp_world(&mut w, &p);
    // Drain residual worldgen dirty like a running demo.
    for _ in 0..30 {
        tick(&mut w);
    }
    clear_all_dirty(&mut w);
    assert!(plan_active(&w).is_empty(), "precondition: quiet world");

    // Paint mid-air sand high above terrain (F3 brush).
    let gx = 40;
    let gy = 140;
    for dx in -3..=3 {
        for dy in 0..=4 {
            w.set_cell(gx + dx, gy + dy, Cell::solid(MaterialId::Sand));
        }
    }
    assert!(!plan_active(&w).is_empty(), "paint must dirty");

    tick(&mut w);

    let floating = (-3..=3)
        .flat_map(|dx| (gy - 5..=gy + 4).map(move |y| (gx + dx, y)))
        .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
        .count();
    assert_eq!(
        floating, 0,
        "mid-air sand must leave the paint height after one unpause tick (left={floating})"
    );
}

#[test]
fn stranded_midair_sand_falls_after_dirty_cleared() {
    // Regression: quiet ticks cleared the F3 paint wake before grain
    // fall ran; sand hung until shear. Wake + tick must seat it.
    let mut w = setup_column_world();
    for x in 3..=8 {
        for y in 30..=35 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    clear_all_dirty(&mut w);
    assert!(plan_active(&w).is_empty());
    // Simulate the periodic stranded-grain scan inside tick.
    wake_unsupported_grains(&mut w);
    assert!(!plan_active(&w).is_empty(), "wake must dirty unsupported sand");
    tick(&mut w);
    let floating = (3..=8)
        .flat_map(|x| (25..=35).map(move |y| (x, y)))
        .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
        .count();
    assert_eq!(floating, 0, "stranded sand must fall ({floating} still high)");
}

#[test]
fn floating_sand_settles_fast_despite_distant_lake_dirty() {
    // Lakes keep a non-empty dirty plan; grain settle must still wake and
    // drop a far mid-air sand blob in one tick (not drip via roof collapse).
    let mut w = setup_column_world();
    for y in 1..=4 {
        w.set_cell(2, y, Cell::water());
    }
    for x in 10..=16 {
        for y in 40..=48 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    // Simulate "only the lake is dirty".
    clear_all_dirty(&mut w);
    w.touch_dirty(2, 4);
    tick(&mut w);
    let floating = (10..=16)
        .flat_map(|x| (35..=48).map(move |y| (x, y)))
        .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
        .count();
    assert_eq!(
        floating, 0,
        "sand blob must fully seat in one tick despite lake dirty (left={floating})"
    );
}

#[test]
fn organic_sinks_through_suspended_full_sat() {
    // Invisible mid-air full water under litter must not pin Organic.
    let mut w = setup_column_world();
    w.set_cell(5, 20, Cell::water()); // suspended full-sat Air
    w.set_cell(5, 21, Cell::solid(MaterialId::Organic));
    tick(&mut w);
    assert_eq!(
        w.get_cell(5, 21).unwrap().material,
        MaterialId::Air,
        "Organic must leave the paint height"
    );
    assert_ne!(
        w.get_cell(5, 1).unwrap().material,
        MaterialId::Air,
        "Organic should seat near bedrock"
    );
}

#[test]
fn organic_still_floats_on_grounded_lake() {
    let mut w = setup_column_world();
    // Bedrock walls so surface flow cannot empty the column under litter.
    for y in 1..=6 {
        w.set_cell(2, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(8, y, Cell::solid(MaterialId::Bedrock));
    }
    for x in 3..=7 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 6, Cell::solid(MaterialId::Organic));
    for _ in 0..5 {
        tick(&mut w);
    }
    let on_surface = (3..=7).any(|x| {
        (5..=7).any(|y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
    });
    assert!(
        on_surface,
        "Organic must remain on the grounded lake surface (sat@5,5={})",
        w.get_cell(5, 5).map(|c| c.sat.0).unwrap_or(0)
    );
}

#[test]
fn dense_grain_punches_through_floating_organic_raft() {
    // Thin Organic on water must not carry Soil / LooseRock piles.
    let mut w = setup_column_world();
    for y in 1..=6 {
        w.set_cell(2, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(8, y, Cell::solid(MaterialId::Bedrock));
    }
    for x in 3..=7 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 6, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 7, Cell::solid(MaterialId::Soil));
    w.set_cell(5, 8, Cell::solid(MaterialId::LooseRock));
    w.set_cell(5, 9, Cell::solid(MaterialId::LooseRock));
    for _ in 0..20 {
        tick(&mut w);
    }
    let mut organs = vec![];
    let mut rocks = vec![];
    let mut soils = vec![];
    for x in 2..=8 {
        for y in 1..=12 {
            match w.get_cell(x, y).map(|c| c.material) {
                Some(MaterialId::Organic) => organs.push((x, y)),
                Some(MaterialId::LooseRock) => rocks.push((x, y)),
                Some(MaterialId::Soil) => soils.push((x, y)),
                _ => {}
            }
        }
    }
    assert!(
        !rocks.iter().any(|&(_, y)| y >= 6),
        "LooseRock must not remain stacked above the lake on Organic ({rocks:?})"
    );
    assert!(
        rocks.iter().any(|&(_, y)| y <= 5),
        "LooseRock must sink into / through the water column ({rocks:?})"
    );
    assert!(
        !soils.iter().any(|&(_, y)| y >= 6),
        "Soil must not ride the floating Organic raft ({soils:?})"
    );
    assert!(
        !organs.is_empty(),
        "Organic raft should still exist somewhere near the lake"
    );
}

#[test]
fn organic_alone_still_floats_without_punch() {
    let mut w = setup_column_world();
    for y in 1..=6 {
        w.set_cell(2, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(8, y, Cell::solid(MaterialId::Bedrock));
    }
    for x in 3..=7 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 6, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 7, Cell::solid(MaterialId::Organic));
    for _ in 0..10 {
        tick(&mut w);
    }
    let n = (3..=7)
        .map(|x| {
            (1..=10)
                .filter(|&y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
                .count()
        })
        .sum::<usize>();
    assert_eq!(n, 2, "stacked Organic litter must stay as a raft (n={n})");
}

#[test]
fn submerged_organic_rises_out_of_water_column() {
    // Glitch line: Organic stuck under a refilled lake surface must buoyancy-rise.
    let mut w = setup_column_world();
    for y in 1..=6 {
        w.set_cell(2, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(8, y, Cell::solid(MaterialId::Bedrock));
    }
    for x in 3..=7 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    // Raft on the surface + a submerged "glitch" cell mid-column.
    w.set_cell(5, 6, Cell::solid(MaterialId::Organic));
    w.set_cell(5, 3, Cell::solid(MaterialId::Organic));
    for _ in 0..8 {
        tick(&mut w);
    }
    assert_ne!(
        w.get_cell(5, 3).unwrap().material,
        MaterialId::Organic,
        "submerged Organic must leave mid-column"
    );
    let organics: Vec<(i32, i32)> = (3..=7)
        .flat_map(|x| {
            (1..=8)
                .filter(|&y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
                .map(|y| (x, y))
                .collect::<Vec<_>>()
        })
        .collect();
    assert!(
        organics.len() >= 2,
        "both Organic cells must survive ({organics:?})"
    );
    assert!(
        organics.iter().all(|&(_, y)| y >= 4),
        "Organic must leave the deep water column ({organics:?})"
    );
    // No cell may remain with full water both above and below (glitch line).
    for &(x, y) in &organics {
        let above_water = matches!(
            w.get_cell(x, y + 1),
            Some(c) if c.material == MaterialId::Air && c.sat.is_full()
        );
        let below_water = matches!(
            w.get_cell(x, y - 1),
            Some(c) if c.material == MaterialId::Air && c.sat.is_full()
        );
        assert!(
            !(above_water && below_water),
            "Organic at ({x},{y}) still fully submerged"
        );
    }
}

#[test]
fn floating_organic_soaks_from_water_column() {
    let mut w = setup_column_world();
    for y in 1..=6 {
        w.set_cell(2, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(8, y, Cell::solid(MaterialId::Bedrock));
    }
    for x in 3..=7 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    w.set_cell(5, 6, Cell::solid(MaterialId::Organic));
    let sat_sum = |w: &World| -> i64 {
        let mut s = 0i64;
        for x in 0..16 {
            for y in 0..16 {
                if let Some(c) = w.get_cell(x, y) {
                    s += c.sat.0 as i64;
                }
            }
        }
        s
    };
    let before = sat_sum(&w);
    for _ in 0..10 {
        tick(&mut w);
    }
    let org = (3..=7)
        .flat_map(|x| (5..=7).map(move |y| (x, y)))
        .find_map(|(x, y)| {
            let c = w.get_cell(x, y)?;
            (c.material == MaterialId::Organic).then_some(c)
        })
        .expect("floating Organic must remain");
    assert!(
        org.sat.0 > 0,
        "floating Organic must soak pore water (sat={})",
        org.sat.0
    );
    assert_eq!(sat_sum(&w), before, "soak must conserve water mass");
}

#[test]
fn plant_can_seat_on_wet_floating_organic() {
    use crate::plant::find_plant_slot;
    let mut w = setup_column_world();
    for y in 1..=6 {
        w.set_cell(2, y, Cell::solid(MaterialId::Bedrock));
        w.set_cell(8, y, Cell::solid(MaterialId::Bedrock));
    }
    for x in 3..=7 {
        for y in 1..=5 {
            w.set_cell(x, y, Cell::water());
        }
    }
    let mut org = Cell::solid(MaterialId::Organic);
    org.sat = Sat(40); // moist enough for spore/sprout gate
    w.set_cell(5, 6, org);
    let slot = find_plant_slot(&w, 5, 6);
    assert_eq!(
        slot,
        Some(7),
        "Air above floating Organic must be a plantable crown"
    );
    // Moisture on the bed cell must clear the spore gate.
    let bed = w.get_cell(5, 6).unwrap();
    let cap = water_capacity(MaterialId::Organic).max(1);
    let moist = bed.sat.0 as f32 / cap as f32;
    assert!(
        moist >= 0.02,
        "wet floating Organic must clear plant moisture gate (moist={moist})"
    );
}


#[test]
fn organic_sky_drop_does_not_leave_cliff_walls() {
    // Vertical Organic "cliff wall" on bedrock must avalanche into a
    // low sprawl in one tick (interleaved fall + repose).
    let mut w = setup_column_world();
    for y in 1..=12 {
        w.set_cell(8, y, Cell::solid(MaterialId::Organic));
    }
    tick(&mut w);
    let max_col_h = (0..16)
        .map(|x| {
            (1..=16)
                .filter(|&y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
                .count()
        })
        .max()
        .unwrap_or(0);
    let width = (0..16)
        .filter(|&x| {
            (1..=16).any(|y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic))
        })
        .count();
    assert!(
        max_col_h <= 4,
        "Organic cliff must sprawl (tallest column={max_col_h}, width={width})"
    );
    assert!(width >= 5, "Organic cliff should spread sideways (width={width})");
}

#[test]
fn organic_sky_blob_lands_as_repose_pile_not_block() {
    let mut w = setup_column_world();
    for x in 6..=10 {
        for y in 20..=28 {
            w.set_cell(x, y, Cell::solid(MaterialId::Organic));
        }
    }
    tick(&mut w);
    // No vertical face taller than a couple of cells: count columns
    // where height >= 4 and a side neighbour is empty at mid-height.
    let mut cliff_faces = 0;
    for x in 1..15 {
        for y in 2..=8 {
            let here = w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Organic);
            let below = w.get_cell(x, y - 1).map(|c| c.material) == Some(MaterialId::Organic);
            let left_air = w.get_cell(x - 1, y).map(|c| c.material) == Some(MaterialId::Air);
            let right_air = w.get_cell(x + 1, y).map(|c| c.material) == Some(MaterialId::Air);
            if here && below && (left_air || right_air) {
                // 2-cell vertical face exposure
                let above = w.get_cell(x, y + 1).map(|c| c.material) == Some(MaterialId::Organic);
                if above {
                    cliff_faces += 1;
                }
            }
        }
    }
    assert!(
        cliff_faces <= 2,
        "sky blob must not land as cliff walls (exposed face cells={cliff_faces})"
    );
}

#[test]
fn sand_sky_drop_does_not_leave_cliff_walls() {
    let mut w = setup_column_world();
    for y in 1..=12 {
        w.set_cell(8, y, Cell::solid(MaterialId::Sand));
    }
    tick(&mut w);
    let max_col_h = (0..16)
        .map(|x| {
            (1..=16)
                .filter(|&y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
                .count()
        })
        .max()
        .unwrap_or(0);
    let width = (0..16)
        .filter(|&x| {
            (1..=16).any(|y| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
        })
        .count();
    assert!(
        max_col_h <= 4,
        "Sand cliff must sprawl (tallest column={max_col_h}, width={width})"
    );
    assert!(width >= 5, "Sand cliff should spread sideways (width={width})");
}

#[test]
fn sand_micro_cliff_on_stone_slope_keeps_reposing() {
    // 2–3 cell vertical sand lips on a rock face (the screenshot case).
    let mut w = setup_column_world();
    // Stone ramp: y = x/2 style steps.
    for x in 2..=12 {
        for y in 1..=(x / 2).max(1) {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
    }
    // Vertical sand lip on the left face at mid slope.
    w.set_cell(6, 4, Cell::solid(MaterialId::Sand));
    w.set_cell(6, 5, Cell::solid(MaterialId::Sand));
    w.set_cell(6, 6, Cell::solid(MaterialId::Sand));
    // Ensure dry Air seats to the left.
    for y in 1..=8 {
        if w.get_cell(5, y).map(|c| c.material) != Some(MaterialId::Stone) {
            w.set_cell(5, y, Cell::air());
        }
    }
    for _ in 0..5 {
        tick(&mut w);
    }
    let lip = (4..=6)
        .filter(|&y| w.get_cell(6, y).map(|c| c.material) == Some(MaterialId::Sand))
        .count();
    assert!(
        lip <= 1,
        "sand micro-cliff must avalanche off the face (cells still stacked={lip})"
    );
}

#[test]
fn sand_micro_cliff_reposes_through_thin_haze() {
    // Inland humidity haze used to freeze sand lips (any sat blocked
    // repose). Thin haze must not; shore film still must.
    let mut w = setup_column_world();
    for x in 2..=12 {
        for y in 1..=(x / 2).max(1) {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
    }
    w.set_cell(6, 4, Cell::solid(MaterialId::Sand));
    w.set_cell(6, 5, Cell::solid(MaterialId::Sand));
    w.set_cell(6, 6, Cell::solid(MaterialId::Sand));
    for y in 1..=8 {
        if w.get_cell(5, y).map(|c| c.material) != Some(MaterialId::Stone) {
            let mut haze = Cell::air();
            haze.sat = Sat(24);
            w.set_cell(5, y, haze);
        }
    }
    for _ in 0..5 {
        tick(&mut w);
    }
    let lip = (4..=6)
        .filter(|&y| w.get_cell(6, y).map(|c| c.material) == Some(MaterialId::Sand))
        .count();
    assert!(
        lip <= 1,
        "sand lip must avalanche through thin haze (stacked={lip})"
    );
}

#[test]
fn sand_ledge_walks_off_sideways_into_open_air() {
    // Diagonal-down blocked by stone; open Air beside with Air below.
    let mut w = setup_column_world();
    for x in 5..=8 {
        for y in 1..=3 {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
    }
    // Ledge sand with a vertical lip above open air to the left.
    w.set_cell(5, 4, Cell::solid(MaterialId::Sand));
    w.set_cell(5, 5, Cell::solid(MaterialId::Sand));
    w.set_cell(4, 1, Cell::solid(MaterialId::Stone));
    w.set_cell(4, 2, Cell::air());
    w.set_cell(4, 3, Cell::air());
    w.set_cell(4, 4, Cell::air());
    w.set_cell(4, 5, Cell::air());
    for _ in 0..8 {
        tick(&mut w);
    }
    assert_ne!(
        w.get_cell(5, 5).unwrap().material,
        MaterialId::Sand,
        "top ledge sand must walk off / fall"
    );
}

#[test]
fn large_sand_blob_does_not_keep_vertical_cliff() {
    let mut w = setup_column_world();
    // Organic bed like the screenshot.
    for x in 0..64 {
        for y in 1..=6 {
            w.set_cell(x, y, Cell::solid(MaterialId::Organic));
        }
    }
    // Large sand blob dropped on/near the bed (partly mid-air).
    for x in 18..=48 {
        for y in 7..=32 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    tick(&mut w);
    // Measure max vertical run of sand with Air to the left / right.
    let mut worst_left = 0i32;
    let mut worst_right = 0i32;
    for x in 1..63 {
        let mut run_l = 0i32;
        let mut run_r = 0i32;
        for y in 1..50 {
            let here = w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand);
            let left_air = w.get_cell(x - 1, y).map(|c| c.material) == Some(MaterialId::Air);
            let right_air = w.get_cell(x + 1, y).map(|c| c.material) == Some(MaterialId::Air);
            if here && left_air {
                run_l += 1;
                worst_left = worst_left.max(run_l);
            } else {
                run_l = 0;
            }
            if here && right_air {
                run_r += 1;
                worst_right = worst_right.max(run_r);
            } else {
                run_r = 0;
            }
        }
    }
    // A hard cliff is a long vertical Air-exposed run. 45° stairs only
    // expose 1 cell at a time on a face.
    assert!(
        worst_left <= 2 && worst_right <= 2,
        "hard cliff face remained (left={worst_left} right={worst_right})"
    );
}

#[test]
fn large_sand_blob_across_chunk_seam_no_cliff() {
    use crate::parallel::set_parallel_enabled;
    set_parallel_enabled(true);
    let mut w = World::new(7);
    w.wrap_width = Some(CHUNK_CELLS_W as i32 * 2);
    for cx in 0..2 {
        w.ensure_chunk(ChunkCoord::new(cx, 0));
        w.ensure_chunk(ChunkCoord::new(cx, 1));
    }
    for x in 0..(CHUNK_CELLS_W as i32 * 2) {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=4 {
            w.set_cell(x, y, Cell::solid(MaterialId::Organic));
        }
    }
    // Blob straddling the x=64 seam.
    for x in 50..=80 {
        for y in 5..=28 {
            w.set_cell(x, y, Cell::solid(MaterialId::Sand));
        }
    }
    for _ in 0..10 {
        tick(&mut w);
    }
    let mut worst_left = 0i32;
    for x in 1..120 {
        let mut run = 0i32;
        for y in 1..40 {
            let here = w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand);
            let left_air = w.get_cell(x - 1, y).map(|c| c.material) == Some(MaterialId::Air);
            if here && left_air {
                run += 1;
                worst_left = worst_left.max(run);
            } else {
                run = 0;
            }
        }
    }
    set_parallel_enabled(true);
    assert!(
        worst_left <= 3,
        "chunk-seam sand blob left a hard cliff (worst_left={worst_left})"
    );
}


#[test]
fn organic_on_lake_tick_is_cheap() {
    use std::time::Instant;
    let mut w = World::new(42);
    for cx in 0..4 {
        for cy in 0..2 {
            w.ensure_chunk(ChunkCoord::new(cx, cy));
        }
    }
    let width = 4 * CHUNK_CELLS_W as i32;
    for x in 0..width {
        w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=20 {
            w.set_cell(x, y, Cell::water());
        }
    }
    for _ in 0..30 {
        tick(&mut w);
    }
    for x in 40..55 {
        for y in 21..28 {
            w.set_cell(x, y, Cell::solid(MaterialId::Organic));
        }
    }
    let t0 = Instant::now();
    for _ in 0..5 {
        tick(&mut w);
    }
    let land_ms = t0.elapsed().as_secs_f64() * 1000.0 / 5.0;
    let t1 = Instant::now();
    for _ in 0..20 {
        tick(&mut w);
    }
    let steady_ms = t1.elapsed().as_secs_f64() * 1000.0 / 20.0;
    eprintln!("organic-on-lake landing={land_ms:.2} ms/tick steady={steady_ms:.2} ms/tick");
    assert!(land_ms < 800.0, "landing too slow: {land_ms:.1}");
    assert!(steady_ms < 500.0, "steady too slow: {steady_ms:.1}");
}











#[test]
fn organic_on_demo_sized_ocean_tick_cost() {
    use std::time::Instant;
    use crate::worldgen::{stamp_world, WorldgenParams};
    let mut w = World::new(9);
    // Half demo width keeps CI sane but still stresses ocean+wake.
    let p = WorldgenParams {
        width_cols: (CHUNK_CELLS_W as i32) * 8,
        sky_ceiling_y: (CHUNK_CELLS_H as i32) * 3,
        ..WorldgenParams::default()
    };
    stamp_world(&mut w, &p);
    for _ in 0..10 {
        tick(&mut w);
    }
    let t_base0 = Instant::now();
    for _ in 0..10 {
        tick(&mut w);
    }
    let base = t_base0.elapsed().as_secs_f64() * 1000.0 / 10.0;

    // Paint organic on open water near sea level.
    let y0 = p.sea_level_y + 1;
    for x in 30..50 {
        for y in y0..y0 + 6 {
            if w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Air) {
                w.set_cell(x, y, Cell::solid(MaterialId::Organic));
            }
        }
    }
    let t0 = Instant::now();
    for _ in 0..5 {
        tick(&mut w);
    }
    let land = t0.elapsed().as_secs_f64() * 1000.0 / 5.0;
    let t1 = Instant::now();
    for _ in 0..15 {
        tick(&mut w);
    }
    let steady = t1.elapsed().as_secs_f64() * 1000.0 / 15.0;
    eprintln!(
        "demo-ocean base={base:.2} landing={land:.2} steady={steady:.2} ms/tick (ratio steady/base={:.2})",
        steady / base.max(0.01)
    );
    assert!(
        steady < base * 8.0 + 50.0,
        "organic on ocean made ticks too expensive: base={base:.1} steady={steady:.1}"
    );
}

