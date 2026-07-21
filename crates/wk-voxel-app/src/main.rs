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
//! - `H` — toggle soft white humidity haze (vapor hint; clouds carry the look)
//! - `N` — toggle cloud drawing (coagulated parcels; darker = wetter)
//! - `T` — toggle temperature heatmap overlay
//! - `I` — toggle phase change master (freeze / thaw / snow / slush; also in Tab)
//! - `F1` — toggle HUD chrome (bottom info/tools + block inspector)
//! - `F2` — creature editor (Atom / plant MS-Paint; `C` stays condensation)
//! - `Tab` — live settings (materials, wind, clouds, day/night, Performance, …)
//! - click — block / organism inspector (hidden while F1 HUD is off)
//! - `Left` / `Right` — pan the camera horizontally (wraps on ring worlds)
//! - `Up` / `Down` — pan vertically
//! - `Esc` — quit (or cancel spawn / close editor / close settings)
//!
//! Sky follows the shared climate clock (sun by day, moon by night).
//! Temperature tiles warm with sun, cool at night, and shade under clouds.

mod editor;
mod inspector;
mod palette;
mod scene;
mod settings;

use macroquad::prelude::*;
use wk_voxel::{
    apply_cold_avalanche, apply_condensation_rain_phased, apply_evaporation_into_humidity,
    apply_flow_erosion, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    celestial_screen_pos_cfg, cloud_floor_y, day_night_factor_cfg, humidity_diffuse_due,
    is_daytime_cfg, is_standing_water, precip_forms_snow_at_air, sky_rgb, sky_rgb_at_height,
    temperature_step_due, tick_with_perf, ClimateConfig, Wind, World, WorldgenParams,
};

use crate::editor::CreatureEditor;
use crate::inspector::{draw_block_inspector, draw_selection_outline, screen_to_world};
use crate::palette::cell_color;
use crate::scene::Scene;
use crate::settings::SimSettings;

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

/// Soft white vapor haze alpha — quiet on purpose so cartoon clouds
/// stay the main atmospheric read.
fn humidity_haze_alpha(mass: f32, max_mass: f32) -> u8 {
    if mass <= 0.0 {
        return 0;
    }
    let norm = (mass / max_mass.max(1.0)).clamp(0.0, 1.0);
    // Floor so thin air isn't a speckled field; cap so it never washes out.
    if norm < 0.12 {
        return 0;
    }
    (18.0 + norm * 42.0) as u8
}

/// Day/night sky gradient + sun or moon arc.
fn draw_sky(tick: u64, sw: f32, sh: f32, climate: &ClimateConfig) {
    let dn = day_night_factor_cfg(tick, climate);
    const BANDS: i32 = 28;
    for i in 0..BANDS {
        let y0 = sh * (i as f32) / BANDS as f32;
        let h = y0 + sh / BANDS as f32;
        let height_01 = (i as f32 + 0.5) / BANDS as f32;
        let [r, g, b] = sky_rgb_at_height(dn, height_01);
        draw_rectangle(
            0.0,
            y0,
            sw,
            h - y0 + 1.0,
            Color::from_rgba(r, g, b, 255),
        );
    }
    let (cx, cy) = celestial_screen_pos_cfg(tick, sw, sh, climate);
    if is_daytime_cfg(tick, climate) {
        draw_circle(cx, cy, 22.0, Color::from_rgba(255, 200, 70, 60));
        draw_circle(cx, cy, 16.0, Color::from_rgba(255, 220, 90, 255));
        draw_circle(cx, cy, 10.0, Color::from_rgba(255, 245, 180, 255));
    } else {
        let [sr, sg, sb] = sky_rgb(dn);
        draw_circle(cx, cy, 14.0, Color::from_rgba(230, 235, 245, 255));
        // Crescent bite using local sky colour.
        draw_circle(
            cx + 5.0,
            cy - 2.0,
            12.0,
            Color::from_rgba(sr, sg, sb, 255),
        );
    }
}

fn temp_overlay_color(temp_c: f32, t_min: f32, t_max: f32) -> Color {
    let u = ((temp_c - t_min) / (t_max - t_min).max(0.5)).clamp(0.0, 1.0);
    // Cold blue → mild cyan → warm yellow → hot red.
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

/// Cartoon clouds from coagulated [`wk_voxel::CloudStore`] parcels.
/// Darker / denser = wetter; raining parcels get falling drops beneath.
fn draw_clouds(
    clouds: &wk_voxel::CloudStore,
    world: &World,
    wind: &Wind,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
    downpour_mass: f32,
    snowing: impl Fn(f32, f32) -> bool,
) {
    if clouds.is_empty() {
        return;
    }
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    // Low parcels first so ridge-top clouds paint over them.
    let mut order: Vec<usize> = (0..clouds.parcels.len()).collect();
    order.sort_by(|&a, &b| {
        clouds.parcels[a]
            .fy
            .partial_cmp(&clouds.parcels[b].fy)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    for &idx in &order {
        let p = &clouds.parcels[idx];
        let wet = p.wetness_with(downpour_mass);
        // Narrow shade/alpha ranges so vapor mass changes don't pulse.
        let shade = (228.0 - wet * 95.0) as u8;
        let alpha = (145.0 + wet * 55.0) as u8;
        let r = p.radius() * cell_px;
        // Highest floor under the silhouette so streaks don't punch
        // through a slope when the parcel centre sits over a valley.
        let r_cells = p.radius();
        let floor = [-0.85_f32, -0.4, 0.0, 0.4, 0.85]
            .iter()
            .map(|t| cloud_floor_y(world, wind, p.fx + t * r_cells))
            .fold(f32::NEG_INFINITY, f32::max);
        let ground_sy = origin_y - (floor - bedrock_floor_y as f32) * cell_px;
        // Flake vs streak from air temp at the parcel, not the ground.
        let as_snow = snowing(p.fx, p.fy);
        for &x_copy in x_copies {
            let sx = origin_x + (p.fx + (x_copy * width_cols) as f32) * cell_px;
            let sy = origin_y - (p.fy - bedrock_floor_y as f32) * cell_px;
            if sx + r * 2.0 < 0.0 || sx - r * 2.0 > sw || sy + r < 0.0 || sy - r > sh {
                continue;
            }
            draw_cartoon_cloud(sx, sy, r, shade, alpha, p.shape_seed, p.deform);
            if p.raining {
                if as_snow {
                    draw_falling_snow(sx, sy, r, ground_sy, wet, sw, sh);
                } else {
                    draw_falling_rain(sx, sy, r, ground_sy, wet, sw, sh);
                }
            }
        }
    }
}

/// Cosmetic falling drops under a raining parcel (physics is separate).
fn draw_falling_rain(
    sx: f32,
    sy: f32,
    r: f32,
    ground_sy: f32,
    wetness: f32,
    sw: f32,
    sh: f32,
) {
    let t = get_time() as f32;
    let top = sy + r * 0.35;
    let bottom = ground_sy.clamp(top + 12.0, sh - 4.0);
    let left = (sx - r * 0.85).max(-12.0);
    let right = (sx + r * 0.85).min(sw + 12.0);
    let band = (right - left).max(1.0);
    let n = ((band / 7.0) * (0.7 + wetness)).ceil().clamp(10.0, 48.0) as usize;
    let drop_len = 10.0 + wetness * 6.0;
    let fall_speed = 380.0 + wetness * 160.0;
    let cycle = (bottom - top + drop_len).max(drop_len + 1.0);
    for i in 0..n {
        let seed = i as f32;
        let x = left + ((seed * 97.371) % band);
        let phase = (seed * 0.6180339) % 1.0;
        let y = top + ((t * fall_speed + phase * cycle) % cycle) - drop_len;
        if y + drop_len < top || y > bottom {
            continue;
        }
        let alpha = (100.0 + wetness * 50.0) as u8;
        draw_line(
            x,
            y,
            x - 2.5,
            y + drop_len,
            1.15,
            Color::from_rgba(195, 215, 240, alpha),
        );
    }
}

/// Soft flakes when the column is at/below freeze — pairs with snow precip.
fn draw_falling_snow(
    sx: f32,
    sy: f32,
    r: f32,
    ground_sy: f32,
    wetness: f32,
    sw: f32,
    sh: f32,
) {
    let t = get_time() as f32;
    let top = sy + r * 0.35;
    let bottom = ground_sy.clamp(top + 12.0, sh - 4.0);
    let left = (sx - r * 0.9).max(-12.0);
    let right = (sx + r * 0.9).min(sw + 12.0);
    let band = (right - left).max(1.0);
    let n = ((band / 9.0) * (0.65 + wetness * 0.85))
        .ceil()
        .clamp(8.0, 40.0) as usize;
    let flake = 2.2 + wetness * 1.4;
    let fall_speed = 95.0 + wetness * 55.0;
    let cycle = (bottom - top + flake * 4.0).max(flake * 4.0 + 1.0);
    for i in 0..n {
        let seed = i as f32;
        let drift = ((t * 18.0 + seed * 11.3).sin()) * 6.0;
        let x = left + ((seed * 97.371) % band) + drift;
        let phase = (seed * 0.6180339) % 1.0;
        let y = top + ((t * fall_speed + phase * cycle) % cycle) - flake;
        if y + flake < top || y > bottom {
            continue;
        }
        let alpha = (130.0 + wetness * 60.0) as u8;
        let c = Color::from_rgba(235, 242, 255, alpha);
        // Tiny plus / diamond flake.
        draw_line(x - flake, y, x + flake, y, 1.1, c);
        draw_line(x, y - flake, x, y + flake, 1.1, c);
        draw_line(x - flake * 0.7, y - flake * 0.7, x + flake * 0.7, y + flake * 0.7, 0.9, c);
    }
}

/// Multi-bump cartoon cloud with per-parcel silhouette + soft ridge squash.
fn draw_cartoon_cloud(
    cx: f32,
    cy: f32,
    r: f32,
    shade: u8,
    alpha: u8,
    shape_seed: u32,
    deform: f32,
) {
    let body = Color::from_rgba(shade, shade, shade.saturating_add(6), alpha);
    let hilite = Color::from_rgba(
        shade.saturating_add(25),
        shade.saturating_add(25),
        shade.saturating_add(30),
        (alpha as f32 * 0.55) as u8,
    );
    let d = deform.clamp(0.0, 1.0);
    // Soft deform: widen and flatten when scraping a ridge.
    let sx = 1.0 + d * 0.22;
    let sy = 1.0 - d * 0.28;
    let s = |n: u32| ((shape_seed.wrapping_mul(0x9E37_79B9).wrapping_add(n * 0x85EB_CA6B)) >> 8) as f32
        / 16_777_216.0;
    let jx = |n: u32| (s(n) - 0.5) * 0.28;
    let jy = |n: u32| (s(n.wrapping_add(17)) - 0.5) * 0.22;
    let jr = |n: u32| 0.88 + s(n.wrapping_add(31)) * 0.28;
    let puff = |ox: f32, oy: f32, rr: f32, n: u32| {
        draw_circle(
            cx + (ox + jx(n)) * r * sx,
            cy + (oy + jy(n)) * r * sy,
            rr * jr(n) * r * ((sx + sy) * 0.5),
            body,
        );
    };
    // Body + side puffs (layout varies with seed).
    puff(0.0, 0.02, 0.95, 1);
    puff(-0.72, 0.08, 0.70, 2);
    puff(0.78, 0.06, 0.68, 3);
    // Upper lobes — count/bias from seed so silhouettes differ.
    puff(-0.32 + jx(4) * 0.4, -0.42, 0.60, 4);
    puff(0.28 + jx(5) * 0.4, -0.52, 0.66, 5);
    if shape_seed & 1 == 0 {
        puff(0.82, -0.22, 0.52, 6);
    }
    if shape_seed & 2 == 0 {
        puff(-0.88, -0.12, 0.48, 7);
    }
    if shape_seed % 5 < 3 {
        puff(jx(8) * 0.5, -0.68, 0.42, 8);
    }
    // Soft highlight on the sun-facing top.
    draw_circle(
        cx + (0.12 + jx(9) * 0.3) * r * sx,
        cy + (-0.48 + jy(9) * 0.2) * r * sy,
        r * 0.32 * jr(9),
        hilite,
    );
}

#[cfg(test)]
mod overlay_tests {
    use super::humidity_haze_alpha;

    #[test]
    fn haze_ignores_thin_vapor_and_stays_soft() {
        assert_eq!(humidity_haze_alpha(0.0, 100.0), 0);
        assert_eq!(humidity_haze_alpha(5.0, 100.0), 0); // below 12% floor
        assert!(humidity_haze_alpha(50.0, 100.0) >= 18);
        assert!(humidity_haze_alpha(100.0, 100.0) <= 70);
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let params = WorldgenParams::default();
    let mut scene = Scene::new(params);
    let mut settings = SimSettings::new(&scene.params);
    settings.apply_material_overrides();
    let mut paused = false;
    // Climatic drizzle is physics-only by default — sky pixels hide thin
    // wet Air so the old rain-streak look doesn't paint over the sky.
    // Clouds do the visible weather.
    let mut rain_on = false;
    let mut cond_rain_on = true;
    let mut evap_on = true;
    let mut karst_on = true;
    let mut organisms_on = true;
    let mut humidity_overlay = false;
    let mut clouds_on = true;
    let mut temp_overlay = false;
    let mut show_hud = true;
    let mut editor = CreatureEditor::default();
    let mut inspect: Option<(i32, i32)> = None;
    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;

    loop {
        // Esc: spawn cancel → close editor → close settings → quit.
        if is_key_pressed(KeyCode::Escape) {
            if editor.open && editor.spawn_picker {
                editor.spawn_picker = false;
                editor.status = "Spawn cancelled".into();
            } else if editor.open {
                editor.open = false;
                editor.spawn_picker = false;
                paused = editor.was_paused;
            } else if settings.open {
                settings.open = false;
            } else {
                break;
            }
        }
        if is_key_pressed(KeyCode::F1) {
            show_hud = !show_hud;
        }
        if is_key_pressed(KeyCode::Tab) && !editor.open {
            settings.open = !settings.open;
        }
        // Editor is F2 only — `C` is condensation in the voxel demo
        // (column-GVSE can use C/F2 because it has no condensation toggle).
        if is_key_pressed(KeyCode::F2) {
            let opening = !editor.open;
            editor.toggle(paused);
            if opening {
                settings.open = false;
                paused = true;
            } else {
                paused = editor.was_paused;
            }
        }
        if editor.open {
            editor.handle_input();
        }

        if (!editor.open || editor.spawn_picker) && !settings.open {
            if is_key_pressed(KeyCode::Space) {
                paused = !paused;
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
                rain_on = !rain_on;
            }
            if is_key_pressed(KeyCode::C) {
                cond_rain_on = !cond_rain_on;
            }
            if is_key_pressed(KeyCode::E) {
                evap_on = !evap_on;
            }
            if is_key_pressed(KeyCode::K) {
                karst_on = !karst_on;
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
            if is_key_pressed(KeyCode::I) {
                settings.phase.enabled = !settings.phase.enabled;
            }
            if is_key_pressed(KeyCode::O) {
                organisms_on = !organisms_on;
            }
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
        scene.temperature.config = settings.temp;
        scene.temperature.climate = settings.climate;
        settings.oro.seed = scene.params.seed;
        settings.oro.width_cols = scene.params.width_cols;
        settings.oro.sea_level_y = scene.params.sea_level_y;
        settings.oro.wind_sign = if settings.wind_vx >= 0.0 { 1 } else { -1 };

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

        // Physics (frozen while the paint editor is open, not spawn).
        let sim_paused = paused || (editor.open && !editor.spawn_picker);
        if !sim_paused {
            if rain_on {
                apply_rain_with_temp(
                    &mut scene.world,
                    &settings.rain,
                    Some(&scene.temperature),
                    Some(&settings.phase),
                );
            }
            if evap_on {
                apply_evaporation_into_humidity(
                    &mut scene.world,
                    &mut scene.humidity,
                    &settings.evap,
                );
            }
            // Vapor drifts with the wind, then coagulates into cloud
            // parcels that rain hard when heavy enough.
            scene
                .humidity
                .advect(scene.wind.climate_vx, scene.wind.climate_vy);
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
            // when cold. Snow packs still come from clouds (flakes).
            if cond_rain_on {
                apply_condensation_rain_phased(
                    &mut scene.world,
                    &mut scene.humidity,
                    &settings.cond,
                    Some(&settings.oro),
                    Some(&scene.temperature),
                    Some(&settings.phase),
                );
            }
            if karst_on {
                apply_karst_dissolution(&mut scene.world, &settings.karst);
            }
            tick_with_perf(&mut scene.world, &settings.perf);
            // Bedload / bank transport after water has moved this tick.
            apply_flow_erosion(&mut scene.world, &settings.grain);
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
            if settings.phase.enabled && settings.phase.enable_cold_avalanche {
                apply_cold_avalanche(
                    &mut scene.world,
                    &scene.temperature,
                    settings.phase.freeze_point_c,
                );
            }
            // Phase after the temp step so a Tab cold/warm snap applies
            // the same frame (column order: thermal → phase change).
            // Master enable lives on PhaseConfig (I / Tab settings).
            apply_phase(&mut scene.world, &scene.temperature, &settings.phase);
            if organisms_on {
                let tick_no = scene.world.tick;
                scene
                    .organisms
                    .step_with_climate(
                        &mut scene.world,
                        tick_no,
                        &settings.climate,
                        Some(&mut scene.humidity),
                    );
            }
        }

        // Render.
        let sw = screen_width();
        let sh = screen_height();
        draw_sky(scene.world.tick, sw, sh, &settings.climate);
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

        // World clicks: spawn picker, or block inspector.
        if is_mouse_button_pressed(MouseButton::Left)
            && (!editor.open || editor.spawn_picker)
        {
            let (mx, my) = mouse_position();
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
                    let body = editor.blueprint.modules_relative_to_nucleus();
                    // Tab plant-gene knobs override blueprint genome on spawn
                    // for plants/fungi; Atoms keep the painted blueprint genes.
                    let g = if editor.blueprint.is_valid_plant()
                        || editor.blueprint.is_valid_fungus()
                    {
                        let mut g = settings.plant_genes.to_genome();
                        // Keep buoyancy from blueprint (plants ignore it).
                        g.buoyancy_bias = editor.blueprint.genome.buoyancy_bias;
                        // Don't invent tissues the painted body never had
                        // (e.g. Root+Nucleus+Leaf chassis → no surprise trunk).
                        wk_voxel::sync_alloc_to_body(&mut g, &body);
                        g
                    } else {
                        editor.blueprint.genome
                    };
                    if scene.organisms.spawn_blueprint(&scene.world, gx, gy, body, 40.0, g) {
                        editor.status = format!(
                            "Spawned {} at ({gx},{gy})  atoms={}",
                            editor.blueprint.name,
                            scene.organisms.len()
                        );
                        editor.spawn_picker = false;
                        editor.open = false;
                        paused = editor.was_paused;
                        inspect = Some((gx, gy));
                    } else {
                        editor.status = if editor.blueprint.is_valid_fungus() {
                            "Spawn failed — need Air above a solid (or pop cap)".into()
                        } else if editor.blueprint.is_valid_plant() {
                            "Spawn failed — need Air above porous soil (or pop cap)".into()
                        } else {
                            "Spawn failed — need a wet Air cell nearby (or pop cap)".into()
                        };
                    }
                } else {
                    inspect = Some((gx, gy));
                }
            }
        }

        // Draw the ring once, plus ±1 world-width copies so the seam
        // never shows a gap while panning.
        let x_copies: &[i32] = if scene.params.wrap_x { &[-1, 0, 1] } else { &[0] };
        for &x_copy in x_copies {
            let x_shift = x_copy * scene.params.width_cols;
            for x in 0..scene.params.width_cols {
                let sx = origin_x + (x + x_shift) as f32 * cell_px;
                if sx + cell_px < 0.0 || sx > sw {
                    continue;
                }
                for y in scene.params.bedrock_floor_y..scene.params.sky_ceiling_y {
                    let sy = origin_y - (y - scene.params.bedrock_floor_y) as f32 * cell_px;
                    if sy + cell_px < 0.0 || sy > sh {
                        continue;
                    }
                    let Some(cell) = scene.world.get_cell(x, y) else {
                        continue;
                    };
                    // Only draw standing water (pools / ocean film / land
                    // puddles). Mid-air sat stays invisible — falling rain
                    // is the cosmetic streak under raining clouds.
                    if cell.material == wk_material::MaterialId::Air {
                        // Any non-zero fill (even 1/255) must paint — the
                        // palette maps that to a faint blue-white film so
                        // trickle / leveling cells stay visible. Mid-air
                        // sat stays invisible; falling rain is the
                        // cosmetic streak under raining clouds.
                        if cell.sat.is_empty() {
                            continue;
                        }
                        let below_sea = y <= scene.params.sea_level_y;
                        if !below_sea && !is_standing_water(&scene.world, x, y) {
                            continue;
                        }
                    }
                    let [r, g, b] = cell_color(cell);
                    draw_rectangle(sx, sy - cell_px, cell_px, cell_px, Color::from_rgba(r, g, b, 255));
                }
            }
        }

        // Soft white vapor haze (optional) — clouds remain the main read.
        if humidity_overlay {
            let tile_px = scene.humidity.tile_cols as f32 * cell_px;
            let max_mass = scene
                .humidity
                .cells
                .values()
                .copied()
                .fold(0.0f32, f32::max)
                .max(1.0);
            let sky_hy_min = (scene.params.sea_level_y + 4).div_euclid(scene.humidity.tile_cols);
            for (&(hx, hy), &mass) in &scene.humidity.cells {
                if mass <= 0.0 || hy < sky_hy_min {
                    continue;
                }
                let alpha = humidity_haze_alpha(mass, max_mass);
                if alpha == 0 {
                    continue;
                }
                let base_gx = hx * scene.humidity.tile_cols;
                let base_gy = hy * scene.humidity.tile_cols;
                for &x_copy in x_copies {
                    let sx = origin_x
                        + (base_gx + x_copy * scene.params.width_cols) as f32 * cell_px;
                    let sy = origin_y
                        - (base_gy - scene.params.bedrock_floor_y + scene.humidity.tile_cols)
                            as f32
                            * cell_px;
                    if sx + tile_px < 0.0 || sx > sw || sy + tile_px < 0.0 || sy > sh {
                        continue;
                    }
                    draw_rectangle(sx, sy, tile_px, tile_px, Color::from_rgba(255, 255, 255, alpha));
                }
            }
        }

        // Coagulated cloud parcels — the atmospheric story.
        if clouds_on {
            let phase = &settings.phase;
            let temp = &scene.temperature;
            draw_clouds(
                &scene.clouds,
                &scene.world,
                &scene.wind,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
                settings.cloud.downpour_mass,
                |fx, fy| {
                    let gx = scene.world.wrap_x(fx.round() as i32);
                    let air_y = fy.round() as i32;
                    precip_forms_snow_at_air(temp, gx, air_y, phase)
                },
            );
        }

        // Temperature heatmap overlay (blue cold → red hot).
        if temp_overlay {
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
                    draw_rectangle(sx, sy, tile_px, tile_px, temp_overlay_color(temp_c, t_min, t_max));
                }
            }
        }

        // Set A organisms: 1×1 module pixels (Nucleus black, Photosystem
        // green) — same palette as column-GVSE, always drawn when present.
        for &(gx, gy, (r, g, b)) in &scene.organisms.draw_list() {
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
                    &scene.world,
                    org,
                    corpse,
                    sw,
                );
            }
        }

        // Creature editor overlay (paint UI, or spawn banner).
        editor.draw();
        settings.draw();

        // HUD chrome (info + hotkeys + inspector) toggled with F1.
        if show_hud {
            let tod = if is_daytime_cfg(scene.world.tick, &settings.climate) {
                "day"
            } else {
                "night"
            };
            let info = format!(
                "fps={:.0}  tick={} {} T̄={:.1}C rain={} evap={} phase={} nimbus={} cloud_m={:.0} hum={:.0} wind={:.2} atoms={} {}",
                fps_smoothed(),
                scene.world.tick,
                tod,
                scene.temperature.mean(),
                if rain_on { "on" } else { "off" },
                if evap_on { "on" } else { "off" },
                if settings.phase.enabled { "on" } else { "off" },
                scene.clouds.len(),
                scene.clouds.total_mass(),
                scene.humidity.total_mass(),
                scene.wind.climate_vx,
                scene.organisms.len(),
                if sim_paused { "[paused]" } else { "" }
            );
            draw_rectangle(0.0, sh - hud_h, sw, hud_h, Color::from_rgba(0, 0, 0, 200));
            draw_text(
                "Tab settings|Space|R|W rain|C drizzle|E/K/O|I phase|N clouds|T temp|H haze|F1|F2|Esc",
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
