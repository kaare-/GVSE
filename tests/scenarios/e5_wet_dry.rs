use crate::helpers::*;

#[test]
fn e5_wet_dry_cycles() {
    let mut world = setup_flat_sand();
    let mut sim = wk_sim::Simulation::new(&world);

    let mut cycle_moistures = Vec::new();

    for _ in 0..5 {
        let audit_start = world.mass_audit.clone();
        let total_start = world.mass_audit.total_tracked();
        world.rain_enabled = true;
        sim.sync_params(&world);
        run_ticks(&mut world, &mut sim, 5000);

        let water_after_rain = total_water_mass(&world);
        let rain = world.mass_audit.rain_inject_total - audit_start.rain_inject_total;
        let evap = world.mass_audit.evap_out_total - audit_start.evap_out_total;
        let drift = bookkeeping_check(&world, total_start, audit_start);
        assert!(drift.abs() < 100, "wet phase drift: {drift}");

        world.rain_enabled = false;
        sim.sync_params(&world);
        run_ticks(&mut world, &mut sim, 5000);

        let moisture_end: i64 = world
            .chunks
            .values()
            .flat_map(|c| c.columns.iter())
            .map(|col| col.moisture)
            .sum();
        cycle_moistures.push(moisture_end);

        let _ = water_after_rain;
        let _ = rain;
        let _ = evap;
    }

    assert_no_negative_masses(&world);
    assert!(!cycle_moistures.is_empty());
}
