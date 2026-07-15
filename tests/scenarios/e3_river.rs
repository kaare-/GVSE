use wk_material::MaterialId;

use crate::helpers::*;

#[test]
fn e3_river_cuts_soft_over_hard() {
    let mut world = setup_river_world();
    let initial_audit = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();

    // record initial stone thickness
    let stone_before: i64 = world
        .chunks
        .values()
        .flat_map(|c| c.columns.iter())
        .flat_map(|col| {
            (0..col.layer_count as usize).filter_map(|i| {
                if col.layers[i].material == MaterialId::Stone {
                    Some(col.layers[i].thickness)
                } else {
                    None
                }
            })
        })
        .sum();

    let mut sim = wk_sim::Simulation::new(&world);
    let elapsed = run_ticks(&mut world, &mut sim, 50_000);
    assert!(elapsed.as_secs() < 120, "E3 perf: {:?}", elapsed);

    assert_no_negative_masses(&world);
    let drift = bookkeeping_check(&world, initial_total, initial_audit);
    assert_eq!(drift, 0, "bookkeeping drift: {drift}");

    let stone_after: i64 = world
        .chunks
        .values()
        .flat_map(|c| c.columns.iter())
        .flat_map(|col| {
            (0..col.layer_count as usize).filter_map(|i| {
                if col.layers[i].material == MaterialId::Stone {
                    Some(col.layers[i].thickness)
                } else {
                    None
                }
            })
        })
        .sum();

    assert_eq!(stone_before, stone_after, "stone should not erode");

    // sand should have eroded somewhere
    let mut total_sand_eroded = 0i64;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            for i in 0..col.layer_count as usize {
                if col.layers[i].material == MaterialId::Sand {
                    total_sand_eroded += 1;
                }
            }
        }
    }
    assert!(total_sand_eroded > 0);
}
