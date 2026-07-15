use crate::helpers::*;

#[test]
fn e6_flow_across_chunk_boundary() {
    let world0 = setup_two_chunks();
    let left0 = world0.column_at(63).unwrap().surface_y;
    let right0 = world0.column_at(64).unwrap().surface_y;
    assert!(
        (left0 - right0).abs() < 0.5,
        "initial seam gap: {}",
        (left0 - right0).abs()
    );

    let mut world = world0;
    let initial_audit = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();
    let mut sim = wk_sim::Simulation::new(&world);

    let elapsed = run_ticks(&mut world, &mut sim, 15_000);
    assert!(elapsed.as_secs() < 20, "E6 perf: {:?}", elapsed);

    assert_no_negative_masses(&world);
    let drift = bookkeeping_check(&world, initial_total, initial_audit);
    assert_eq!(drift, 0, "bookkeeping drift: {drift}");

    // After flow, seam columns should both have interacted with water
    let left = world.column_at(63).unwrap();
    let right = world.column_at(64).unwrap();
    assert!(
        left.surface_water > 0 || right.surface_water > 0 || left.moisture > 0 || right.moisture > 0,
        "expected water activity at seam"
    );
}
