//! wk-voxel-app: renderer / demo binary for wk-voxel.
//!
//! Isolation contract: wk-voxel-app is part of the greenfield voxel
//! stack. It depends on wk-voxel + wk-material only. It MUST NOT
//! import from wk-world / wk-field / wk-agents / wk-sim / wk-io /
//! wk-app. See docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Draws each `Cell` as a filled rectangle. Palette comes straight
//! from [`wk_material::MaterialRegistry::colour_rgb`] so worlds line
//! up visually with column-based GVSE. Water is `Air + sat = FULL`;
//! we blend Air's sky-blue toward the Water palette entry as sat
//! rises so raindrops shade from blue-white to lake blue.
//!
//! Hotkeys:
//! - `Space` — pause / resume physics ticks
//! - `R` — regenerate the world with a new seed
//! - `W` — toggle background rain (climatic, always-on cloud row)
//! - `C` — toggle condensation rain (feedback from humidity heatmap)
//! - `E` — toggle evaporation (routes into the humidity heatmap)
//! - `K` — toggle karst dissolution
//! - `O` — toggle Set A organisms (Atom step)
//! - `H` — toggle humidity tile diagnostic + wind streaks (default on)
//! - `N` — toggle soft clouds at all depths (active parcels + far/mid/front echoes + precip)
//! - `T` — toggle temperature heatmap overlay
//! - `U` — toggle ground saturation heatmap (pores + free water)
//! - `M` — toggle mycelium strain overlay (bright per-network colors)
//! - `G` — cycle geotech overlay (shear → σᵥ → wet → off)
//! - `I` — toggle phase change master (freeze / thaw / snow / slush; also in Tab)
//! - `F1` — toggle HUD chrome (bottom info/tools + block inspector)
//! - `F2` — creature editor (Atom / plant MS-Paint; `C` stays condensation)
//! - `F3` — terrain editor (paint / erase block types; world stays visible)
//! - `F4` — creature list (living / dead roster; click row to inspect)
//! - `F5` / `F9` — save / load simulation (`saves/*.gvsesim`)
//! - `F6` — glossary / how-it-works (keys, water, sky, HUD words)
//! - `Tab` — live settings (world size, materials, wind, clouds, …)
//! - click — block / organism inspector (hidden while F1 HUD is off)
//! - `Left` / `Right` — pan the camera horizontally (wraps on ring worlds)
//! - `Up` / `Down` — pan vertically
//! - `Esc` — close overlays, or quit confirm (save / discard / cancel)
//!
//! Sky follows the shared climate clock (pixel sun by day, pixel moon by night).
//! Temperature tiles warm with sun, cool at night, and shade under pixel clouds.
//! Atmosphere stack: `docs/SKY.md` / [`atmosphere`].

mod atmosphere;
mod creature_list;
mod editor;
mod glossary;
mod inspector;
mod palette;
mod quit;
mod scene;
mod settings;
mod spore_fx;
mod terrain;

use macroquad::prelude::*;
use wk_voxel::{
    apply_cold_avalanche_bound, apply_condensation_rain_phased,
    apply_evaporation_into_humidity_climate,
    apply_flow_erosion_bound, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    apply_weather_rgb, celestial_local_cfg, celestial_moon_screen_pos_cfg,
    celestial_sun_screen_pos_cfg, collect_live_root_world_cells, day_night_factor_cfg,
    geotech_map_due, humidity_diffuse_due, is_daytime_cfg, is_standing_water,
    pore_wetness_with, precip_forms_snow_at_air, sail_plants_on_wind_rafts_cfg, set_parallel_enabled,
    step_carbon_budget, temperature_step_due, tick_with_life, wake_unsupported_grains,
    wake_unstable_slopes, GeotechOverlayMode, SimSnapshot, WorldgenParams,
};

use crate::atmosphere::{
    apply_celestial_key_rgb, apply_organism_celestial_key_rgb, draw_canopy_air_dim,
    draw_celestials, draw_clouds, draw_depth_cloud_layer, draw_haze_and_wind, CloudDepthLayer,
    draw_ridge_silhouettes, draw_sky, estimate_snow_bias, is_organism_aboveground,
    organism_celestial_rim, sky_weather_for_scene, terrain_celestial_key_strength,
    toward_light_celestial, RidgeSilhouette,
};
use crate::creature_list::CreatureList;
use crate::editor::CreatureEditor;
use crate::glossary::Glossary;
use crate::inspector::{draw_block_inspector, draw_selection_outline, screen_to_world};
use crate::palette::cell_color;
use crate::quit::{QuitChoice, QuitDialog};
use crate::scene::Scene;
use crate::settings::SimSettings;
use crate::spore_fx::SporeFx;
use crate::terrain::{TerrainEditor, TerrainTool};

fn window_conf() -> Conf {
    Conf {
        window_title: "wk-voxel demo".into(),
        window_width: 1280,
        window_height: 720,
        high_dpi: true,
        ..Default::default()
    }
}

/// Cell size in on-screen pixels. Independent of world coordinates —
/// change this to zoom.
const PX_PER_CELL: f32 = 3.0;
/// Info strip height (tick / fps / toggles). Tool line adds another
/// band. Both are omitted when `F1` hides HUD chrome.
const INFO_H: f32 = 24.0;
const TOOL_H: f32 = 20.0;

fn hud_height(show_hud: bool) -> f32 {
    if show_hud {
        INFO_H + TOOL_H
    } else {
        0.0
    }
}

/// Smoothed FPS — EMA of `get_frame_time()` (steadier than raw
/// `get_fps()`, same approach as column-GVSE).
fn fps_smoothed() -> f32 {
    thread_local! {
        static AVG_DT: std::cell::Cell<f32> = const { std::cell::Cell::new(1.0 / 60.0) };
    }
    let dt = get_frame_time().max(1e-4);
    AVG_DT.with(|cell| {
        let prev = cell.get();
        let next = prev * 0.9 + dt * 0.1;
        cell.set(next);
        1.0 / next
    })
}

/// Cool cyan → hot amber for geotech shear score on face cells.
fn geotech_overlay_color(score: f32, s_max: f32) -> Color {
    let t = if s_max > 0.0 {
        (score / s_max).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let r = (40.0 + t * 215.0) as u8;
    let g = (180.0 - t * 100.0) as u8;
    let b = (220.0 - t * 200.0) as u8;
    let a = (90.0 + t * 130.0) as u8;
    Color::from_rgba(r, g, b, a)
}

fn temp_overlay_color(temp_c: f32, t_min: f32, t_max: f32) -> Color {
    let u = ((temp_c - t_min) / (t_max - t_min).max(0.5)).clamp(0.0, 1.0);
    let (r, g, b) = if u < 0.33 {
        let t = u / 0.33;
        (
            (20.0 + t * 40.0) as u8,
            (80.0 + t * 120.0) as u8,
            (200.0 - t * 40.0) as u8,
        )
    } else if u < 0.66 {
        let t = (u - 0.33) / 0.33;
        (
            (60.0 + t * 180.0) as u8,
            (200.0 - t * 40.0) as u8,
            (160.0 - t * 140.0) as u8,
        )
    } else {
        let t = (u - 0.66) / 0.34;
        (
            (240.0 - t * 20.0) as u8,
            (160.0 - t * 140.0) as u8,
            (20.0 + t * 20.0) as u8,
        )
    };
    Color::from_rgba(r, g, b, 120)
}

/// Dry tan → wet deep blue for ground pore / free-water saturation.
fn sat_overlay_color(wet: f32) -> Color {
    let t = wet.clamp(0.0, 1.0);
    let (r, g, b) = if t < 0.5 {
        let u = t / 0.5;
        (
            (190.0 - u * 130.0) as u8,
            (155.0 - u * 45.0) as u8,
            (95.0 + u * 85.0) as u8,
        )
    } else {
        let u = (t - 0.5) / 0.5;
        (
            (60.0 - u * 45.0) as u8,
            (110.0 - u * 55.0) as u8,
            (180.0 + u * 55.0) as u8,
        )
    };
    let a = (75.0 + t * 155.0) as u8;
    Color::from_rgba(r, g, b, a)
}

fn scale_color_alpha(c: Color, k: f32) -> Color {
    let k = k.clamp(0.0, 1.0);
    Color {
        a: c.a * k,
        ..c
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let params = WorldgenParams::default();
    let mut scene = Scene::new(params);
    let mut settings = SimSettings::new(&scene.params);
    settings.apply_material_overrides(&mut scene.world);
    let mut spore_fx = SporeFx::new();
    let mut paused = false;
    let mut organisms_on = true;
    // Humidity diagnostic default on (`H`); soft clouds default on (`N`).
    let mut humidity_overlay = true;
    let mut clouds_on = true;
    let mut temp_overlay = false;
    let mut sat_overlay = false;
    let mut mycelium_overlay = false;
    let mut geotech_mode = GeotechOverlayMode::Off;
    let mut show_hud = true;
    let mut editor = CreatureEditor::default();
    let mut terrain = TerrainEditor::default();
    let mut creature_list = CreatureList::default();
    let mut glossary = Glossary::default();
    let mut quit_dialog = QuitDialog::default();
    let mut inspect: Option<(i32, i32)> = None;
    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;
    let mut should_quit = false;
    let mut ridges = RidgeSilhouette::default();

    loop {
        if should_quit {
            break;
        }

        // Quit dialog takes Esc / Y / Q / N before other Esc handlers.
        if quit_dialog.open {
            match quit_dialog.handle_input() {
                Some(QuitChoice::Cancel) => {
                    quit_dialog.close();
                }
                Some(QuitChoice::QuitWithoutSave) => {
                    should_quit = true;
                    continue;
                }
                Some(QuitChoice::SaveAndQuit) => {
                    match scene.to_snapshot().save_to_disk(&terrain.save_name) {
                        Ok(p) => {
                            eprintln!("[wk-voxel-app] saved {} — quitting", p.display());
                            should_quit = true;
                            continue;
                        }
                        Err(e) => {
                            quit_dialog.status = format!("Save failed: {e}  (fix again or Quit)");
                            eprintln!("[wk-voxel-app] quit-save failed: {e}");
                        }
                    }
                }
                None => {}
            }
        } else if is_key_pressed(KeyCode::Escape) {
            // Esc: spawn cancel → close editors → close settings → quit confirm.
            if editor.open && editor.spawn_picker {
                editor.spawn_picker = false;
                editor.status = "Spawn cancelled".into();
            } else if editor.open {
                editor.open = false;
                editor.spawn_picker = false;
                paused = editor.was_paused;
            } else if terrain.open {
                terrain.open = false;
                paused = terrain.was_paused;
                // Mid-air F3 paint can lose its dirty wake; re-dirty
                // unsupported sand/Organic so the next tick seats them.
                wake_unsupported_grains(&mut scene.world);
                wake_unstable_slopes(&mut scene.world);
            } else if glossary.open {
                glossary.close();
            } else if creature_list.open {
                creature_list.open = false;
            } else if settings.open {
                settings.open = false;
            } else {
                quit_dialog.open_with_slot(&terrain.save_name);
            }
        }
        if !quit_dialog.open && is_key_pressed(KeyCode::F1) {
            show_hud = !show_hud;
        }
        if !quit_dialog.open && is_key_pressed(KeyCode::Tab) && !editor.open && !terrain.open {
            settings.open = !settings.open;
        }
        if !quit_dialog.open && is_key_pressed(KeyCode::F4) {
            creature_list.toggle();
        }
        if !quit_dialog.open && is_key_pressed(KeyCode::F6) {
            glossary.toggle();
        }
        // Editor is F2 only — `C` is condensation in the voxel demo
        // (column-GVSE can use C/F2 because it has no condensation toggle).
        if !quit_dialog.open && is_key_pressed(KeyCode::F2) {
            let opening = !editor.open;
            editor.toggle(paused);
            if opening {
                settings.open = false;
                terrain.open = false;
                // F4 list panel overlaps the paint canvas — close it.
                creature_list.open = false;
                glossary.close();
                paused = true;
            } else {
                paused = editor.was_paused;
            }
        }
        if !quit_dialog.open && is_key_pressed(KeyCode::F3) {
            let opening = !terrain.open;
            terrain.toggle(paused);
            if opening {
                settings.open = false;
                editor.open = false;
                editor.spawn_picker = false;
                glossary.close();
                paused = true;
            } else {
                paused = terrain.was_paused;
                wake_unsupported_grains(&mut scene.world);
                wake_unstable_slopes(&mut scene.world);
            }
        }
        if !quit_dialog.open && editor.open {
            editor.handle_input();
        }
        if !quit_dialog.open && terrain.open {
            terrain.handle_input();
        }

        // Save / load simulation (F5 / F9, or S / L while terrain open).
        let want_save =
            !quit_dialog.open && (is_key_pressed(KeyCode::F5) || terrain.request_save);
        let want_load =
            !quit_dialog.open && (is_key_pressed(KeyCode::F9) || terrain.request_load);
        terrain.request_save = false;
        terrain.request_load = false;
        if want_save {
            match scene.to_snapshot().save_to_disk(&terrain.save_name) {
                Ok(p) => {
                    let msg = format!("Saved {}", p.display());
                    terrain.status = msg.clone();
                    eprintln!("[wk-voxel-app] {msg}");
                }
                Err(e) => {
                    terrain.status = format!("Save failed: {e}");
                    eprintln!("[wk-voxel-app] save failed: {e}");
                }
            }
        }
        if want_load {
            let path = SimSnapshot::list_disk()
                .into_iter()
                .find(|p| {
                    p.file_stem()
                        .and_then(|s| s.to_str())
                        .map(|s| s == terrain.save_name)
                        .unwrap_or(false)
                })
                .or_else(|| SimSnapshot::list_disk().into_iter().next());
            match path {
                Some(path) => match SimSnapshot::load_from_disk(&path) {
                    Ok(snap) => {
                        scene = Scene::from_snapshot(snap);
                        settings.on_world_reseed(&scene.params);
                        settings.sync_caps_from_organisms(&scene.organisms);
                        inspect = None;
                        let msg = format!("Loaded {}", path.display());
                        terrain.status = msg.clone();
                        eprintln!("[wk-voxel-app] {msg}");
                    }
                    Err(e) => {
                        terrain.status = format!("Load failed: {e}");
                        eprintln!("[wk-voxel-app] load failed: {e}");
                    }
                },
                None => {
                    terrain.status = "No saves/*.gvsesim yet — press F5 or S".into();
                }
            }
        }

        if settings.request_regen {
            settings.request_regen = false;
            let params = settings.draft_world_params(&scene.params);
            scene = Scene::new(params);
            settings.on_world_reseed(&scene.params);
            inspect = None;
            terrain.status = format!(
                "Regenerated {}×{} (sea={})",
                scene.params.width_cols,
                scene.params.sky_ceiling_y,
                scene.params.sea_level_y
            );
        }

        if (!editor.open || editor.spawn_picker) && !settings.open && !terrain.open {
            if is_key_pressed(KeyCode::Space) {
                let was = paused;
                paused = !paused;
                if was && !paused {
                    // Same stranded-grain wake as F3 close.
                    wake_unsupported_grains(&mut scene.world);
                    wake_unstable_slopes(&mut scene.world);
                }
            }
            if is_key_pressed(KeyCode::R) {
                let new_seed = scene.params.seed.wrapping_add(1);
                scene = Scene::new(WorldgenParams {
                    seed: new_seed,
                    ..scene.params
                });
                settings.on_world_reseed(&scene.params);
                inspect = None;
            }
            if is_key_pressed(KeyCode::W) {
                settings.climatic_rain_on = !settings.climatic_rain_on;
            }
            if is_key_pressed(KeyCode::C) {
                settings.cond_rain_on = !settings.cond_rain_on;
            }
            if is_key_pressed(KeyCode::E) {
                settings.evap_on = !settings.evap_on;
            }
            if is_key_pressed(KeyCode::K) {
                settings.karst_on = !settings.karst_on;
            }
            if is_key_pressed(KeyCode::H) {
                humidity_overlay = !humidity_overlay;
            }
            if is_key_pressed(KeyCode::N) {
                clouds_on = !clouds_on;
            }
            if is_key_pressed(KeyCode::T) {
                temp_overlay = !temp_overlay;
            }
            if is_key_pressed(KeyCode::U) {
                sat_overlay = !sat_overlay;
            }
            if is_key_pressed(KeyCode::M) {
                mycelium_overlay = !mycelium_overlay;
            }
            if is_key_pressed(KeyCode::G) {
                geotech_mode = geotech_mode.next();
            }
            if is_key_pressed(KeyCode::I) {
                settings.phase.enabled = !settings.phase.enabled;
            }
            if is_key_pressed(KeyCode::O) {
                organisms_on = !organisms_on;
            }
        }
        // Pan works while the terrain editor is open so you can paint elsewhere.
        if (!editor.open || editor.spawn_picker) && !settings.open {
            let pan = 200.0 * get_frame_time();
            if is_key_down(KeyCode::Left) {
                cam_x -= pan;
            }
            if is_key_down(KeyCode::Right) {
                cam_x += pan;
            }
            if is_key_down(KeyCode::Up) {
                cam_y -= pan;
            }
            if is_key_down(KeyCode::Down) {
                cam_y += pan;
            }
        }
        // Ring camera: keep pan offset inside one world width so the
        // seam is just "further left / right" rather than empty space.
        let world_w_px_for_wrap = scene.params.width_cols as f32 * PX_PER_CELL;
        if scene.params.wrap_x && world_w_px_for_wrap > 0.0 {
            cam_x = cam_x.rem_euclid(world_w_px_for_wrap);
        }

        // Sync live settings into scene subsystems.
        scene.wind.climate_vx = settings.wind_vx;
        scene.wind.variance = settings.wind_variance;
        let wind_vx = scene.wind.effective_vx(scene.world.tick);
        let wind_vy = scene.wind.effective_vy(scene.world.tick);
        scene.temperature.config = settings.temp;
        scene.temperature.climate = settings.climate;
        settings.apply_pop_caps(&mut scene.organisms);
        settings.oro.seed = scene.params.seed;
        settings.oro.width_cols = scene.params.width_cols;
        settings.oro.sea_level_y = scene.params.sea_level_y;
        settings.oro.wind_sign = if wind_vx >= 0.0 { 1 } else { -1 };

        if settings.apply_genes_to_living {
            settings.apply_genes_to_living = false;
            let g = settings.plant_genes.to_genome();
            for atom in &mut scene.organisms.atoms {
                if wk_voxel::is_land_plant(atom) || wk_voxel::is_fungus(atom) {
                    let mut next = g;
                    next.buoyancy_bias = atom.genome.buoyancy_bias;
                    atom.genome = next;
                    atom.clone_fidelity = atom.genome.clone_fidelity;
                }
            }
        }

        // Physics (frozen while paint editors / Tab settings / quit are open).
        // Tab must pause too — a busy tick (raft drift, settle) otherwise
        // starves the frame loop and the settings UI feels dead.
        let sim_paused = paused
            || settings.open
            || (editor.open && !editor.spawn_picker)
            || terrain.open
            || quit_dialog.open;
        if !sim_paused {
            // Frame-shell scans touch many loaded chunks — always worth
            // rayon. CA physics stays on the Tab toggle (demo dirty plans
            // are too narrow for parallel to win).
            set_parallel_enabled(true);
            if settings.climatic_rain_on {
                apply_rain_with_temp(
                    &mut scene.world,
                    &settings.rain,
                    Some(&scene.temperature),
                    Some(&settings.phase),
                    Some(&mut scene.humidity),
                );
            }
            if settings.evap_on {
                apply_evaporation_into_humidity_climate(
                    &mut scene.world,
                    &mut scene.humidity,
                    &settings.evap,
                    Some(&scene.temperature),
                    wind_vx.abs().max(wind_vy.abs()),
                );
            }
            // Vapor drifts with the wind, then warm air rises and
            // condenses where it meets colder air / ground.
            scene.humidity.advect(wind_vx, wind_vy);
            let tick_no = scene.world.tick;
            scene.clouds.step_with_precip(
                &mut scene.world,
                &mut scene.humidity,
                &scene.wind,
                scene.params.sea_level_y,
                scene.params.sky_ceiling_y,
                tick_no,
                &settings.cloud,
                Some(&scene.temperature),
                Some(&settings.phase),
            );
            // Leftover vapor: liquid drizzle when warm, thin ice frost
            // when cold. Packed snow still comes from the W faucet.
            if settings.cond_rain_on {
                apply_condensation_rain_phased(
                    &mut scene.world,
                    &mut scene.humidity,
                    &settings.cond,
                    Some(&settings.oro),
                    Some(&scene.temperature),
                    Some(&settings.phase),
                );
            }
            if settings.karst_on {
                apply_karst_dissolution(&mut scene.world, &settings.karst);
            }
            // Period-20 stress map: refresh before failure (S3 gate), then
            // again after CA so the HUD matches post-tick geometry.
            let geotech_due = geotech_map_due(scene.world.tick);
            if geotech_due {
                scene.geotech.rebuild_smart(&scene.world);
            }
            // Living roots bind grain repose / bedload (legacy E15).
            let rooted = if organisms_on {
                Some(collect_live_root_world_cells(&scene.organisms.atoms))
            } else {
                None
            };
            set_parallel_enabled(settings.perf.parallel_physics);
            let _ = tick_with_life(
                &mut scene.world,
                &settings.perf,
                &settings.failure,
                Some(&scene.geotech),
                rooted.as_ref(),
                Some(&settings.grain),
                Some(&settings.fungi),
            );
            set_parallel_enabled(true);
            // Crude CO₂ buckets: surface Organic oxidation + atm↔lake exchange.
            step_carbon_budget(&mut scene.carbon, &mut scene.world, &settings.carbon);
            // Floating Organic drifts with the wind; root-bound mats sail plants.
            if organisms_on {
                sail_plants_on_wind_rafts_cfg(
                    &mut scene.world,
                    &mut scene.organisms.atoms,
                    wind_vx,
                    scene.wind.tile_cols,
                    &settings.grain,
                );
            } else {
                let _ = wk_voxel::drift_floating_organic_cfg(
                    &mut scene.world,
                    wind_vx,
                    scene.wind.tile_cols,
                    None,
                    None,
                    &settings.grain,
                );
            }
            // Current shoves unbound film so mats don't dam / comb the lake.
            let _ = wk_voxel::shove_floating_organic_with_current(&mut scene.world);
            // Bedload / bank transport after water has moved this tick.
            apply_flow_erosion_bound(&mut scene.world, &settings.grain, rooted.as_ref());
            if geotech_due {
                // Post-CA dirty halo → incremental column update (S5).
                scene.geotech.rebuild_smart(&scene.world);
            }
            // Atmospheric diffusion is periodic (column-GVSE
            // HumidityField cadence: every 20 ticks). Evap still
            // deposits every tick; only the spread step is throttled.
            if humidity_diffuse_due(scene.world.tick) {
                scene
                    .humidity
                    .diffuse(settings.humidity_diffusion_alpha);
            }
            if temperature_step_due(scene.world.tick) {
                let tick_no = scene.world.tick;
                scene
                    .temperature
                    .step(Some(&scene.world), &scene.humidity, tick_no);
            }
            // Cold wet-sand / snow / hillside ice spill onto lake ice
            // after the thermal step, then phase may break thin lids.
            // Same `period_ticks` cadence as [`apply_phase`].
            if settings.phase.enabled
                && settings.phase.enable_cold_avalanche
                && scene.world.tick % settings.phase.period_ticks.max(1) == 0
            {
                let rooted = if organisms_on {
                    Some(collect_live_root_world_cells(&scene.organisms.atoms))
                } else {
                    None
                };
                apply_cold_avalanche_bound(
                    &mut scene.world,
                    &scene.temperature,
                    settings.phase.freeze_point_c,
                    rooted.as_ref(),
                );
            }
            // Phase after the temp step so a Tab cold/warm snap applies
            // the same frame (column order: thermal → phase change).
            // Master enable lives on PhaseConfig (I / Tab settings).
            apply_phase(&mut scene.world, &scene.temperature, &settings.phase);
            if organisms_on {
                let tick_no = scene.world.tick;
                let outcome = scene.organisms.step_with_weather(
                    &mut scene.world,
                    tick_no,
                    &settings.climate,
                    Some(&mut scene.humidity),
                    wind_vx,
                    Some(&scene.temperature),
                    Some(&mut scene.carbon),
                    &settings.carbon,
                    Some(&scene.clouds),
                    settings.cloud.downpour_mass,
                );
                spore_fx.burst_all(&outcome.spores, wind_vx);
            }
        }

        // Spore puffs keep drifting while paused so the wind trail stays readable.
        let draw_wind_vx = scene.wind.effective_vx(scene.world.tick);
        spore_fx.update(
            get_frame_time(),
            draw_wind_vx,
            if scene.params.wrap_x {
                Some(scene.params.width_cols)
            } else {
                None
            },
        );

        // Render.
        let sw = screen_width();
        let sh = screen_height();
        let cell_px = PX_PER_CELL;
        let hud_h = hud_height(show_hud);
        // Convert screen space to world cell range, centred on the
        // world extent minus the camera offset.
        let world_w_px = scene.params.width_cols as f32 * cell_px;
        let world_h_px = (scene.params.sky_ceiling_y - scene.params.bedrock_floor_y) as f32
            * cell_px;
        // Vertical lock: bedrock bottom sits on the HUD. `cam_y_min`
        // stops the sky-blue clear colour showing under the floor;
        // `cam_y_max` stops panning past the sky ceiling.
        // origin_y = (sh + world_h_px)/2 + cam_y  is the screen-y of
        // the bottom edge of the bedrock row.
        // Nudge the top clamp 3px past the window edge so a thin
        // clear-colour strip can't peek above the rain band.
        const TOP_OVERSCAN_PX: f32 = 3.0;
        let cam_y_min = (sh - hud_h) - (sh + world_h_px) * 0.5;
        let cam_y_max = world_h_px - (sh + world_h_px) * 0.5 - TOP_OVERSCAN_PX;
        cam_y = cam_y.clamp(cam_y_min, cam_y_max.max(cam_y_min));

        let origin_x = (sw - world_w_px) * 0.5 - cam_x;
        // Screen +y is down. World +y is up. Flip when placing rows.
        let origin_y = (sh + world_h_px) * 0.5 + cam_y;

        // Creature list (F4) / glossary (F6) — before world clicks.
        if !quit_dialog.open {
            if let Some(at) = creature_list.handle_input(&scene.organisms) {
                inspect = Some(at);
                show_hud = true;
            }
            glossary.handle_input();
        }

        // World clicks: terrain paint, spawn picker, or block inspector.
        let (mx, my) = mouse_position();
        if !quit_dialog.open
            && terrain.open
            && !terrain.hits_panel(mx, my)
            && !creature_list.hits_panel(mx, my)
            && !glossary.hits_panel(mx, my)
            && !terrain.blocks_world_paint()
        {
            let paint = is_mouse_button_down(MouseButton::Left);
            let erase = is_mouse_button_down(MouseButton::Right);
            if paint || erase {
                if let Some((gx, gy)) = screen_to_world(
                    mx,
                    my,
                    origin_x,
                    origin_y,
                    cell_px,
                    scene.params.width_cols,
                    scene.params.bedrock_floor_y,
                    scene.params.sky_ceiling_y,
                    scene.params.wrap_x,
                ) {
                    let prev_tool = terrain.tool;
                    if erase {
                        terrain.tool = TerrainTool::Erase;
                    }
                    terrain.apply_at(&mut scene.world, gx, gy);
                    terrain.tool = prev_tool;
                    inspect = Some((gx, gy));
                }
            }
        } else if !quit_dialog.open
            && is_mouse_button_pressed(MouseButton::Left)
            && (!editor.open || editor.spawn_picker)
            && !terrain.open
            && !settings.open
            && !creature_list.hits_panel(mx, my)
            && !glossary.hits_panel(mx, my)
        {
            if let Some((gx, gy)) = screen_to_world(
                mx,
                my,
                origin_x,
                origin_y,
                cell_px,
                scene.params.width_cols,
                scene.params.bedrock_floor_y,
                scene.params.sky_ceiling_y,
                scene.params.wrap_x,
            ) {
                if editor.spawn_picker {
                    // Fungi: plant as mycelium infection only — no visible
                    // fruiting body until a rich cream network emerges.
                    if editor.blueprint.is_valid_fungus() {
                        let body = editor.blueprint.modules_relative_to_nucleus();
                        let genome = editor.blueprint.genome;
                        match wk_voxel::infect_mycelium_with_lineage(
                            &mut scene.world,
                            gx,
                            gy,
                            Some((genome, body)),
                        ) {
                            Some((ox, oy)) => {
                                editor.status = format!(
                                    "Inoculated mycelium at ({ox},{oy}) — stalk emerges later matching this design"
                                );
                                editor.spawn_picker = false;
                                editor.open = false;
                                paused = editor.was_paused;
                                inspect = Some((ox, oy));
                            }
                            None => {
                                editor.status =
                                    "Inoculate failed — click Organic (or litter above it)".into();
                            }
                        }
                    } else {
                    let body = editor.blueprint.modules_relative_to_nucleus();
                    // Tab plant-gene knobs apply when the body has land/fungus
                    // tissues; pure plankton keeps painted blueprint genes.
                    let has_land_tissue = body.iter().any(|(_, _, m)| {
                        matches!(
                            m,
                            wk_voxel::ModuleId::Root
                                | wk_voxel::ModuleId::Stem
                                | wk_voxel::ModuleId::Digest
                                | wk_voxel::ModuleId::Hypha
                        )
                    });
                    let g = if has_land_tissue {
                        let mut g = settings.plant_genes.to_genome();
                        let bp = &editor.blueprint.genome;
                        g.buoyancy_bias = bp.buoyancy_bias;
                        // Stemless ribbons (seaweed) keep template alloc /
                        // shade knobs — Tab plant defaults assume a trunk.
                        let stemless = !body
                            .iter()
                            .any(|(_, _, m)| *m == wk_voxel::ModuleId::Stem);
                        if stemless {
                            g.alloc_stem = bp.alloc_stem;
                            g.alloc_leaf = bp.alloc_leaf;
                            g.alloc_root = bp.alloc_root;
                            g.root_depth_bias = bp.root_depth_bias;
                            g.shade_efficiency = bp.shade_efficiency;
                        }
                        // Don't invent tissues the painted body never had.
                        wk_voxel::sync_alloc_to_body(&mut g, &body);
                        g
                    } else {
                        editor.blueprint.genome
                    };
                    match scene.organisms.spawn_blueprint_free(
                        &scene.world,
                        gx,
                        gy,
                        body,
                        40.0,
                        g,
                    ) {
                        Ok(()) => {
                            editor.status = format!(
                                "Spawned {} at ({gx},{gy})  creatures={}/{} (entities, not pixels)",
                                editor.blueprint.name,
                                scene.organisms.len(),
                                scene.organisms.atom_cap()
                            );
                            editor.spawn_picker = false;
                            editor.open = false;
                            paused = editor.was_paused;
                            inspect = Some((gx, gy));
                        }
                        Err(wk_voxel::SpawnFail::PopCap) => {
                            editor.status = format!(
                                "Pop cap full — {}/{} living creatures (each plant counts as 1)",
                                scene.organisms.len(),
                                scene.organisms.atom_cap()
                            );
                        }
                        Err(wk_voxel::SpawnFail::NoAir) => {
                            editor.status =
                                "Spawn failed — need an Air cell near the click".into();
                        }
                        Err(wk_voxel::SpawnFail::InvalidBody) => {
                            editor.status = "Spawn failed — need a Nucleus on the canvas".into();
                        }
                    }
                    }
                } else {
                    inspect = Some((gx, gy));
                }
            }
        }

        // Atmosphere: sky → far clouds → ridges → mid clouds → active clouds → terrain.
        let phase = &settings.phase;
        let temp = &scene.temperature;
        let snow_bias = estimate_snow_bias(&scene.clouds, |fx, fy| {
            let gx = scene.world.wrap_x(fx.round() as i32);
            let air_y = fy.round() as i32;
            precip_forms_snow_at_air(temp, gx, air_y, phase)
        });
        let sky_weather = sky_weather_for_scene(
            scene.world.tick,
            &settings.climate,
            &scene.clouds,
            &scene.humidity,
            &scene.temperature,
            &scene.carbon,
            scene.params.width_cols,
            scene.params.wrap_x,
            scene.params.sea_level_y,
            settings.cloud.downpour_mass,
            snow_bias,
        );
        draw_sky(
            scene.world.tick,
            sw,
            sh,
            &settings.climate,
            &sky_weather,
            &settings.atmosphere,
        );

        ridges.ensure(
            &scene.world,
            scene.params.width_cols,
            scene.params.bedrock_floor_y,
            scene.params.sky_ceiling_y,
            scene.params.sea_level_y,
            scene.world.tick,
            scene.params.seed,
        );
        let dn_fg = day_night_factor_cfg(scene.world.tick, &settings.climate);
        let sun_local = celestial_local_cfg(scene.world.tick, &settings.climate);
        let sun_day = is_daytime_cfg(scene.world.tick, &settings.climate);
        let (celestial_sx, celestial_sy) = if sun_day {
            celestial_sun_screen_pos_cfg(
                scene.world.tick,
                sw,
                sh,
                &settings.climate,
            )
        } else {
            celestial_moon_screen_pos_cfg(
                scene.world.tick,
                sw,
                sh,
                &settings.climate,
            )
        };

        // Sun/moon behind the far ridge — soft reveal as they clear the crest.
        draw_celestials(
            scene.world.tick,
            sw,
            sh,
            &settings.climate,
            &sky_weather,
            &settings.atmosphere,
            &ridges,
            cam_x,
            cam_y,
            origin_x,
            origin_y,
            cell_px,
            scene.params.bedrock_floor_y,
            scene.params.wrap_x,
            scene.params.width_cols,
        );

        // Soft clouds (N): far echoes → ridges → mid echoes → active parcels + precip.
        if clouds_on {
            draw_depth_cloud_layer(
                &scene.clouds,
                &scene.humidity,
                &scene.wind,
                scene.world.tick,
                CloudDepthLayer::Far,
                &settings.atmosphere,
                scene.params.seed,
                scene.params.sea_level_y,
                settings.cloud.downpour_mass,
                cam_x,
                cam_y,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
            );
        }

        draw_ridge_silhouettes(
            &ridges,
            dn_fg,
            &sky_weather,
            &settings.atmosphere,
            cam_x,
            cam_y,
            origin_x,
            origin_y,
            cell_px,
            scene.params.bedrock_floor_y,
            scene.params.sea_level_y,
            scene.params.wrap_x,
            scene.params.width_cols,
            sw,
            sh,
        );

        if clouds_on {
            draw_depth_cloud_layer(
                &scene.clouds,
                &scene.humidity,
                &scene.wind,
                scene.world.tick,
                CloudDepthLayer::Mid,
                &settings.atmosphere,
                scene.params.seed,
                scene.params.sea_level_y,
                settings.cloud.downpour_mass,
                cam_x,
                cam_y,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
            );
            draw_clouds(
                &scene.clouds,
                &scene.humidity,
                &scene.world,
                &scene.wind,
                scene.world.tick,
                cam_x,
                cam_y,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.sea_level_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
                settings.cloud.downpour_mass,
                &settings.atmosphere,
                scene.params.seed,
                |fx, fy| {
                    let gx = scene.world.wrap_x(fx.round() as i32);
                    let air_y = fy.round() as i32;
                    precip_forms_snow_at_air(temp, gx, air_y, phase)
                },
            );
        }

        // Draw the ring once, plus ±1 world-width copies so the seam
        // never shows a gap while panning. Y range is pre-clamped to
        // the visible frustum so we don't iterate hidden sky rows.
        let x_copies: &[i32] = if scene.params.wrap_x { &[-1, 0, 1] } else { &[0] };
        // Solve sy = origin_y - (y - bedrock_floor_y) * cell_px for the
        // visible strip. draw_rectangle uses (sx, sy - cell_px) as top-
        // left, so a cell is visible when [sy - cell_px, sy] ⊂ [0, sh].
        let y_max_vis = {
            let y = scene.params.bedrock_floor_y as f32 + (origin_y + cell_px) / cell_px;
            (y.ceil() as i32).min(scene.params.sky_ceiling_y)
        };
        let y_min_vis = {
            let y = scene.params.bedrock_floor_y as f32 + (origin_y - sh) / cell_px;
            (y.floor() as i32).max(scene.params.bedrock_floor_y)
        };

        // Heatmap blend: 0 = landscape only, 1 = heatmap only (Tab slider).
        let heatmap_on = sat_overlay
            || temp_overlay
            || mycelium_overlay
            || geotech_mode != GeotechOverlayMode::Off;
        let blend = settings.heatmap_blend.clamp(0.0, 1.0);
        let terrain_alpha = if heatmap_on {
            ((1.0 - blend) * 255.0).round() as u8
        } else {
            255
        };
        let overlay_k = if heatmap_on { blend } else { 1.0 };

        for &x_copy in x_copies {
            let x_shift = x_copy * scene.params.width_cols;
            for x in 0..scene.params.width_cols {
                let sx = origin_x + (x + x_shift) as f32 * cell_px;
                if sx + cell_px < 0.0 || sx > sw {
                    continue;
                }
                for y in y_min_vis..y_max_vis {
                    let sy = origin_y - (y - scene.params.bedrock_floor_y) as f32 * cell_px;
                    // Guard for the rounding slop on the frustum bounds.
                    if sy + cell_px < 0.0 || sy > sh {
                        continue;
                    }
                    let Some(cell) = scene.world.get_cell(x, y) else {
                        continue;
                    };
                    // Only draw standing water (pools / ocean film / land
                    // puddles). Mid-air sat stays invisible — falling rain
                    // is the cosmetic streak under raining clouds.
                    // Thin wet-air films (condensation residual / haze sat)
                    // must not paint as a bright blue-white ground outline.
                    if cell.material == wk_material::MaterialId::Air {
                        if cell.sat.is_empty() {
                            continue;
                        }
                        // Match grain haze band: ≤32 is atmospheric film, not puddle.
                        if cell.sat.0 <= wk_voxel::GRAIN_REPOSE_HAZE_MAX {
                            continue;
                        }
                        let below_sea = y <= scene.params.sea_level_y;
                        if !below_sea && !is_standing_water(&scene.world, x, y) {
                            continue;
                        }
                    }
                    let [r0, g0, b0] = cell_color(cell);
                    let [mut r, mut g, mut b] =
                        apply_weather_rgb([r0, g0, b0], dn_fg, &sky_weather);
                    // Crest key + soft bleed into subsurface / water.
                    let key = terrain_celestial_key_strength(
                        &scene.world,
                        x,
                        y,
                        sun_local,
                        sun_day,
                    );
                    if key > 0.03 {
                        let lit = apply_celestial_key_rgb([r, g, b], key, sun_local, sun_day);
                        r = lit[0];
                        g = lit[1];
                        b = lit[2];
                    }
                    draw_rectangle(
                        sx,
                        sy - cell_px,
                        cell_px,
                        cell_px,
                        Color::from_rgba(r, g, b, terrain_alpha),
                    );
                }
            }
        }

        // Day sun cast / under-canopy / cloud dim — after terrain, before front vapour.
        // Night moon cast is drawn after organisms so lee covers bodies.
        if organisms_on && sun_day {
            draw_canopy_air_dim(
                &scene.world,
                &scene.organisms,
                &scene.clouds,
                scene.world.tick,
                draw_wind_vx,
                sun_local,
                celestial_sx,
                celestial_sy,
                true,
                settings.cloud.downpour_mass,
                &settings.atmosphere,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                y_min_vis,
                y_max_vis,
                sw,
                sh,
            );
        }

        // Front soft cloud echoes (N) — ahead of land for scale; plants stay readable.
        if clouds_on {
            draw_depth_cloud_layer(
                &scene.clouds,
                &scene.humidity,
                &scene.wind,
                scene.world.tick,
                CloudDepthLayer::Front,
                &settings.atmosphere,
                scene.params.seed,
                scene.params.sea_level_y,
                settings.cloud.downpour_mass,
                cam_x,
                cam_y,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
            );
        }

        // Humidity tile diagnostic (H) — not clouds.
        if humidity_overlay {
            draw_haze_and_wind(
                &scene.humidity,
                &scene.world,
                &scene.wind,
                scene.world.tick,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.sea_level_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
            );
        }

        // Temperature heatmap overlay (blue cold → red hot).
        if temp_overlay && overlay_k > 0.01 {
            let tile_px = scene.temperature.tile_cols as f32 * cell_px;
            let (t_min, t_max) = scene
                .temperature
                .cells
                .values()
                .copied()
                .fold((f32::MAX, f32::MIN), |(lo, hi), v| (lo.min(v), hi.max(v)));
            let t_min = t_min.min(8.0);
            let t_max = t_max.max(t_min + 4.0).max(28.0);
            for (&(hx, hy), &temp_c) in &scene.temperature.cells {
                let base_gx = hx * scene.temperature.tile_cols;
                let base_gy = hy * scene.temperature.tile_cols;
                for &x_copy in x_copies {
                    let sx =
                        origin_x + (base_gx + x_copy * scene.params.width_cols) as f32 * cell_px;
                    let sy = origin_y
                        - (base_gy - scene.params.bedrock_floor_y + scene.temperature.tile_cols)
                            as f32
                            * cell_px;
                    if sx + tile_px < 0.0 || sx > sw || sy + tile_px < 0.0 || sy > sh {
                        continue;
                    }
                    draw_rectangle(
                        sx,
                        sy,
                        tile_px,
                        tile_px,
                        scale_color_alpha(temp_overlay_color(temp_c, t_min, t_max), overlay_k),
                    );
                }
            }
        }

        // Ground saturation heatmap (U): pore fill + free water.
        if sat_overlay && overlay_k > 0.01 {
            for &x_copy in x_copies {
                let x_shift = x_copy * scene.params.width_cols;
                for x in 0..scene.params.width_cols {
                    let sx = origin_x + (x + x_shift) as f32 * cell_px;
                    if sx + cell_px < 0.0 || sx > sw {
                        continue;
                    }
                    for y in y_min_vis..y_max_vis {
                        let sy =
                            origin_y - (y - scene.params.bedrock_floor_y) as f32 * cell_px;
                        if sy + cell_px < 0.0 || sy > sh {
                            continue;
                        }
                        let Some(cell) = scene.world.get_cell(x, y) else {
                            continue;
                        };
                        let wet = if cell.material == wk_material::MaterialId::Air {
                            if cell.sat.is_empty()
                                || cell.sat.0 <= wk_voxel::GRAIN_REPOSE_HAZE_MAX
                            {
                                continue;
                            }
                            cell.sat.as_f32()
                        } else {
                            let cap = scene.world.water_capacity(cell.material);
                            if cap == 0 {
                                continue;
                            }
                            pore_wetness_with(cell, &scene.world.hydro)
                        };
                        draw_rectangle(
                            sx,
                            sy - cell_px,
                            cell_px,
                            cell_px,
                            scale_color_alpha(sat_overlay_color(wet), overlay_k),
                        );
                    }
                }
            }
        }

        // Mycelium strain overlay: bright per-network colors by cream intensity.
        if mycelium_overlay && overlay_k > 0.01 {
            for &x_copy in x_copies {
                let x_shift = x_copy * scene.params.width_cols;
                for x in 0..scene.params.width_cols {
                    let sx = origin_x + (x + x_shift) as f32 * cell_px;
                    if sx + cell_px < 0.0 || sx > sw {
                        continue;
                    }
                    for y in y_min_vis..y_max_vis {
                        let sy =
                            origin_y - (y - scene.params.bedrock_floor_y) as f32 * cell_px;
                        if sy + cell_px < 0.0 || sy > sh {
                            continue;
                        }
                        let Some(cell) = scene.world.get_cell(x, y) else {
                            continue;
                        };
                        let myc = cell.mycelium();
                        if myc == 0 {
                            continue;
                        }
                        let shares = wk_voxel::mycelium_shares_at(&scene.world, x, y);
                        let rgba = if shares.is_empty() {
                            // Legacy unowned cream — stable hash color.
                            let mut h = (x as u32).wrapping_mul(0x9E37_79B9)
                                ^ (y as u32).wrapping_mul(0x85EB_CA6B);
                            h ^= h >> 16;
                            wk_voxel::mycelium_shares_overlay_rgba(&[(h | 1, myc)], myc)
                        } else {
                            // Multi-strain cells blend by share weight.
                            wk_voxel::mycelium_shares_overlay_rgba(shares, myc)
                        };
                        if rgba[3] == 0 {
                            continue;
                        }
                        draw_rectangle(
                            sx,
                            sy - cell_px,
                            cell_px,
                            cell_px,
                            scale_color_alpha(
                                Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                                overlay_k,
                            ),
                        );
                    }
                }
            }
        }

        // Geotech overlay: G cycles shear → σᵥ → wet → off.
        if geotech_mode != GeotechOverlayMode::Off && overlay_k > 0.01 {
            match geotech_mode {
                GeotechOverlayMode::Shear | GeotechOverlayMode::Wetness => {
                    let s_max = match geotech_mode {
                        GeotechOverlayMode::Shear => scene
                            .geotech
                            .faces
                            .values()
                            .map(|f| f.shear_score)
                            .fold(0.0f32, f32::max)
                            .max(2.0),
                        _ => 1.0,
                    };
                    for (&(gx, gy), stress) in &scene.geotech.faces {
                        let value = match geotech_mode {
                            GeotechOverlayMode::Shear => stress.shear_score,
                            GeotechOverlayMode::Wetness => stress.wetness.max(
                                stress.hydro_load as f32 / 32.0,
                            ),
                            _ => 0.0,
                        };
                        for &x_copy in x_copies {
                            let sx = origin_x
                                + (gx + x_copy * scene.params.width_cols) as f32 * cell_px;
                            let sy = origin_y
                                - (gy - scene.params.bedrock_floor_y + 1) as f32 * cell_px;
                            if sx + cell_px < 0.0 || sx > sw || sy + cell_px < 0.0 || sy > sh {
                                continue;
                            }
                            draw_rectangle(
                                sx,
                                sy,
                                cell_px,
                                cell_px,
                                scale_color_alpha(geotech_overlay_color(value, s_max), overlay_k),
                            );
                        }
                    }
                }
                GeotechOverlayMode::Overburden => {
                    let s_max = scene
                        .geotech
                        .overburden
                        .values()
                        .copied()
                        .fold(0.0f32, f32::max)
                        .max(4.0);
                    for (&(gx, gy), &sigma) in &scene.geotech.overburden {
                        if sigma <= 0.05 {
                            continue;
                        }
                        for &x_copy in x_copies {
                            let sx = origin_x
                                + (gx + x_copy * scene.params.width_cols) as f32 * cell_px;
                            let sy = origin_y
                                - (gy - scene.params.bedrock_floor_y + 1) as f32 * cell_px;
                            if sx + cell_px < 0.0 || sx > sw || sy + cell_px < 0.0 || sy > sh {
                                continue;
                            }
                            draw_rectangle(
                                sx,
                                sy,
                                cell_px,
                                cell_px,
                                scale_color_alpha(geotech_overlay_color(sigma, s_max), overlay_k),
                            );
                        }
                    }
                }
                GeotechOverlayMode::Off => {}
            }
        }

        // Organisms: skip the shaded draw while the F2 canvas or Tab
        // settings cover interaction (pose + Beer–Lambert was starving
        // menu input on dense meadows). Spawn-picker keeps the world
        // visible, so draw then.
        let editor_covers_world = editor.open && !editor.spawn_picker;
        // Body dimness at night; sun/moon key only on the lit rim (not whole body).
        let body_lit = ((dn_fg + 1.0) * 0.5).clamp(0.20, 1.0);
        if !editor_covers_world && !settings.open {
            let draw_cells = scene.organisms.draw_list(
                &scene.world,
                scene.world.tick,
                draw_wind_vx,
            );
            let occupied: std::collections::HashSet<(i32, i32)> =
                draw_cells.iter().map(|&(x, y, _)| (x, y)).collect();
            for &(gx, gy, (r, g, b)) in &draw_cells {
                let r = (r as f32 * body_lit) as u8;
                let g = (g as f32 * body_lit) as u8;
                let b = (b as f32 * body_lit) as u8;
                // Roots / buried modules stay dark — no sun/moon key underground.
                let [r, g, b] = if is_organism_aboveground(&scene.world, gx, gy) {
                    let toward = toward_light_celestial(sun_local);
                    let rim = organism_celestial_rim(
                        &occupied,
                        gx,
                        gy,
                        toward,
                        sun_local,
                        sun_day,
                    );
                    if rim > 0.0 {
                        apply_organism_celestial_key_rgb([r, g, b], rim, sun_local, sun_day)
                    } else {
                        [r, g, b]
                    }
                } else {
                    [r, g, b]
                };
                for &x_copy in x_copies {
                    let sx = origin_x + (gx + x_copy * scene.params.width_cols) as f32 * cell_px;
                    let sy = origin_y - (gy - scene.params.bedrock_floor_y) as f32 * cell_px;
                    if sx + cell_px < 0.0 || sx > sw || sy < 0.0 || sy - cell_px > sh {
                        continue;
                    }
                    draw_rectangle(
                        sx,
                        sy - cell_px,
                        cell_px,
                        cell_px,
                        Color::from_rgba(r, g, b, 255),
                    );
                }
            }
        }

        // Night moon cast — after organisms so plants/creatures in lee go dark.
        if organisms_on && !sun_day {
            draw_canopy_air_dim(
                &scene.world,
                &scene.organisms,
                &scene.clouds,
                scene.world.tick,
                draw_wind_vx,
                sun_local,
                celestial_sx,
                celestial_sy,
                false,
                settings.cloud.downpour_mass,
                &settings.atmosphere,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                y_min_vis,
                y_max_vis,
                sw,
                sh,
            );
        }

        spore_fx.draw(
            origin_x,
            origin_y,
            cell_px,
            scene.params.bedrock_floor_y,
            scene.params.width_cols,
            scene.params.wrap_x,
            sw,
            sh,
        );

        if let Some((gx, gy)) = inspect {
            if show_hud {
                draw_selection_outline(
                    gx,
                    gy,
                    origin_x,
                    origin_y,
                    cell_px,
                    scene.params.bedrock_floor_y,
                    scene.params.width_cols,
                    scene.params.wrap_x,
                    sw,
                    sh,
                );
                let cell = scene.world.get_cell(gx, gy);
                let org = scene
                    .organisms
                    .pick_at(gx, gy)
                    .map(|id| (id, &scene.organisms.atoms[id]));
                let corpse = scene
                    .organisms
                    .pick_corpse_at(gx, gy)
                    .map(|id| (id, &scene.organisms.corpses[id]));
                draw_block_inspector(
                    gx,
                    gy,
                    cell,
                    &scene.humidity,
                    &scene.temperature,
                    &scene.geotech,
                    &scene.world,
                    &scene.organisms.atoms,
                    org,
                    corpse,
                    sw,
                );
            }
        }

        // Creature / terrain editor overlays (paint UI, or spawn banner).
        editor.draw();
        terrain.draw();
        creature_list.draw(&scene.organisms);
        settings.draw(&mut scene.world, &scene.carbon);
        glossary.draw();
        quit_dialog.draw();

        // HUD chrome (info + hotkeys + inspector) toggled with F1.
        if show_hud && !quit_dialog.open {
            let tod = if is_daytime_cfg(scene.world.tick, &settings.climate) {
                "day"
            } else {
                "night"
            };
            let rain_tag = if !settings.climatic_rain_on {
                "off"
            } else if settings.rain.closed_loop {
                "on/closed"
            } else {
                "on/MINT"
            };
            let info = format!(
                "fps={:.0}  tick={} {} T̄={:.1}C rain={} drizzle={} evap={} phase={} nimbus={} echo={:.0} hum={:.0} C={:.0}/{:.0} spores={} wind={:.2} creatures={}/{} ({}) dead={} {}",
                fps_smoothed(),
                scene.world.tick,
                tod,
                scene.temperature.mean(),
                rain_tag,
                if settings.cond_rain_on { "on" } else { "off" },
                if settings.evap_on { "on" } else { "off" },
                if settings.phase.enabled { "on" } else { "off" },
                scene.clouds.len(),
                scene.clouds.visual_mass(),
                scene.humidity.total_mass(),
                scene.carbon.atmosphere,
                scene.carbon.dissolved,
                scene.world.spore_bank.len(),
                draw_wind_vx,
                scene.organisms.len(),
                scene.organisms.atom_cap(),
                {
                    let (p, f, a) = scene.organisms.habit_counts();
                    format!("p={p} f={f} a={a}")
                },
                scene.organisms.corpse_count(),
                if sim_paused { "[paused]" } else { "" }
            );
            draw_rectangle(0.0, sh - hud_h, sw, hud_h, Color::from_rgba(0, 0, 0, 200));
            draw_text(
                "Tab|Space|R|W/C/E/K/O|I|N/T/U/H/M/G|F1 HUD|F2 creat|F3 terra|F4 list|F5/F9 save|F6 gloss|Esc quit",
                8.0,
                sh - INFO_H - 4.0,
                14.0,
                LIGHTGRAY,
            );
            draw_text(&info, 8.0, sh - 8.0, 16.0, WHITE);
        }

        next_frame().await;
    }
}
