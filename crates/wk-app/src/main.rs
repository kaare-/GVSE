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
        let inspect = app
            .selected_organism
            .and_then(|e| app.sim.agents.inspect_organism(e));
        let ambient_c = inspect.as_ref().map(|info| {
            app.world.temperature_at_point(
                info.x.floor() as i32,
                info.y,
                app.sim.clock.tick,
            )
        });
        let highlight = app
            .selected_organism
            .and_then(|e| app.sim.agents.organism_highlight_aabb(e))
            .map(|a| (a.min_x, a.max_x, a.min_y, a.max_y));
        // Vertical temp profile for the column tool — skin alone hides the
        // thermocline / geothermal gradient.
        let temp_profile: Vec<(f32, f32)> = app
            .selected_column
            .map(|wx| {
                let (y_top, y_bot) = app
                    .world
                    .column_at(wx)
                    .map(|col| {
                        let top = col.surface_y.max(app.world.sea_level);
                        let bot = (top - 48.0).max(col.climate_elevation() - 4.0);
                        (top, bot)
                    })
                    .unwrap_or((app.world.sea_level, app.world.sea_level - 48.0));
                app.world
                    .sample_temp_column(wx, y_top, y_bot, app.sim.clock.tick)
            })
            .unwrap_or_default();
        // Show a handful of depths (surface → deep), not every 2 m sample.
        let temp_profile_hud: Vec<(f32, f32)> = {
            let n = temp_profile.len();
            if n <= 5 {
                temp_profile
            } else {
                let idxs = [0, n / 4, n / 2, (3 * n) / 4, n - 1];
                idxs.into_iter()
                    .map(|i| temp_profile[i])
                    .collect()
            }
        };
        render::draw_frame(
            &snap,
            app.selected_column,
            app.camera_y_offset,
            &app.status_msg,
            &organisms,
            inspect.as_ref(),
            highlight,
            ambient_c,
            app.show_status_line,
            &temp_profile_hud,
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
