//! World Kernel 0.1 — interactive debug application.

mod editor;
mod render;
mod state;

use macroquad::prelude::*;

use state::AppState;

#[macroquad::main("GVSE World Kernel 0.1")]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "--soak") {
        let ticks: u64 = args
            .iter()
            .position(|a| a == "--soak")
            .and_then(|i| args.get(i + 1))
            .and_then(|s| s.parse().ok())
            .unwrap_or(1_000_000);
        run_headless_soak(ticks);
        return;
    }

    let mut app = AppState::new();

    loop {
        app.handle_input();
        app.update();

        let snap = app.snapshot();
        let organisms = app.sim.agents.organism_draw_list();
        render::draw_frame(
            &snap,
            app.selected_column,
            app.camera_y_offset,
            &app.status_msg,
            &organisms,
        );
        if app.editor.open {
            app.editor.draw();
        } else {
            app.draw_settings_ui();
        }

        next_frame().await;
    }
}

fn run_headless_soak(ticks: u64) {
    use std::time::Instant;
    use wk_sim::Simulation;
    use wk_world::terrain::generate_chunk_stratified_tilt;
    use wk_world::world::World;

    let mut world = World::new(42);
    world.rain_enabled = true;
    world.rain_rate = 100.0;
    for c in 0..8 {
        world.insert_chunk(generate_chunk_stratified_tilt(c, world.seed, 0.0, 0.01));
    }
    world.wake_all();
    world.recompute_mass_audit();

    let mut sim = Simulation::new(&world);
    let start = Instant::now();
    sim.run_ticks(&mut world, ticks);
    let elapsed = start.elapsed();
    let tps = ticks as f64 / elapsed.as_secs_f64();
    println!(
        "Soak: {} ticks in {:?} ({:.0} tps), total mass={}",
        ticks,
        elapsed,
        tps,
        world.mass_audit.total_tracked()
    );
}
