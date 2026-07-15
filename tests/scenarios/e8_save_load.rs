use std::time::Instant;

use wk_io::{load_from_bytes, save_to_bytes};

use crate::helpers::*;

#[test]
fn e8_save_load_continuation() {
    let mut world = setup_river_world();
    let mut sim = wk_sim::Simulation::new(&world);

    run_ticks(&mut world, &mut sim, 25_000);

    let bytes = save_to_bytes(&world, sim.clock.tick);
    let (world2, tick2) = load_from_bytes(&bytes).expect("load");

    assert_eq!(tick2, 25_000);

    // compare mass audit
    assert_eq!(
        world.mass_audit.by_material,
        world2.mass_audit.by_material
    );

    for (coord, chunk1) in &world.chunks {
        let chunk2 = world2.chunks.get(coord).expect("chunk exists");
        for i in 0..wk_material::CHUNK_W {
            let c1 = &chunk1.columns[i];
            let c2 = &chunk2.columns[i];
            assert_eq!(c1.surface_y, c2.surface_y);
            assert_eq!(c1.layer_count, c2.layer_count);
            for j in 0..c1.layer_count as usize {
                assert_eq!(c1.layers[j].thickness, c2.layers[j].thickness);
                assert_eq!(c1.layers[j].material, c2.layers[j].material);
            }
            assert_eq!(c1.residual, c2.residual);
        }
    }

    let start = Instant::now();
    let _ = save_to_bytes(&world2, tick2);
    let _ = load_from_bytes(&bytes);
    assert!(start.elapsed().as_millis() < 500, "save/load perf");

    let mut world3 = world2;
    let mut sim3 = wk_sim::Simulation::new(&world3);
    sim3.clock.tick = tick2;
    run_ticks(&mut world3, &mut sim3, 25_000);
    assert_no_negative_masses(&world3);
}
