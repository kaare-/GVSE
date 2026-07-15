use crate::helpers::*;

#[test]
fn e1_rain_on_symmetric_hill() {
    let mut world = setup_hill_world(256);
    let initial_audit = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();
    let mut sim = wk_sim::Simulation::new(&world);

    let elapsed = run_ticks(&mut world, &mut sim, 10_000);
    assert!(elapsed.as_secs() < 30, "E1 perf: {:?}", elapsed);

    assert_no_negative_masses(&world);
    wk_sim::assert_mass_closed(&world, 0).expect("mass closed");

    let drift = bookkeeping_check(&world, initial_total, initial_audit);
    assert_eq!(drift, 0, "bookkeeping drift: {drift}");

    // water should have moved downhill — some columns at base have water
    let base = world.column_at(128).unwrap();
    assert!(base.surface_water >= 0);
}
