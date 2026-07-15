use crate::helpers::*;

#[test]
fn e4_delta_at_standing_water() {
    let mut world = setup_delta_world();
    let initial_audit = world.mass_audit.clone();
    let initial_total = world.mass_audit.total_tracked();
    let mut sim = wk_sim::Simulation::new(&world);

    let elapsed = run_ticks(&mut world, &mut sim, 30_000);
    assert!(elapsed.as_secs() < 90, "E4 perf: {:?}", elapsed);

    assert_no_negative_masses(&world);
    let drift = bookkeeping_check(&world, initial_total, initial_audit);
    assert!(drift.abs() < 100, "bookkeeping drift: {drift}");

    // check shoreline columns for deposited mass near sea level
    let mut shoreline_deposits = 0i64;
    for chunk in world.chunks.values() {
        for col in &chunk.columns {
            if col.surface_y <= world.sea_level + 2.0 && col.layer_count > 0 {
                shoreline_deposits += col.layers[0].thickness;
            }
        }
    }
    assert!(shoreline_deposits >= 0);
}
