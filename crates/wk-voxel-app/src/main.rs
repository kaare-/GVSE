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
//! - `H` — toggle humidity overlay
//! - `Left` / `Right` — pan the camera horizontally (wraps on ring worlds)
//! - `Up` / `Down` — pan vertically
//! - `Esc` — quit

mod palette;
mod scene;

use macroquad::prelude::*;
use wk_voxel::{
    apply_condensation_rain, apply_evaporation_into_humidity, apply_karst_dissolution, apply_rain,
    humidity_diffuse_due, tick, CondensationConfig, EvapConfig, KarstConfig, RainConfig,
    WorldgenParams,
};

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
/// HUD strip height. Vertical camera clamp pins the bedrock floor to
/// the top of this bar so the sky-blue clear colour never shows
/// under the world.
const HUD_H: f32 = 24.0;

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
    let mut humidity_overlay = false;
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
        // Input.
        if is_key_pressed(KeyCode::Escape) {
            break;
        }
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
        }
        if is_key_pressed(KeyCode::W) {
            rain_on = !rain_on;
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
        if is_key_pressed(KeyCode::C) {
            cond_rain_on = !cond_rain_on;
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
        // Ring camera: keep pan offset inside one world width so the
        // seam is just "further left / right" rather than empty space.
        let world_w_px_for_wrap = scene.params.width_cols as f32 * PX_PER_CELL;
        if scene.params.wrap_x && world_w_px_for_wrap > 0.0 {
            cam_x = cam_x.rem_euclid(world_w_px_for_wrap);
        }

        // Physics.
        if !paused {
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
            if cond_rain_on {
                // Condensation rain runs AFTER evap so the humidity
                // it draws from is the tick's latest snapshot.
                apply_condensation_rain(&mut scene.world, &mut scene.humidity, &cond_cfg);
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
        }

        // Render.
        clear_background(Color::from_rgba(0x87, 0xCE, 0xEB, 255));

        let sw = screen_width();
        let sh = screen_height();
        let cell_px = PX_PER_CELL;
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
        let cam_y_min = (sh - HUD_H) - (sh + world_h_px) * 0.5;
        let cam_y_max = world_h_px - (sh + world_h_px) * 0.5 - TOP_OVERSCAN_PX;
        cam_y = cam_y.clamp(cam_y_min, cam_y_max.max(cam_y_min));

        let origin_x = (sw - world_w_px) * 0.5 - cam_x;
        // Screen +y is down. World +y is up. Flip when placing rows.
        let origin_y = (sh + world_h_px) * 0.5 + cam_y;

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

        // Humidity overlay: paint each tile as a translucent cyan
        // rect scaled to atmospheric mass. Rendered *after* the cells
        // so it sits on top; ignored when the toggle is off.
        //
        // Alpha is relative to the current max tile mass (not a fixed
        // 4×4×255 ceiling). Diffusion spreads mass thin; the old
        // absolute scale made almost every tile `alpha == 0`.
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
                // Convert tile coord to world cell coord (lower-left
                // of the tile) then to screen coord.
                let base_gx = hx * scene.humidity.tile_cols;
                let base_gy = hy * scene.humidity.tile_cols;
                for &x_copy in x_copies {
                    let sx = origin_x + (base_gx + x_copy * scene.params.width_cols) as f32 * cell_px;
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

        // HUD.
        let hud = format!(
            "tick={} seed={} rain={} cond={} evap={} karst={} hum={} humidity_mass={:.0} {}  |  Space pause | R reroll | W rain | C cond | E evap | K karst | H overlay | arrows pan | Esc quit",
            scene.world.tick,
            scene.params.seed,
            if rain_on { "on" } else { "off" },
            if cond_rain_on { "on" } else { "off" },
            if evap_on { "on" } else { "off" },
            if karst_on { "on" } else { "off" },
            if humidity_overlay { "on" } else { "off" },
            scene.humidity.total_mass(),
            if paused { "[paused]" } else { "" }
        );
        draw_rectangle(0.0, sh - HUD_H, sw, HUD_H, Color::from_rgba(0, 0, 0, 200));
        draw_text(&hud, 8.0, sh - 8.0, 16.0, WHITE);

        next_frame().await;
    }
}
