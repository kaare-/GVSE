use macroquad::prelude::*;
use wk_io::{load_simulation, save_simulation};
use wk_world::{OverlayMode, RenderSnapshot};

use crate::editor::CreatureEditor;
use crate::render::{self, SAVE_PATH};

/// Columns scrolled per second while A/D is held.
const SCROLL_SPEED_COLS_PER_SEC: f32 = 70.0;
/// Screen pixels per second while W/S is held to pan the camera up/down.
const CAMERA_Y_SPEED_PX_PER_SEC: f32 = 320.0;
/// Ceiling on catch-up ticks per rendered frame. Kept low so a slow
/// frame can't recruit dozens of extra sim ticks (positive-feedback
/// death spiral: slow frame → big dt → more catch-up ticks → slower
/// frame → even bigger dt → …). Better to let sim time drift a hair
/// behind real time than freeze the app.
const MAX_TICKS_PER_FRAME: u64 = 4;
/// Frame-time clamp for sim scheduling. Real render dt can spike to
/// hundreds of ms during hitches; treating that as “advance 30 ticks”
/// makes the next frame ten times worse. Cap the sim clock's view of
/// wall-clock progress so `tick_accum` can't runaway.
const MAX_SIM_DT_SEC: f32 = 1.0 / 30.0;
/// Default ring circumference (192 × 64 cols ≈ 3 km). Matches
/// `WorldGenParams::default_ring`; `MAX_LOADED_CHUNKS` must be ≥ this.
const RING_CHUNKS: u32 = 192;

pub struct AppState {
    pub world: wk_world::world::World,
    pub sim: wk_sim::Simulation,
    pub viewport_x: i32,
    scroll_accum: f32,
    /// Screen-pixel offset added to `sea_y` when rendering. Zero puts
    /// the sea line at the default `SEA_SCREEN_FRAC` position. Positive
    /// shifts the world downward on screen (i.e. the camera looks up
    /// at higher elevations); negative shifts it upward (camera looks
    /// down). Driven by W/S; deliberately manual so the view never
    /// slides on its own when terrain grows or shrinks.
    pub camera_y_offset: f32,
    pub paused: bool,
    pub speed: u32,
    pub overlay_mode: OverlayMode,
    pub selected_column: Option<i32>,
    /// Clicked Set A organism (cleared when it dies).
    pub selected_organism: Option<wk_sim::Entity>,
    pub tick_accum: f32,
    pub status_msg: String,
    /// Bottom info strip (tick / fps / clouds / last key). Toggle with F3.
    pub show_status_line: bool,
    pub show_settings: bool,
    /// Scratch UI-bound copies (macroquad sliders need `&mut f32`, and
    /// day/night length is more natural to edit in minutes than raw ticks).
    settings_day_minutes: f32,
    settings_night_minutes: f32,
    settings_max_clouds: f32,
    settings_cloud_spawn_secs: f32,
    pub editor: CreatureEditor,
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
        world.gen = wk_world::WorldGenParams {
            topology: wk_world::WorldTopology::Ring {
                chunks: RING_CHUNKS,
            },
            profile: wk_world::WorldGenProfile::RingFacies,
        };

        for c in 0..RING_CHUNKS as i32 {
            let chunk = wk_world::terrain::generate_chunk(
                c,
                world.seed,
                wk_world::terrain::BEDROCK_FLOOR_M,
                world.sea_level,
                world.gen,
            );
            world.insert_chunk(chunk);
        }
        world.wake_all();
        world.recompute_mass_audit();
        // Stage 6.2 / 6.3: chunk thermal + humidity fields.
        // Off by default in World::new so older scenario tests keep the
        // climate-only / constant-RH paths; the live app always opts in.
        world.enable_thermal_fields();
        world.enable_humidity_fields();
        world.enable_pressure_wind_fields();
        world.enable_groundwater_head_fields();
        world.enable_dissolved_fields();
        // Free-surface momentum: wind setup / seiches + a gentle tide.
        // Lake-level still flattens shallow ponds but leaves deep water alone.
        world.surface_waves_enabled = true;
        world.tide_enabled = true;

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
            camera_y_offset: 0.0,
            paused: true,
            speed: 1,
            overlay_mode: OverlayMode::None,
            selected_column: None,
            selected_organism: None,
            tick_accum: 0.0,
            status_msg:
                "Space run | click creature to inspect | A/D scroll | C/F2 creature | F3 HUD | Tab settings"
                    .into(),
            show_status_line: true,
            show_settings: false,
            settings_day_minutes,
            settings_night_minutes,
            settings_max_clouds,
            settings_cloud_spawn_secs,
            editor: CreatureEditor::default(),
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
        if self.world.topology().is_ring() {
            if let Some(w) = self.world.topology().width_columns() {
                self.viewport_x = self.viewport_x.rem_euclid(w);
            }
            return;
        }
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
        // Freeze the world while the creature editor is open.
        if self.editor.open {
            self.clamp_viewport();
            return;
        }
        if !self.paused {
            // Clamp frame dt for sim scheduling so a slow frame (or a
            // pause/resume gap) doesn't request a burst of catch-up
            // ticks that makes the next frame even slower.
            let dt = get_frame_time().min(MAX_SIM_DT_SEC);
            self.tick_accum += dt * self.speed as f32 * 60.0;
            let mut n = 0u64;
            while self.tick_accum >= 1.0 && n < MAX_TICKS_PER_FRAME {
                self.step_sim(1);
                self.tick_accum -= 1.0;
                n += 1;
            }
            // Any leftover accum from a hitch is thrown away — better
            // than carrying it into a queued catch-up burst.
            if n == MAX_TICKS_PER_FRAME {
                self.tick_accum = 0.0;
            }
        }
        self.clamp_viewport();
    }

    fn open_or_close_editor(&mut self) {
        let opening = !self.editor.open;
        self.editor.toggle(self.paused);
        if opening {
            self.paused = true;
            self.show_settings = false;
            self.status_msg =
                "Creature editor OPEN — paint Atom, Enter to spawn, C/F2 to close"
                    .into();
        } else {
            self.paused = self.editor.was_paused;
            self.status_msg = "Creature editor closed".into();
        }
    }

    pub fn handle_input(&mut self) {
        // Show last key on the HUD so we can tell if the window has focus.
        if let Some(k) = get_last_key_pressed() {
            if !self.editor.open {
                self.status_msg = format!(
                    "key {:?} | Space run | C/F2 creature | F3 HUD | Tab settings",
                    k
                );
            }
        }

        // Creature editor: C or F2.
        if is_key_pressed(KeyCode::C) || is_key_pressed(KeyCode::F2) {
            self.open_or_close_editor();
        }
        if is_key_pressed(KeyCode::F3) {
            self.show_status_line = !self.show_status_line;
            self.status_msg = format!("Status line: {}", self.show_status_line);
        }
        if self.editor.open {
            let _ = self.editor.handle_input();
            if self.editor.spawn_picker && is_mouse_button_pressed(MouseButton::Left) {
                let (mx, _my) = mouse_position();
                let col = render::screen_x_to_world_x(mx, self.viewport_x);
                if self.world.column_at(col).is_some() {
                    let mut bp = self.editor.blueprint.clone();
                    bp.name = self.editor.blueprint.name.clone();
                    let plankton = bp.is_plankton();
                    match self.sim.agents.spawn_from_blueprint(
                        &self.world,
                        col,
                        bp,
                        50.0,
                    ) {
                        Some(_) => {
                            let where_ = if plankton { "water/lit band" } else { "land" };
                            self.status_msg = format!(
                                "Spawned {} on {where_} at x={col} (organisms={})",
                                self.editor.blueprint.name,
                                self.sim.agents.organism_count()
                            );
                            self.editor.spawn_picker = false;
                            self.editor.open = false;
                            self.paused = self.editor.was_paused;
                        }
                        None => {
                            self.editor.status =
                                "Spawn failed (need nucleus+photo; rooted designs need land; or at cap)"
                                    .into();
                        }
                    }
                }
            }
            return;
        }
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

        // Vertical camera pan (W = look higher, S = look lower). Deliberately
        // manual — nothing auto-scrolls when terrain grows, since the previous
        // "surface-relative depth window" made the ground appear to swell
        // upward whenever weather piled up.
        let mut vdir = 0.0f32;
        if is_key_down(KeyCode::W) {
            vdir += 1.0;
        }
        if is_key_down(KeyCode::S) {
            vdir -= 1.0;
        }
        if vdir != 0.0 {
            let dt = get_frame_time();
            self.camera_y_offset += vdir * CAMERA_Y_SPEED_PX_PER_SEC * dt;
        }

        if is_key_pressed(KeyCode::R) {
            self.world.rain_enabled = !self.world.rain_enabled;
            self.status_msg = format!("Rain: {}", self.world.rain_enabled);
        }
        if is_key_pressed(KeyCode::Y) {
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
                OverlayMode::Conservation => OverlayMode::TemperatureField,
                OverlayMode::TemperatureField => OverlayMode::HumidityField,
                OverlayMode::HumidityField => OverlayMode::Co2Field,
                OverlayMode::Co2Field => OverlayMode::O2Field,
                OverlayMode::O2Field => OverlayMode::None,
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
        // Save/load moved off S/L now that S is a continuous camera-pan key.
        if is_key_pressed(KeyCode::F5) {
            match std::fs::write(SAVE_PATH, save_simulation(&self.world, &self.sim)) {
                Ok(()) => {
                    self.status_msg = format!("Saved tick {} → {SAVE_PATH}", self.sim.clock.tick);
                }
                Err(e) => self.status_msg = format!("Save failed: {e}"),
            }
        }
        if is_key_pressed(KeyCode::F9) {
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
            let (mx, my) = mouse_position();
            let wx = render::screen_x_to_world_x_frac(mx, self.viewport_x);
            let wy = render::screen_y_to_world_y(
                my,
                self.world.sea_level,
                screen_height(),
                self.camera_y_offset,
            );
            if let Some(e) = self.sim.agents.pick_organism_at(wx, wy) {
                self.selected_organism = Some(e);
                self.selected_column = Some(wx.floor() as i32);
                if let Some(info) = self.sim.agents.inspect_organism(e) {
                    self.status_msg = format!(
                        "Inspect #{} gen={} energy={:.0}/{:.0} clones={}",
                        info.entity_id,
                        info.generation,
                        info.energy,
                        info.energy_max,
                        info.clones_produced
                    );
                }
            } else {
                self.selected_organism = None;
                let col = render::screen_x_to_world_x(mx, self.viewport_x);
                if self.world.column_at(col).is_some() {
                    self.selected_column = Some(col);
                }
            }
        }

        // Drop selection if the creature died.
        if let Some(e) = self.selected_organism {
            if !self.sim.agents.organism_alive(e) {
                self.selected_organism = None;
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
                    ui.checkbox(hash!(), "Weather enabled (Y)", &mut self.world.weather.weather_enabled);
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
                    ui.slider(hash!(), "Max clouds", 1.0f32..40.0, &mut self.settings_max_clouds);
                    ui.slider(
                        hash!(),
                        "Cloud spawn interval (sec)",
                        1.0f32..600.0,
                        &mut self.settings_cloud_spawn_secs,
                    );
                    ui.label(None, &format!("Active clouds: {}", self.world.clouds.len()));
                });
                ui.separator();
                ui.tree_node(hash!(), "Waves + tide", |ui| {
                    ui.checkbox(
                        hash!(),
                        "Surface waves (wind + gravity)",
                        &mut self.world.surface_waves_enabled,
                    );
                    ui.checkbox(hash!(), "Tide enabled", &mut self.world.tide_enabled);
                    ui.slider(
                        hash!(),
                        "Tide amplitude (m)",
                        0.0f32..2.0,
                        &mut self.world.tide_amplitude_m,
                    );
                    let mut period_min =
                        self.world.tide_period_ticks as f32 / 60.0;
                    ui.slider(hash!(), "Tide period (sec)", 60.0f32..7200.0, &mut period_min);
                    self.world.tide_period_ticks = period_min.max(60.0) as u64;
                    ui.label(
                        None,
                        &format!(
                            "Tide η now: {:+.2} m",
                            self.world.tide_eta_m(self.sim.clock.tick)
                        ),
                    );
                });
                ui.separator();
                if ui.button(None, "Open creature editor") {
                    // Close settings; editor opens next frame via flag.
                    self.show_settings = false;
                    if !self.editor.open {
                        self.open_or_close_editor();
                    }
                }
                ui.label(None, &format!("Sim tick: {}", self.sim.clock.tick));
                ui.label(None, "Tip: C / F2 creature editor · F3 status line");
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
