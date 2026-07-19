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
//! - `H` — toggle cyan humidity debug overlay
//! - `N` — toggle cloud drawing (dark = wetter; wind-advected)
//! - `F1` — toggle the bottom tool / hotkey line
//! - `F2` — creature editor (Set A MS-Paint; `C` stays condensation here)
//! - click — block / organism inspector
//! - `Left` / `Right` — pan the camera horizontally (wraps on ring worlds)
//! - `Up` / `Down` — pan vertically
//! - `Esc` — quit (or cancel spawn / close editor)

mod editor;
mod inspector;
mod palette;
mod scene;

use macroquad::prelude::*;
use wk_voxel::{
    apply_condensation_rain_with_orographic, apply_evaporation_into_humidity,
    apply_karst_dissolution, apply_rain, humidity_diffuse_due, tick, CondensationConfig, EvapConfig,
    KarstConfig, OrographicConfig, RainConfig, WorldgenParams,
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

/// Cyan overlay alpha for a humidity tile, given the map's current
/// peak mass. Always ≥ 48 when `mass > 0` so thin diffused haze stays
/// visible (an absolute 4×4×255 scale used to paint `alpha == 0`).
fn humidity_overlay_alpha(mass: f32, max_mass: f32) -> u8 {
    if mass <= 0.0 {
        return 0;
    }
    let norm = (mass / max_mass.max(1.0)).clamp(0.0, 1.0);
    (48.0 + norm * 152.0) as u8
}

/// Dark cloud puffs — darker / denser when the tile holds more water.
/// Sub-tile `scroll` (from advection residual) keeps motion smooth.
fn draw_clouds(
    humidity: &wk_voxel::Humidity,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    width_cols: i32,
    wrap_x: bool,
    sw: f32,
    sh: f32,
) {
    if humidity.cells.is_empty() {
        return;
    }
    let tile_cols = humidity.tile_cols;
    let tile_px = tile_cols as f32 * cell_px;
    let max_mass = humidity
        .cells
        .values()
        .copied()
        .fold(0.0f32, f32::max)
        .max(1.0);
    let scroll = humidity.advect_rx * tile_px;
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };

    for (&(hx, hy), &mass) in &humidity.cells {
        if mass <= 0.0 {
            continue;
        }
        let norm = (mass / max_mass).clamp(0.0, 1.0);
        // Darker grey as water content rises.
        let shade = (210.0 - norm * 160.0) as u8;
        let alpha = (55.0 + norm * 170.0) as u8;
        let base_gx = hx * tile_cols;
        let base_gy = hy * tile_cols;
        let r = tile_px * (0.45 + 0.35 * norm);
        for &x_copy in x_copies {
            let sx = origin_x
                + (base_gx + x_copy * width_cols) as f32 * cell_px
                + scroll
                + tile_px * 0.5;
            let sy = origin_y
                - (base_gy - bedrock_floor_y + tile_cols) as f32 * cell_px
                + tile_px * 0.5;
            if sx + r < 0.0 || sx - r > sw || sy + r < 0.0 || sy - r > sh {
                continue;
            }
            let c = Color::from_rgba(shade, shade, shade.saturating_add(8), alpha);
            draw_circle(sx, sy, r, c);
            draw_circle(sx - r * 0.45, sy + r * 0.1, r * 0.7, c);
            draw_circle(sx + r * 0.4, sy + r * 0.05, r * 0.65, c);
        }
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::humidity_overlay_alpha;

    #[test]
    fn faint_tiles_still_get_visible_alpha() {
        // Old scale: (20/4080)*180 → 0. New scale floors at 48.
        assert!(humidity_overlay_alpha(20.0, 100.0) >= 48);
        assert_eq!(humidity_overlay_alpha(100.0, 100.0), 200);
        assert_eq!(humidity_overlay_alpha(0.0, 100.0), 0);
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let params = WorldgenParams::default();
    let mut scene = Scene::new(params);
    let mut paused = false;
    let mut rain_on = true;
    let mut cond_rain_on = true;
    let mut evap_on = true;
    let mut karst_on = true;
    let mut organisms_on = true;
    let mut humidity_overlay = false;
    let mut clouds_on = true;
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
            // Wind advects cloud / humidity mass before rain so
            // orographic dumps see the latest plume position.
            scene
                .humidity
                .advect(scene.wind.climate_vx, scene.wind.climate_vy);
            if cond_rain_on {
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
                    &cond_cfg,
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
            if organisms_on {
                let tick_no = scene.world.tick;
                scene.organisms.step(&mut scene.world, tick_no);
            }
        }

        // Render.
        clear_background(Color::from_rgba(0x87, 0xCE, 0xEB, 255));

        let sw = screen_width();
        let sh = screen_height();
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
                    let [r, g, b] = cell_color(cell);
                    // Skip sky-blue empty air — background already
                    // paints that colour, so this cuts draw calls hard.
                    if cell.material == wk_material::MaterialId::Air && cell.sat.is_empty() {
                        continue;
                    }
                    draw_rectangle(sx, sy - cell_px, cell_px, cell_px, Color::from_rgba(r, g, b, 255));
                }
            }
        }

        // Clouds: humidity mass drawn as dark puffs (darker = wetter),
        // scrolled by wind advection residual.
        if clouds_on {
            draw_clouds(
                &scene.humidity,
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

        // Optional cyan humidity debug overlay (tile rects).
        if humidity_overlay {
            let tile_px = scene.humidity.tile_cols as f32 * cell_px;
            let max_mass = scene
                .humidity
                .cells
                .values()
                .copied()
                .fold(0.0f32, f32::max)
                .max(1.0);
            for (&(hx, hy), &mass) in &scene.humidity.cells {
                if mass <= 0.0 {
                    continue;
                }
                let base_gx = hx * scene.humidity.tile_cols;
                let base_gy = hy * scene.humidity.tile_cols;
                for &x_copy in x_copies {
                    let sx = origin_x + (base_gx + x_copy * scene.params.width_cols) as f32 * cell_px
                        + scene.humidity.advect_rx * tile_px;
                    let sy = origin_y
                        - (base_gy - scene.params.bedrock_floor_y + scene.humidity.tile_cols)
                            as f32
                            * cell_px;
                    if sx + tile_px < 0.0 || sx > sw || sy + tile_px < 0.0 || sy > sh {
                        continue;
                    }
                    let alpha = humidity_overlay_alpha(mass, max_mass);
                    draw_rectangle(sx, sy, tile_px, tile_px, Color::from_rgba(160, 200, 240, alpha));
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
            draw_block_inspector(gx, gy, cell, &scene.humidity, org, sw);
        }

        // Creature editor overlay (paint UI, or spawn banner).
        editor.draw();

        // HUD: info line always; tool / hotkey line toggled with F1.
        let info = format!(
            "fps={:.0}  tick={} seed={} rain={} cond={} evap={} karst={} org={} atoms={} clouds={} hum={} wind={:.2} humidity={:.0} {}",
            fps_smoothed(),
            scene.world.tick,
            scene.params.seed,
            if rain_on { "on" } else { "off" },
            if cond_rain_on { "on" } else { "off" },
            if evap_on { "on" } else { "off" },
            if karst_on { "on" } else { "off" },
            if organisms_on { "on" } else { "off" },
            scene.organisms.len(),
            if clouds_on { "on" } else { "off" },
            if humidity_overlay { "on" } else { "off" },
            scene.wind.climate_vx,
            scene.humidity.total_mass(),
            if sim_paused { "[paused]" } else { "" }
        );
        draw_rectangle(0.0, sh - hud_h, sw, hud_h, Color::from_rgba(0, 0, 0, 200));
        if show_tool_line {
            draw_text(
                "Space|R|W/C/E/K/O|N clouds|H hum|F1 tools|F2 editor|click inspect|Esc",
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
