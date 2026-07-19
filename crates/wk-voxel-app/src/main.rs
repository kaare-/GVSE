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
//! - `W` — toggle rain
//! - `E` — toggle evaporation
//! - `K` — toggle karst dissolution
//! - `Left` / `Right` — pan the camera horizontally
//! - `Up` / `Down` — pan vertically
//! - `Esc` — quit

mod palette;
mod scene;

use macroquad::prelude::*;
use wk_voxel::{
    apply_evaporation, apply_karst_dissolution, apply_rain, tick, EvapConfig, KarstConfig,
    RainConfig, WorldgenParams,
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

#[macroquad::main(window_conf)]
async fn main() {
    let params = WorldgenParams::default();
    let mut scene = Scene::new(params);
    let mut paused = false;
    let mut rain_on = true;
    let mut evap_on = true;
    let mut karst_on = true;
    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;

    // Rain cloud spans the full width just under the sky ceiling.
    let rain_cfg = RainConfig {
        top_y: scene.params.sky_ceiling_y - 2,
        x_range: (0, scene.params.width_cols - 1),
        prob_per_col_per_tick: 0.02,
        droplet_sat: 64,
        seed_salt: 0xC10D_5EED,
    };
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

        // Physics.
        if !paused {
            if rain_on {
                apply_rain(&mut scene.world, &rain_cfg);
            }
            if evap_on {
                apply_evaporation(&mut scene.world, &evap_cfg);
            }
            if karst_on {
                apply_karst_dissolution(&mut scene.world, &karst_cfg);
            }
            tick(&mut scene.world);
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
        let origin_x = (sw - world_w_px) * 0.5 - cam_x;
        // Screen +y is down. World +y is up. Flip when placing rows.
        let origin_y = (sh + world_h_px) * 0.5 + cam_y;

        for x in 0..scene.params.width_cols {
            let sx = origin_x + x as f32 * cell_px;
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

        // HUD.
        let hud = format!(
            "tick={} seed={} rain={} evap={} karst={} {}  |  Space pause | R reroll | W rain | E evap | K karst | arrows pan | Esc quit",
            scene.world.tick,
            scene.params.seed,
            if rain_on { "on" } else { "off" },
            if evap_on { "on" } else { "off" },
            if karst_on { "on" } else { "off" },
            if paused { "[paused]" } else { "" }
        );
        draw_rectangle(0.0, sh - 24.0, sw, 24.0, Color::from_rgba(0, 0, 0, 200));
        draw_text(&hud, 8.0, sh - 8.0, 16.0, WHITE);

        next_frame().await;
    }
}
