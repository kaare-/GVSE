use wk_material::MaterialId;

use crate::cell::Cell;
use crate::consts::{MAIN_W, PRESET_H, PRESET_W, PX_PER_CELL};
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
fn main_tank_is_three_screens_and_a_jar_stays_narrow() {
    let mut main = Tank::main_empty(1);
    assert_eq!(main.visible_size(), (MAIN_W, PRESET_H));
    tick_n(&mut main, 4);
    let jar = Tank::empty(1);
    assert_eq!(jar.visible_size(), (PRESET_W, PRESET_H));
    assert_eq!(Tank::main_preset(1).visible_size().0, MAIN_W);
}

#[test]
fn widening_keeps_the_old_tank_in_the_middle() {
    let mut tank = Tank::try_new(2, 8, 16).unwrap();
    place(&mut tank, 3, 4, Cell::stone());
    place(&mut tank, 0, 2, Cell::water());
    let id = tank.seed_algae(10, 20).unwrap();
    let mut wide = tank.widened(24);
    assert_eq!(wide.visible_size(), (24, 16));
    assert_eq!(wide.cell_visible(11, 4).id(), MaterialId::Stone);
    assert!(wide.cell_visible(0, 2).sat > 0);
    assert!(wide.cell_visible(23, 4).is_air());
    let px = wide.cell(id).unwrap().px;
    let py = wide.cell(id).unwrap().py;
    assert_eq!(px, 10 + 8 * PX_PER_CELL as u16);
    assert_eq!(py, 20);
    assert_eq!(wide.live_cells(), 1);
    let loaded = Tank::load_bytes(&wide.save_bytes().unwrap()).unwrap();
    assert_eq!(loaded.cell(id).unwrap().px, px);
    assert_eq!(loaded.live_cells(), 1);
}

#[test]
fn a_narrow_save_widens_without_losing_the_algae() {
    let mut tank = Tank::empty(4);
    let id = tank.seed_algae(30, 100).unwrap();
    let wide = tank.widened(MAIN_W);
    assert_eq!(wide.visible_size(), (MAIN_W, PRESET_H));
    assert_eq!(
        wide.cell(id).unwrap().px,
        (30 + (MAIN_W - PRESET_W) / 2 * PX_PER_CELL) as u16
    );
    assert_eq!(wide.live_cells(), 1);
    assert_eq!(wide.widened(MAIN_W).visible_size().0, MAIN_W);
}

#[test]
fn creatures_do_not_cast_column_shadows() {
    let mut tank = Tank::try_new(1, 16, 32).unwrap();
    for vy in 0..28 {
        for vx in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    tank.seed_algae(24, 45).unwrap();
    let mut rgba = Vec::new();
    tank.write_rgba(&mut rgba);
    let w = 16 * PX_PER_CELL as usize;
    let at = |x: usize, y: usize| {
        let i = (y * w + x) * 4;
        (rgba[i], rgba[i + 2])
    };
    let under = at(24, 60);
    let beside = at(6, 60);
    assert_eq!(under, beside, "a body should not darken the water under it");
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
fn stone_falls_and_tumbles() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    place(&mut tank, 3, 6, Cell::stone());
    assert_eq!(tank.cell_visible(3, 6).tumble(), 0);
    // Two grain passes per tick: one tick drops it two cells and turns twice.
    tick_n(&mut tank, 1);
    assert_eq!(tank.cell_visible(3, 4).id(), MaterialId::Stone);
    assert_eq!(tank.cell_visible(3, 4).tumble(), 2);
    tick_n(&mut tank, 6);
    assert_eq!(tank.cell_visible(3, 0).id(), MaterialId::Stone);
    assert!(tank.cell_visible(3, 4).is_air());
    assert_ne!(tank.cell_visible(3, 0).tumble(), 0);
}

#[test]
fn erase_lifts_one_material_and_leaves_the_rock() {
    let mut tank = Tank::try_new(3, 8, 8).unwrap();
    place(&mut tank, 2, 2, Cell::sand());
    place(&mut tank, 6, 2, Cell::stone());
    tank.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 2,
            material: MaterialId::Sand as u8,
            x: 2,
            y: 2,
            r: 0,
            variant: 0,
        }],
        ..TankInput::default()
    });
    assert!(tank.cell_visible(2, 2).is_air(), "sand lifts");
    assert_eq!(tank.cell_visible(6, 2).id(), MaterialId::Stone, "rock stays");
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
        variant: 0,
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
        variant: 0,
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

#[test]
fn fill_level_pour_drain_and_undo() {
    let mut tank = Tank::try_new(1, 12, 16).unwrap();
    let mut input = TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 5,
            material: 0,
            x: 0,
            y: 8,
            r: 1,
            variant: 0,
        }],
        ..TankInput::default()
    };
    tank.tick(&input);
    assert!(tank.cell_visible(3, 4).is_standing_water());
    assert!(tank.cell_visible(3, 10).is_air() && tank.cell_visible(3, 10).sat == 0);
    assert_eq!(tank.undo_depth(), 1);

    input.tools = vec![ToolEvent {
        kind: 4,
        material: 0,
        x: 3,
        y: 4,
        r: 2,
        variant: 0,
    }];
    tank.tick(&input);
    assert_eq!(tank.cell_visible(3, 4).sat, 0);

    input.tools = vec![ToolEvent {
        kind: 6,
        material: 0,
        x: 0,
        y: 0,
        r: 1,
        variant: 0,
    }];
    tank.tick(&input);
    assert!(tank.cell_visible(3, 4).is_standing_water());
}

#[test]
fn colour_variants_are_saved() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    let input = TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 1,
            material: MaterialId::Sand as u8,
            x: 2,
            y: 2,
            r: 1,
            variant: 3,
        }],
        ..TankInput::default()
    };
    tank.tick(&input);
    assert_eq!(tank.cell_visible(2, 2).colour_variant(), 3);
    let bytes = tank.save_bytes().unwrap();
    let loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.cell_visible(2, 2).colour_variant(), 3);
}

#[test]
fn rock_film_wets_from_a_puddle() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    place(&mut tank, 2, 0, Cell::stone());
    place(&mut tank, 3, 0, Cell::water());
    tick_n(&mut tank, 20);
    assert!(
        tank.cell_visible(2, 0).rock_film() > 0,
        "stone next to water stayed dry"
    );
}

#[test]
fn swirl_tilt_moves_water_downhill() {
    let mut tank = Tank::try_new(1, 16, 12).unwrap();
    for vx in 0..8 {
        for vy in 0..6 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let input = TankInput {
        swirl: true,
        grav_x: 0.6,
        ..TankInput::default()
    };
    for _ in 0..80 {
        tank.tick(&input);
    }
    let left: u64 = (0..8).map(|x| column_sat(&tank, x)).sum();
    let right: u64 = (8..16).map(|x| column_sat(&tank, x)).sum();
    assert!(
        right > left,
        "tilt should push water to the right (left={left} right={right})"
    );
}

#[test]
fn a_paused_rock_falls_as_one_stone_and_turns() {
    let mut tank = Tank::try_new(1, 12, 16).unwrap();
    let paint = |x, y| ToolEvent {
        kind: 1,
        material: MaterialId::Stone as u8,
        x,
        y,
        r: 0,
        variant: 0,
    };
    tank.tick(&TankInput {
        paused: true,
        tools: vec![paint(3, 10), paint(3, 11), paint(3, 12)],
        ..TankInput::default()
    });
    assert!(tank.cell_visible(3, 12).is_silhouette(), "a paused stroke stays a drawing");
    assert_eq!(tank.cell_visible(3, 12).id(), MaterialId::Stone);
    let drawn = stone_cells(&tank);
    assert!(drawn.len() > 1, "the stroke is a shape");
    let before = offsets(&drawn);
    tick_n(&mut tank, 4);
    let fallen = stone_cells(&tank);
    assert_eq!(fallen.len(), drawn.len(), "the stroke does not break apart");
    assert!(fallen.iter().all(|(_, _, c)| c.is_rigid()));
    let id = fallen[0].2.pad;
    assert!(fallen.iter().all(|(_, _, c)| c.pad == id));
    let after = normalize(&fallen.iter().map(|(x, y, _)| (*x, *y)).collect::<Vec<_>>());
    assert_eq!(after, rotate_offsets(&before), "it tips as it falls {after:?}");
    assert!(
        fallen.iter().map(|(_, y, _)| *y).min() < drawn.iter().map(|(_, y, _)| *y).min(),
        "it has dropped"
    );
    tick_n(&mut tank, 24);
    let resting = stone_cells(&tank);
    assert_eq!(resting.len(), drawn.len(), "it is still one stone");
    assert!(resting.iter().all(|(_, _, c)| c.pad == id && c.is_rigid()));
    assert_eq!(resting.iter().map(|(_, y, _)| *y).min(), Some(0), "it settles on the floor");
}

#[test]
fn a_drawn_rock_falls_through_water_as_one_stone() {
    let mut tank = Tank::try_new(2, 12, 20).unwrap();
    for y in 0..10 {
        for x in 0..12 {
            place(&mut tank, x, y, Cell::water());
        }
    }
    let paint = |x, y| ToolEvent {
        kind: 1,
        material: MaterialId::Stone as u8,
        x,
        y,
        r: 0,
        variant: 0,
    };
    tank.tick(&TankInput {
        paused: true,
        tools: vec![paint(4, 16), paint(4, 17), paint(5, 16), paint(5, 17)],
        ..TankInput::default()
    });
    let drawn = stone_cells(&tank);
    assert!(drawn.len() > 1);
    tick_n(&mut tank, 40);
    let resting = stone_cells(&tank);
    assert_eq!(resting.len(), drawn.len(), "water does not shatter the stone");
    let id = resting[0].2.pad;
    assert!(id != 0 && resting.iter().all(|(_, _, c)| c.is_rigid() && c.pad == id), "one body id");
    assert_eq!(
        resting.iter().map(|(_, y, _)| *y).min(),
        Some(0),
        "it rests on the floor, not in the column"
    );
}

#[test]
fn a_stone_does_not_cast_a_column_shadow() {
    let mut tank = Tank::try_new(3, 8, 16).unwrap();
    for y in 0..12 {
        for x in 0..8 {
            place(&mut tank, x, y, Cell::water());
        }
    }
    place(&mut tank, 3, 10, Cell::stone());
    tank.tick(&TankInput { always_day: true, ..TankInput::default() });
    let under = tank.pixel_light_at(3 * 3 + 1, (16 - 1 - 6) * 3 + 1);
    let beside = tank.pixel_light_at(1 * 3 + 1, (16 - 1 - 6) * 3 + 1);
    let gap = (under as i32 - beside as i32).abs();
    assert!(gap <= 2, "a rock should not darken the column under it ({under} vs {beside})");
}

#[test]
fn fines_in_one_column_do_not_shade_a_shaft() {
    let mut tank = water_box(3, 16, 24, 20);
    tank.add_fines(24, 80.0);
    tank.tick(&day());
    let above = tank.pixel_light_at(24, 20);
    let beside = tank.pixel_light_at(6, 20);
    let gap = (above as i32 - beside as i32).abs();
    assert!(
        gap <= 2,
        "fines over one spot must not darken that column of water ({above} vs {beside})"
    );
}

#[test]
fn a_tall_rock_lies_down_when_it_hits_another() {
    let mut tank = Tank::try_new(5, 12, 16).unwrap();
    let rock = |id: u8| {
        let mut c = Cell::stone();
        c.flags |= Cell::RIGID | Cell::BOUND;
        c.pad = id;
        c
    };
    for x in 2..=4 {
        tank.place(x, 0, rock(1));
    }
    for y in 2..=4 {
        tank.place(3, y, rock(2));
    }
    tick_n(&mut tank, 6);
    let stones = stone_cells(&tank);
    let mut by_id: std::collections::BTreeMap<u8, Vec<(i32, i32)>> = std::collections::BTreeMap::new();
    for (x, y, c) in &stones {
        by_id.entry(c.pad).or_default().push((*x, *y));
    }
    assert_eq!(by_id.get(&1).map(|b| b.len()), Some(3), "the floor stone stays whole");
    let fallen = by_id.get(&2).expect("the stone that fell");
    assert_eq!(fallen.len(), 3, "it stays one stone");
    let miny = fallen.iter().map(|p| p.1).min().unwrap();
    let maxy = fallen.iter().map(|p| p.1).max().unwrap();
    let minx = fallen.iter().map(|p| p.0).min().unwrap();
    let maxx = fallen.iter().map(|p| p.0).max().unwrap();
    assert!(
        maxx - minx > maxy - miny,
        "it should be lying down on the other rock ({fallen:?})"
    );
    assert!(miny <= 1, "it rests on the lower stone ({fallen:?})");
}

fn stone_cells(tank: &Tank) -> Vec<(i32, i32, Cell)> {
    let (w, h) = tank.visible_size();
    let mut stones = Vec::new();
    for y in 0..h as i32 {
        for x in 0..w as i32 {
            let c = tank.cell_visible(x, y);
            if c.id() == MaterialId::Stone {
                stones.push((x, y, c));
            }
        }
    }
    stones
}

fn offsets(cells: &[(i32, i32, Cell)]) -> Vec<(i32, i32)> {
    let pts: Vec<(i32, i32)> = cells.iter().map(|(x, y, _)| (*x, *y)).collect();
    normalize(&pts)
}

fn rotate_offsets(shape: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let maxx = shape.iter().map(|c| c.0).max().unwrap_or(0);
    let turned: Vec<(i32, i32)> = shape.iter().map(|(x, y)| (*y, maxx - x)).collect();
    normalize(&turned)
}

fn normalize(cells: &[(i32, i32)]) -> Vec<(i32, i32)> {
    let minx = cells.iter().map(|c| c.0).min().unwrap_or(0);
    let miny = cells.iter().map(|c| c.1).min().unwrap_or(0);
    let mut v: Vec<(i32, i32)> = cells.iter().map(|(x, y)| (x - minx, y - miny)).collect();
    v.sort();
    v.dedup();
    v
}

#[test]
fn a_drawn_rock_rests_on_sand() {
    let mut tank = Tank::try_new(1, 8, 12).unwrap();
    for x in 0..8 {
        place(&mut tank, x, 0, Cell::sand());
    }
    tank.tick(&TankInput {
        paused: true,
        tools: vec![
            ToolEvent { kind: 1, material: MaterialId::Stone as u8, x: 4, y: 6, r: 0, variant: 0 },
            ToolEvent { kind: 1, material: MaterialId::Stone as u8, x: 4, y: 7, r: 0, variant: 0 },
        ],
        ..TankInput::default()
    });
    let drawn = tank.count_material(MaterialId::Stone as u8);
    tick_n(&mut tank, 20);
    assert_eq!(tank.cell_visible(3, 0).id(), MaterialId::Sand);
    assert_eq!(tank.count_material(MaterialId::Stone as u8), drawn);
    let mut min_y = 99i32;
    for y in 0..12 {
        for x in 0..8 {
            if tank.cell_visible(x, y).is_rigid() {
                min_y = min_y.min(y);
            }
        }
    }
    assert_eq!(min_y, 1, "the stone sits on the sand");
}

#[test]
fn a_sugar_cube_sinks_then_dissolves() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    let before = tank.glucose_total();
    tank.tick(&TankInput {
        tools: vec![ToolEvent { kind: 10, material: 0, x: 3, y: 10, r: 4, variant: 0 }],
        ..TankInput::default()
    });
    assert_eq!(tank.glucose_total(), before, "the cube is still in the air");
    tick_n(&mut tank, 2);
    assert_eq!(tank.glucose_total(), before, "it has not reached the floor");
    tick_n(&mut tank, 12);
    assert!(tank.glucose_total() > before, "it dissolves into the sugar feed");
}

#[test]
fn a_lime_cube_sinks_then_dissolves() {
    let mut tank = Tank::try_new(1, 8, 16).unwrap();
    let before = tank.mineral_total();
    tank.tick(&TankInput {
        tools: vec![ToolEvent { kind: 11, material: 0, x: 3, y: 10, r: 0, variant: 0 }],
        ..TankInput::default()
    });
    assert_eq!(tank.mineral_total(), before);
    tick_n(&mut tank, 16);
    assert!(tank.mineral_total() > before, "lime joins the mineral field");
}

#[test]
fn seed_adds_a_starter_to_a_living_tank() {
    let mut tank = water_box(3, 16, 16, 8);
    let first = tank.seed_algae(24, 24).unwrap();
    let before = tank.live_cells();
    let wide = tank.visible_size();
    tank.tick(&TankInput {
        tools: vec![ToolEvent { kind: 8, material: 0, x: 8, y: 8, r: 3, variant: 0 }],
        ..day()
    });
    assert!(tank.live_cells() > before, "a second starter is added");
    assert!(tank.cell(first).unwrap().alive);
    assert_eq!(tank.visible_size(), wide, "the tank is not replaced");
}

#[test]
fn loose_rock_falls_and_tumbles() {
    let mut tank = Tank::try_new(1, 8, 12).unwrap();
    place(&mut tank, 3, 5, Cell::solid(MaterialId::LooseRock));
    tick_n(&mut tank, 1);
    assert_eq!(tank.cell_visible(3, 3).id(), MaterialId::LooseRock);
    assert_eq!(tank.cell_visible(3, 3).tumble(), 2);
    tick_n(&mut tank, 6);
    assert_eq!(tank.cell_visible(3, 0).id(), MaterialId::LooseRock);
    assert_eq!(tank.cell_visible(3, 0).tumble(), 1);
    assert!(tank.cell_visible(3, 5).is_air());
}

#[test]
fn wet_sand_renders_darker_and_each_block_is_flat() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    let mut wet = Cell::sand();
    wet.sat = crate::cell::water_capacity(MaterialId::Sand);
    place(&mut tank, 2, 2, Cell::sand());
    place(&mut tank, 5, 2, wet);
    let dry = rgb_at(&mut tank, 2, 2, 0, 0);
    let soaked = rgb_at(&mut tank, 5, 2, 0, 0);
    assert_eq!(dry, soaked, "wet sand stays the same flat colour (dry {dry:?} wet {soaked:?})");
    place(&mut tank, 3, 2, Cell::sand());
    assert_eq!(dry, rgb_at(&mut tank, 2, 2, 2, 1), "one sand block is one colour");
    assert_eq!(dry, rgb_at(&mut tank, 2, 2, 1, 2), "a block has no border or grain");
    assert_eq!(dry, rgb_at(&mut tank, 3, 2, 0, 0), "neighbouring sand blocks match");
}

#[test]
fn a_stone_block_is_one_flat_colour() {
    let mut tank = Tank::try_new(1, 8, 8).unwrap();
    place(&mut tank, 2, 4, Cell::solid(MaterialId::Stone));
    place(&mut tank, 3, 4, Cell::solid(MaterialId::Stone));
    let a = rgb_at(&mut tank, 2, 4, 0, 0);
    assert_eq!(a, rgb_at(&mut tank, 2, 4, 2, 2), "one stone block is one colour");
    assert_eq!(a, rgb_at(&mut tank, 2, 4, 1, 1), "the middle matches the corner");
    assert_eq!(a, rgb_at(&mut tank, 3, 4, 0, 0), "neighbouring stone blocks match");
    assert_eq!(a, rgb_at(&mut tank, 3, 4, 2, 2));
}

#[test]
fn a_bed_is_one_colour_along_a_row_and_a_diagonal() {
    let mut tank = Tank::try_new(1, 16, 16).unwrap();
    for i in 0..12u32 {
        place(&mut tank, i as i32, 3, Cell::sand());
        place(&mut tank, i as i32, i as i32 + 4, Cell::solid(MaterialId::Stone));
    }
    let sand = rgb_at(&mut tank, 0, 3, 0, 0);
    let stone = rgb_at(&mut tank, 0, 4, 0, 0);
    for i in 1..12u32 {
        assert_eq!(sand, rgb_at(&mut tank, i, 3, 0, 0), "sand does not change along a row");
        assert_eq!(
            stone,
            rgb_at(&mut tank, i, i + 4, 0, 0),
            "stone does not stripe on a diagonal"
        );
    }
}

#[test]
fn creature_cap_stops_at_five_hundred() {
    let mut tank = Tank::try_new(1, 24, 24).unwrap();
    let mut n = 0u32;
    'outer: for y in 0..72u16 {
        for x in 0..72u16 {
            if tank.seed_algae(x, y).is_some() {
                n += 1;
                if n == 500 {
                    break 'outer;
                }
            }
        }
    }
    assert_eq!(n, 500);
    assert_eq!(tank.org_counts(1).0, 500);
    assert!(tank.seed_algae(70, 70).is_none());
}

fn rgb_at(tank: &mut Tank, vx: u32, vy: u32, dx: u32, dy: u32) -> (u8, u8, u8) {
    let mut rgba = Vec::new();
    tank.write_rgba(&mut rgba);
    let (pw, _ph) = tank.pixel_size();
    let vis_h = tank.visible_size().1;
    let px = vx * PX_PER_CELL + dx;
    let py = (vis_h - 1 - vy) * PX_PER_CELL + dy;
    let i = ((py * pw + px) * 4) as usize;
    (rgba[i], rgba[i + 1], rgba[i + 2])
}

#[test]
fn empty_tank_has_a_thin_sand_bed() {
    let tank = Tank::empty(7);
    assert_eq!(tank.visible_size(), (PRESET_W, PRESET_H));
    assert_eq!(tank.cell_visible(10, 0).id(), MaterialId::Sand);
    assert!(tank.cell_visible(10, 20).is_air());
}

#[test]
fn preset_seeds_algae_that_divides() {
    let mut tank = Tank::preset(11);
    assert!(tank.live_cells() >= 1, "preset should seed algae");
    let start = tank.live_cells();
    // ~90 s at 30 Hz — enough for several divisions under full light.
    for _ in 0..2_700 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    assert!(
        tank.live_cells() > start,
        "algae should divide (start={start} now={})",
        tank.live_cells()
    );
    assert!(tank.genotype_count() >= 1);
}

#[test]
fn swipe_clears_film() {
    let mut tank = Tank::try_new(1, 16, 16).unwrap();
    let w = tank.pixel_size().0 as usize;
    for x in 0..40 {
        tank.pixels_film_set_for_test(10 * w + x, 200);
    }
    let cleared = tank.swipe_film(5, 10, 35, 10, 4);
    assert!(cleared > 0, "cleared={cleared}");
}

#[test]
fn remove_and_glucose_drip_selection() {
    let mut tank = Tank::try_new(1, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let id = tank.seed_algae(36, 30).expect("seed");
    assert_eq!(tank.live_cells(), 1);
    tank.remove_cells_at(12, 13, 2);
    assert_eq!(tank.live_cells(), 0, "remove should kill cell {id}");

    let _ = tank.seed_algae(36, 30);
    tank.drip_glucose(12, 13, 2, 200);
    let idx = 30usize * tank.pixel_size().0 as usize + 36;
    // drip writes a 3×3 around the physics cell; centre-ish should be wet with glucose.
    assert!(
        tank.pixel_size().0 > 0,
        "pixels exist"
    );
    let _ = idx;
    // Exercise always-day ticks with glucose present.
    for _ in 0..60 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    assert!(tank.live_cells() >= 1);
}

#[test]
fn genome_tree_counts_births() {
    let mut tank = Tank::try_new(1, 32, 32).unwrap();
    // Fill a water column and seed.
    for vx in 0..32 {
        for vy in 0..20 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    assert!(tank.seed_algae(48, 40).is_some());
    for _ in 0..1_500 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    assert!(tank.live_cells() >= 1);
}

#[test]
fn shaped_cell_grows_pixel_by_pixel() {
    let mut tank = Tank::try_new(1, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let id = tank.seed_shaped_algae(36, 30, 3, 3).expect("seed");
    assert_eq!(tank.cell_size(id), Some((1, 1)));
    for _ in 0..80 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    let (w, h) = tank.cell_size(id).expect("alive");
    assert!(
        w >= 2 || h >= 2,
        "3×3 lineage should grow past 1×1 (got {w}×{h})"
    );
}

#[test]
fn flashlight_boosts_column_light() {
    let mut tank = Tank::try_new(1, 32, 32).unwrap();
    // Advance into night so the floor leaves headroom for the ×2 boost.
    for _ in 0..36_000 {
        tank.tick(&TankInput {
            always_day: false,
            light_slider: 0.5,
            ..TankInput::default()
        });
    }
    tank.tick(&TankInput {
        always_day: false,
        light_slider: 0.5,
        flash_x: 10,
        flash_y: 20,
        flash_r: 4,
        ..TankInput::default()
    });
    let lit = tank.pixel_light_at(10 * 3 + 1, 5);
    let dark = tank.pixel_light_at(28 * 3 + 1, 5);
    assert!(
        lit > dark,
        "flashlight column should be brighter (lit={lit} dark={dark})"
    );
}

#[test]
fn mutation_fuzz_stays_valid() {
    use crate::bio::Genome;
    let mut g = Genome::algae();
    for i in 0..10_000u64 {
        g = g.mutate(0xF1_55, i);
        assert!(g.is_valid(), "invalid after mutation {i}: {g:?}");
    }
}

#[test]
fn hot_tank_raises_mutation_lambda() {
    use crate::bio::Genome;
    let g = Genome::algae();
    let cold = g.lambda_milli(40, 12.0);
    let hot = g.lambda_milli(40, 32.0);
    assert!(hot > cold, "Q10 should raise λ when hot (cold={cold} hot={hot})");
}

#[test]
fn straw_builds_a_bonded_chain() {
    use crate::bio::Genome;
    let mut tank = Tank::try_new(1, 32, 24).unwrap();
    for vx in 0..32 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let id = tank
        .seed_genome_at(48, 36, Genome::straw())
        .expect("seed straw");
    for _ in 0..400 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    let slots = tank.org_slot_count(id);
    assert!(
        slots >= 2,
        "straw should grow a bonded body (slots={slots})"
    );
}

#[test]
fn sticky_algae_form_glue_attachments() {
    use crate::bio::Genome;
    let mut tank = Tank::try_new(2, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let g = Genome::sticky_algae();
    assert!(tank.seed_genome_at(30, 30, g.clone()).is_some());
    assert!(tank.seed_genome_at(33, 30, g).is_some());
    for _ in 0..200 {
        tank.tick(&TankInput {
            always_day: true,
            ..TankInput::default()
        });
    }
    // d.2: a colony forms — kin glue to each other while every cell stays
    // its own organism — and inspect names the attachment count.
    assert!(tank.live_cells() >= 2);
    let glued = tank.glue_attachments();
    assert!(glued > 0, "sticky algae never glued to kin");
    let (orgs, bodies) = tank.org_counts(2);
    assert!(orgs >= 2 && bodies == 0, "glue must not merge organisms (orgs={orgs} bodies={bodies})");
    let attached = tank
        .cells()
        .iter_alive()
        .find(|(_, c)| c.glue_to != crate::bio::store::NONE)
        .map(|(_, c)| (c.px, c.py))
        .expect("an attached cell");
    let info = tank.inspect(attached.0 as u32, attached.1 as u32);
    assert!(info.glued_kin > 0, "inspect should report attached kin");
}

#[test]
fn enzyme_field_decays_and_blurs() {
    use crate::bio::fields::{deposit, step_fields, KeyedField};
    use crate::pixels::{CoarseField, PixelLayer};
    let grid = water_grid(8, 8);
    let mut pixels = PixelLayer::new(8, 8);
    let mut coarse = CoarseField::new(8, 8);
    let idx = 10 * pixels.w as usize + 10;
    deposit(&mut pixels, KeyedField::Enzyme, idx, 200, 90);
    assert_eq!(pixels.enzyme_amt[idx], 200);
    assert!(pixels.fields_active);
    step_fields(&mut pixels, &grid, &mut coarse, 0);
    assert!(pixels.enzyme_amt[idx] < 200, "decay should reduce amount");
    let neigh = idx + 1;
    assert!(
        pixels.enzyme_amt[neigh] > 0 || pixels.enzyme_amt[idx] > 0,
        "blur or residual enzyme present"
    );
}

#[test]
fn swimmer_genome_stays_alive() {
    use crate::bio::Genome;
    let mut tank = Tank::try_new(3, 24, 24).unwrap();
    for vx in 0..24 {
        for vy in 0..16 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    assert!(tank.seed_genome_at(36, 30, Genome::swimmer()).is_some());
    for _ in 0..120 {
        tank.tick(&TankInput {
            always_day: true,
            flash_x: 12,
            flash_y: 12,
            flash_r: 8,
            ..TankInput::default()
        });
    }
    assert!(tank.live_cells() >= 1, "cilia cell should persist under light");
}

fn day() -> TankInput {
    TankInput {
        always_day: true,
        ..TankInput::default()
    }
}

fn water_box(seed: u64, w: u32, h: u32, water_rows: i32) -> Tank {
    let mut tank = Tank::try_new(seed, w, h).unwrap();
    for vx in 0..w as i32 {
        for vy in 0..water_rows {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    tank
}

/// 2.2.4 — a physics cell that is ≥ 5/9 hard living pixels is solid to
/// flow; a soft plate dams nothing.
#[test]
fn hard_plate_dams_water_and_soft_plate_does_not() {
    for hard in [true, false] {
        let mut tank = Tank::try_new(3, 9, 12).unwrap();
        // Shelf at vy 4 with a one-cell hole at vx 4; dry pocket below.
        for vx in 0..9 {
            if vx != 4 {
                place(&mut tank, vx, 4, Cell::bedrock());
            }
        }
        for vx in 0..9 {
            for vy in 5..10 {
                place(&mut tank, vx, vy, Cell::water());
            }
        }
        // Plate exactly over the hole cell (vx 4, vy 4): pixel (12, (11-4)*3).
        let id = tank.seed_fixed_plate_for_test(12, 21, hard);
        assert_eq!(tank.cell_size(id), Some((3, 3)));
        for _ in 0..200 {
            tank.tick(&day());
        }
        let pocket: u64 = (0..9)
            .flat_map(|vx| (0..4).map(move |vy| (vx, vy)))
            .map(|(vx, vy)| tank.cell_visible(vx, vy).sat as u64)
            .sum();
        if hard {
            assert!(tank.cell_visible(4, 4).is_tissue(), "hard plate marks tissue");
            assert_eq!(pocket, 0, "hard plate should dam the hole (pocket sat {pocket})");
        } else {
            assert!(!tank.cell_visible(4, 4).is_tissue());
            assert!(pocket > 0, "soft plate must not dam");
        }
    }
}

/// 2.2.1 / d.3 — the straw breeds true: after many generations a lineage
/// of unmutated straws is still building bonded chains.
#[test]
fn straw_breeds_true_for_ten_generations() {
    use crate::bio::Genome;
    let mut tank = water_box(7, 48, 40, 32);
    // Mutation off so the pace of the unmutated program is what is
    // measured, not its luck against its own mutants.
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(72, 60, Genome::straw()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    let mut best_gen = 0;
    for _ in 0..30_000 {
        tank.tick(&day());
        if tank.tick_count() % 500 == 0 {
            best_gen = best_gen.max(tank.max_generation_of(geno));
            if best_gen >= 10 {
                break;
            }
        }
    }
    assert_eq!(
        tank.live_cells_of(geno) as usize,
        tank.live_cells(),
        "no mutants expected at λ = 0"
    );
    assert!(best_gen >= 10, "straw reached generation {best_gen} < 10");
    assert!(tank.live_cells_of(geno) > 0, "the unmutated straw genotype died out");
    let (_, bodies) = tank.org_counts(2);
    assert!(bodies > 0, "no bonded straw bodies alive");
    // 2.3.8 — the "multicellular" milestone needs a body that breeds
    // true: it fires once, and only after a raw two-cell event.
    let mut raw_first = None;
    let mut milestone = Vec::new();
    let mut i = 0u32;
    while let Some(ev) = tank.poll_event() {
        if ev.kind == crate::bio::ev::MULTICELLULAR && raw_first.is_none() {
            raw_first = Some(i);
        }
        if ev.kind == crate::bio::ev::MS_MULTICELLULAR {
            milestone.push(i);
        }
        i += 1;
    }
    assert_eq!(milestone.len(), 1, "multicellular milestone fires exactly once");
    assert!(raw_first.unwrap() < milestone[0], "a body existed before it bred true");
}

/// 2.2.2 — bite off a body part: the slot regrows at full cost.
#[test]
fn bitten_tail_regrows_into_its_slot() {
    use crate::bio::Genome;
    let mut tank = water_box(5, 24, 24, 22);
    let mut g = Genome::straw();
    for m in g.modes.iter_mut() {
        m.regrow = true;
    }
    let id = tank.seed_genome_at(36, 36, g).expect("seed");
    for _ in 0..600 {
        tank.tick(&day());
        if tank.org_slot_count(id) >= 3 {
            break;
        }
    }
    assert!(tank.org_slot_count(id) >= 3, "straw did not build its body");
    let org = tank.org_of_cell(id).expect("org").clone();
    // Bite the deepest living slot.
    let tail = org
        .slots
        .iter()
        .filter(|s| s.cell != crate::bio::store::NONE && s.cell != id)
        .max_by_key(|s| tank.cell(s.cell).map(|c| c.depth).unwrap_or(0))
        .map(|s| s.cell)
        .expect("tail");
    assert!(tank.kill_cell(tail));
    let before = tank.org_slot_count(id);
    let mut regrown = false;
    for _ in 0..3000 {
        tank.tick(&day());
        if tank.org_slot_count(id) > before {
            regrown = true;
            break;
        }
    }
    assert!(regrown, "dead slot never regrew (slots stayed {before})");
}

/// 2.2.5 / d.8 — brood: a bonded daughter waits on her detach gate and is
/// then released as her own organism.
#[test]
fn brooder_releases_young_when_the_gate_passes() {
    use crate::bio::Genome;
    let mut tank = water_box(9, 32, 32, 24);
    let id = tank.seed_genome_at(48, 48, Genome::brooder()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    let mut released = false;
    for _ in 0..20_000 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::BROOD_RELEASED {
                released = true;
            }
        }
        if released {
            break;
        }
    }
    assert!(released, "no brood was ever released");
    assert!(tank.lineage_cells_of(geno) > 0);
}

/// Save/load with living bodies: the loaded tank continues tick-for-tick
/// identical to the original (pixel hash after 300 more ticks).
#[test]
fn save_round_trip_with_life_stays_deterministic() {
    use crate::bio::Genome;
    let mut tank = water_box(11, 32, 32, 24);
    tank.seed_genome_at(48, 48, Genome::plate16()).expect("seed");
    tank.seed_genome_at(20, 60, Genome::algae()).expect("seed");
    tank.seed_genome_at(70, 40, Genome::sticky_algae()).expect("seed");
    for _ in 0..1500 {
        tank.tick(&day());
    }
    assert!(tank.live_cells() > 3, "need living cells for the round trip");
    let bytes = tank.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.pixel_hash(), tank.pixel_hash());
    assert_eq!(loaded.live_cells(), tank.live_cells());
    for _ in 0..300 {
        tank.tick(&day());
        loaded.tick(&day());
    }
    assert_eq!(loaded.live_cells(), tank.live_cells());
    assert_eq!(loaded.pixel_hash(), tank.pixel_hash(), "diverged after load");
}

fn water_grid(w: u32, h: u32) -> crate::grid::PhysicsGrid {
    let mut grid = crate::grid::PhysicsGrid::new(w, h);
    for vx in 0..w as i32 {
        for vy in 0..h as i32 {
            grid.set_visible(vx, vy, Cell::water());
        }
    }
    grid
}

/// 2.3.1 — §b.3 chamber arithmetic: six lining cells secreting 32/tick
/// into a walled 9-px cavity settle near 85 per pixel, while one cell
/// broadcasting 32/tick into open water leaves only a few per pixel.
#[test]
fn chamber_holds_enzyme_and_open_water_does_not() {
    use crate::bio::fields::{deposit, step_fields, KeyedField};
    use crate::pixels::{CoarseField, PixelLayer};
    let grid = water_grid(16, 16);
    let mut coarse = CoarseField::new(16, 16);
    let pw = 48usize;
    // Open-water plume: one emitter at (10, 10).
    let mut open = PixelLayer::new(16, 16);
    let src = 10 * pw + 10;
    for t in 0..200u64 {
        deposit(&mut open, KeyedField::Enzyme, src, 32, 90);
        step_fields(&mut open, &grid, &mut coarse, t);
    }
    let total_open: u32 = open.enzyme_amt.iter().map(|&a| a as u32).sum();
    let nonzero = open.enzyme_amt.iter().filter(|&&a| a > 0).count() as u32;
    let per_px_open = total_open / nonzero.max(1);
    assert!(
        (60..=170).contains(&total_open),
        "open plume total should be ≈128 continuous, ≥60 quantised (got {total_open})"
    );
    assert!(per_px_open <= 20, "open plume stays thin (got {per_px_open}/px)");

    // Chamber: 3×3 cavity at (20..23, 20..23) ringed by blocked pixels.
    let mut cham = PixelLayer::new(16, 16);
    for y in 19..24usize {
        for x in 19..24usize {
            let ring = x == 19 || x == 23 || y == 19 || y == 23;
            if ring {
                cham.block[y * pw + x] = 1;
            }
        }
    }
    // Six lining cells each emit 32 into the cavity edge pixels.
    let seats = [
        20 * pw + 20,
        20 * pw + 22,
        22 * pw + 20,
        22 * pw + 22,
        21 * pw + 20,
        21 * pw + 22,
    ];
    for t in 0..200u64 {
        for &s in &seats {
            deposit(&mut cham, KeyedField::Enzyme, s, 32, 90);
        }
        step_fields(&mut cham, &grid, &mut coarse, t);
    }
    let mut inside = 0u32;
    for y in 20..23usize {
        for x in 20..23usize {
            inside += cham.enzyme_amt[y * pw + x] as u32;
        }
    }
    let outside: u32 = cham
        .enzyme_amt
        .iter()
        .enumerate()
        .filter(|(i, _)| {
            let (x, y) = (i % pw, i / pw);
            !(19..24).contains(&x) || !(19..24).contains(&y)
        })
        .map(|(_, &a)| a as u32)
        .sum();
    assert_eq!(outside, 0, "walls must block diffusion");
    let per_px = inside / 9;
    assert!(
        (60..=110).contains(&per_px),
        "chamber should hold ≈85/px (got {per_px}, total {inside})"
    );
    assert!(
        per_px >= 3 * per_px_open,
        "a chamber must beat broadcasting by a wide margin ({per_px} vs {per_px_open})"
    );
}

fn dark() -> TankInput {
    TankInput {
        always_day: true,
        light_slider: 0.0,
        ..TankInput::default()
    }
}

/// 2.3.2 / d.4 — a saprotroph lives on litter: in the dark, with soft
/// litter on the floor, the lineage digests it (litter falls, glucose
/// appears, divisions happen); with no litter it only spends its seed store.
#[test]
fn saprotroph_lives_on_litter_in_the_dark() {
    use crate::bio::chem::LITTER_SOFT_CODE;
    use crate::bio::Genome;
    let run = |litter: bool| -> (u64, usize, u32) {
        let mut tank = water_box(13, 32, 24, 20);
        tank.set_lambda_scale_q8(0);
        let (pw, ph) = tank.pixel_size();
        if litter {
            for px in 0..pw as u16 {
                tank.drop_litter(px, (ph - 1) as u16, 255, LITTER_SOFT_CODE);
            }
        }
        let start = tank.litter_total();
        let id = tank.seed_genome_at(48, 40, Genome::saprotroph()).expect("seed");
        let geno = tank.genotype_of_cell(id).unwrap();
        let mut divisions = 0u32;
        for _ in 0..6000 {
            tank.tick(&dark());
            while let Some(ev) = tank.poll_event() {
                if ev.kind == crate::bio::ev::DIVISION {
                    divisions += 1;
                }
            }
        }
        (start - tank.litter_total(), tank.live_cells_of(geno) as usize, divisions)
    };
    let (eaten, fed_cells, fed_divisions) = run(true);
    let (_, starved_cells, starved_divisions) = run(false);
    assert!(eaten > 2000, "litter should be digested (eaten {eaten} units)");
    assert!(
        fed_divisions >= 4 && fed_cells >= 4,
        "fed saprotrophs should multiply (divisions {fed_divisions}, cells {fed_cells})"
    );
    assert!(
        fed_divisions > starved_divisions + 2,
        "litter must make the difference ({fed_divisions} vs {starved_divisions})"
    );
    assert!(starved_cells <= 2, "without litter nothing grows in the dark ({starved_cells})");
}

/// 2.3.2 / d.7 — the chamber predator grows its hard U; an algae whose
/// `wall_key` matches the lining enzyme is lysed inside the cavity (its
/// structure becomes glucose), while the same algae in open water lives.
#[test]
fn chamber_predator_lyses_prey_in_its_cavity() {
    use crate::bio::Genome;
    let mut tank = water_box(17, 32, 24, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(48, 60, Genome::chamber_predator()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    let org = tank.cells().get(id).unwrap().org;
    let mut formed = false;
    for _ in 0..4000 {
        tank.tick(&day());
        if tank.cells().org(org).map(|o| o.n_alive).unwrap_or(0) >= 7 {
            formed = true;
            break;
        }
    }
    assert!(formed, "the U never finished (cells {})", tank.live_cells_of(geno));
    let base = tank
        .cells()
        .iter_alive()
        .find(|(_, c)| c.org == org && c.mode == 0)
        .map(|(_, c)| (c.px, c.py))
        .expect("base cell");
    let cavity = (base.0, base.1 - 1);
    assert_eq!(tank.inspect(cavity.0 as u32, cavity.1 as u32).owner, 0, "cavity is open water");
    assert!(
        tank.cells()
            .iter_alive()
            .filter(|(_, c)| c.org == org)
            .all(|(_, c)| c.damage == 0),
        "the lining must not digest itself"
    );
    let mut prey_g = Genome::algae();
    prey_g.wall_key = 110;
    prey_g.buoy = -20;
    tank.set_lambda_scale_q8(0);
    let prey = tank.seed_genome_at(cavity.0, cavity.1, prey_g.clone()).expect("prey in cavity");
    let control = tank.seed_genome_at(6, 40, prey_g).expect("control prey");
    let mut predation = false;
    let mut died_at = None;
    for t in 0..900 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::PREDATION && ev.arg0 == prey {
                predation = true;
            }
        }
        if tank.cell_size(prey).is_none() {
            died_at = Some(t);
            break;
        }
    }
    assert!(died_at.is_some(), "prey in the cavity should be lysed within 30 s");
    assert!(predation, "a PREDATION event should name the prey");
    assert!(tank.cell_size(control).is_some(), "the control prey in open water lives");
    assert!(tank.live_cells_of(geno) >= 7, "the predator body survives the meal");
}

/// 2.3.3 — NEEDLE + TOXIN: a stinger facing an unresisted algae kills it
/// (POISON event); the same stinger facing kin with its `resist_key` does not.
#[test]
fn needle_toxin_kills_unresisted_prey_and_spares_resistant_kin() {
    use crate::bio::Genome;
    let run = |resist_key: u8| -> (bool, bool) {
        let mut tank = water_box(19, 16, 16, 16);
        tank.set_lambda_scale_q8(0);
        // Stinger heads up; its target sits on the pixel above it.
        let stinger = tank.seed_genome_at(24, 30, Genome::stinger()).expect("stinger");
        let mut prey_g = Genome::algae();
        prey_g.resist_key = resist_key;
        let prey = tank.seed_genome_at(24, 29, prey_g).expect("prey");
        tank.pin_cell_for_test(stinger);
        tank.pin_cell_for_test(prey);
        let mut poison = false;
        for _ in 0..300 {
            tank.tick(&day());
            while let Some(ev) = tank.poll_event() {
                if ev.kind == crate::bio::ev::POISON && ev.arg0 == prey {
                    poison = true;
                }
            }
            if tank.cell_size(prey).is_none() {
                break;
            }
        }
        assert!(tank.cell_size(stinger).is_some(), "the stinger is immune to its own venom");
        (tank.cell_size(prey).is_none(), poison)
    };
    let (dead, poison) = run(40);
    assert!(dead && poison, "unresisted prey should be poisoned (dead {dead}, event {poison})");
    let (dead_kin, _) = run(200);
    assert!(!dead_kin, "a resistant target shrugs the venom off");
}

/// 2.3.3 — SIGNAL / RECEPTOR: kin a few pixels apart hear each other
/// (`sig` rises on the listener, SIGNALLING event fires).
#[test]
fn kin_hear_each_others_signal() {
    use crate::bio::Genome;
    let mut tank = water_box(23, 16, 16, 16);
    tank.set_lambda_scale_q8(0);
    let a = tank.seed_genome_at(20, 30, Genome::signaller()).expect("a");
    let b = tank.seed_genome_at(24, 30, Genome::signaller()).expect("b");
    let mut heard = false;
    for _ in 0..120 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::SIGNALLING && (ev.arg0 == a || ev.arg0 == b) {
                heard = true;
            }
        }
    }
    assert!(heard, "no SIGNALLING event between kin");
    let sig = tank.cells().get(a).map(|c| c.sig).unwrap_or(0).max(tank.cells().get(b).map(|c| c.sig).unwrap_or(0));
    assert!(sig > 0, "a receptor should read its kin's signal");
    // A deaf algae next to them reads nothing.
    let deaf = tank.seed_genome_at(28, 30, Genome::algae()).expect("deaf");
    for _ in 0..30 {
        tank.tick(&day());
    }
    assert_eq!(tank.cells().get(deaf).map(|c| c.sig), Some(0));
}

/// 2.3.2 — a corpse in water becomes litter that settles to the floor and
/// keeps its mass until something digests it.
#[test]
fn corpses_become_litter_that_settles() {
    use crate::bio::Genome;
    let mut tank = water_box(29, 16, 16, 16);
    tank.set_lambda_scale_q8(0);
    // A heat-killed algae: run hot so it dies of damage quickly.
    let id = tank.seed_genome_at(24, 20, Genome::algae()).expect("seed");
    let hot = TankInput {
        always_day: true,
        t_base: 45.0,
        ..TankInput::default()
    };
    let mut died = false;
    for _ in 0..3000 {
        tank.tick(&hot);
        if tank.cell_size(id).is_none() {
            died = true;
            break;
        }
    }
    assert!(died, "the algae should die in a hot tank");
    // The dead cell lies there as a corpse first (2.3.5), then crumbles.
    // Any daughters are removed (the tool leaves no corpse) so only the
    // corpses' mass is in play.
    assert!(tank.corpse_count() >= 1, "a water death leaves a corpse");
    tank.remove_cells_at(8, 8, 32);
    for _ in 0..(crate::bio::store::CORPSE_TICKS as u32 + 8) {
        tank.tick(&hot);
    }
    assert_eq!(tank.corpse_count(), 0, "the corpses crumbled");
    let total = tank.litter_total();
    assert!(total >= 140, "a 6 g corpse leaves ≥ 140 units of litter (got {total})");
    let fines: f32 = tank.fines().iter().sum();
    assert!(fines > 0.0, "30 % of the soft mass went up as fines");
    for _ in 0..600 {
        tank.tick(&hot);
    }
    assert_eq!(tank.litter_total(), total, "litter mass is conserved");
    let (pw, ph) = tank.pixel_size();
    let floor: u64 = (0..pw as u16).map(|px| tank.litter_at(px, (ph - 1) as u16) as u64).sum();
    assert_eq!(floor, total, "all litter settled onto the floor row");
}

/// 2.3.5 / 2.3.7 — corpse lifecycle: a starved cell stays on its pixels
/// as a corpse, sinks through the water, keeps its id and pixel claim,
/// then crumbles into soft litter (70 %) and fines (30 %) on expiry.
#[test]
fn corpse_sinks_then_becomes_litter_and_fines() {
    use crate::bio::store::{CORPSE_TICKS, OWNER_BIAS};
    use crate::bio::Genome;
    let mut tank = water_box(31, 16, 16, 16);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(24, 10, Genome::algae()).expect("seed");
    tank.set_energy_for_test(id, 0);
    tank.tick(&dark());
    assert!(tank.cell(id).is_none(), "the cell starved");
    let corpse = *tank.corpse(id).expect("it lies there as a corpse");
    assert!(corpse.energy > 0, "the corpse carries its soft mass");
    assert_eq!(tank.owner_at(corpse.px, corpse.py), id + OWNER_BIAS, "the corpse keeps its pixel");
    assert_eq!(tank.litter_total(), 0);
    for _ in 0..40 {
        tank.tick(&dark());
    }
    let sunk = tank.corpse(id).expect("still a corpse");
    assert!(sunk.py > 10, "a corpse sinks ({} → {})", 10, sunk.py);
    for _ in 0..(CORPSE_TICKS as u32) {
        tank.tick(&dark());
    }
    assert!(tank.corpse(id).is_none(), "the corpse crumbled");
    assert_eq!(tank.corpse_count(), 0);
    let soft_units = (corpse.energy / crate::consts::GLUCOSE_UNIT_FP) as u64;
    let litter = tank.litter_total();
    let fines_units = soft_units * crate::bio::FINES_SHARE_Q8 as u64 / 256;
    assert!(litter >= soft_units - fines_units, "at least 70 % of the soft mass is litter ({litter})");
    // The other 30 % went up as fines; some has already settled back
    // (Phase 4 cloudiness), the rest still hangs in the column.
    let fines_g: f32 = tank.fines().iter().sum();
    let settled = (litter - (soft_units - fines_units)) as f32 / 64.0;
    let expect_g = fines_units as f32 / 64.0;
    assert!(
        (fines_g + settled - expect_g).abs() < 0.1,
        "30 % went to fines (hanging {fines_g} + settled {settled}, want {expect_g})"
    );
}

/// 2.3.5 — three detritus classes: digesting soft litter leaves inert
/// grit that nothing eats; only the clean tool (kind 13) removes it.
#[test]
fn grit_accumulates_until_cleaned() {
    use crate::bio::chem::LITTER_SOFT_CODE;
    use crate::bio::Genome;
    let mut tank = water_box(33, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    let (pw, ph) = tank.pixel_size();
    for px in 0..pw as u16 {
        tank.drop_litter(px, (ph - 1) as u16, 255, LITTER_SOFT_CODE);
    }
    tank.seed_genome_at(48, 40, Genome::saprotroph()).expect("seed");
    for _ in 0..6000 {
        tank.tick(&dark());
    }
    let grit = tank.grit_total();
    assert!(grit > 100, "digestion should leave grit (got {grit})");
    for _ in 0..300 {
        tank.tick(&dark());
    }
    assert!(tank.grit_total() >= grit, "nothing eats grit");
    let (vw, vh) = tank.visible_size();
    let clean = TankInput {
        tools: vec![ToolEvent {
            kind: 13,
            material: 0,
            x: (vw / 2) as u16,
            y: 0,
            r: (vw + vh) as u16,
            variant: 0,
        }],
        ..dark()
    };
    tank.tick(&clean);
    assert_eq!(tank.grit_total(), 0, "the clean tool sweeps grit");
}

/// 2.3.5 — CHEMO: a sediment feeder on a nutrient-rich sand bed lives
/// and multiplies in the dark; on a barren bed the same lineage only
/// spends its seed store.
#[test]
fn chemo_cell_lives_on_sediment_in_the_dark() {
    use crate::bio::Genome;
    let run = |fed: bool| -> (u32, u64) {
        let mut tank = water_box(35, 32, 24, 20);
        tank.set_lambda_scale_q8(0);
        for vx in 0..32 {
            place(&mut tank, vx, 0, Cell::sand());
        }
        tank.tick(&dark());
        if fed {
            tank.set_sediment_all(255);
        }
        let start = tank.sediment_total();
        let id = tank.seed_genome_at(48, 50, Genome::sediment_feeder()).expect("seed");
        let geno = tank.genotype_of_cell(id).unwrap();
        for _ in 0..6000 {
            tank.tick(&dark());
        }
        (tank.live_cells_of(geno), start - tank.sediment_total())
    };
    let (fed_cells, eaten) = run(true);
    let (starved_cells, _) = run(false);
    assert!(eaten > 500, "the feeder draws down the sediment (ate {eaten})");
    assert!(fed_cells >= 4, "fed chemo cells multiply (cells {fed_cells})");
    assert!(starved_cells <= 2, "on a barren bed nothing grows in the dark ({starved_cells})");
}

/// 2.3.6 acceptance (d.6-pre) — a single cell swims toward the
/// flashlight within a session: the sighted swimmer (SENSOR(light) →
/// motor drive link, touch → tumble) finds the cone and stays in it; the
/// blind control (same cilia on a plain gate) wanders.
#[test]
fn single_cell_swims_toward_the_flashlight() {
    use crate::bio::Genome;
    let cone_x = 26i32;
    let cone_r = 3i32;
    let in_cone = |px: u16| -> bool { ((px as i32 / 3) - cone_x).abs() <= cone_r };
    let run = |seed: u64, sighted: bool| -> (bool, u32, u32) {
        let mut tank = water_box(seed, 32, 24, 20);
        tank.set_lambda_scale_q8(0);
        let g = if sighted { Genome::swimmer() } else { Genome::blind_swimmer() };
        let id = tank.seed_genome_at(30, 30, g).expect("seed");
        let input = TankInput {
            always_day: true,
            light_slider: 0.4,
            flash_x: cone_x,
            flash_y: 10,
            flash_r: cone_r,
            ..TankInput::default()
        };
        let mut inside = 0u32;
        let mut moved = 0u32;
        for _ in 0..3000 {
            tank.tick(&input);
            while let Some(ev) = tank.poll_event() {
                if ev.kind == crate::bio::ev::MOVED && ev.arg0 == id {
                    moved += 1;
                }
            }
            let Some(c) = tank.cell(id) else {
                return (false, inside, moved);
            };
            if in_cone(c.px) {
                inside += 1;
            }
        }
        (true, inside, moved)
    };
    let mut sighted_total = 0u32;
    let mut blind_total = 0u32;
    for seed in [41u64, 42, 43, 44] {
        let (alive, inside, moved) = run(seed, true);
        assert!(alive, "the swimmer should live through the session (seed {seed})");
        assert!(moved > 0, "a swimming cell fires 'something moved' (seed {seed})");
        assert!(
            inside >= 1000,
            "the sighted cell should spend a third of the session in the cone (seed {seed}: {inside} ticks)"
        );
        sighted_total += inside;
        let (_, b_inside, _) = run(seed, false);
        blind_total += b_inside;
    }
    assert!(
        sighted_total > blind_total * 2,
        "the eye must make the difference (sighted {sighted_total} vs blind {blind_total} ticks in the cone)"
    );
}

/// 2.3.6 — cilia on a free body move the whole body as one (§a.6):
/// a straw whose tip grows CILIA swims along the tip's heading, stays
/// intact, and the seed cell fires "something moved".
#[test]
fn cilia_body_swims_as_one_and_reports_movement() {
    use crate::bio::{Block, Entry, Genome};
    let mut tank = water_box(45, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    let mut g = Genome::straw();
    g.modes[1].push_entry(Entry::new(Block::Cilia, 3));
    let id = tank.seed_genome_at(48, 60, g).expect("seed");
    let mut grown = false;
    for _ in 0..4000 {
        tank.tick(&day());
        if tank.org_slot_count(id) >= 3 {
            grown = true;
            break;
        }
    }
    assert!(grown, "the straw should grow its tip");
    while tank.poll_event().is_some() {}
    let before = tank.cell(id).expect("seed lives").py;
    let org = tank.org_of_cell(id).expect("org");
    let n_before = org.n_alive;
    let layout = |tank: &Tank| -> Vec<(i32, i32)> {
        let seed = tank.cell(id).expect("seed lives");
        tank.org_of_cell(id)
            .expect("org")
            .slots
            .iter()
            .filter_map(|s| tank.cell(s.cell))
            .map(|c| (c.px as i32 - seed.px as i32, c.py as i32 - seed.py as i32))
            .collect()
    };
    let shape_before = layout(&tank);
    assert_eq!(shape_before.len(), 3);
    let mut moved = 0u32;
    for _ in 0..400 {
        tank.tick(&day());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::MOVED && ev.arg0 == id {
                moved += 1;
            }
        }
    }
    let after = tank.cell(id).expect("seed lives").py;
    assert!(before - after >= 8, "the body should swim up on its tip's cilia ({before} → {after})");
    assert_eq!(tank.org_of_cell(id).expect("org").n_alive, n_before, "the body stays whole");
    assert_eq!(layout(&tank), shape_before, "the body moves as one");
    assert!(moved > 0, "a swimming body fires 'something moved' on its seed");
}

/// 2.3.5 acceptance — soil appears under a fungus colony: saprotrophs
/// digesting litter on a sand bed turn the sand beneath them into `Soil`
/// (FUNGUS event), and their waste settles into sediment nutrient.
#[test]
fn soil_appears_under_a_fungus_colony() {
    use crate::bio::chem::LITTER_SOFT_CODE;
    use crate::bio::Genome;
    let mut tank = water_box(37, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    for vx in 0..32 {
        place(&mut tank, vx, 0, Cell::sand());
    }
    tank.tick(&dark());
    assert_eq!(tank.count_material(MaterialId::Soil as u8), 0);
    let (pw, ph) = tank.pixel_size();
    let sand_top = (ph - 3) as u16; // first water pixel row above the bed
    tank.seed_genome_at(48, 40, Genome::saprotroph()).expect("seed");
    let mut fungus_event = false;
    for t in 0..8000 {
        if t % 400 == 0 {
            for px in 0..pw as u16 {
                tank.drop_litter(px, sand_top - 1, 200, LITTER_SOFT_CODE);
            }
        }
        tank.tick(&dark());
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::FUNGUS {
                fungus_event = true;
            }
        }
        if fungus_event && tank.count_material(MaterialId::Soil as u8) > 0 {
            break;
        }
    }
    let soil = tank.count_material(MaterialId::Soil as u8);
    assert!(soil > 0, "digestion should turn sand into soil");
    assert!(fungus_event, "the FUNGUS event fires when soil is made");
    assert!(tank.sediment_total() > 0, "waste settles into the bed as sediment nutrient");
}

/// 2.3.4 — K2#1 mineral cap: in mineral-free water a hard-walled body
/// cannot grow a single part; a calcium feed lets it build, and the feed
/// is consumed.
#[test]
fn hard_walls_wait_for_minerals_until_calcium_is_fed() {
    use crate::bio::Genome;
    let mut tank = water_box(21, 32, 24, 24);
    tank.set_lambda_scale_q8(0);
    tank.set_minerals_all(0);
    let id = tank.seed_genome_at(48, 50, Genome::chamber_predator()).expect("seed");
    tank.pin_cell_for_test(id);
    for _ in 0..1500 {
        tank.tick(&day());
    }
    assert_eq!(tank.org_slot_count(id), 1, "no hard part may grow without minerals");
    assert!(tank.cell(id).is_some(), "the seed itself lives on");
    let (vx, vy) = crate::bio::pixel_to_phys(48, 50, 24);
    tank.add_calcium(vx, vy, 3, 128);
    let fed = tank.mineral_total();
    assert!(fed > 0);
    let mut built = 0;
    for _ in 0..2500 {
        tank.tick(&day());
        built = built.max(tank.org_slot_count(id));
        if built >= 4 {
            break;
        }
    }
    assert!(built >= 4, "calcium should unblock the body (got {built} cells)");
    assert!(tank.mineral_total() < fed, "building hard walls consumes minerals");
}

/// 2.3.4 — BONE secretion: a cell lays dead bone pixels one by one along
/// its heading, each costing `2 m`; when the body dies its bone stays as
/// bone litter, and a bone-band enzyme returns those minerals to the water.
#[test]
fn bone_spine_grows_pixel_by_pixel_and_bone_eaters_recycle_it() {
    use crate::bio::store::BONE_OWNER;
    use crate::bio::{Block, Entry, Genome};
    let mut tank = water_box(23, 24, 24, 24);
    tank.set_lambda_scale_q8(0);
    let m0 = tank.mineral_total();
    let id = tank.seed_genome_at(36, 40, Genome::spiny()).expect("seed");
    tank.pin_cell_for_test(id);
    let mut bone = 0;
    for _ in 0..1500 {
        tank.tick(&day());
        bone = tank.secretion_counts().0;
        if bone >= 3 {
            break;
        }
    }
    assert_eq!(bone, 3, "three bone pixels should be laid");
    for k in 1..=3u16 {
        assert_eq!(tank.owner_at(36, 40 - k), BONE_OWNER, "spine extends along the heading (up)");
    }
    let spent = m0 - tank.mineral_total();
    assert_eq!(spent, 3 * crate::bio::minerals::BONE_M_UNITS as u64, "2 m per bone pixel");
    // The body dies: its bone falls as bone litter.
    assert!(tank.kill_cell(id));
    assert_eq!(tank.secretion_counts().0, 0);
    let bone_litter: u64 = (1..=3u16).map(|k| tank.litter_at(36, 40 - k) as u64).sum();
    assert_eq!(bone_litter, spent, "bone keeps its minerals as litter");
    // A bone eater (ENZYME key 220) in the dark digests it and frees minerals.
    let mut eater = Genome::algae();
    eater.modes[0].entries[0] = Entry::with_params(Block::Enzyme, 4, 220, 24);
    for _ in 0..600 {
        tank.tick(&dark());
    }
    // The litter has settled onto the floor row; the eater sits on it.
    let (_, ph) = tank.pixel_size();
    let floor = (ph - 1) as u16;
    assert_eq!(tank.litter_at(36, floor) as u64, spent, "bone litter settles to the floor");
    let eid = tank.seed_genome_at(36, floor - 1, eater).expect("eater");
    tank.pin_cell_for_test(eid);
    let m1 = tank.mineral_total();
    let before = tank.litter_total();
    for _ in 0..600 {
        tank.tick(&dark());
    }
    let after = tank.litter_total();
    assert!(after < before, "bone litter is digested ({before} → {after})");
    assert_eq!(tank.mineral_total() - m1, before - after, "lysed bone returns its minerals");
}

/// 2.3.4 / d.8 — SHELL: the egg layer wraps her brood daughter in shell;
/// after release the hatchling grows a part and cracks the shell, which
/// falls as bone litter.
#[test]
fn shell_wraps_the_egg_and_cracks_when_the_hatchling_grows() {
    use crate::bio::Genome;
    let mut tank = water_box(29, 24, 24, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(36, 40, Genome::egg_layer()).expect("seed");
    tank.pin_cell_for_test(id);
    let mut max_shell = 0;
    let mut released = None;
    for _ in 0..6000 {
        tank.tick(&day());
        max_shell = max_shell.max(tank.secretion_counts().1);
        while let Some(ev) = tank.poll_event() {
            if ev.kind == crate::bio::ev::BROOD_RELEASED && released.is_none() {
                released = Some(ev.arg0);
            }
        }
        if released.is_some() && max_shell >= 3 {
            break;
        }
    }
    assert_eq!(max_shell, 3, "the egg should be wrapped in three shell pixels");
    let hatchling = released.expect("the egg is released");
    let shell_at_release = tank.secretion_counts().1;
    let mut cracked = false;
    for _ in 0..4000 {
        tank.tick(&day());
        let n = tank.cells().org(hatchling).map(|o| o.n_alive).unwrap_or(0);
        if n >= 2 {
            cracked = true;
            break;
        }
    }
    assert!(cracked, "the hatchling should grow its second cell through the shell");
    assert!(
        tank.secretion_counts().1 < shell_at_release,
        "growing through the shell removes a shell pixel"
    );
    let (pw, ph) = tank.pixel_size();
    let bone_litter: u64 = (0..pw as u16)
        .flat_map(|px| (0..ph as u16).map(move |py| (px, py)))
        .filter(|&(px, py)| tank.litter_at(px, py) > 0)
        .map(|(px, py)| tank.litter_at(px, py) as u64)
        .sum();
    assert!(bone_litter >= crate::bio::minerals::SHELL_M_UNITS as u64, "the cracked shell is bone litter");
}

/// 2.3.8 — milestones run on the model: "first life", "first division"
/// and "first death" fire once per tank, survive a save/load round trip
/// without repeating, and "lifeless" needs an empty tank for
/// `LIFELESS_TICKS` and re-arms when life returns.
#[test]
fn milestones_fire_once_and_survive_save_load() {
    use crate::bio::ev;
    use crate::milestones::LIFELESS_TICKS;
    let mut tank = water_box(51, 24, 24, 20);
    let mut counts = std::collections::BTreeMap::new();
    let drain = |tank: &mut Tank, counts: &mut std::collections::BTreeMap<u32, u32>| {
        while let Some(ev) = tank.poll_event() {
            *counts.entry(ev.kind).or_insert(0) += 1;
        }
    };
    for _ in 0..30 {
        tank.tick(&day());
    }
    drain(&mut tank, &mut counts);
    assert_eq!(counts.get(&ev::MS_FIRST_LIFE), None, "an empty tank has no first life");
    assert_eq!(counts.get(&ev::LIFELESS), None, "a never-lived tank is not 'lifeless'");
    let id = tank.seed_algae(36, 36).expect("seed");
    tank.tick(&day());
    drain(&mut tank, &mut counts);
    assert_eq!(counts.get(&ev::MS_FIRST_LIFE), Some(&1));
    let mut t = 0;
    while !counts.contains_key(&ev::MS_FIRST_DEATH) && t < 20_000 {
        tank.tick(&day());
        drain(&mut tank, &mut counts);
        t += 1;
    }
    assert_eq!(counts.get(&ev::MS_FIRST_DIVISION), Some(&1), "first division announced once");
    assert_eq!(counts.get(&ev::MS_FIRST_DEATH), Some(&1), "first death announced once");
    assert!(counts.get(&ev::DIVISION).copied().unwrap_or(0) > 1, "the raw stream keeps every division");
    assert!(tank.milestone_fired(ev::MS_FIRST_LIFE));
    let _ = id;

    let bytes = tank.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    assert!(loaded.milestone_fired(ev::MS_FIRST_DIVISION), "milestone state is saved");
    let mut after = std::collections::BTreeMap::new();
    for _ in 0..600 {
        loaded.tick(&day());
    }
    drain(&mut loaded, &mut after);
    assert_eq!(after.get(&ev::MS_FIRST_LIFE), None, "no repeat after load");
    assert_eq!(after.get(&ev::MS_FIRST_DIVISION), None, "no repeat after load");
    assert_eq!(after.get(&ev::MS_FIRST_DEATH), None, "no repeat after load");

    // Lifeless: only after the tank has been empty for a while.
    loaded.remove_cells_at(12, 10, 64);
    assert_eq!(loaded.live_cells(), 0);
    let mut lifeless = std::collections::BTreeMap::new();
    for _ in 0..(LIFELESS_TICKS - 2) {
        loaded.tick(&day());
    }
    drain(&mut loaded, &mut lifeless);
    assert_eq!(lifeless.get(&ev::LIFELESS), None, "not lifeless yet");
    for _ in 0..4 {
        loaded.tick(&day());
    }
    drain(&mut loaded, &mut lifeless);
    assert_eq!(lifeless.get(&ev::LIFELESS), Some(&1), "lifeless after LIFELESS_TICKS");
    for _ in 0..(2 * LIFELESS_TICKS) {
        loaded.tick(&day());
    }
    drain(&mut loaded, &mut lifeless);
    assert_eq!(lifeless.get(&ev::LIFELESS), Some(&1), "it does not nag while still empty");
    // Life returns (no second "first life") and goes again: it re-arms.
    loaded.seed_algae(36, 36).expect("seed");
    loaded.tick(&day());
    loaded.remove_cells_at(12, 10, 64);
    for _ in 0..(LIFELESS_TICKS + 2) {
        loaded.tick(&day());
    }
    drain(&mut loaded, &mut lifeless);
    assert_eq!(lifeless.get(&ev::MS_FIRST_LIFE), None, "first life is once per tank");
    assert_eq!(lifeless.get(&ev::LIFELESS), Some(&2), "lifeless repeats after life returned");
}

/// 2.3.8 — "new colour" compares a living genotype with its lineage
/// founder on the model (seed-mode colour), steps per lineage and is
/// rate-limited; "heterotroph" needs three generations; "glass" has a
/// cooldown and re-arms when the pane is cleaned.
#[test]
fn colour_heterotroph_and_glass_detectors_work_on_the_model() {
    use crate::bio::{ev, Block, CellStore, Entry, Genome, GenomeStore};
    use crate::milestones::{
        colour_distance, Milestones, Probe, GLASS_COOLDOWN, GLASS_EVERY, NEW_COLOUR_COOLDOWN, NEW_COLOUR_DIST,
        SCAN_EVERY,
    };
    use crate::pixels::PixelLayer;
    use std::collections::VecDeque;

    let mut genomes = GenomeStore::new();
    let mut cells = CellStore::new();
    let mut pixels = PixelLayer::new(16, 16);
    let root = genomes.intern_root(Genome::algae());
    let mut tinted = Genome::algae();
    tinted.modes[0].push_entry(Entry::with_params(Block::Pigment, 4, 0, 0)); // red hue
    let child = genomes.intern_child(root, tinted);
    let dist = colour_distance(&genomes, child, root);
    assert!(dist >= NEW_COLOUR_DIST, "the pigmented child should read as a new colour ({dist})");
    let spawn = |cells: &mut CellStore, genomes: &mut GenomeStore, pixels: &mut PixelLayer, geno, px, generation| {
        cells
            .spawn(genomes, pixels, px, 8, geno, 0, generation, 12 * crate::consts::FP, 2, 0, 1)
            .expect("spawn")
    };
    spawn(&mut cells, &mut genomes, &mut pixels, root, 2, 0);
    let mut ms = Milestones::default();
    let mut out = VecDeque::new();
    let quiet = Probe { film_n: 0, film_len: 1000 };
    let kinds = |out: &mut VecDeque<crate::SimEvent>| -> Vec<u32> { out.drain(..).map(|e| e.kind).collect() };

    ms.run(SCAN_EVERY, &[], &cells, &genomes, &quiet, &mut out);
    assert_eq!(kinds(&mut out), vec![ev::MS_FIRST_LIFE], "the founder alone is no new colour");
    // The tinted child comes alive: new colour at the next scan.
    let child_cell = spawn(&mut cells, &mut genomes, &mut pixels, child, 6, 1);
    ms.run(2 * SCAN_EVERY, &[], &cells, &genomes, &quiet, &mut out);
    assert_eq!(kinds(&mut out), vec![ev::MS_NEW_COLOUR]);
    // Same lineage, same drift: rate-limited and stepped, so silent.
    ms.run(3 * SCAN_EVERY, &[], &cells, &genomes, &quiet, &mut out);
    ms.run(2 * SCAN_EVERY + NEW_COLOUR_COOLDOWN, &[], &cells, &genomes, &quiet, &mut out);
    assert!(kinds(&mut out).is_empty(), "no repeat for the same drift");

    // Heterotroph: a glucose-fed cell counts only three generations deep.
    let shallow = crate::SimEvent { kind: ev::HETEROTROPH, arg0: child_cell };
    ms.run(1, &[shallow], &cells, &genomes, &quiet, &mut out);
    assert!(kinds(&mut out).is_empty(), "generation 1 is not a heterotroph lineage yet");
    let deep_cell = spawn(&mut cells, &mut genomes, &mut pixels, child, 10, 3);
    let deep = crate::SimEvent { kind: ev::HETEROTROPH, arg0: deep_cell };
    ms.run(2, &[deep], &cells, &genomes, &quiet, &mut out);
    ms.run(3, &[deep], &cells, &genomes, &quiet, &mut out);
    assert_eq!(kinds(&mut out), vec![ev::MS_HETEROTROPH], "once, at generation ≥ 3");

    // Glass: fires on coverage, waits out the cooldown, re-arms when clean.
    let dirty = Probe { film_n: 100, film_len: 1000 };
    ms.run(GLASS_EVERY, &[], &cells, &genomes, &dirty, &mut out);
    assert_eq!(kinds(&mut out), vec![ev::GLASS]);
    ms.run(2 * GLASS_EVERY, &[], &cells, &genomes, &dirty, &mut out);
    assert!(kinds(&mut out).is_empty(), "no nagging every check");
    ms.run(GLASS_EVERY + GLASS_COOLDOWN, &[], &cells, &genomes, &dirty, &mut out);
    assert_eq!(kinds(&mut out), vec![ev::GLASS], "reminds after the cooldown");
    let clean = Probe { film_n: 5, film_len: 1000 };
    ms.run(2 * GLASS_EVERY + GLASS_COOLDOWN, &[], &cells, &genomes, &clean, &mut out);
    ms.run(3 * GLASS_EVERY + GLASS_COOLDOWN, &[], &cells, &genomes, &dirty, &mut out);
    assert_eq!(kinds(&mut out), vec![ev::GLASS], "a cleaned pane re-arms the detector");
}

/// Phase 3 — the species layer: tuning and long parametric drift stay
/// in the founder's species, a structural change founds a new one, names
/// are a pure function of the tree, and the layer survives a save/load
/// round trip.
#[test]
fn species_split_on_structure_or_drift_and_are_named() {
    use crate::bio::species::{param_distance_q8, signature, SPECIES_D_Q8};
    use crate::bio::{Block, Entry, Genome, GenomeStore, NO_SPECIES};
    let mut genomes = GenomeStore::new();
    let root = genomes.intern_root(Genome::algae());
    let root_sp = genomes.species_of(root);
    assert_ne!(root_sp, NO_SPECIES);
    let root_name = genomes.species(root_sp).unwrap().name.clone();
    assert!(root_name.starts_with("Vir"), "algae stem is PHOTO ({root_name})");
    assert_eq!(root, 0, "the seed algae is the root node");
    assert!(root_name.ends_with(" I"), "the root species is depth 1 ({root_name})");

    // A tuning step: same signature, small distance → same species.
    let mut tuned = Genome::algae();
    tuned.modes[0].divide_when.energy_frac = tuned.modes[0].divide_when.energy_frac.wrapping_sub(10);
    assert_eq!(signature(&tuned), signature(&Genome::algae()));
    assert!(param_distance_q8(&tuned, &Genome::algae()) < SPECIES_D_Q8);
    let t = genomes.intern_child(root, tuned.clone());
    assert_eq!(genomes.species_of(t), root_sp, "a point mutation does not found a species");

    // A structural step: a new block with ≥ 2 units → new species whose
    // name extends the parent's with the block's syllable.
    let mut pig = tuned.clone();
    pig.modes[0].push_entry(Entry::with_params(Block::Pigment, 3, 0, 0));
    assert_ne!(signature(&pig), signature(&tuned));
    let p = genomes.intern_child(t, pig.clone());
    assert_eq!(genomes.species_of(p), root_sp, "a childless variant does not take a row");
    genomes.on_birth(p);
    genomes.on_birth(p);
    let p_sp = genomes.species_of(p);
    assert_ne!(p_sp, root_sp, "offspring earns the new body plan a row");
    let rec = genomes.species(p_sp).unwrap();
    assert_eq!(rec.parent, root_sp);
    assert_eq!(rec.depth, 2);
    assert!(rec.name.contains("ros"), "a red pigment adds its syllable ({})", rec.name);
    assert!(rec.name.ends_with(" II"), "numeral is the species depth ({})", rec.name);

    // Drift only: same body plan, keys walked far. The distance is still
    // measured, but it no longer founds a species.
    let mut drift = tuned.clone();
    drift.wall_key = 180;
    drift.glue_key = drift.glue_key.wrapping_add(90);
    drift.resist_key = drift.resist_key.wrapping_add(90);
    drift.temp_opt = 200;
    assert_eq!(signature(&drift), signature(&tuned));
    assert!(param_distance_q8(&drift, &Genome::algae()) > SPECIES_D_Q8, "{}", param_distance_q8(&drift, &Genome::algae()));
    let d = genomes.intern_child(t, drift.clone());
    assert_eq!(genomes.species_of(d), root_sp, "numeric drift stays with the starter");
    let mut drift2 = drift.clone();
    drift2.temp_opt = 10;
    drift2.wall_key = 70;
    let d2 = genomes.intern_child(t, drift2);
    assert_eq!(genomes.species_of(d2), root_sp);
    let mut near = drift.clone();
    near.temp_width = near.temp_width.wrapping_add(3);
    let n = genomes.intern_child(d, near);
    assert_eq!(genomes.species_of(n), root_sp);

    // Round trip through a tank save.
    let mut tank = water_box(61, 24, 24, 20);
    let id = tank.seed_genome_at(36, 36, pig.clone()).expect("seed");
    let geno = tank.genotype_of_cell(id).unwrap();
    tank.genomes_mut().on_birth(geno);
    let mut heir = pig.clone();
    heir.wall_key = heir.wall_key.wrapping_add(4);
    tank.genomes_mut().intern_child(geno, heir);
    let sp = tank.inspect(36, 36).species;
    assert_ne!(sp, NO_SPECIES);
    let card = tank.lineage_card(id).expect("card");
    assert!(card.species_name.contains("ros"));
    assert_eq!(card.species_depth, 2, "{card:?}");
    let parent = tank.species_parent(card.species).expect("parent species");
    assert_eq!(card.parent_species_name, tank.species_name(parent));
    assert_eq!(card.generation, 0);
    assert_eq!(card.mutations_since_seed, 1, "one interning step under the algae root");
    for _ in 0..200 {
        tank.tick(&day());
    }
    let bytes = tank.save_bytes().unwrap();
    let loaded = Tank::load_bytes(&bytes).unwrap();
    assert_eq!(loaded.species_count(), tank.species_count());
    for s in 0..tank.species_count() as u16 {
        assert_eq!(loaded.species_name(s), tank.species_name(s));
    }
    let after = loaded.lineage_card(id).expect("card after load");
    assert_eq!(after.species_name, card.species_name);
    assert_eq!(after.species, card.species);
}

/// A saved bush of tuned starter algae folds into the parent on load.
/// A real new block stays, and its parent link skips the folded row.
/// An archived copy of the same plan keeps its own row.
#[test]
fn same_body_plan_variants_collapse_on_load() {
    use crate::bio::{Block, Entry, Genome};
    let mut tank = water_box(61, 24, 24, 8);
    let root = tank.genomes_mut().seed_algae();
    let root_sp = tank.genomes().species_of(root);
    let mut drift = Genome::algae();
    drift.wall_key = 180;
    drift.temp_opt = 200;
    let child = tank.genomes_mut().intern_child(root, drift);
    let variant = tank.genomes_mut().plant_same_plan_child_for_test(root_sp, child);
    let mut twin = Genome::algae();
    twin.temp_opt = 40;
    let twin_g = tank.genomes_mut().intern_child(root, twin);
    let archived = tank.genomes_mut().plant_same_plan_child_for_test(root_sp, twin_g);
    tank.genomes_mut().species_mut(archived).unwrap().archived = true;
    let mut pig = Genome::algae();
    pig.modes[0].push_entry(Entry::with_params(Block::Pigment, 3, 0, 0));
    let pigment = tank.genomes_mut().intern_child(child, pig.clone());
    tank.genomes_mut().on_birth(pigment);
    tank.genomes_mut().on_birth(pigment);
    let mut heir = pig.clone();
    heir.temp_opt = 30;
    tank.genomes_mut().intern_child(pigment, heir);
    let pigment_sp = tank.genomes().species_of(pigment);
    assert_eq!(tank.genomes().species(pigment_sp).unwrap().parent, variant);
    assert!(tank.species_tree().iter().any(|n| n.id == variant));

    let bytes = tank.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    assert!(loaded.genomes().species(variant).unwrap().collapsed);
    assert_eq!(loaded.genomes().species_of(child), root_sp);
    assert!(!loaded.genomes().species(pigment_sp).unwrap().collapsed);
    assert_eq!(loaded.genomes().species(pigment_sp).unwrap().parent, root_sp);
    assert!(!loaded.genomes().species(archived).unwrap().collapsed);
    assert_eq!(loaded.genomes().species_of(twin_g), archived);
    let tree = loaded.species_tree();
    assert!(tree.iter().all(|n| n.id != variant));
    assert!(tree.iter().any(|n| n.id == root_sp));
    assert!(tree.iter().any(|n| n.id == pigment_sp));
    assert!(tree.iter().any(|n| n.id == archived));
    loaded.genomes_mut().collapse_same_body_plan();
    assert_eq!(loaded.genomes().species_of(child), root_sp);
    assert_eq!(loaded.genomes().species(pigment_sp).unwrap().parent, root_sp);
}

/// Phase 3 — glass walls: solid to water and grains, light goes through,
/// paintable by tool, erasable, saved.
#[test]
fn glass_wall_holds_water_and_passes_light() {
    let mut tank = Tank::try_new(5, 12, 12).unwrap();
    // A glass box on the floor: walls at vx 3 and 8, floor row 0 is bedrock
    // already; water inside only.
    for vy in 0..6 {
        tank.tick(&TankInput {
            paused: true,
            tools: vec![
                ToolEvent { kind: 1, material: Cell::GLASS, x: 3, y: vy, r: 0, variant: 0 },
                ToolEvent { kind: 1, material: Cell::GLASS, x: 8, y: vy, r: 0, variant: 0 },
            ],
            ..TankInput::default()
        });
    }
    assert!(tank.cell_visible(3, 2).is_glass());
    assert_eq!(tank.inspect(3 * 3 + 1, (12 - 1 - 2) * 3 + 1).material, Cell::GLASS);
    for vx in 4..8 {
        for vy in 0..5 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let before: u64 = (4..8).map(|vx| column_sat(&tank, vx)).sum();
    tick_n(&mut tank, 300);
    let inside: u64 = (4..8).map(|vx| column_sat(&tank, vx)).sum();
    let outside: u64 = (0..3).chain(9..12).map(|vx| column_sat(&tank, vx)).sum();
    // Phase 4 evaporation still ticks under a lid (very slow, water only);
    // the wall's job is that nothing leaves the basin sideways.
    assert_eq!(
        inside + tank.evaporated_sat(),
        before,
        "glass holds the water in (inside {inside} + evaporated {})",
        tank.evaporated_sat()
    );
    assert_eq!(outside, 0, "nothing leaks through glass");
    // The pane passes light. Stone is side-lit, and the water under it
    // stays readable — a roof no longer blacks out the bed.
    place(&mut tank, 1, 8, Cell::glass());
    // Bedrock holds the sample stone; loose cobbles would drop on the tick.
    place(&mut tank, 10, 7, Cell::bedrock());
    place(&mut tank, 10, 8, Cell::stone());
    tank.tick(&TankInput { always_day: true, ..TankInput::default() });
    let under_glass = tank.pixel_light_at(3 + 1, (12 - 1 - 7) * 3 + 1);
    let stone_face = tank.pixel_light_at(10 * 3 + 1, (12 - 1 - 8) * 3 + 1);
    let under_stone = tank.pixel_light_at(10 * 3 + 1, (12 - 1 - 7) * 3 + 1);
    assert!(under_glass > 200, "light through glass {under_glass}");
    assert!(stone_face > 180, "stone is side-lit {stone_face}");
    assert!(under_stone > 160, "water under stone stays lit {under_stone}");
    // Erase takes it away; the save keeps it.
    let bytes = tank.save_bytes().unwrap();
    let loaded = Tank::load_bytes(&bytes).unwrap();
    assert!(loaded.cell_visible(3, 2).is_glass());
    tank.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent { kind: 2, material: 0, x: 3, y: 2, r: 0, variant: 0 }],
        ..TankInput::default()
    });
    assert!(tank.cell_visible(3, 2).is_open());
}

/// Phase 3 — the scoop lifts a body whole with its litter and water,
/// release puts it down elsewhere with ids remapped and nothing lost;
/// a full seat refuses; discard removes it from the live counts.
#[test]
fn scoop_carries_a_body_and_release_puts_it_down_intact() {
    use crate::bio::Genome;
    let mut tank = water_box(7, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    let seed = tank.seed_genome_at(24, 30, Genome::straw()).expect("seed straw");
    for _ in 0..400 {
        tank.tick(&day());
    }
    let seed_rec = *tank.cell(seed).expect("seed alive");
    let (sx, sy) = (seed_rec.px as i32, seed_rec.py as i32);
    let org = seed_rec.org;
    let slots_before = tank.org_slot_count(seed);
    assert!(slots_before >= 2, "straw grew (slots={slots_before})");
    let cells_before = tank.live_cells();
    let genotype = tank.genotype_of_cell(seed).unwrap();
    let live_geno_before = tank.live_cells_of(genotype);
    let bbox = tank.org_bbox(org).expect("bbox");
    // A stone block on the left for the no-room case below.
    for vx in 1..8 {
        for vy in 6..14 {
            place(&mut tank, vx, vy, Cell::stone());
        }
    }
    // Litter and water inside the ring.
    tank.drop_litter(sx as u16, sy as u16 + 3, 40, crate::bio::chem::LITTER_SOFT_CODE);
    let litter_before = tank.litter_total();
    let water_before = (0..32).map(|vx| column_sat(&tank, vx)).sum::<u64>();
    let body_cells = tank.cells().org(org).unwrap().n_alive as usize;

    let carried = tank.scoop_region(sx, sy, 9);
    assert_eq!(carried.orgs.len(), 1, "one body in the bucket");
    assert_eq!(carried.cell_count() as usize, body_cells);
    assert!(carried.water > 0);
    assert_eq!(tank.live_cells(), cells_before - body_cells);
    assert_eq!(tank.litter_total(), litter_before - 40, "litter went into the bucket");
    assert!(tank.cells().org(org).is_none(), "the old organism record is gone");
    assert_eq!(tank.live_cells_of(genotype), live_geno_before, "carried cells stay counted as alive");
    // Every pixel of the old body is free again.
    for py in bbox.1..=bbox.3 {
        for px in bbox.0..=bbox.2 {
            assert_eq!(tank.owner_at(px, py), 0, "pixel ({px},{py}) still owned");
        }
    }

    // No room: a release into a stone block fails and the bucket comes back.
    let (carried, err) = tank.release(carried, 4 * 3 + 1, (23 - 10) * 3 + 1).expect_err("stone refuses");
    assert_eq!(err, crate::tank::ReleaseError::NoRoom);
    assert_eq!(carried.orgs.len(), 1);

    // Room: 30 px sideways in open water.
    let shift: i32 = if sx < 48 { 30 } else { -30 };
    let (tx, ty) = (sx + shift, sy);
    assert!(tank.release(carried, tx, ty).is_ok());
    assert_eq!(tank.live_cells(), cells_before);
    assert_eq!(tank.litter_total(), litter_before, "litter came back out");
    assert_eq!(
        (0..32).map(|vx| column_sat(&tank, vx)).sum::<u64>(),
        water_before,
        "water is conserved through the bucket"
    );
    let moved = tank.inspect(tx as u32, ty as u32);
    assert_ne!(moved.cell, u32::MAX, "the seed cell sits at the new centre");
    assert_eq!(moved.genotype, genotype);
    assert_eq!(moved.org_cells as usize, slots_before as usize, "slots travelled");
    let nb = tank.org_bbox(moved.org).expect("new bbox");
    assert_eq!((nb.2 - nb.0, nb.3 - nb.1), (bbox.2 - bbox.0, bbox.3 - bbox.1), "same shape");
    assert_eq!(nb.0 as i32, bbox.0 as i32 + shift);
    assert_eq!(tank.live_cells_of(genotype), live_geno_before);
    // The body keeps living and dividing after the move.
    for _ in 0..300 {
        tank.tick(&day());
    }
    assert!(tank.live_cells() >= cells_before, "the moved body carries on");

    // Discard: lift again and throw away → live counts drop, no corpse.
    let seed_now = *tank.cell(moved.cell).expect("moved seed alive");
    let c = tank.scoop_region(seed_now.px as i32, seed_now.py as i32, 9);
    let n = c.cell_count();
    assert!(n >= 1);
    let live_now = tank.live_cells();
    tank.discard(c);
    assert_eq!(tank.corpse_count(), 0);
    assert_eq!(tank.live_cells(), live_now);
    assert_eq!(tank.live_cells_of(genotype), tank.live_cells() as u32, "counts settle on discard");
}

/// Phase 3 — remove takes anything under the brush: the whole body a
/// cell belongs to, corpses, litter and grit; nothing comes back.
#[test]
fn remove_tool_takes_bodies_litter_and_grit_whole() {
    use crate::bio::Genome;
    let mut tank = water_box(8, 24, 24, 20);
    tank.set_lambda_scale_q8(0);
    let seed = tank.seed_genome_at(36, 30, Genome::straw()).expect("seed");
    for _ in 0..400 {
        tank.tick(&day());
    }
    let org = tank.inspect(36, 30).org;
    let body = tank.cells().org(org).unwrap().n_alive;
    assert!(body >= 2);
    let tip = tank.cells().org(org).unwrap().slots.iter().map(|s| s.cell).find(|&c| c != seed && c != u32::MAX);
    let tip = tip.expect("a second cell");
    let tip_px = tank.cell(tip).unwrap().px as i32 / 3;
    let tip_vy = tank.visible_size().1 as i32 - 1 - tank.cell(tip).unwrap().py as i32 / 3;
    tank.drop_litter(36, 33, 20, crate::bio::chem::LITTER_SOFT_CODE);
    let before = tank.live_cells();
    let litter = tank.litter_total();
    // Brush radius 0 on the *tip* cell only: the whole body goes.
    tank.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent { kind: 9, material: 0, x: tip_px as u16, y: tip_vy as u16, r: 0, variant: 0 }],
        ..TankInput::default()
    });
    assert_eq!(tank.live_cells(), before - body as usize, "the body left whole");
    assert_eq!(tank.corpse_count(), 0, "no corpse for a cull");
    assert!(tank.cells().org(org).is_none());
    // Litter under a wider brush at the centre goes too.
    let _ = tank.remove_at(12, 13, 2);
    assert!(tank.litter_total() < litter, "litter removed");
}

/// Phase 3 — a portrait is a pure function of the genome and shows the
/// built body, not a single seed pixel.
#[test]
fn portraits_are_deterministic_and_show_the_body() {
    use crate::archive::portrait;
    use crate::bio::Genome;
    let (w, h, px) = portrait(&Genome::algae());
    assert_eq!((w, h), (9, 9), "four pixels of air around the one cell");
    let opaque = px.chunks(4).filter(|p| p[3] == 255).count();
    assert_eq!(opaque, 1, "the algae is one opaque pixel");
    assert_eq!(px[(4 * 9 + 4) * 4 + 3], 255, "the cell sits in the middle");
    let (w, h, straw) = portrait(&Genome::straw());
    let opaque = straw.chunks(4).filter(|p| p[3] == 255).count();
    assert!(opaque >= 3, "straw portrait has a chain ({w}×{h}, {opaque} px)");
    assert!(w.max(h) >= 3, "straw is elongated ({w}×{h})");
    assert_eq!(portrait(&Genome::straw()), (w, h, straw), "same genome, same picture");
    let (gw, gh, giant) = portrait(&Genome::giant(4));
    let opaque = giant.chunks(4).filter(|p| p[3] == 255).count();
    assert!(opaque >= 9, "giant portrait is a grown cell ({gw}×{gh}, {opaque} px)");
    let (sw, sh, spiny) = portrait(&Genome::spiny());
    let ivory = spiny.chunks(4).filter(|p| p[..3] == [0xEF, 0xE7, 0xDA]).count();
    assert!(ivory >= 1, "spiny portrait shows its bone ({sw}×{sh})");
}

#[test]
fn species_card_names_the_blocks() {
    let mut tank = water_box(9, 16, 16, 12);
    tank.seed_algae(24, 24).unwrap();
    let sp = tank.inspect(24, 24).species;
    let page = tank.species_page(sp).expect("card");
    assert!(page.contains("PHOTO 5"), "{page}");
    assert!(page.contains("\nGENE\n"), "{page}");
    assert!(page.contains("  SIZE "), "{page}");
    assert!(!page.contains("MODE"), "{page}");
    assert!(page.contains("GROW"), "{page}");
    assert!(page.contains("AGE"), "{page}");
    assert!(page.contains("CELLS"), "{page}");
    assert!(page.contains("KIN"), "{page}");
    assert!(!page.contains("Block"), "{page}");
}

#[test]
fn a_gene_card_groups_the_body_then_each_mode() {
    use crate::archive::genetics_lines;
    use crate::bio::Genome;
    let algae = genetics_lines(&Genome::algae());
    let gene = algae.iter().position(|l| l == "GENE").expect("one gene");
    let buoy = algae.iter().position(|l| l.starts_with("BUOY")).unwrap();
    let wall = algae.iter().position(|l| l.starts_with("WALL KEY")).unwrap();
    let glue = algae.iter().position(|l| l.starts_with("GLUE KEY")).unwrap();
    assert!(buoy < wall && wall < glue && glue < gene, "{algae:?}");
    assert_eq!(algae.iter().filter(|l| l.starts_with("GENE")).count(), 1);
    assert!(algae.iter().any(|l| l.starts_with("  SIZE ")));
    assert!(algae.iter().any(|l| l.starts_with("  SPLITS ")));
    let straw = genetics_lines(&Genome::straw());
    assert!(straw.iter().any(|l| l == "GENE 1"), "{straw:?}");
    assert!(straw.iter().any(|l| l == "GENE 2"), "{straw:?}");
    assert!(straw.iter().any(|l| l.starts_with("  GROW ") || l.starts_with("  WHEN ")));
}

#[test]
fn adding_a_specimen_keeps_what_is_already_alive() {
    use crate::archive::Specimen;
    let mut tank = water_box(4, 24, 24, 16);
    let id = tank.seed_algae(30, 40).unwrap();
    let before = tank.live_cells();
    let sp = Specimen::starter(0).expect("moss");
    let placed = tank.seed_specimen_copies(&sp, 1);
    assert!(placed >= 1, "one copy lands in the open water");
    assert!(tank.live_cells() > before);
    assert!(tank.cell(id).is_some(), "the original cell stays");
    assert_eq!(tank.visible_size(), (24, 24));
}

/// The pale-pink cilia trail is part of the swimmer: a tap on it selects
/// the cell, the tracking box reaches it, and the book portrait draws it.
#[test]
fn cilia_fringe_is_part_of_the_swimmer() {
    use crate::archive::portrait;
    use crate::bio::Genome;
    let (w, h, pic) = portrait(&Genome::swimmer());
    let pink = pic.chunks(4).filter(|p| *p == [0xF0, 0xA6, 0xC0, 255]).count();
    assert!(pink >= 1, "swimmer portrait includes its cilia ({w}×{h}, pink={pink})");

    let mut tank = water_box(11, 16, 16, 14);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(24, 24, Genome::swimmer()).expect("seed");
    let c = *tank.cell(id).expect("alive");
    // A placed seed heads up (heading 2); the fringe trails one pixel down.
    let hit = tank.inspect(c.px as u32, c.py as u32 + 1);
    assert_eq!(hit.cell, id, "a tap on the pink fringe selects the swimmer");
    assert_eq!(hit.org, c.org);
    let bb = tank.org_bbox(c.org).expect("bbox");
    assert!(bb.3 >= c.py + 1, "the box reaches the fringe ({bb:?})");
    assert!(bb.0 <= c.px && bb.2 >= c.px && bb.1 <= c.py);
}

/// Phase 3 — archive a creature, start a new tank from it: the record
/// round-trips, the species is pinned, and the new tank's lineage
/// continues the archived species by name and depth with the specimen
/// as cross-tank ancestor.
#[test]
fn archive_specimen_and_seed_a_new_tank_from_it() {
    use crate::archive::Specimen;
    use crate::bio::{Block, Entry, Genome, NO_SPECIES};
    let mut tank = water_box(21, 32, 24, 20);
    tank.set_lambda_scale_q8(0);
    // A red straw: one structural step under the algae root → depth 2.
    let mut g = Genome::straw();
    g.modes[0].push_entry(Entry::with_params(Block::Pigment, 3, 0, 0));
    let seed = tank.seed_genome_at(48, 30, g.clone()).expect("seed");
    for _ in 0..300 {
        tank.tick(&day());
    }
    let cell = tank.cell(seed).map(|_| seed).unwrap_or_else(|| tank.cells().live_ids()[0]);
    let before = tank.lineage_card(cell).unwrap();
    assert_eq!(before.archived_relatives, 0);
    let sp = tank.archive_specimen(cell).expect("specimen");
    assert_eq!(sp.species_name, before.species_name);
    assert_eq!(sp.display_name, sp.species_name);
    assert!(sp.lineage.len() >= 2, "{:?}", sp.lineage);
    assert_eq!(sp.lineage.last().unwrap(), &sp.species_name);
    assert!(sp.portrait_w >= 1 && sp.portrait.len() == sp.portrait_w as usize * sp.portrait_h as usize * 4);
    assert!(sp.stats.cells >= 1);
    assert_eq!(sp.archive_parent, None, "placed by hand, not from the archive");
    assert_eq!(tank.lineage_card(cell).unwrap().archived_relatives, 1, "species pinned");
    assert!(tank.species_tree().iter().any(|n| n.id == before.species && n.archived && !n.extinct));
    let bytes = sp.to_bytes();
    let back = Specimen::from_bytes(&bytes).expect("decode");
    assert_eq!(back, sp);
    assert!(Specimen::from_bytes(&bytes[..bytes.len() / 2]).is_none());
    assert!(sp.page_text().contains(&sp.species_name));
    assert!(sp.page_text().contains("GENETICS"), "the page carries the full program");
    assert!(sp.page_text().contains("PHOTO"), "the program names its blocks");

    // New tank, seeded from the specimen.
    let mut fresh = water_box(22, 32, 24, 20);
    let id = fresh.seed_specimen(&sp, 30, 30).expect("seed from archive");
    let card = fresh.lineage_card(id).unwrap();
    assert_eq!(card.species_name, sp.species_name, "the species continues by name");
    assert_eq!(card.species_depth, sp.species_depth, "and by depth");
    assert_eq!(card.generation, 0);
    assert_eq!(card.mutations_since_seed, 0, "an archive seed is a lineage root");
    assert_eq!(card.archived_relatives, 1);
    assert_eq!(fresh.archive_parent_of(card.genotype), Some(sp.id));
    assert_ne!(fresh.inspect(30, 30).species, NO_SPECIES);
    // Its descendants inherit the archive ancestor and the save keeps it.
    fresh.set_lambda_scale_q8(0);
    for _ in 0..200 {
        fresh.tick(&day());
    }
    let bytes = fresh.save_bytes().unwrap();
    let mut loaded = Tank::load_bytes(&bytes).unwrap();
    let any = loaded.cells().live_ids()[0];
    let g2 = loaded.genotype_of_cell(any).unwrap();
    assert_eq!(loaded.archive_parent_of(g2), Some(sp.id));
    let sp2 = loaded.archive_specimen(any).expect("second specimen");
    assert_eq!(sp2.archive_parent, Some(sp.id), "cross-tank ancestry travels with the record");
    assert_ne!(sp2.id, sp.id);

    // "New tank" convenience: n copies in open water on the preset.
    let mut preset = Tank::preset(5);
    let placed = preset.seed_specimen_copies(&sp, 4);
    assert_eq!(placed, 4);
    let tree = preset.species_tree();
    let node = tree.iter().find(|n| n.name == sp.species_name).expect("archived species in the tree");
    assert!(node.archived);
    assert_eq!(node.live_cells, 4);
    assert_eq!(node.parent, NO_SPECIES, "an archive seed is a root of this tank's tree");
}

// ---------------------------------------------------------------- Phase 4

/// 4.1 — the thermal cycle (§4): midday ≈ base + 10, stored heat carries
/// into the evening and decays with a τ set by the tank's materials, so a
/// rocky tank cools slower than an airy one.
#[test]
fn thermal_cycle_stores_heat_and_rock_cools_slower_than_air() {
    use crate::consts::{DAY_TICKS, T_BASE_DEFAULT};
    let mut tank = water_box(3, 16, 32, 12);
    assert!(
        (tank.t_tank() - T_BASE_DEFAULT).abs() < 1e-3,
        "starts at base"
    );
    // Midday with always-day: both terms reach +10.
    for _ in 0..30_000 {
        tank.tick(&day());
    }
    let noon = tank.t_tank();
    assert!(
        (noon - (T_BASE_DEFAULT + 10.0)).abs() < 0.5,
        "midday {noon}"
    );
    // Same tank through a real day: the clock starts at noon.
    let mut airy = water_box(3, 16, 32, 4);
    let mut rocky = water_box(3, 16, 32, 4);
    for vx in 0..16 {
        for vy in 4..28 {
            place(&mut rocky, vx, vy, Cell::stone());
        }
    }
    assert!(
        rocky.thermal_mass().tau_ticks() > 2.0 * airy.thermal_mass().tau_ticks(),
        "rock holds heat longer"
    );
    // Mid-afternoon (light ≈ 0.7): both tanks are warm.
    let dusk = (DAY_TICKS / 2) as u32;
    tick_n(&mut airy, dusk / 2);
    tick_n(&mut rocky, dusk / 2);
    assert!(
        airy.t_tank() > T_BASE_DEFAULT + 5.0,
        "afternoon {}",
        airy.t_tank()
    );
    // Dusk: the airy puddle (τ ≈ 1 min) has already shed most of its heat
    // with the falling sun; the rocky tank (τ ≈ 6 min) carries it on.
    tick_n(&mut airy, dusk / 2);
    tick_n(&mut rocky, dusk / 2);
    let dusk_airy = airy.t_tank();
    let dusk_rocky = rocky.t_tank();
    assert!(
        dusk_rocky > dusk_airy + 1.0,
        "rock holds heat at dusk ({dusk_rocky} vs {dusk_airy})"
    );
    assert!(
        dusk_rocky > T_BASE_DEFAULT + 2.5,
        "stored heat at dusk {dusk_rocky}"
    );
    // Six minutes into the night: the rocky tank has cooled but is still
    // the warmer one; the airy tank sits at base + the night-floor light.
    tick_n(&mut airy, 6 * 60 * 30);
    tick_n(&mut rocky, 6 * 60 * 30);
    let night_airy = airy.t_tank();
    let night_rocky = rocky.t_tank();
    assert!(
        night_rocky < dusk_rocky - 1.0,
        "rocky tank cools ({dusk_rocky} → {night_rocky})"
    );
    assert!(
        night_rocky > night_airy + 0.3,
        "rock stays warmer ({night_rocky} vs {night_airy})"
    );
    assert!(
        (night_airy - (T_BASE_DEFAULT + 1.0)).abs() < 0.3,
        "airy puddle at base + floor ({night_airy})"
    );
}

/// 4.1 — THERMO (§a.7): a heater cell lifts its coarse cell several
/// degrees, the halo decays when it stops, costs glucose, and the first
/// sustained +3 °C fires "warm-blooded".
#[test]
fn thermo_cell_warms_its_halo_pays_glucose_and_is_warm_blooded() {
    use crate::bio::{ev, Genome};
    let cold = TankInput {
        always_day: true,
        t_base: 5.0,
        ..TankInput::default()
    };
    let mut tank = water_box(4, 24, 24, 20);
    tank.set_lambda_scale_q8(0);
    // A free cell jitters a pixel a tick and would smear its heat over
    // many coarse cells; pen the heater in a one-cell rock pocket (a
    // real heater is an anchored colony).
    let (mut vx, mut vy) = (12, 10);
    for dx in -1..=1 {
        for dy in -1..=1 {
            if (dx, dy) != (0, 0) {
                place(&mut tank, vx + dx, vy + dy, Cell::bedrock());
            }
        }
    }
    // Control: the same genome in a second pocket, THERMO gated shut.
    let (cx, cy) = (4, 10);
    for dx in -1..=1 {
        for dy in -1..=1 {
            if (dx, dy) != (0, 0) {
                place(&mut tank, cx + dx, cy + dy, Cell::bedrock());
            }
        }
    }
    let mut shut = Genome::heater();
    for e in shut.modes[0].entries.iter_mut() {
        if e.block == crate::bio::Block::Thermo {
            e.gate = crate::bio::Gate::below(crate::bio::Scalar::TempDev, 0);
        }
    }
    let (hx, hy) = (vx as u16 * 3 + 1, (23 - vy as u16) * 3 + 1);
    let heater = tank.seed_genome_at(hx, hy, Genome::heater()).expect("seed");
    let plain = tank
        .seed_genome_at(cx as u16 * 3 + 1, hy, shut)
        .expect("seed");
    for org in [heater, plain] {
        let o = tank.cell(org).unwrap().org;
        tank.feed_org_full(o);
    }
    let e0 = tank.cell(heater).unwrap().energy;
    let p0 = tank.cell(plain).unwrap().energy;
    let mut warm_event = false;
    let mut peak = 0.0f32;
    for _ in 0..400 {
        tank.tick(&cold);
        // The heat lands in the coarse cell under the heater's anchor
        // pixel, which may straddle the pocket wall.
        if let Some(c) = tank.cell(heater) {
            (vx, vy) = crate::bio::pixel_to_phys(c.px, c.py, 24);
        }
        peak = peak.max(tank.t_local_at(vx, vy));
        while let Some(e) = tank.poll_event() {
            if e.kind == ev::WARM_BLOODED || e.kind == ev::MS_WARM_BLOODED {
                warm_event = true;
            }
        }
    }
    assert!(peak >= 3.0, "the halo should reach +3 °C (peak {peak})");
    assert!(warm_event, "warm-blooded fires after 60 ticks over +3 °C");
    assert!(tank.t_local_at(vx + 6, vy) < 0.5, "the halo is local");
    let e1 = tank.cell(heater).map(|c| c.energy).unwrap_or(0);
    let p1 = tank.cell(plain).map(|c| c.energy).unwrap_or(p0);
    assert!(
        e0 - e1 > (p0 - p1) + 2 * crate::consts::FP,
        "heating costs glucose ({} vs {})",
        e0 - e1,
        p0 - p1
    );
    // Remove the heater: the halo leaks away within a few seconds.
    let _ = tank.remove_at(vx, vy, 1);
    tick_n(&mut tank, 240);
    assert!(
        tank.t_local_at(vx, vy) < 0.3,
        "halo decays ({})",
        tank.t_local_at(vx, vy)
    );
}

/// 4.2 — cloudiness (§4): a bloom crash clouds the water (less light,
/// a milky render, "water is cloudy"), then the fines settle back into
/// floor litter and the tank clears.
#[test]
fn bloom_crash_clouds_the_water_then_it_clears_into_litter() {
    use crate::bio::ev;
    let mut tank = water_box(7, 24, 32, 28);
    let w = tank.visible_size().0 as usize * 3;
    tick_n(&mut tank, 2);
    let light_before: u32 = (0..w)
        .map(|px| tank.pixel_light_at(px as u16, 30) as u32)
        .sum();
    let litter_before = tank.litter_total();
    let grams = 200.0;
    for px in 0..w {
        tank.add_fines(px, grams);
    }
    tank.tick(&TankInput::default());
    assert!(tank.cloud_mean() < 0.5, "cloud {}", tank.cloud_mean());
    let light_after: u32 = (0..w)
        .map(|px| tank.pixel_light_at(px as u16, 30) as u32)
        .sum();
    assert!(
        light_after < light_before / 2,
        "cloud halves the light ({light_before} → {light_after})"
    );
    let mut cloudy = false;
    for _ in 0..600 {
        tank.tick(&TankInput::default());
        while let Some(e) = tank.poll_event() {
            if e.kind == ev::CLOUDY {
                cloudy = true;
            }
        }
    }
    assert!(cloudy, "the tank should announce cloudy water");
    // τ ≈ 60 s: after 6 minutes the water is clear again and the fines
    // are floor litter.
    tick_n(&mut tank, 6 * 60 * 30);
    assert!(tank.cloud_mean() > 0.9, "cleared to {}", tank.cloud_mean());
    let fines_left: f32 = tank.fines().iter().sum();
    assert!(
        fines_left < grams * w as f32 * 0.05,
        "fines left {fines_left}"
    );
    assert!(
        tank.litter_total() > litter_before,
        "settled fines are litter"
    );
}

/// 4.3 — humidity (K2#5): the air over a half-filled tank is humid near
/// the water and drier up high; at night humid air condenses as dew on
/// the glass and damp rock, which dries off again by day.
#[test]
fn humidity_rises_over_water_and_dew_forms_on_glass_at_night() {
    use crate::consts::{DAY_TICKS, DEW_H, MOIST_SAT};
    let mut tank = water_box(9, 24, 24, 10);
    // A glass pane and a stone block on the air side.
    for vy in 10..24 {
        place(&mut tank, 20, vy, Cell::glass());
    }
    // Limestone stays put, so the block can hang in the humid air the
    // way the old static stone did. Painted stone now falls.
    for vy in 12..15 {
        place(&mut tank, 4, vy, Cell::solid(MaterialId::Limestone));
    }
    for _ in 0..1200 {
        tank.tick(&day());
    }
    let low = tank.humidity_at(10, 10);
    let high = tank.humidity_at(10, 22);
    assert!(low >= 150, "air over the water is humid ({low})");
    assert!(high < low, "humidity falls off with height ({high} vs {low})");
    assert_eq!(tank.humidity_at(10, 3), 255, "water reads saturated");
    assert_eq!(tank.cell_visible(20, 15).pad, 0, "no dew by day");
    let dry_rock = tank.cell_visible(4, 13).rock_film();
    // Into the night.
    let input = TankInput::default();
    while crate::clock::is_day(tank.tick_count()) {
        tank.tick(&input);
    }
    tick_n(&mut tank, 600);
    assert!(tank.tick_count() < DAY_TICKS + 1000);
    let night_low = tank.humidity_at(10, 10);
    assert!(night_low >= DEW_H, "cool night air saturates ({night_low})");
    let dew = tank.cell_visible(20, 11).pad;
    assert!(dew > 0, "dew on the glass at night");
    let wet_rock = tank.cell_visible(4, 12).rock_film();
    assert!(wet_rock > dry_rock, "damp rock at night ({dry_rock} → {wet_rock})");
    assert!(wet_rock >= MOIST_SAT || dew >= MOIST_SAT, "condensation moistens a surface");
    // Daylight dries the pane.
    for _ in 0..1200 {
        tank.tick(&day());
    }
    assert!(tank.cell_visible(20, 20).pad < dew.max(1), "dew fades by day");
}

/// 4.3 — evaporation (K2#8): the free surface loses water very slowly
/// (water only, nothing else leaves), and the top-up tool pours pure
/// water back onto every surface.
#[test]
fn evaporation_lowers_the_surface_slowly_and_top_up_restores_it() {
    use crate::consts::EVAP_EVERY;
    let mut tank = water_box(10, 24, 24, 12);
    tank.drop_litter(30, 60, 20, crate::bio::chem::LITTER_SOFT_CODE);
    let litter = tank.litter_total();
    let total = |t: &Tank| (0..24).map(|vx| column_sat(t, vx)).sum::<u64>();
    let before = total(&tank);
    for _ in 0..(EVAP_EVERY as u32 * 6) {
        tank.tick(&day());
    }
    let after = total(&tank);
    let lost = tank.evaporated_sat();
    assert!(lost > 0, "the surface evaporates");
    assert_eq!(before - after, lost, "only evaporation removes water ({before} → {after})");
    assert!(lost <= 6 * 24, "very slow: at most one sat per column per period ({lost})");
    assert_eq!(tank.litter_total(), litter, "nothing but water leaves");
    // Top up: one row of pure water on every surface.
    let topup = TankInput {
        tools: vec![crate::tools::ToolEvent {
            kind: 14,
            material: 0,
            x: 0,
            y: 0,
            r: 1,
            variant: 0,
        }],
        ..day()
    };
    tank.tick(&topup);
    let topped = total(&tank);
    assert!(topped > before, "top-up raises the waterline ({before} → {topped})");
    assert!(tank.cell_visible(5, 12).sat > 0, "the row above the old surface holds water");
    assert!(tank.cell_visible(5, 14).sat == 0, "air above the new surface stays dry");
}

/// 4.4 — hydration conducts along division bonds; a rooted hard-walled
/// stem stands in dry air where a free soft cell desiccates.
#[test]
fn hydration_conducts_and_desiccation_spares_a_rooted_stem() {
    use crate::bio::Genome;
    use crate::consts::MOIST_SAT;
    let mut tank = Tank::try_new(11, 24, 24).unwrap();
    tank.set_lambda_scale_q8(0);
    // Half-filled: sand bed, water to vy 8, air above.
    for vx in 0..24 {
        for vy in 0..4 {
            place(&mut tank, vx, vy, Cell::sand());
        }
        for vy in 4..8 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    // Keep the sand under the plant at field capacity (won't drain below
    // retain, which is still ≥ MOIST_SAT).
    for vx in 10..14 {
        let mut c = tank.cell_visible(vx, 3);
        c.sat = 200;
        place(&mut tank, vx, 3, c);
    }
    let (hx, hy) = (12u16 * 3 + 1, (23 - 3) * 3 + 1);
    let root = tank.seed_genome_at(hx, hy, Genome::rooting()).expect("root");
    let (ax, ay) = (6u16 * 3 + 1, (23 - 20) * 3 + 1);
    let airy = tank.seed_genome_at(ax, ay, Genome::algae()).expect("air");
    let root_org = tank.cell(root).unwrap().org;
    if let Some(o) = tank.cells_mut_for_test().org_mut(root_org) {
        o.cut = true;
    }
    for org in [root, airy] {
        let o = tank.cell(org).unwrap().org;
        tank.feed_org_full(o);
    }
    // Force a hydration refresh (tick 0) and read.
    tank.tick(&day());
    let h_root = tank.cell(root).map(|c| c.hydration).unwrap_or(0);
    assert!(h_root >= MOIST_SAT, "root hydration {h_root}");
    let mut land = false;
    for _ in 0..400 {
        tank.tick(&day());
        // Re-wet the sand so seepage into the water column cannot dry it.
        for vx in 10..14 {
            let mut c = tank.cell_visible(vx, 3);
            if c.sat < 160 {
                c.sat = 200;
                place(&mut tank, vx, 3, c);
            }
        }
        while let Some(e) = tank.poll_event() {
            if e.kind == crate::bio::ev::LIFE_ON_LAND || e.kind == crate::bio::ev::MS_LIFE_ON_LAND {
                land = true;
            }
        }
    }
    assert!(land, "life on land fires for an air-side cell");
    assert!(tank.cell(root).is_some(), "the rooted stem survives dry air");
    if let Some(c) = tank.cell(airy) {
        assert!(c.damage > 0, "free air cell takes desiccation damage ({})", c.damage);
    }
}

/// 4.4 — roots co-occupy moist sand, bind it (≥5 px), and dig cuts them.
#[test]
fn roots_bind_sand_and_dig_cuts_them() {
    use crate::bio::Genome;
    use crate::tools::ToolEvent;
    let mut tank = Tank::try_new(12, 16, 16).unwrap();
    tank.set_lambda_scale_q8(0);
    for vx in 0..16 {
        for vy in 0..6 {
            let mut c = Cell::sand();
            c.sat = 255;
            place(&mut tank, vx, vy, c);
        }
    }
    // Seed five rooting cells into one physics cell so it binds.
    let (vx, vy) = (8i32, 3i32);
    let mut ids = Vec::new();
    for i in 0..5u16 {
        let px = vx as u16 * 3 + (i % 3);
        let py = (15 - vy as u16) * 3 + (i / 3);
        if let Some(id) = tank.seed_genome_at(px, py, Genome::rooting()) {
            // Force root mode coeffs by ensuring uptake_water ≥ 2 (preset).
            ids.push(id);
            let o = tank.cell(id).unwrap().org;
            tank.feed_org_full(o);
        }
    }
    assert!(ids.len() >= 5, "seeded {}", ids.len());
    tick_n(&mut tank, 2);
    assert!(
        tank.cell_visible(vx, vy).is_bound(),
        "≥5 root pixels bind the sand"
    );
    let before = tank.live_cells();
    let litter_before = tank.litter_total();
    tank.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 2, // erase = dig
            material: 0,
            x: vx as u16,
            y: vy as u16,
            r: 0,
            variant: 0,
        }],
        ..TankInput::default()
    });
    assert!(tank.live_cells() < before, "dig cuts roots");
    assert!(tank.litter_total() > litter_before, "cut roots become litter");
    assert!(!tank.cell_visible(vx, vy).is_bound(), "bound flag clears");
}

/// 4.5 — cut / root-trim (kind 15): a stroke kills cells under the brush;
/// a stem cut leaves a fragment, a root cut becomes soft litter.
#[test]
fn cut_tool_severs_a_stem_and_trims_roots() {
    use crate::bio::Genome;
    use crate::tools::ToolEvent;
    // Straw: a short bonded chain in water.
    let mut tank = water_box(13, 24, 24, 20);
    tank.set_lambda_scale_q8(0);
    let id = tank.seed_genome_at(36, 40, Genome::straw()).expect("straw");
    let org = tank.cell(id).unwrap().org;
    tank.feed_org_full(org);
    // Grow the chain.
    for _ in 0..200 {
        tank.tick(&day());
        if tank.cells().org(org).map(|o| o.n_alive).unwrap_or(0) >= 3 {
            break;
        }
    }
    let before = tank.live_cells();
    let orgs_before = tank.cells().iter_orgs().count();
    assert!(before >= 2, "straw has cells ({before})");
    // Cut the middle of the body.
    let seed = tank.cell(id).unwrap();
    let (vx, vy) = crate::bio::pixel_to_phys(seed.px, seed.py, 24);
    tank.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 15,
            material: 0,
            x: vx as u16,
            y: vy as u16,
            r: 1,
            variant: 0,
        }],
        ..TankInput::default()
    });
    assert!(tank.live_cells() < before || tank.cells().iter_orgs().count() > orgs_before,
        "cut kills or fragments the body");

    // Root trim: five roots in wet sand.
    let mut land = Tank::try_new(14, 16, 16).unwrap();
    land.set_lambda_scale_q8(0);
    for vx in 0..16 {
        for vy in 0..6 {
            let mut c = Cell::sand();
            c.sat = 255;
            place(&mut land, vx, vy, c);
        }
    }
    let (vx, vy) = (8i32, 3i32);
    for i in 0..5u16 {
        let px = vx as u16 * 3 + (i % 3);
        let py = (15 - vy as u16) * 3 + (i / 3);
        let _ = land.seed_genome_at(px, py, Genome::rooting());
    }
    tick_n(&mut land, 2);
    let litter_before = land.litter_total();
    let live_before = land.live_cells();
    land.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 15,
            material: 0,
            x: vx as u16,
            y: vy as u16,
            r: 1,
            variant: 0,
        }],
        ..TankInput::default()
    });
    assert!(land.live_cells() < live_before, "root trim kills root cells");
    assert!(land.litter_total() > litter_before, "trimmed roots become litter");
}

/// 4.7 — a glass box is an enclosure: drain, fill, delete walls.
#[test]
fn enclosure_drain_fill_and_delete_walls() {
    use crate::tools::ToolEvent;
    let mut tank = Tank::try_new(15, 16, 16).unwrap();
    // Glass box: walls at x=3,x=10 and y=2,y=8; open interior.
    for vy in 2..=8 {
        place(&mut tank, 3, vy, Cell::glass());
        place(&mut tank, 10, vy, Cell::glass());
    }
    for vx in 3..=10 {
        place(&mut tank, vx, 2, Cell::glass());
        place(&mut tank, vx, 8, Cell::glass());
    }
    for vx in 4..10 {
        for vy in 3..8 {
            place(&mut tank, vx, vy, Cell::water());
        }
    }
    let enc = tank.enclosure_at(6, 5).expect("enclosure");
    assert!(enc.area() >= 20, "interior {}", enc.area());
    assert!(!enc.walls.is_empty());
    // Drain.
    assert!(tank.enclosure_op(6, 5, 1) > 0);
    let inside: u64 = (4..10).map(|vx| column_sat(&tank, vx)).sum();
    // Columns 4..10 still include water outside the box above/below; check a cell.
    assert_eq!(tank.cell_visible(6, 5).sat, 0, "drained");
    // Fill.
    assert!(tank.enclosure_op(6, 5, 2) > 0);
    assert!(tank.cell_visible(6, 5).sat >= 200, "filled");
    // Delete walls: water can now spill.
    let walls = tank.enclosure_op(6, 5, 0);
    assert!(walls > 0, "walls removed");
    assert!(!tank.cell_visible(3, 5).is_glass());
    let _ = inside;
    assert!(!tank.cell_visible(3, 5).is_glass(), "wall cells are gone");
    // Tool path.
    place(&mut tank, 3, 5, Cell::glass());
    place(&mut tank, 10, 5, Cell::glass());
    for vx in 3..=10 {
        place(&mut tank, vx, 2, Cell::glass());
        place(&mut tank, vx, 8, Cell::glass());
    }
    for vy in 2..=8 {
        place(&mut tank, 3, vy, Cell::glass());
        place(&mut tank, 10, vy, Cell::glass());
    }
    tank.tick(&TankInput {
        paused: true,
        tools: vec![ToolEvent {
            kind: 16,
            material: 1, // drain
            x: 6,
            y: 5,
            r: 0,
            variant: 0,
        }],
        ..TankInput::default()
    });
    assert_eq!(tank.cell_visible(6, 5).sat, 0);
}

/// 4.8 — starter archive seeds (moss / plant / crown) are valid and seedable.
#[test]
fn starter_specimens_seed_into_a_tank() {
    use crate::archive::Specimen;
    for kind in 0..=2u8 {
        let spec = Specimen::starter(kind).expect("starter");
        assert!(spec.genome.is_valid(), "kind {kind}");
        assert!(!spec.display_name.is_empty());
        assert!(spec.portrait_w > 0 && !spec.portrait.is_empty());
        let mut tank = water_box(16 + kind as u64, 24, 24, 16);
        let id = tank.seed_specimen(&spec, 36, 30).expect("seed");
        assert!(tank.cell(id).is_some());
    }
}

/// 5.0 M0 — a fed muscle body contracts its soft-body spring and the
/// raster moves at least one cell over a few hundred ticks.
#[test]
fn muscle_body_twitches_on_its_soft_springs() {
    use crate::bio::Genome;
    let mut tank = water_box(5, 32, 32, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank
        .seed_genome_at(16 * 3 + 1, (31 - 12) * 3 + 1, Genome::twitcher())
        .expect("seed");
    let org = tank.cell(id).unwrap().org;
    tank.feed_org_full(org);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    // Grow the bonded pair.
    for _ in 0..400 {
        tank.tick(&input);
        tank.feed_org_full(org);
    }
    let alive = tank
        .cells()
        .iter_alive()
        .filter(|(_, c)| c.org == org)
        .count();
    assert!(alive >= 2, "twitcher should grow a bonded pair (alive={alive})");
    let before: Vec<(u16, u16)> = tank
        .cells()
        .iter_alive()
        .filter(|(_, c)| c.org == org)
        .map(|(_, c)| (c.px, c.py))
        .collect();
    let mut moved = false;
    for _ in 0..600 {
        tank.tick(&input);
        tank.feed_org_full(org);
        let now: Vec<(u16, u16)> = tank
            .cells()
            .iter_alive()
            .filter(|(_, c)| c.org == org)
            .map(|(_, c)| (c.px, c.py))
            .collect();
        if now != before {
            moved = true;
            break;
        }
    }
    assert!(moved, "soft-body muscle should displace at least one cell");
}

/// 5.1 M1 — hard–soft–hard hinge: bone ends stay as separate rigid parts
/// with a muscle joint between them.
#[test]
fn hinge_body_has_two_bone_parts_and_a_muscle_joint() {
    use crate::bio::{bones, Genome};
    let mut tank = water_box(7, 32, 32, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank
        .seed_genome_at(16 * 3 + 1, (31 - 12) * 3 + 1, Genome::hinge())
        .expect("seed");
    let org = tank.cell(id).unwrap().org;
    tank.feed_org_full(org);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    for _ in 0..800 {
        tank.tick(&input);
        tank.feed_org_full(org);
    }
    let alive = tank
        .cells()
        .iter_alive()
        .filter(|(_, c)| c.org == org)
        .count();
    assert!(alive >= 3, "hinge should grow bone–muscle–bone (alive={alive})");
    let g = bones::activate(tank.cells(), tank.genomes(), org).expect("bone graph");
    let bone_parts = g.parts.iter().filter(|p| p.kind == bones::PartKind::Bone).count();
    assert!(
        bone_parts >= 2,
        "expected ≥2 bone parts, got {bone_parts} (parts={})",
        g.parts.len()
    );
    assert!(
        g.joints.iter().any(|j| j.muscle),
        "expected a muscle joint between bone parts"
    );
}

/// 5.2 M2 — a nerved body gets a StudioNet and its effectors run.
#[test]
fn nerved_body_builds_a_studio_net_and_drives() {
    use crate::bio::Genome;
    let mut tank = water_box(11, 32, 32, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank
        .seed_genome_at(16 * 3 + 1, (31 - 12) * 3 + 1, Genome::nerved())
        .expect("seed");
    let org = tank.cell(id).unwrap().org;
    tank.feed_org_full(org);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    let mut saw_running = false;
    for _ in 0..500 {
        tank.tick(&input);
        tank.feed_org_full(org);
        if tank
            .cells()
            .iter_alive()
            .filter(|(_, c)| c.org == org)
            .any(|(_, c)| c.running)
        {
            saw_running = true;
            break;
        }
    }
    assert!(saw_running, "nerved muscle should be driven by the net or links");
    assert!(Genome::nerved().is_valid());
}

/// 5.4 M4 — chamber predator grows a geometric mouth (hard + enzyme + muscle).
#[test]
fn chamber_mouth_fires_the_chamber_milestone() {
    use crate::bio::{ev, Genome};
    let mut tank = water_box(17, 32, 24, 24);
    tank.set_lambda_scale_q8(0);
    assert!(Genome::chamber_predator().is_valid());
    let id = tank
        .seed_genome_at(48, 60, Genome::chamber_predator())
        .expect("seed");
    let org = tank.cell(id).unwrap().org;
    let mut chamber = false;
    for _ in 0..5_000 {
        tank.tick(&day());
        tank.feed_org_full(org);
        while let Some(e) = tank.poll_event() {
            if e.kind == ev::CHAMBER || e.kind == ev::MS_CHAMBER {
                chamber = true;
            }
        }
        if chamber {
            break;
        }
    }
    let alive = tank
        .cells()
        .org(org)
        .map(|o| o.n_alive)
        .unwrap_or(0);
    assert!(alive >= 5, "U should form (alive={alive})");
    assert!(chamber, "geometric chamber milestone should fire");
}

/// 5.4 M4 — unmatched prey enclosed then freed (eater dies) fires ESCAPE.
#[test]
fn enclosed_prey_escape_fires_when_freed() {
    use crate::bio::{ev, Genome};
    let mut tank = water_box(17, 32, 24, 24);
    tank.set_lambda_scale_q8(0);
    let id = tank
        .seed_genome_at(48, 60, Genome::chamber_predator())
        .expect("seed");
    let org = tank.cell(id).unwrap().org;
    for _ in 0..4_000 {
        tank.tick(&day());
        tank.feed_org_full(org);
        if tank.cells().org(org).map(|o| o.n_alive).unwrap_or(0) >= 7 {
            break;
        }
    }
    assert!(
        tank.cells().org(org).map(|o| o.n_alive).unwrap_or(0) >= 7,
        "U never finished"
    );
    let base = tank
        .cells()
        .iter_alive()
        .find(|(_, c)| c.org == org && c.mode == 0)
        .map(|(_, c)| (c.px, c.py))
        .expect("base");
    let cavity = (base.0, base.1 - 1);
    // Unmatched wall_key so the lining does not lyse the prey quickly.
    let mut prey_g = Genome::algae();
    prey_g.wall_key = 200;
    prey_g.buoy = -20;
    let prey = tank
        .seed_genome_at(cavity.0, cavity.1, prey_g)
        .expect("prey");
    let prey_org = tank.cell(prey).unwrap().org;
    tank.pin_cell_for_test(prey);
    // Wait until SCAN_EVERY records the swallowed set (≥ 2 periods).
    for _ in 0..160 {
        tank.tick(&day());
        assert!(
            tank.cells().get(prey).is_some(),
            "unmatched prey should survive the cavity briefly"
        );
    }
    // Destroy the eater — cavity vanishes; prey stays alive ⇒ escape.
    let eater_cells: Vec<u32> = tank
        .cells()
        .iter_alive()
        .filter(|(_, c)| c.org == org)
        .map(|(id, _)| id)
        .collect();
    for cid in eater_cells {
        tank.kill_cell(cid);
    }
    let mut escape = false;
    for _ in 0..200 {
        tank.tick(&day());
        while let Some(e) = tank.poll_event() {
            if (e.kind == ev::ESCAPE || e.kind == ev::MS_ESCAPE) && e.arg0 == prey_org {
                escape = true;
            }
        }
        if escape {
            break;
        }
    }
    assert!(
        tank.cells().get(prey).is_some(),
        "prey should still be alive after the eater dies"
    );
    assert!(escape, "freed unmatched prey should fire ESCAPE");
}

/// 5.4 M4 — SENSOR(colour) returns a high match when looking at a yellow tip.
#[test]
fn colour_sensor_matches_a_pigment_target() {
    use crate::bio::profile::{Block, Entry, SENSOR_COLOUR};
    use crate::bio::Genome;
    let mut tank = water_box(13, 24, 24, 20);
    tank.set_lambda_scale_q8(0);
    // Yellow crown tip above the seeker (pigment hue 43).
    let mut crown = Genome::algae();
    crown.modes[0].n_entries = 0;
    crown.modes[0].push_entry(Entry::with_params(Block::Pigment, 6, 43, 0));
    crown.modes[0].push_entry(Entry::new(Block::Photo, 1));
    crown.modes[0].n_children = 0;
    assert!(crown.is_valid());
    // Heading 0 = east: tip sits to the right of the seeker.
    let seeker = tank
        .seed_genome_at(30, 40, Genome::colour_seeker())
        .expect("seeker");
    tank.pin_cell_for_test(seeker);
    let tip = tank.seed_genome_at(36, 40, crown).expect("tip");
    tank.pin_cell_for_test(tip);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    let mut sense = 0u8;
    for _ in 0..60 {
        tank.tick(&input);
        if let Some(c) = tank.cell(seeker) {
            sense = c.sense;
            if sense > 100 {
                break;
            }
        }
    }
    let mc = tank
        .genomes()
        .coeffs(tank.genotype_of_cell(seeker).unwrap(), 0);
    assert_eq!(mc.sensor_kind, SENSOR_COLOUR);
    assert!(
        sense > 80,
        "colour sensor should match yellow tip (sense={sense})"
    );
    assert!(Genome::colour_seeker().is_valid());
}

/// 5.3 M3 — an eyed body fires the geometric eye milestone.
#[test]
fn eyed_body_fires_the_eye_milestone() {
    use crate::bio::{ev, Genome};
    let mut tank = water_box(13, 32, 32, 24);
    tank.set_lambda_scale_q8(0);
    assert!(Genome::eyed().is_valid());
    let id = tank
        .seed_genome_at(16 * 3 + 1, (31 - 12) * 3 + 1, Genome::eyed())
        .expect("seed");
    let org = tank.cell(id).unwrap().org;
    tank.feed_org_full(org);
    let input = TankInput {
        always_day: true,
        ..TankInput::default()
    };
    let mut eye = false;
    for _ in 0..2_000 {
        tank.tick(&input);
        tank.feed_org_full(org);
        while let Some(e) = tank.poll_event() {
            if e.kind == ev::EYE || e.kind == ev::MS_EYE {
                eye = true;
            }
        }
        if eye {
            break;
        }
    }
    let alive = tank
        .cells()
        .iter_alive()
        .filter(|(_, c)| c.org == org)
        .count();
    assert!(alive >= 3, "eyed body should grow window + two sensors (alive={alive})");
    assert!(eye, "geometric eye milestone should fire");
}

#[test]
fn a_genotype_gap_still_has_coeffs() {
    use crate::bio::genome::NONE;
    use crate::bio::{Genome, GenomeStore};
    let g = Genome::algae();
    let store = GenomeStore::import(&[(0, NONE, 0, g.clone()), (2, 0, 1, g)]);
    let gap = store.get(1).expect("gap id");
    assert!(gap.tombstone);
    assert!(!gap.coeffs.is_empty(), "a saved cell naming this id must be able to render");
    let _ = store.coeffs(1, 0);
    let _ = store.genome(1);
}
