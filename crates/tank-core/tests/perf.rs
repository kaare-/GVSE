use std::time::Instant;

use tank_core::{Tank, TankInput};

/// Phase 0 budget: an agitated 128×256 tick is about 0.8 ms in release
/// on this bench (`tank-sim tank --ticks 1000`). The assert is looser
/// because `cargo test` builds with `opt-level = 2` plus debuginfo.
#[test]
fn preset_thousand_ticks_stay_in_budget() {
    let mut tank = Tank::preset(1);
    let stone_y = (256.0 * 0.18) as i32 + 4;
    let stone = tank.cell_visible(16, stone_y).material;
    let sand = tank.cell_visible(0, 0).material;
    let sand_n = tank.count_material(sand);
    let sat = tank.sat_sum();
    let input = TankInput::default();
    let start = Instant::now();
    for _ in 0..1000 {
        tank.tick(&input);
    }
    let ms = start.elapsed().as_secs_f64() * 1000.0 / 1000.0;
    eprintln!("preset ms/tick = {ms:.3}");
    assert!(ms < 4.0, "tick {ms:.3} ms is too slow for Phase 0");
    assert_eq!(tank.tick_count(), 1000);
    // Soft litter from algae death may soak a few sat units; free water
    // must not appear from nowhere and must not drain the lake.
    assert!(tank.sat_sum() <= sat, "water was created");
    assert!(
        sat - tank.sat_sum() < sat / 100,
        "lost too much water: before={sat} after={}",
        tank.sat_sum()
    );
    assert_eq!(tank.count_material(stone), 1);
    assert!(tank.count_material(sand) <= sand_n);
    assert_eq!(tank.cell_visible(16, stone_y).material, stone);
}
