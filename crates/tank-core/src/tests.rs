use wk_material::MaterialId;

use crate::cell::Cell;
use crate::consts::{PRESET_H, PRESET_W, PX_PER_CELL};
use crate::tank::{Tank, TankInput, ToolEvent};

fn tick_n(tank: &mut Tank, n: u32) {
    let input = TankInput::default();
    for _ in 0..n {
        tank.tick(&input);
    }
}

fn place(tank: &mut Tank, vx: i32, vy: i32, cell: Cell) {
    tank.place_for_test(vx, vy, cell);
}

fn column_sat(tank: &Tank, vx: i32) -> u64 {
    let h = tank.visible_size().1 as i32;
    (0..h).map(|vy| tank.cell_visible(vx, vy).sat as u64).sum()
}

#[test]
fn preset_pixel_buffer_is_the_phone_size() {
    let mut tank = Tank::preset(1);
    assert_eq!(tank.visible_size(), (PRESET_W, PRESET_H));
    assert_eq!(
        tank.pixel_size(),
        (PRESET_W * PX_PER_CELL, PRESET_H * PX_PER_CELL)
    );
    let mut rgba = Vec::new();
    tank.write_rgba(&mut rgba);
    assert_eq!(rgba.len(), 384 * 768 * 4);
    assert!(rgba.iter().any(|&b| b != 0));
}

#[test]
fn sand_falls_onto_the_bedrock_floor() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    place(&mut tank, 3, 12, Cell::sand());
    tick_n(&mut tank, 20);
    assert_eq!(tank.cell_visible(3, 0).id(), MaterialId::Sand);
    assert!(tank.cell_visible(3, 12).is_air());
    assert_eq!(tank.count_material(MaterialId::Sand as u8), 1);
}

#[test]
fn stone_does_not_fall() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    place(&mut tank, 3, 8, Cell::stone());
    tick_n(&mut tank, 30);
    assert_eq!(tank.cell_visible(3, 8).id(), MaterialId::Stone);
    assert_ne!(tank.cell_visible(3, 0).id(), MaterialId::Stone);
}

#[test]
fn water_pools_and_levels_and_is_conserved() {
    let mut tank = Tank::try_new(2, 12, 16).unwrap();
    for vx in 0..6 {
        for vy in 0..8 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let before = tank.sat_sum();
    tick_n(&mut tank, 200);
    assert_eq!(tank.sat_sum(), before);
    let left: u64 = (0..6).map(|x| column_sat(&tank, x)).sum();
    let right: u64 = (6..12).map(|x| column_sat(&tank, x)).sum();
    let total = left + right;
    let diff = (left as i64 - right as i64).unsigned_abs();
    assert!(diff * 100 / total.max(1) < 20, "left {left} right {right}");
}

#[test]
fn sand_under_a_lake_gets_wet_without_losing_water() {
    let mut tank = Tank::try_new(3, 8, 10).unwrap();
    for vx in 0..8 {
        place(&mut tank, vx, 0, Cell::sand());
        place(&mut tank, vx, 1, Cell::water());
    }
    let before = tank.sat_sum();
    tick_n(&mut tank, 30);
    assert_eq!(tank.sat_sum(), before);
    assert!(tank.cell_visible(2, 0).sat > 0, "sand stayed dry");
    assert!(
        tank.cell_visible(2, 1).sat > 0,
        "the lake cell above was emptied"
    );
}

#[test]
fn a_sand_tower_slumps_and_two_seeds_can_differ() {
    let run = |seed| {
        let mut tank = Tank::try_new(seed, 12, 16).unwrap();
        for vx in 0..12 {
            place(&mut tank, vx, 0, Cell::sand());
        }
        for vy in 1..8 {
            place(&mut tank, 2, vy, Cell::sand());
        }
        tick_n(&mut tank, 12);
        tank.pixel_hash()
    };
    let a = run(1);
    let b = run(1);
    let c = run(2);
    assert_eq!(a, b);
    let mut differed = false;
    for n in 1..24 {
        let mut left = Tank::try_new(1, 12, 16).unwrap();
        let mut right = Tank::try_new(2, 12, 16).unwrap();
        for vx in 0..12 {
            place(&mut left, vx, 0, Cell::sand());
            place(&mut right, vx, 0, Cell::sand());
        }
        for vy in 1..8 {
            place(&mut left, 2, vy, Cell::sand());
            place(&mut right, 2, vy, Cell::sand());
        }
        tick_n(&mut left, n);
        tick_n(&mut right, n);
        if left.pixel_hash() != right.pixel_hash() {
            differed = true;
            break;
        }
    }
    assert!(
        differed,
        "repose tie-break never changed the picture (end hash {a:x} vs {c:x})"
    );

    let mut tank = Tank::try_new(1, 12, 16).unwrap();
    for vx in 0..12 {
        place(&mut tank, vx, 0, Cell::sand());
    }
    for vy in 1..8 {
        place(&mut tank, 2, vy, Cell::sand());
    }
    let sand = tank.count_material(MaterialId::Sand as u8);
    tick_n(&mut tank, 40);
    assert_eq!(tank.count_material(MaterialId::Sand as u8), sand);
    assert!(
        tank.cell_visible(2, 7).is_air(),
        "tower top should have slumped"
    );
}

#[test]
fn same_seed_and_input_match_after_save() {
    let mut tank = Tank::try_new(9, 16, 20).unwrap();
    for vx in 0..16 {
        for vy in 0..4 {
            place(&mut tank, vx, vy, Cell::sand());
        }
    }
    let mut input = TankInput::default();
    input.tools.push(ToolEvent {
        kind: 3,
        material: 0,
        x: 4,
        y: 12,
        r: 2,
    });
    for _ in 0..40 {
        tank.tick(&input);
        input.tools.clear();
    }
    let hash = tank.pixel_hash();
    let bytes = tank.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.tick_count(), tank.tick_count());
    assert_eq!(loaded.pixel_hash(), hash);
    assert_eq!(loaded.sat_sum(), tank.sat_sum());

    let mut again = Tank::try_new(9, 16, 20).unwrap();
    for vx in 0..16 {
        for vy in 0..4 {
            place(&mut again, vx, vy, Cell::sand());
        }
    }
    let mut input = TankInput::default();
    input.tools.push(ToolEvent {
        kind: 3,
        material: 0,
        x: 4,
        y: 12,
        r: 2,
    });
    for _ in 0..40 {
        again.tick(&input);
        input.tools.clear();
    }
    assert_eq!(again.pixel_hash(), hash);
}

#[test]
fn corrupt_save_is_rejected() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    let mut bytes = tank.save_bytes().unwrap();
    bytes[0] ^= 0xff;
    assert!(Tank::load_bytes(&bytes).is_err());
    assert!(Tank::try_new(1, 2, 2).is_err());
}

#[test]
fn paused_tick_does_not_advance() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    place(&mut tank, 3, 6, Cell::sand());
    let input = TankInput {
        paused: true,
        ..TankInput::default()
    };
    tank.tick(&input);
    assert_eq!(tank.tick_count(), 0);
    assert_eq!(tank.cell_visible(3, 6).id(), MaterialId::Sand);
}

#[test]
fn always_day_holds_noon_light_at_midnight() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    // Advance to midnight by ticking. 20 min * 30 Hz = 36_000 ticks is
    // a lot for an 8×8 tank but each tick is tiny.
    tick_n(&mut tank, 36_000);
    assert!(!tank.is_day_now());
    assert!(tank.day_factor_now() < 0.2);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    tank.tick(&input);
    assert!(tank.is_day_now());
    assert!((tank.day_factor_now() - 1.0).abs() < 1e-4);
}
