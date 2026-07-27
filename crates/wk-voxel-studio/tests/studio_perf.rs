//! Headless studio profiling (ignored by default).
//!
//! ```text
//! cargo test -p wk-voxel-studio --test studio_perf --release -- --ignored --nocapture
//! ```

use std::time::Instant;

use wk_voxel_studio::{
    evolve_morphology, fin_hydro_arena, hill_climb, rough_walk_arena, StudioPhysicsConfig,
};

#[test]
#[ignore]
fn profile_fin_hydro_and_ga() {
    let mut arena = fin_hydro_arena();
    arena.activate().unwrap();
    let warm = Instant::now();
    for _ in 0..30 {
        arena.tick();
    }
    let _ = warm.elapsed();

    let n = 200u64;
    let t0 = Instant::now();
    for _ in 0..n {
        arena.tick();
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0 / n as f64;
    let tension = arena.body.graph.as_ref().map(|g| g.mean_tension()).unwrap_or(0.0);
    eprintln!("=== fin hydro (scripted muscle) ===");
    eprintln!("  wall {ms:.3} ms/tick  mean_tension={tension:.3}");
    eprintln!(
        "  physics={:?}  size={}x{}",
        "hydro_fin",
        arena.cfg.width,
        arena.cfg.height
    );

    let t1 = Instant::now();
    let (_net, best) = hill_climb(fin_hydro_arena, 6, 50, 11);
    eprintln!("=== hill_climb 6×50 ===");
    eprintln!(
        "  wall {:.1} ms  fitness={:.3} travel={:.3}",
        t1.elapsed().as_secs_f64() * 1000.0,
        best.fitness,
        best.bone_travel
    );

    let t2 = Instant::now();
    let (ind, hist) = evolve_morphology(fin_hydro_arena, 4, 3, 40, 5);
    eprintln!("=== GA morph 4 pop × 3 gen ===");
    eprintln!(
        "  wall {:.1} ms  best={:.3} history={:?}",
        t2.elapsed().as_secs_f64() * 1000.0,
        ind.fitness,
        hist
    );

    let mut walk = rough_walk_arena();
    walk.physics = StudioPhysicsConfig::dry_walk();
    let t3 = Instant::now();
    for _ in 0..100 {
        walk.tick();
    }
    eprintln!(
        "=== rough dry_walk 100 ticks === {:.3} ms/tick  {}x{}",
        t3.elapsed().as_secs_f64() * 10.0,
        walk.cfg.width,
        walk.cfg.height
    );
}
