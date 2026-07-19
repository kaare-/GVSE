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
//! - `F1` — toggle the bottom tool / hotkey line
//! - `F2` — creature editor (Set A MS-Paint; `C` stays condensation here)
//! - click — block / organism inspector
//! - `Left` / `Right` — pan the camera horizontally (wraps on ring worlds)
//! - `Up` / `Down` — pan vertically
//! - `Esc` — quit (or cancel spawn / close editor)
//!
//! Sky follows the shared climate clock (sun by day, moon by night).
//! Temperature tiles warm with sun, cool at night, and shade under clouds.

mod editor;
mod inspector;
mod palette;
mod scene;

use macroquad::prelude::*;
use wk_voxel::{
    apply_condensation_rain_with_orographic, apply_evaporation_into_humidity,
    apply_karst_dissolution, apply_rain, celestial_screen_pos, day_night_factor, humidity_diffuse_due,
    is_daytime, sky_rgb, sky_rgb_at_height, temperature_step_due, tick, CondensationConfig,
    EvapConfig, KarstConfig, OrographicConfig, RainConfig, WorldgenParams,
};

use crate::editor::CreatureEditor;
use crate::inspector::{draw_block_inspector, draw_selection_outline, screen_to_world};
use crate::palette::cell_color;
use crate::scene::Scene;

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
/// band when `F1` shows hotkeys.
const INFO_H: f32 = 24.0;
const TOOL_H: f32 = 20.0;

fn hud_height(show_tool_line: bool) -> f32 {
    if show_tool_line {
        INFO_H + TOOL_H
    } else {
        INFO_H
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
fn draw_sky(tick: u64, sw: f32, sh: f32) {
    let dn = day_night_factor(tick);
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
    let (cx, cy) = celestial_screen_pos(tick, sw, sh);
    if is_daytime(tick) {
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
/// Darker / denser = wetter; raining parcels get a streak veil.
fn draw_clouds(
    clouds: &wk_voxel::CloudStore,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    width_cols: i32,
    wrap_x: bool,
    sw: f32,
    sh: f32,
) {
    if clouds.is_empty() {
        return;
    }
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    for p in &clouds.parcels {
        let wet = p.wetness();
        let shade = (235.0 - wet * 140.0) as u8;
        let alpha = (120.0 + wet * 100.0) as u8;
        let r = p.radius() * cell_px;
        for &x_copy in x_copies {
            let sx = origin_x + (p.fx + (x_copy * width_cols) as f32) * cell_px;
            let sy = origin_y - (p.fy - bedrock_floor_y as f32) * cell_px;
            if sx + r * 2.0 < 0.0 || sx - r * 2.0 > sw || sy + r < 0.0 || sy - r > sh {
                continue;
            }
            draw_cartoon_cloud(sx, sy, r, shade, alpha);
            if p.raining {
                // A few soft streaks — not a full sheet (old rain wash).
                let streak = Color::from_rgba(190, 210, 230, 55);
                for i in 0..5 {
                    let ox = sx - r * 0.5 + (i as f32) * (r * 0.25);
                    draw_rectangle(ox, sy + r * 0.2, 2.0, r * 1.4, streak);
                }
            }
        }
    }
}

/// Classic multi-bump cartoon cloud (body + side puffs + top lobes).
fn draw_cartoon_cloud(cx: f32, cy: f32, r: f32, shade: u8, alpha: u8) {
    let body = Color::from_rgba(shade, shade, shade.saturating_add(6), alpha);
    let hilite = Color::from_rgba(
        shade.saturating_add(25),
        shade.saturating_add(25),
        shade.saturating_add(30),
        (alpha as f32 * 0.55) as u8,
    );
    // Flat-ish underside body.
    draw_circle(cx, cy, r * 0.95, body);
    draw_circle(cx - r * 0.75, cy + r * 0.1, r * 0.72, body);
    draw_circle(cx + r * 0.80, cy + r * 0.08, r * 0.70, body);
    // Upper bumps.
    draw_circle(cx - r * 0.35, cy - r * 0.45, r * 0.62, body);
    draw_circle(cx + r * 0.30, cy - r * 0.55, r * 0.68, body);
    draw_circle(cx + r * 0.85, cy - r * 0.25, r * 0.55, body);
    draw_circle(cx - r * 0.90, cy - r * 0.15, r * 0.50, body);
    // Soft highlight on the sun-facing top.
    draw_circle(cx + r * 0.15, cy - r * 0.50, r * 0.35, hilite);
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
    let mut show_tool_line = true;
    let mut editor = CreatureEditor::default();
    let mut inspect: Option<(i32, i32)> = None;
    // Fraction of pairwise humidity difference transferred per tick.
    // 0.15 gives a visible "clouds spread as they drift" feel
    // without going near the 0.25 stability cap.
    let humidity_diffusion_alpha: f32 = 0.15;
    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;

    // Cloud row at the topmost stamped cell so no empty air (sky-blue
    // clear colour) peeks above the rain band.
    let cloud_cfg = |params: &WorldgenParams| {
        let rain = RainConfig {
            top_y: params.sky_ceiling_y - 1,
            x_range: (0, params.width_cols - 1),
            prob_per_col_per_tick: 0.02,
            droplet_sat: 64,
            seed_salt: 0xC10D_5EED,
        };
        let cond = CondensationConfig {
            top_y: params.sky_ceiling_y - 2,
            ..CondensationConfig::default()
        };
        (rain, cond)
    };
    let (mut rain_cfg, mut cond_cfg) = cloud_cfg(&scene.params);
    let evap_cfg = EvapConfig::default();
    let karst_cfg = KarstConfig::default();

    loop {
        // Esc: spawn cancel → close editor → quit.
        if is_key_pressed(KeyCode::Escape) {
            if editor.open && editor.spawn_picker {
                editor.spawn_picker = false;
                editor.status = "Spawn cancelled".into();
            } else if editor.open {
                editor.open = false;
                editor.spawn_picker = false;
                paused = editor.was_paused;
            } else {
                break;
            }
        }
        if is_key_pressed(KeyCode::F1) {
            show_tool_line = !show_tool_line;
        }
        // Editor is F2 only — `C` is condensation in the voxel demo
        // (column-GVSE can use C/F2 because it has no condensation toggle).
        if is_key_pressed(KeyCode::F2) {
            let opening = !editor.open;
            editor.toggle(paused);
            if opening {
                paused = true;
            } else {
                paused = editor.was_paused;
            }
        }
        if editor.open {
            editor.handle_input();
        }

        if !editor.open || editor.spawn_picker {
            if is_key_pressed(KeyCode::Space) {
                paused = !paused;
            }
            if is_key_pressed(KeyCode::R) {
                let new_seed = scene.params.seed.wrapping_add(1);
                scene = Scene::new(WorldgenParams {
                    seed: new_seed,
                    ..scene.params
                });
                (rain_cfg, cond_cfg) = cloud_cfg(&scene.params);
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

        // Physics (frozen while the paint editor is open, not spawn).
        let sim_paused = paused || (editor.open && !editor.spawn_picker);
        if !sim_paused {
            if rain_on {
                apply_rain(&mut scene.world, &rain_cfg);
            }
            if evap_on {
                apply_evaporation_into_humidity(
                    &mut scene.world,
                    &mut scene.humidity,
                    &evap_cfg,
                );
            }
            // Vapor drifts with the wind, then coagulates into cloud
            // parcels that rain hard when heavy enough.
            scene
                .humidity
                .advect(scene.wind.climate_vx, scene.wind.climate_vy);
            let tick_no = scene.world.tick;
            scene.clouds.step(
                &mut scene.world,
                &mut scene.humidity,
                &scene.wind,
                scene.params.sea_level_y,
                scene.params.sky_ceiling_y,
                tick_no,
            );
            // Light drizzle from leftover vapor (clouds do the downpours).
            if cond_rain_on {
                let drizzle = CondensationConfig {
                    min_mass_to_rain: 220.0,
                    max_prob_per_tick: 0.12,
                    mass_per_droplet: 48.0,
                    ..cond_cfg
                };
                let oro = OrographicConfig {
                    seed: scene.params.seed,
                    width_cols: scene.params.width_cols,
                    sea_level_y: scene.params.sea_level_y,
                    wind_sign: if scene.wind.climate_vx >= 0.0 { 1 } else { -1 },
                    ..OrographicConfig::default()
                };
                apply_condensation_rain_with_orographic(
                    &mut scene.world,
                    &mut scene.humidity,
                    &drizzle,
                    Some(&oro),
                );
            }
            if karst_on {
                apply_karst_dissolution(&mut scene.world, &karst_cfg);
            }
            tick(&mut scene.world);
            // Atmospheric diffusion is periodic (column-GVSE
            // HumidityField cadence: every 20 ticks). Evap still
            // deposits every tick; only the spread step is throttled.
            if humidity_diffuse_due(scene.world.tick) {
                scene.humidity.diffuse(humidity_diffusion_alpha);
            }
            if temperature_step_due(scene.world.tick) {
                let tick_no = scene.world.tick;
                scene.temperature.step(&scene.humidity, tick_no);
            }
            if organisms_on {
                let tick_no = scene.world.tick;
                scene.organisms.step(&mut scene.world, tick_no);
            }
        }

        // Render.
        let sw = screen_width();
        let sh = screen_height();
        draw_sky(scene.world.tick, sw, sh);
        let cell_px = PX_PER_CELL;
        let hud_h = hud_height(show_tool_line);
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
                    let g = editor.blueprint.genome;
                    if scene.organisms.spawn_blueprint(
                        &scene.world,
                        gx,
                        gy,
                        body,
                        40.0,
                        g.buoyancy_bias,
                        g.clone_fidelity,
                    ) {
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
                        editor.status =
                            "Spawn failed — need a wet Air cell nearby (or pop cap)".into();
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
                    // Let the day/night sky show through: skip empty Air
                    // and thin sky drizzle (old rain-streak look). Pooled
                    // water (near-full sat) and everything below sea still
                    // draws normally.
                    if cell.material == wk_material::MaterialId::Air {
                        let sky_drizzle = y > scene.params.sea_level_y && cell.sat.0 < 240;
                        if cell.sat.is_empty() || sky_drizzle {
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
            draw_clouds(
                &scene.clouds,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.width_cols,
                scene.params.wrap_x,
                sw,
                sh,
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
            draw_block_inspector(gx, gy, cell, &scene.humidity, &scene.temperature, org, sw);
        }

        // Creature editor overlay (paint UI, or spawn banner).
        editor.draw();

        // HUD: info line always; tool / hotkey line toggled with F1.
        let tod = if is_daytime(scene.world.tick) {
            "day"
        } else {
            "night"
        };
        let info = format!(
            "fps={:.0}  tick={} {} T̄={:.1}C rain={} evap={} nimbus={} cloud_m={:.0} hum={:.0} wind={:.2} atoms={} {}",
            fps_smoothed(),
            scene.world.tick,
            tod,
            scene.temperature.mean(),
            if rain_on { "on" } else { "off" },
            if evap_on { "on" } else { "off" },
            scene.clouds.len(),
            scene.clouds.total_mass(),
            scene.humidity.total_mass(),
            scene.wind.climate_vx,
            scene.organisms.len(),
            if sim_paused { "[paused]" } else { "" }
        );
        draw_rectangle(0.0, sh - hud_h, sw, hud_h, Color::from_rgba(0, 0, 0, 200));
        if show_tool_line {
            draw_text(
                "Space|R|W rain|C drizzle|E/K/O|N clouds|T temp|H haze|F1|F2 editor|Esc",
                8.0,
                sh - INFO_H - 4.0,
                14.0,
                LIGHTGRAY,
            );
        }
        draw_text(&info, 8.0, sh - 8.0, 16.0, WHITE);

        next_frame().await;
    }
}
