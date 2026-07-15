use macroquad::prelude::*;
use wk_io::{load_simulation, save_simulation};
use wk_world::{OverlayMode, RenderSnapshot};

use crate::render::{self, SAVE_PATH};

/// Columns scrolled per second while A/D is held.
const SCROLL_SPEED_COLS_PER_SEC: f32 = 70.0;
const MAX_TICKS_PER_FRAME: u64 = 60;
/// Chunks to generate: 52 chunks × 64 cols = 3328 columns (~832 m) — wide
/// enough for the full mountain range (multiple peaks + enclosed valleys).
const MAP_CHUNK_MIN: i32 = -4;
const MAP_CHUNK_MAX: i32 = 48;

pub struct AppState {
    pub world: wk_world::world::World,
    pub sim: wk_sim::Simulation,
    pub viewport_x: i32,
    scroll_accum: f32,
    pub paused: bool,
    pub speed: u32,
    pub overlay_mode: OverlayMode,
    pub selected_column: Option<i32>,
    pub tick_accum: f32,
    pub status_msg: String,
    pub show_settings: bool,
    /// Scratch UI-bound copies (macroquad sliders need `&mut f32`, and
    /// day/night length is more natural to edit in minutes than raw ticks).
    settings_day_minutes: f32,
    settings_night_minutes: f32,
    settings_max_clouds: f32,
    settings_cloud_spawn_secs: f32,
}

impl AppState {
    pub fn new() -> Self {
        let mut world = wk_world::world::World::new(42);
        world.sea_level = 12.0;
        world.rain_enabled = false;
        // Modest by design: a whole mountainside's worth of columns raining
        // at once adds up fast, and heavier rates turned a few seconds of
        // rain into a flood before it could drain/evaporate/infiltrate.
        // (RainInject now fires every tick instead of every 6th, so this is
        // ~1/6 of the old nominal value to deliver the same average total.)
        world.rain_rate = 1.0;

        for c in MAP_CHUNK_MIN..MAP_CHUNK_MAX {
            let chunk = wk_world::terrain::generate_chunk_continental(
                c,
                world.seed,
                wk_world::terrain::BEDROCK_FLOOR_M,
                world.sea_level,
            );
            world.insert_chunk(chunk);
        }
        world.wake_all();
        world.recompute_mass_audit();

        let sim = wk_sim::Simulation::new(&world);
        let viewport_x = Self::initial_viewport_x(&world);
        let settings_day_minutes = world.climate.day_length_ticks as f32 / 60.0 / 60.0;
        let settings_night_minutes = world.climate.night_length_ticks as f32 / 60.0 / 60.0;
        let settings_max_clouds = world.weather.max_clouds as f32;
        let settings_cloud_spawn_secs = world.weather.cloud_spawn_interval_ticks as f32 / 60.0;
        Self {
            world,
            sim,
            viewport_x,
            scroll_accum: 0.0,
            paused: true,
            speed: 1,
            overlay_mode: OverlayMode::None,
            selected_column: None,
            tick_accum: 0.0,
            status_msg: "Space run | A/D scroll | R rain | W weather | Tab settings".into(),
            show_settings: false,
            settings_day_minutes,
            settings_night_minutes,
            settings_max_clouds,
            settings_cloud_spawn_secs,
        }
    }

    fn initial_viewport_x(world: &wk_world::world::World) -> i32 {
        let Some((x_min, x_max)) = world.world_x_bounds() else {
            return 0;
        };
        let land = world
            .first_emergent_x(world.sea_level)
            .unwrap_or(x_min + (x_max - x_min) / 3);
        (land - 80).clamp(x_min, x_max)
    }

    fn scroll_bounds(&self) -> (i32, i32) {
        let Some((x_min, x_max)) = self.world.world_x_bounds() else {
            return (0, 0);
        };
        let sw = screen_width();
        let visible = render::viewport_column_count(sw) as i32;
        let max_x = (x_max - visible + 1).max(x_min);
        (x_min, max_x)
    }

    fn clamp_viewport(&mut self) {
        let (lo, hi) = self.scroll_bounds();
        self.viewport_x = self.viewport_x.clamp(lo, hi);
    }

    pub fn snapshot(&self) -> RenderSnapshot {
        let sw = screen_width();
        let width = render::viewport_column_count(sw);
        let mut overlay = self.sim.overlay();
        overlay.mode = self.overlay_mode;
        self.world.snapshot(
            self.sim.clock.tick,
            self.viewport_x,
            width,
            overlay,
            false,
        )
    }

    pub fn step_sim(&mut self, n: u64) {
        self.sim.run_ticks(&mut self.world, n);
    }

    pub fn update(&mut self) {
        if !self.paused {
            let dt = get_frame_time();
            self.tick_accum += dt * self.speed as f32 * 60.0;
            let mut n = 0u64;
            while self.tick_accum >= 1.0 && n < MAX_TICKS_PER_FRAME {
                self.step_sim(1);
                self.tick_accum -= 1.0;
                n += 1;
            }
        }
        self.clamp_viewport();
    }

    pub fn handle_input(&mut self) {
        if is_key_pressed(KeyCode::Space) {
            self.paused = !self.paused;
        }
        if is_key_pressed(KeyCode::Period) {
            self.step_sim(1);
        }
        if is_key_pressed(KeyCode::Key1) {
            self.speed = 1;
        }
        if is_key_pressed(KeyCode::Key2) {
            self.speed = 5;
        }
        if is_key_pressed(KeyCode::Key3) {
            self.speed = 20;
        }
        if is_key_pressed(KeyCode::Key4) {
            self.speed = 100;
        }
        let mut dir = 0.0f32;
        if is_key_down(KeyCode::A) || is_key_down(KeyCode::Left) {
            dir -= 1.0;
        }
        if is_key_down(KeyCode::D) || is_key_down(KeyCode::Right) {
            dir += 1.0;
        }
        if dir != 0.0 {
            let dt = get_frame_time();
            self.scroll_accum += dir * SCROLL_SPEED_COLS_PER_SEC * dt;
        }
        let whole = self.scroll_accum.trunc() as i32;
        if whole != 0 {
            self.viewport_x += whole;
            self.scroll_accum -= whole as f32;
            self.clamp_viewport();
        }
        if is_key_pressed(KeyCode::R) {
            self.world.rain_enabled = !self.world.rain_enabled;
            self.status_msg = format!("Rain: {}", self.world.rain_enabled);
        }
        if is_key_pressed(KeyCode::W) {
            self.world.weather.weather_enabled = !self.world.weather.weather_enabled;
            self.status_msg = format!("Weather: {}", self.world.weather.weather_enabled);
        }
        if is_key_pressed(KeyCode::Tab) {
            self.show_settings = !self.show_settings;
        }
        if is_key_pressed(KeyCode::LeftBracket) {
            self.world.sea_level -= 1.0;
        }
        if is_key_pressed(KeyCode::RightBracket) {
            self.world.sea_level += 1.0;
        }
        if is_key_pressed(KeyCode::O) {
            self.overlay_mode = match self.overlay_mode {
                OverlayMode::None => OverlayMode::WaterFlux,
                OverlayMode::WaterFlux => OverlayMode::Erosion,
                OverlayMode::Erosion => OverlayMode::Activity,
                OverlayMode::Activity => OverlayMode::Conservation,
                OverlayMode::Conservation => OverlayMode::None,
            };
        }
        if is_key_pressed(KeyCode::M) {
            if let Some(wx) = self.selected_column {
                let _ = self.world.add_marker(
                    wx,
                    format!("m{}", self.sim.clock.tick),
                    self.sim.clock.tick,
                );
                self.status_msg = format!("Marker at x={wx}");
            }
        }
        if is_key_pressed(KeyCode::S) {
            match std::fs::write(SAVE_PATH, save_simulation(&self.world, &self.sim)) {
                Ok(()) => {
                    self.status_msg = format!("Saved tick {} → {SAVE_PATH}", self.sim.clock.tick);
                }
                Err(e) => self.status_msg = format!("Save failed: {e}"),
            }
        }
        if is_key_pressed(KeyCode::L) {
            match std::fs::read(SAVE_PATH) {
                Ok(bytes) => match load_simulation(&bytes) {
                    Ok((world, sim)) => {
                        self.world = world;
                        self.sim = sim;
                        self.viewport_x = Self::initial_viewport_x(&self.world);
                        self.clamp_viewport();
                        self.status_msg =
                            format!("Loaded tick {} from {SAVE_PATH}", self.sim.clock.tick);
                    }
                    Err(e) => self.status_msg = format!("Load parse failed: {e}"),
                },
                Err(e) => self.status_msg = format!("Load read failed: {e}"),
            }
        }

        if is_mouse_button_pressed(MouseButton::Left) {
            let (mx, _) = mouse_position();
            let col = render::screen_x_to_world_x(mx, self.viewport_x);
            if self.world.column_at(col).is_some() {
                self.selected_column = Some(col);
            }
        }
    }

    /// Live-editable climate parameters, for tuning day/night length and
    /// temperature behaviour without recompiling. Toggle with Tab.
    pub fn draw_settings_ui(&mut self) {
        if !self.show_settings {
            return;
        }
        use macroquad::hash;
        use macroquad::ui::{root_ui, widgets};

        widgets::Window::new(hash!(), vec2(20.0, 20.0), vec2(360.0, 480.0))
            .label("Settings (Tab to close)")
            .ui(&mut root_ui(), |ui| {
                ui.tree_node(hash!(), "Day / night / temperature", |ui| {
                    ui.slider(hash!(), "Day length (min)", 0.5f32..60.0, &mut self.settings_day_minutes);
                    ui.slider(hash!(), "Night length (min)", 0.5f32..60.0, &mut self.settings_night_minutes);
                    ui.slider(hash!(), "Base temp (C)", -20.0f32..40.0, &mut self.world.climate.base_temp_c);
                    ui.slider(
                        hash!(),
                        "Lapse rate (C/m)",
                        0.0f32..1.0,
                        &mut self.world.climate.lapse_rate_c_per_m,
                    );
                    ui.slider(
                        hash!(),
                        "Day/night swing (C)",
                        0.0f32..20.0,
                        &mut self.world.climate.day_night_amplitude_c,
                    );
                    ui.slider(
                        hash!(),
                        "Freeze point (C)",
                        -20.0f32..20.0,
                        &mut self.world.climate.freeze_point_c,
                    );
                });
                ui.separator();
                ui.tree_node(hash!(), "Weather (wind + clouds)", |ui| {
                    ui.checkbox(hash!(), "Weather enabled (W)", &mut self.world.weather.weather_enabled);
                    ui.slider(
                        hash!(),
                        "Wind speed (col/tick)",
                        -2.0f32..2.0,
                        &mut self.world.climate.wind_speed,
                    );
                    ui.slider(
                        hash!(),
                        "Cloud rain rate (kg/tick)",
                        0.0f32..10.0,
                        &mut self.world.weather.cloud_rain_rate,
                    );
                    ui.slider(hash!(), "Max clouds", 1.0f32..20.0, &mut self.settings_max_clouds);
                    ui.slider(
                        hash!(),
                        "Cloud spawn interval (sec)",
                        1.0f32..600.0,
                        &mut self.settings_cloud_spawn_secs,
                    );
                    ui.label(None, &format!("Active clouds: {}", self.world.clouds.len()));
                });
                ui.separator();
                ui.label(None, &format!("Sim tick: {}", self.sim.clock.tick));
            });

        self.world.weather.max_clouds = self.settings_max_clouds.round().max(1.0) as usize;
        self.world.weather.cloud_spawn_interval_ticks =
            (self.settings_cloud_spawn_secs.max(1.0) * 60.0) as u64;

        self.world.climate.day_length_ticks =
            (self.settings_day_minutes.max(0.1) * 60.0 * 60.0) as u64;
        self.world.climate.night_length_ticks =
            (self.settings_night_minutes.max(0.1) * 60.0 * 60.0) as u64;
    }
}
