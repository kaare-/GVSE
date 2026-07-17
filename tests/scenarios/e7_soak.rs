use crate::helpers::*;

#[test]
fn e7_one_million_updates() {
    let ticks: u64 = std::env::var("WK_SOAK_TICKS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(100_000);

    let mut world = setup_river_world();
    let initial_audit = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();
    world.wake_all();
    let mut sim = wk_sim::Simulation::new(&world);

    let elapsed = run_ticks(&mut world, &mut sim, ticks);
    let max_secs = if ticks >= 1_000_000 { 300 } else { 180 };
    assert!(elapsed.as_secs() < max_secs, "E7 perf: {:?}", elapsed);

    assert_no_negative_masses(&world);
    let drift = bookkeeping_check(&world, initial_total, initial_audit);
    assert!(drift.abs() < 100_000, "bookkeeping drift: {drift}");

    let tps = ticks as f64 / elapsed.as_secs_f64().max(0.001);
    eprintln!("E7: {} ticks in {:?} ({:.0} tps)", ticks, elapsed, tps);
    assert!(tps > 100.0, "performance too low: {tps} tps");
}
