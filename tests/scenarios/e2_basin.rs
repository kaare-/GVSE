use crate::helpers::*;

#[test]
fn e2_basin_collection() {
    let mut world = setup_basin_world();
    let initial_audit = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();
    let mut sim = wk_sim::Simulation::new(&world);

    let elapsed = run_ticks(&mut world, &mut sim, 20_000);
    assert!(elapsed.as_secs() < 10, "E2 perf: {:?}", elapsed);

    assert_no_negative_masses(&world);
    let drift = bookkeeping_check(&world, initial_total, initial_audit);
    assert_eq!(drift, 0, "bookkeeping drift: {drift}");

    // lowest point should accumulate water
    let mut max_water = 0i64;
    let mut max_moisture = 0i64;
    for i in 0..64 {
        let col = &world.chunks.get(&0).unwrap().columns[i];
        max_water = max_water.max(col.surface_water);
        max_moisture = max_moisture.max(col.moisture);
    }
    assert!(max_water > 0 || max_moisture > 0, "basin should hold water");
}
