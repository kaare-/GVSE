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
//! - `M` — toggle mycelium strain overlay (bright per-network colors)
//! - `G` — cycle geotech overlay (shear → σᵥ → wet → off)
//! - `I` — toggle phase change master (freeze / thaw / snow / slush; also in Tab)
//! - `F1` — toggle HUD chrome (bottom info/tools + block inspector)
//! - `F2` — creature editor (Atom / plant MS-Paint; `C` stays condensation)
//! - `F3` — terrain editor (paint / erase block types; world stays visible)
//! - `F4` — creature list (living / dead roster; click row to inspect)
//! - `F5` / `F9` — save / load simulation (`saves/*.gvsesim`)
//! - `Tab` — live settings (world size, materials, wind, clouds, …)
//! - click — block / organism inspector (hidden while F1 HUD is off)
//! - `Left` / `Right` — pan the camera horizontally (wraps on ring worlds)
//! - `Up` / `Down` — pan vertically
//! - `Esc` — close overlays, or quit confirm (save / discard / cancel)
//!
//! Sky follows the shared climate clock (pixel sun by day, pixel moon by night).
//! Temperature tiles warm with sun, cool at night, and shade under pixel clouds.

mod creature_list;
mod editor;
mod inspector;
mod palette;
mod quit;
mod scene;
mod settings;
mod spore_fx;
mod terrain;

use macroquad::prelude::*;
use wk_voxel::{
    apply_cold_avalanche_bound, apply_condensation_rain_phased, apply_evaporation_into_humidity,
    apply_flow_erosion_bound, apply_karst_dissolution, apply_phase, apply_rain_with_temp,
    celestial_screen_pos_cfg, cloud_floor_y, collect_live_root_world_cells, continental_surface_y,
    day_night_factor_cfg, geotech_map_due, humidity_diffuse_due, is_daytime_cfg, is_standing_water,
    precip_forms_snow_at_air, sail_plants_on_wind_rafts_cfg, sky_rgb_at_height,
    temperature_step_due, step_carbon_budget, tick_with_life, wake_unsupported_grains,
    wake_unstable_slopes, ClimateConfig, GeotechOverlayMode, Humidity, SimSnapshot, Wind, World,
    WorldgenParams,
};

use crate::creature_list::CreatureList;
use crate::editor::CreatureEditor;
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

/// Soft vapor colour for a bilinear-sampled humidity mass.
///
/// Thin mist stays translucent. Denser mid-altitude banks go **opaque**
/// with a sky-tinted fog colour so they read as soft volume without
/// letting the sun / far ridges show through (tile-rect alpha looked like
/// a chessboard and punched holes in the middle banks).
fn humidity_haze_color(mass: f32, max_mass: f32, sky: [u8; 3]) -> Option<Color> {
    if mass <= 0.0 {
        return None;
    }
    let norm = (mass / max_mass.max(1.0)).clamp(0.0, 1.0);
    if norm < 0.08 {
        return None;
    }
    let fog = [
        lerp_u8(232, sky[0], 0.35),
        lerp_u8(236, sky[1], 0.35),
        lerp_u8(242, sky[2], 0.35),
    ];
    if norm < 0.40 {
        let t = (norm - 0.08) / 0.32;
        let a = (12.0 + t * 48.0) as u8;
        Some(Color::from_rgba(fog[0], fog[1], fog[2], a))
    } else {
        let t = ((norm - 0.40) / 0.60).clamp(0.0, 1.0);
        let body = [
            lerp_u8(fog[0], 248, t * 0.5),
            lerp_u8(fog[1], 250, t * 0.5),
            lerp_u8(fog[2], 252, t * 0.5),
        ];
        let a = if norm < 0.48 {
            (160.0 + (norm - 0.40) / 0.08 * 95.0) as u8
        } else {
            255
        };
        Some(Color::from_rgba(body[0], body[1], body[2], a))
    }
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Parallax ridge silhouettes behind the playable strip.
///
/// Mid layer is **opaque** but sky-faded (atmospheric perspective without
/// see-through). Far layer may stay translucent — only sky sits behind it.
fn draw_distance_ridges(
    tick: u64,
    sw: f32,
    sh: f32,
    climate: &ClimateConfig,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    params: &WorldgenParams,
    cam_x: f32,
) {
    if cell_px <= 0.0 || params.width_cols <= 0 {
        return;
    }
    let dn = day_night_factor_cfg(tick, climate);
    let sky = sky_rgb_at_height(dn, 0.55);
    let rock = [110u8, 108, 102];

    // (parallax, height_scale, y_bias_cells, sky_blend, alpha, seed_salt)
    let layers: [(f32, f32, f32, f32, u8, u64); 2] = [
        (0.20, 0.58, 14.0, 0.78, 155, 0xFA20_01D6),
        (0.45, 0.78, 4.0, 0.50, 255, 0x41D0_B1D6),
    ];

    let x0 = ((-origin_x) / cell_px).floor() as i32 - 2;
    let x1 = ((sw - origin_x) / cell_px).ceil() as i32 + 2;
    let baseline = params.sea_level_y as f32 - 6.0;

    for (parallax, h_scale, y_bias, sky_blend, alpha, salt) in layers {
        let col = [
            lerp_u8(rock[0], sky[0], sky_blend),
            lerp_u8(rock[1], sky[1], sky_blend),
            lerp_u8(rock[2], sky[2], sky_blend),
        ];
        let color = Color::from_rgba(col[0], col[1], col[2], alpha);
        let scroll_cells = (cam_x / cell_px) * parallax;

        for gx in x0..=x1 {
            let sample_x = gx as f32 + scroll_cells;
            let surf = continental_surface_y(
                params.seed ^ salt,
                sample_x.round() as i32,
                params.sea_level_y,
                params.width_cols,
            ) as f32;
            let peak = baseline + (surf - baseline).max(0.0) * h_scale + y_bias;
            if peak <= baseline + 2.0 {
                continue;
            }
            let sx = origin_x + gx as f32 * cell_px;
            if sx + cell_px < 0.0 || sx > sw {
                continue;
            }
            let top_sy = origin_y - (peak - params.bedrock_floor_y as f32) * cell_px;
            let bot_sy = origin_y - (baseline - params.bedrock_floor_y as f32) * cell_px;
            let h = (bot_sy - top_sy).max(cell_px);
            if top_sy > sh || top_sy + h < 0.0 {
                continue;
            }
            draw_rectangle(sx, top_sy, cell_px, h, color);
        }
    }
}

/// Paint smooth humidity haze (bilinear) into Air / empty sky cells only.
fn draw_humidity_haze(
    humidity: &Humidity,
    world: &World,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
    sky_hy_min_world_y: i32,
    sky: [u8; 3],
) {
    if humidity.cells.is_empty() || cell_px <= 0.0 {
        return;
    }
    let max_mass = humidity
        .cells
        .values()
        .copied()
        .fold(0.0f32, f32::max)
        .max(1.0);
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    let tc = humidity.tile_cols.max(1);

    let mut seeds: Vec<(i32, i32)> = Vec::with_capacity(humidity.cells.len() * 2);
    for &(hx, hy) in humidity.cells.keys() {
        for dy in -1..=1 {
            for dx in -1..=1 {
                seeds.push((hx + dx, hy + dy));
            }
        }
    }
    seeds.sort_unstable();
    seeds.dedup();

    for (hx, hy) in seeds {
        let base_gx = hx * tc;
        let base_gy = hy * tc;
        for ly in 0..tc {
            for lx in 0..tc {
                let gx = base_gx + lx;
                let gy = base_gy + ly;
                if gy < sky_hy_min_world_y {
                    continue;
                }
                if let Some(cell) = world.get_cell(gx, gy) {
                    if cell.material != wk_material::MaterialId::Air {
                        continue;
                    }
                } else {
                    continue;
                }
                let mass = humidity.sample_bilinear(gx as f32 + 0.5, gy as f32 + 0.5);
                let Some(color) = humidity_haze_color(mass, max_mass, sky) else {
                    continue;
                };
                for &x_copy in x_copies {
                    let sx = origin_x + (gx + x_copy * width_cols) as f32 * cell_px;
                    let sy = origin_y - (gy - bedrock_floor_y) as f32 * cell_px;
                    if sx + cell_px < 0.0 || sx > sw || sy < 0.0 || sy - cell_px > sh {
                        continue;
                    }
                    draw_rectangle(sx, sy - cell_px, cell_px, cell_px, color);
                }
            }
        }
    }
}

/// Day/night sky gradient + round sun/moon built from world-sized pixels.
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
    let px = PX_PER_CELL;
    if is_daytime_cfg(tick, climate) {
        // Round silhouette (radius in cells), square pixels only as the fill.
        draw_pixel_disk_cells(cx, cy, 8, px, Color::from_rgba(255, 190, 60, 85));
        draw_pixel_disk_cells(cx, cy, 6, px, Color::from_rgba(255, 215, 70, 255));
        draw_pixel_disk_cells(cx, cy, 4, px, Color::from_rgba(255, 238, 150, 255));
        draw_pixel_disk_cells(cx, cy, 2, px, Color::from_rgba(255, 252, 220, 255));
    } else {
        // Round crescent: moon disk minus offset bite (still cell lattice).
        draw_pixel_crescent_cells(
            cx,
            cy,
            6,
            2,
            -1,
            5,
            px,
            Color::from_rgba(220, 226, 238, 255),
        );
    }
}

/// Round disk centered on `(cx, cy)`, filled with `pixel`-sized squares.
/// Lattice is body-centered so the silhouette stays circular as it moves.
fn draw_pixel_disk_cells(cx: f32, cy: f32, radius_cells: i32, pixel: f32, color: Color) {
    if radius_cells <= 0 || pixel <= 0.0 {
        return;
    }
    // Slight expand so staircase edges still read as a circle, not a diamond.
    let r2 = (radius_cells as f32 + 0.35).powi(2);
    for dy in -radius_cells..=radius_cells {
        for dx in -radius_cells..=radius_cells {
            let fx = dx as f32;
            let fy = dy as f32;
            if fx * fx + fy * fy > r2 {
                continue;
            }
            let x = cx + fx * pixel - pixel * 0.5;
            let y = cy + fy * pixel - pixel * 0.5;
            draw_rectangle(x, y, pixel, pixel, color);
        }
    }
}

/// Round crescent: moon disk minus a circular bite, body-centered cells.
fn draw_pixel_crescent_cells(
    cx: f32,
    cy: f32,
    radius_cells: i32,
    bite_dx: i32,
    bite_dy: i32,
    bite_radius: i32,
    pixel: f32,
    color: Color,
) {
    if radius_cells <= 0 || pixel <= 0.0 {
        return;
    }
    let r2 = (radius_cells as f32 + 0.35).powi(2);
    let b2 = (bite_radius as f32 + 0.15).powi(2);
    let bcx = bite_dx as f32;
    let bcy = bite_dy as f32;
    for dy in -radius_cells..=radius_cells {
        for dx in -radius_cells..=radius_cells {
            let fx = dx as f32;
            let fy = dy as f32;
            if fx * fx + fy * fy > r2 {
                continue;
            }
            let bx = fx - bcx;
            let by = fy - bcy;
            if bx * bx + by * by <= b2 {
                continue;
            }
            let x = cx + fx * pixel - pixel * 0.5;
            let y = cy + fy * pixel - pixel * 0.5;
            draw_rectangle(x, y, pixel, pixel, color);
        }
    }
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

/// Cartoon → pixel clouds from coagulated [`wk_voxel::CloudStore`] parcels.
///
/// Overlapping parcels stamp into one occupancy mask and paint each
/// world-cell once, so close blobs read as a single bank (and neck /
/// split when physics drifts them apart). Rain streaks stay per-parcel.
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
    if clouds.is_empty() || cell_px <= 0.0 {
        return;
    }
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };

    // Screen-cell occupancy: max wetness wins (no alpha-stack rings).
    let mut mask: std::collections::HashMap<(i32, i32), f32> =
        std::collections::HashMap::with_capacity(clouds.parcels.len() * 96);

    for p in &clouds.parcels {
        let wet = p.wetness_with(downpour_mass);
        let r = p.radius() * cell_px;
        for &x_copy in x_copies {
            let sx = origin_x + (p.fx + (x_copy * width_cols) as f32) * cell_px;
            let sy = origin_y - (p.fy - bedrock_floor_y as f32) * cell_px;
            if sx + r * 2.0 < 0.0 || sx - r * 2.0 > sw || sy + r < 0.0 || sy - r > sh {
                continue;
            }
            stamp_pixel_cloud_mask(&mut mask, sx, sy, r, wet, p.shape_seed, p.deform, cell_px);
        }
    }

    // Paint the union once. Lighten only the top silhouette edge — no
    // circular hilite lobes (those read as holes against the sky).
    for (&(ix, iy), &wet) in &mask {
        let shade = (228.0 - wet * 95.0) as u8;
        let alpha = (190.0 + wet * 50.0).min(235.0) as u8;
        let top_edge = !mask.contains_key(&(ix, iy - 1));
        let color = if top_edge {
            Color::from_rgba(
                shade.saturating_add(22),
                shade.saturating_add(22),
                shade.saturating_add(28),
                alpha,
            )
        } else {
            Color::from_rgba(shade, shade, shade.saturating_add(6), alpha)
        };
        draw_rectangle(
            ix as f32 * cell_px,
            iy as f32 * cell_px,
            cell_px,
            cell_px,
            color,
        );
    }

    // Cosmetic precip under each raining parcel (physics is separate).
    for p in &clouds.parcels {
        if !p.raining {
            continue;
        }
        let wet = p.wetness_with(downpour_mass);
        let r = p.radius() * cell_px;
        let r_cells = p.radius();
        let floor = [-0.85_f32, -0.4, 0.0, 0.4, 0.85]
            .iter()
            .map(|t| cloud_floor_y(world, wind, p.fx + t * r_cells))
            .fold(f32::NEG_INFINITY, f32::max);
        let ground_sy = origin_y - (floor - bedrock_floor_y as f32) * cell_px;
        let as_snow = snowing(p.fx, p.fy);
        for &x_copy in x_copies {
            let sx = origin_x + (p.fx + (x_copy * width_cols) as f32) * cell_px;
            let sy = origin_y - (p.fy - bedrock_floor_y as f32) * cell_px;
            if sx + r * 2.0 < 0.0 || sx - r * 2.0 > sw || sy + r < 0.0 || sy - r > sh {
                continue;
            }
            if as_snow {
                draw_falling_snow(sx, sy, r, ground_sy, wet, sw, sh, cell_px);
            } else {
                draw_falling_rain(sx, sy, r, ground_sy, wet, sw, sh, cell_px);
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
    cell_px: f32,
) {
    let t = get_time() as f32;
    let top = sy + r * 0.35;
    let bottom = ground_sy.clamp(top + 12.0, sh - 4.0);
    let left = (sx - r * 0.85).max(-12.0);
    let right = (sx + r * 0.85).min(sw + 12.0);
    let band = (right - left).max(1.0);
    let n = ((band / 7.0) * (0.7 + wetness)).ceil().clamp(10.0, 48.0) as usize;
    let drop_len = (2.0 + wetness).ceil().clamp(2.0, 4.0) * cell_px;
    let fall_speed = 380.0 + wetness * 160.0;
    let cycle = (bottom - top + drop_len).max(drop_len + 1.0);
    for i in 0..n {
        let seed = i as f32;
        let x = ((left + ((seed * 97.371) % band)) / cell_px).floor() * cell_px;
        let phase = (seed * 0.6180339) % 1.0;
        let y = top + ((t * fall_speed + phase * cycle) % cycle) - drop_len;
        if y + drop_len < top || y > bottom {
            continue;
        }
        let alpha = (100.0 + wetness * 50.0) as u8;
        // Blocky rain streak (1 cell wide).
        draw_rectangle(
            x,
            y,
            cell_px,
            drop_len,
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
    cell_px: f32,
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
    let flake = cell_px;
    let fall_speed = 95.0 + wetness * 55.0;
    let cycle = (bottom - top + flake * 4.0).max(flake * 4.0 + 1.0);
    for i in 0..n {
        let seed = i as f32;
        let drift = ((t * 18.0 + seed * 11.3).sin()) * 6.0;
        let x = ((left + ((seed * 97.371) % band) + drift) / cell_px).floor() * cell_px;
        let phase = (seed * 0.6180339) % 1.0;
        let y = top + ((t * fall_speed + phase * cycle) % cycle) - flake;
        if y + flake < top || y > bottom {
            continue;
        }
        let alpha = (130.0 + wetness * 60.0) as u8;
        let c = Color::from_rgba(235, 242, 255, alpha);
        // 2×2 block flake.
        draw_rectangle(x, y, flake, flake, c);
        if wetness > 0.45 {
            draw_rectangle(x + flake, y, flake, flake, c);
        }
    }
}

/// Soft lobe offsets for one parcel silhouette (normalized cloud space).
fn cloud_lobe_layout(shape_seed: u32, deform: f32) -> (f32, f32, Vec<(f32, f32, f32)>) {
    let d = deform.clamp(0.0, 1.0);
    let sx = 1.0 + d * 0.22;
    let sy = 1.0 - d * 0.28;
    let s = |n: u32| {
        ((shape_seed
            .wrapping_mul(0x9E37_79B9)
            .wrapping_add(n.wrapping_mul(0x85EB_CA6B)))
            >> 8) as f32
            / 16_777_216.0
    };
    let jx = |n: u32| (s(n) - 0.5) * 0.28;
    let jr = |n: u32| 0.88 + s(n.wrapping_add(31)) * 0.28;
    let mut lobes: Vec<(f32, f32, f32)> = vec![
        (0.0, 0.02, 0.95 * jr(1)),
        (-0.72, 0.08, 0.70 * jr(2)),
        (0.78, 0.06, 0.68 * jr(3)),
        (-0.32 + jx(4) * 0.4, -0.42, 0.60 * jr(4)),
        (0.28 + jx(5) * 0.4, -0.52, 0.66 * jr(5)),
    ];
    if shape_seed & 1 == 0 {
        lobes.push((0.82, -0.22, 0.52 * jr(6)));
    }
    if shape_seed & 2 == 0 {
        lobes.push((-0.88, -0.12, 0.48 * jr(7)));
    }
    if shape_seed % 5 < 3 {
        lobes.push((jx(8) * 0.5, -0.68, 0.42 * jr(8)));
    }
    (sx, sy, lobes)
}

/// Stamp one parcel's lobe mask into the shared sky occupancy map.
fn stamp_pixel_cloud_mask(
    mask: &mut std::collections::HashMap<(i32, i32), f32>,
    cx: f32,
    cy: f32,
    r: f32,
    wet: f32,
    shape_seed: u32,
    deform: f32,
    cell_px: f32,
) {
    if r <= 0.0 || cell_px <= 0.0 {
        return;
    }
    let (sx, sy, lobes) = cloud_lobe_layout(shape_seed, deform);
    let jx1 = {
        let s = ((shape_seed.wrapping_mul(0x9E37_79B9).wrapping_add(0x85EB_CA6B)) >> 8) as f32
            / 16_777_216.0;
        (s - 0.5) * 0.28
    };
    let half_w = r * sx * 1.35;
    let half_h = r * sy * 1.15;
    let min_x = ((cx - half_w) / cell_px).floor() as i32;
    let max_x = ((cx + half_w) / cell_px).ceil() as i32;
    let min_y = ((cy - half_h) / cell_px).floor() as i32;
    let max_y = ((cy + half_h) / cell_px).ceil() as i32;
    let inv_rx = 1.0 / (r * sx).max(1e-3);
    let inv_ry = 1.0 / (r * sy).max(1e-3);

    for iy in min_y..=max_y {
        for ix in min_x..=max_x {
            let px = (ix as f32 + 0.5) * cell_px;
            let py = (iy as f32 + 0.5) * cell_px;
            let nx = (px - cx) * inv_rx;
            let ny = (py - cy) * inv_ry;
            let mut inside = false;
            for &(ox, oy, rr) in &lobes {
                let dx = nx - (ox + jx1 * 0.05);
                let dy = ny - oy;
                if dx * dx + dy * dy <= rr * rr {
                    inside = true;
                    break;
                }
            }
            if !inside {
                continue;
            }
            mask.entry((ix, iy))
                .and_modify(|w| *w = (*w).max(wet))
                .or_insert(wet);
        }
    }
}

#[cfg(test)]
mod overlay_tests {
    use super::{humidity_haze_color, stamp_pixel_cloud_mask};
    use std::collections::HashMap;

    #[test]
    fn haze_ignores_thin_vapor_and_densifies_opaque() {
        let sky = [120, 160, 200];
        assert!(humidity_haze_color(0.0, 100.0, sky).is_none());
        assert!(humidity_haze_color(5.0, 100.0, sky).is_none()); // below floor
        let thin = humidity_haze_color(25.0, 100.0, sky).expect("thin mist");
        assert!(thin.a < 0.35, "thin mist stays translucent");
        let dense = humidity_haze_color(80.0, 100.0, sky).expect("dense bank");
        assert!(
            (dense.a - 1.0).abs() < 1e-3,
            "dense mid bank must be opaque so sun/far ridges don't show through"
        );
    }

    #[test]
    fn overlapping_parcels_union_into_one_mask() {
        let cell = 3.0_f32;
        let mut mask = HashMap::new();
        // Two nearby parcels — silhouettes overlap in screen space.
        stamp_pixel_cloud_mask(&mut mask, 100.0, 80.0, 36.0, 0.4, 1, 0.0, cell);
        let alone = mask.len();
        stamp_pixel_cloud_mask(&mut mask, 118.0, 82.0, 36.0, 0.9, 2, 0.0, cell);
        assert!(alone > 20, "first parcel should stamp a real footprint");
        assert!(
            mask.len() < alone * 2,
            "overlap must share cells (alone={alone} union={})",
            mask.len()
        );
        // Max wetness wins in the overlap (no additive alpha stack).
        let wet_max = mask.values().cloned().fold(0.0_f32, f32::max);
        assert!((wet_max - 0.9).abs() < 1e-5);
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
    let mut mycelium_overlay = false;
    let mut geotech_mode = GeotechOverlayMode::Off;
    let mut show_hud = true;
    let mut editor = CreatureEditor::default();
    let mut terrain = TerrainEditor::default();
    let mut creature_list = CreatureList::default();
    let mut quit_dialog = QuitDialog::default();
    let mut inspect: Option<(i32, i32)> = None;
    let mut cam_x = 0.0f32;
    let mut cam_y = 0.0f32;
    let mut should_quit = false;

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
            if rain_on {
                apply_rain_with_temp(
                    &mut scene.world,
                    &settings.rain,
                    Some(&scene.temperature),
                    Some(&settings.phase),
                    Some(&mut scene.humidity),
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
            let _ = tick_with_life(
                &mut scene.world,
                &settings.perf,
                &settings.failure,
                Some(&scene.geotech),
                rooted.as_ref(),
                Some(&settings.grain),
                Some(&settings.fungi),
            );
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
            if settings.phase.enabled && settings.phase.enable_cold_avalanche {
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
                let outcome = scene.organisms.step_with_carbon(
                    &mut scene.world,
                    tick_no,
                    &settings.climate,
                    Some(&mut scene.humidity),
                    wind_vx,
                    Some(&scene.temperature),
                    Some(&mut scene.carbon),
                    &settings.carbon,
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

        draw_distance_ridges(
            scene.world.tick,
            sw,
            sh,
            &settings.climate,
            origin_x,
            origin_y,
            cell_px,
            &scene.params,
            cam_x,
        );

        // Creature list (F4) — before world clicks so rows steal the mouse.
        if !quit_dialog.open {
            if let Some(at) = creature_list.handle_input(&scene.organisms) {
                inspect = Some(at);
                show_hud = true;
            }
        }

        // World clicks: terrain paint, spawn picker, or block inspector.
        let (mx, my) = mouse_position();
        if !quit_dialog.open
            && terrain.open
            && !terrain.hits_panel(mx, my)
            && !creature_list.hits_panel(mx, my)
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
                    if cell.material == wk_material::MaterialId::Air {
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

        // Soft vapor haze (optional) — bilinear, Air-only; dense banks occlude.
        if humidity_overlay {
            let dn = day_night_factor_cfg(scene.world.tick, &settings.climate);
            let sky = sky_rgb_at_height(dn, 0.45);
            draw_humidity_haze(
                &scene.humidity,
                &scene.world,
                origin_x,
                origin_y,
                cell_px,
                scene.params.bedrock_floor_y,
                scene.params.wrap_x,
                scene.params.width_cols,
                sw,
                sh,
                scene.params.sea_level_y + 4,
                sky,
            );
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

        // Mycelium strain overlay: bright per-network colors by cream intensity.
        if mycelium_overlay {
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
                            Color::from_rgba(rgba[0], rgba[1], rgba[2], rgba[3]),
                        );
                    }
                }
            }
        }

        // Geotech overlay: G cycles shear → σᵥ → wet → off.
        if geotech_mode != GeotechOverlayMode::Off {
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
                                geotech_overlay_color(value, s_max),
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
                                geotech_overlay_color(sigma, s_max),
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
        if !editor_covers_world && !settings.open {
            for &(gx, gy, (r, g, b)) in &scene.organisms.draw_list(
                &scene.world,
                scene.world.tick,
                draw_wind_vx,
            ) {
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
        quit_dialog.draw();

        // HUD chrome (info + hotkeys + inspector) toggled with F1.
        if show_hud && !quit_dialog.open {
            let tod = if is_daytime_cfg(scene.world.tick, &settings.climate) {
                "day"
            } else {
                "night"
            };
            let rain_tag = if !rain_on {
                "off"
            } else if settings.rain.closed_loop {
                "on/closed"
            } else {
                "on/MINT"
            };
            let info = format!(
                "fps={:.0}  tick={} {} T̄={:.1}C rain={} evap={} phase={} nimbus={} cloud_m={:.0} hum={:.0} C={:.0}/{:.0} spores={} wind={:.2} creatures={}/{} ({}) dead={} {}",
                fps_smoothed(),
                scene.world.tick,
                tod,
                scene.temperature.mean(),
                rain_tag,
                if evap_on { "on" } else { "off" },
                if settings.phase.enabled { "on" } else { "off" },
                scene.clouds.len(),
                scene.clouds.total_mass(),
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
                "Tab|Space|R|W/C/E/K/O|I|N/T/H/M/G|F1 HUD|F2 creat|F3 terra|F4 list|F5/F9 save|Esc quit",
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
