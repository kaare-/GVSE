use macroquad::prelude::*;
use wk_material::{MaterialId, MaterialRegistry, SAMPLE_WIDTH_M};
use wk_world::terrain::BEDROCK_FLOOR_M;
use wk_world::column::Activity;
use wk_world::{OverlayMode, RenderSnapshot};

/// Fixed side-view scale (Terraria / Rain World style — no zoom).
pub const COL_W: f32 = 4.0;
pub const PX_PER_M: f32 = 3.0;
pub const SEA_SCREEN_FRAC: f32 = 0.58;

pub const SAVE_PATH: &str = "world_save.bin";

pub fn viewport_column_count(screen_w: f32) -> usize {
    (screen_w / COL_W).ceil() as usize
}

pub fn world_y_to_screen(elev_m: f32, sea_level: f32, sh: f32, camera_y_offset: f32) -> f32 {
    let sea_y = sh * SEA_SCREEN_FRAC + camera_y_offset;
    sea_y - (elev_m - sea_level) * PX_PER_M
}

pub fn screen_x_to_world_x(mx: f32, viewport_x: i32) -> i32 {
    viewport_x + (mx / COL_W).floor() as i32
}

/// Draws a column's actual layers, top to bottom, each sized by real
/// mass. Under the unified material model this handles solids AND
/// water/ice/snow uniformly: they're all layers, they all get sized by
/// mass, and each material's `render_alpha` prop gives it its natural
/// translucency without a special rendering pass.
///
/// No visible-depth cutoff: the earlier "detail band + uniform depths
/// fill" pattern shifted its dark region upward as `surface_y` grew,
/// which read as the bedrock rising. Everything is drawn at its real
/// height now; a tall column that goes off the top of the screen is
/// fine — the user can pan the camera vertically with W/S to see it.
fn draw_terrain_column(
    x: f32,
    surface_y: f32,
    sea_level: f32,
    sh: f32,
    camera_y_offset: f32,
    layers: &[(MaterialId, i64, u64, u64)],
    voids: &[(f32, f32, i64, u8)],
    px_per_m: f32,
    saturation: f32,
) {
    // Paint solid layers from the surface down, inserting void cutouts at
    // their absolute elevations so caves read as dark bands (with pooled
    // water when present).
    let mut void_bands: Vec<(f32, f32, i64, u8)> = voids.to_vec();
    void_bands.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

    let mut y_m = surface_y;
    let mut vi = 0usize;
    for &(mat, thickness, _, _) in layers {
        if thickness <= 0 {
            continue;
        }
        let mut remaining_m = {
            let density = MaterialRegistry::props(mat).density.max(1) as f32;
            (thickness as f32 / density) / SAMPLE_WIDTH_M
        };
        while remaining_m > 1e-5 {
            // If a void starts at/above current y, paint it first.
            if vi < void_bands.len() {
                let (vtop, vh, vwater, vlight) = void_bands[vi];
                if vtop >= y_m - 1e-3 {
                    let top_px = world_y_to_screen(vtop.min(y_m), sea_level, sh, camera_y_offset);
                    let bot_y = (vtop - vh).min(y_m);
                    let bot_px = world_y_to_screen(bot_y, sea_level, sh, camera_y_offset);
                    let h = (bot_px - top_px).max(0.6);
                    let dark = 18u8.saturating_add(vlight / 8);
                    draw_rectangle(
                        x,
                        top_px,
                        COL_W,
                        h,
                        Color::from_rgba(dark, dark, dark.saturating_add(8), 255),
                    );
                    if vwater > 0 {
                        let fill_m = {
                            let density =
                                MaterialRegistry::props(MaterialId::Water).density.max(1) as f32;
                            ((vwater as f32 / density) / SAMPLE_WIDTH_M).min(vh)
                        };
                        let water_top = bot_y + fill_m;
                        let wt_px = world_y_to_screen(water_top, sea_level, sh, camera_y_offset);
                        let wh = (bot_px - wt_px).max(0.5);
                        draw_rectangle(
                            x,
                            wt_px,
                            COL_W,
                            wh,
                            Color::from_rgba(0x23, 0x64, 0xD2, 180),
                        );
                    }
                    y_m = bot_y;
                    vi += 1;
                    continue;
                }
            }
            // Paint solid down to the next void top (or all remaining).
            let next_void_top = void_bands.get(vi).map(|v| v.0);
            let paint_m = match next_void_top {
                Some(vt) if vt < y_m => (y_m - vt).min(remaining_m),
                _ => remaining_m,
            };
            let top_px = world_y_to_screen(y_m, sea_level, sh, camera_y_offset);
            let bot_y = y_m - paint_m;
            let bot_px = world_y_to_screen(bot_y, sea_level, sh, camera_y_offset);
            let h = (bot_px - top_px).max(0.6);
            let [mut r, mut g, mut b] = MaterialRegistry::colour_rgb(mat);
            if saturation > 0.05
                && matches!(
                    mat,
                    MaterialId::Sand
                        | MaterialId::Clay
                        | MaterialId::Gravel
                        | MaterialId::LooseRock
                        | MaterialId::Stone
                        | MaterialId::Limestone
                        | MaterialId::Organic
                )
            {
                let t = (saturation * 0.35).min(0.35);
                r = lerp_u8(r, 40, t);
                g = lerp_u8(g, 55, t);
                b = lerp_u8(b, 85, t);
            }
            let a = MaterialRegistry::props(mat).render_alpha;
            draw_rectangle(x, top_px, COL_W, h, Color::from_rgba(r, g, b, a));
            y_m = bot_y;
            remaining_m -= paint_m;
            let _ = px_per_m;
        }
    }
    // Trailing voids below the solid stack.
    while vi < void_bands.len() {
        let (vtop, vh, vwater, vlight) = void_bands[vi];
        let top_px = world_y_to_screen(vtop.min(y_m), sea_level, sh, camera_y_offset);
        let bot_y = vtop - vh;
        let bot_px = world_y_to_screen(bot_y, sea_level, sh, camera_y_offset);
        let h = (bot_px - top_px).max(0.6);
        let dark = 18u8.saturating_add(vlight / 8);
        draw_rectangle(
            x,
            top_px,
            COL_W,
            h,
            Color::from_rgba(dark, dark, dark.saturating_add(8), 255),
        );
        if vwater > 0 {
            let fill_m = {
                let density = MaterialRegistry::props(MaterialId::Water).density.max(1) as f32;
                ((vwater as f32 / density) / SAMPLE_WIDTH_M).min(vh)
            };
            let water_top = bot_y + fill_m;
            let wt_px = world_y_to_screen(water_top, sea_level, sh, camera_y_offset);
            let wh = (bot_px - wt_px).max(0.5);
            draw_rectangle(x, wt_px, COL_W, wh, Color::from_rgba(0x23, 0x64, 0xD2, 180));
        }
        y_m = bot_y;
        vi += 1;
    }
}

fn mass_to_px(material: MaterialId, mass: i64, px_per_m: f32) -> f32 {
    let density = MaterialRegistry::props(material).density.max(1) as f32;
    let height_m = (mass as f32 / density) / SAMPLE_WIDTH_M;
    height_m * px_per_m
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Sky colour driven by the day/night cycle, with a warm twilight glow right
/// at sunrise/sunset, dimmed further if it's currently raining.
fn sky_color_for(snap: &RenderSnapshot) -> Color {
    let factor = snap.climate.day_night_factor(snap.tick); // -1..1
    let t = (factor + 1.0) / 2.0; // 0 (deep night) .. 1 (solar noon)
    let night = (8u8, 10u8, 30u8);
    let day = (135u8, 206u8, 235u8);
    let twilight = (255u8, 150u8, 95u8);

    let base = (
        lerp_u8(night.0, day.0, t),
        lerp_u8(night.1, day.1, t),
        lerp_u8(night.2, day.2, t),
    );
    // Peaks exactly at sunrise/sunset (factor == 0), fades out toward both
    // full day and full night.
    let twilight_amt = (1.0 - factor.abs() * 2.5).clamp(0.0, 1.0) * 0.6;
    let blended = (
        lerp_u8(base.0, twilight.0, twilight_amt),
        lerp_u8(base.1, twilight.1, twilight_amt),
        lerp_u8(base.2, twilight.2, twilight_amt),
    );

    if snap.rain_enabled {
        Color::from_rgba(
            lerp_u8(blended.0, 90, 0.65),
            lerp_u8(blended.1, 100, 0.65),
            lerp_u8(blended.2, 120, 0.65),
            255,
        )
    } else {
        Color::from_rgba(blended.0, blended.1, blended.2, 255)
    }
}

/// A simple sun (day) or moon (night) tracing a low arc across the sky,
/// purely cosmetic feedback for the current time of day.
fn draw_celestial_body(sw: f32, sh: f32, snap: &RenderSnapshot) {
    let climate = &snap.climate;
    let cycle_tick = snap.tick % climate.cycle_length_ticks();
    let (local_phase, color, radius) = if climate.is_daytime(snap.tick) {
        let p = cycle_tick as f32 / climate.day_length_ticks.max(1) as f32;
        (p, Color::from_rgba(255, 236, 160, 235), 22.0)
    } else {
        let p = (cycle_tick - climate.day_length_ticks) as f32
            / climate.night_length_ticks.max(1) as f32;
        (p, Color::from_rgba(225, 228, 240, 215), 16.0)
    };
    let x = sw * 0.08 + local_phase.clamp(0.0, 1.0) * sw * 0.84;
    let arc_h = sh * 0.26;
    let y = sh * 0.10 + arc_h * (1.0 - (local_phase * std::f32::consts::PI).sin());
    draw_circle(x, y, radius, color);
}

const RAIN_STREAKS: usize = 90;

/// Animated diagonal rain streaks above the water line, looping via get_time().
fn draw_rain(sw: f32, sh: f32, sea_y: f32) {
    let t = get_time() as f32;
    let bottom = sea_y.min(sh);
    let drop_len = 14.0;
    let fall_speed = 620.0; // px/sec
    for i in 0..RAIN_STREAKS {
        let seed = i as f32;
        let x = ((seed * 92.371) % (sw + 40.0)) - 20.0;
        let phase = (seed * 0.6180339) % 1.0;
        let cycle = bottom + drop_len;
        let y = ((t * fall_speed + phase * cycle) % cycle) - drop_len;
        if y < -drop_len || y > bottom {
            continue;
        }
        draw_line(
            x,
            y,
            x - 3.0,
            y + drop_len,
            1.0,
            Color::from_rgba(200, 220, 255, 130),
        );
    }
}

/// Simplified drifting cloud shapes (a small cluster of overlapping soft
/// ellipses, not particles) — opacity signals how much rain potential is
/// left, so a cloud visibly looks "spent" right before it disappears.
fn draw_clouds(snap: &RenderSnapshot, sw: f32, sh: f32) {
    for cloud in &snap.clouds {
        let cx = (cloud.x - snap.viewport_x as f32) * COL_W;
        if cx < -cloud.half_width * COL_W * 1.5 || cx > sw + cloud.half_width * COL_W * 1.5 {
            continue;
        }
        let cy = sh * 0.14;
        let w = cloud.half_width * COL_W;
        let moisture_frac = (cloud.moisture / 1300.0).clamp(0.15, 1.0);
        let alpha = (90.0 + moisture_frac * 120.0) as u8;
        let base = Color::from_rgba(235, 238, 245, alpha);
        for (dx, dy, scale) in [
            (0.0f32, 0.0f32, 1.0f32),
            (-0.5, 0.15, 0.7),
            (0.5, 0.12, 0.75),
            (-0.2, -0.2, 0.6),
            (0.3, -0.15, 0.55),
        ] {
            draw_circle(cx + dx * w, cy + dy * w * 0.4, (w * scale * 0.45).max(6.0), base);
        }
    }
}

pub fn draw_frame(
    snap: &RenderSnapshot,
    selected: Option<i32>,
    camera_y_offset: f32,
    status_line: &str,
) {
    let sw = screen_width();
    let sh = screen_height();
    let sea_y = world_y_to_screen(snap.sea_level, snap.sea_level, sh, camera_y_offset);
    let bedrock_px = world_y_to_screen(BEDROCK_FLOOR_M, snap.sea_level, sh, camera_y_offset);

    // Sky fills the entire background. Anything above the terrain
    // (mountain top sticking up into open air, gap where a submerged
    // column's water has evaporated below sea_level, etc.) reads as
    // sky, not as a placeholder colour from an earlier version of the
    // renderer that had a separate "under sea_y" band.
    let sky_color = sky_color_for(snap);
    clear_background(sky_color);
    draw_celestial_body(sw, sh, snap);
    draw_clouds(snap, sw, sh);

    // Mantle void below deepest bedrock — never water
    if bedrock_px < sh {
        draw_rectangle(
            0.0,
            bedrock_px,
            sw,
            sh - bedrock_px,
            Color::from_rgba(18, 18, 24, 255),
        );
    }

    // Unified terrain-and-water pass: every substance (sand, stone,
    // organic, water, ice, snow) is a Layer in the column stack, and
    // draw_terrain_column paints them all with their own colour and
    // per-material alpha. No more separate "solid pass" + "water pass":
    // the ocean is just Water layers filling submerged columns up to
    // sea level, and a puddle is just a thin Water layer on top of
    // whatever else is there.
    let mut tops: Vec<Option<f32>> = Vec::with_capacity(snap.columns.len());
    for (i, col) in snap.columns.iter().enumerate() {
        if col.layers.is_empty() {
            tops.push(None);
            continue;
        }

        let x = i as f32 * COL_W;
        // Draw layers at each column's *actual* surface — no smoothing
        // across neighbours. Layer positions are absolute (relative to
        // bedrock, which is fixed), so a melting snow cap doesn't drag
        // the layers underneath around on screen just because its
        // surface_y dropped this tick. Neighbours may honestly differ
        // in height; that's what having a per-column simulation means.
        let surface_px = world_y_to_screen(col.surface_y, snap.sea_level, sh, camera_y_offset);
        let col_bedrock_px = world_y_to_screen(col.bedrock_y, snap.sea_level, sh, camera_y_offset);

        draw_terrain_column(
            x,
            col.surface_y,
            snap.sea_level,
            sh,
            camera_y_offset,
            &col.layers,
            &col.voids,
            PX_PER_M,
            col.saturation,
        );
        tops.push(Some(surface_px));

        if Some(col.world_x) == selected {
            draw_rectangle_lines(
                x,
                surface_px,
                COL_W,
                (col_bedrock_px - surface_px).max(2.0),
                1.5,
                Color::from_rgba(255, 255, 100, 220),
            );
        }
    }

    // Overlays
    for (i, col) in snap.columns.iter().enumerate() {
        let Some(top) = tops[i] else {
            continue;
        };
        let x = i as f32 * COL_W;
        match snap.overlay.mode {
            OverlayMode::WaterFlux if col.water_flux > 0 => {
                let a = (col.water_flux.min(100) as f32 / 100.0 * 255.0) as u8;
                draw_rectangle(x, top - 5.0, COL_W, 2.0, Color::from_rgba(0, 200, 255, a));
            }
            OverlayMode::Erosion if col.erosion_flux > 0 => {
                draw_rectangle(x, top, COL_W, 3.0, Color::from_rgba(255, 0, 0, 200));
            }
            OverlayMode::Activity => {
                let c = match col.activity {
                    Activity::Dormant => Color::from_rgba(80, 80, 80, 128),
                    Activity::HydrologyActive => Color::from_rgba(0, 255, 0, 128),
                };
                draw_rectangle(x, top - 3.0, COL_W, 3.0, c);
            }
            OverlayMode::Conservation => {
                let t = snap.mass_audit.total_tracked();
                let a = ((t % 256) as u8).max(64);
                draw_rectangle(x, sh - 8.0, COL_W, 4.0, Color::from_rgba(a, 255 - a, 128, 200));
            }
            OverlayMode::TemperatureField => {
                // Cold blue → hot red across roughly 0–55 °C (sky→geothermal).
                let c = temperature_overlay_color(col.temperature_c);
                draw_rectangle(x, top - 6.0, COL_W, 5.0, c);
            }
            OverlayMode::HumidityField => {
                let c = humidity_overlay_color(col.humidity_rh);
                draw_rectangle(x, top - 6.0, COL_W, 5.0, c);
            }
            OverlayMode::None => {}
        }
    }

    draw_line(0.0, sea_y, sw, sea_y, 1.0, Color::from_rgba(255, 255, 255, 90));

    if snap.rain_enabled {
        draw_rain(sw, sh, sea_y);
    }

    for m in &snap.markers {
        let lx = (m.world_x - snap.viewport_x) as f32 * COL_W;
        if (0.0..sw).contains(&lx) {
            draw_rectangle(lx, 10.0, COL_W, 8.0, Color::from_rgba(255, 0, 255, 255));
        }
    }

    let phase = snap.climate.phase_fraction(snap.tick);
    let clock_minutes = (phase * 24.0 * 60.0) as u32;
    let (clock_h, clock_m) = (clock_minutes / 60, clock_minutes % 60);
    let day_or_night = if snap.climate.is_daytime(snap.tick) { "day" } else { "night" };
    let hud = format!(
        "tick={} sea={:.0}m x={}..{} | {clock_h:02}:{clock_m:02} ({day_or_night}) | clouds={} | {}",
        snap.tick,
        snap.sea_level,
        snap.viewport_x,
        snap.viewport_x + snap.columns.len() as i32,
        snap.clouds.len(),
        status_line
    );
    draw_rectangle(0.0, sh - 24.0, sw, 24.0, Color::from_rgba(0, 0, 0, 200));
    draw_text(&hud, 8.0, sh - 8.0, 16.0, WHITE);

    if let Some(wx) = selected {
        draw_inspector(snap, wx, sw);
    }
}

fn material_name(mat: MaterialId) -> &'static str {
    match mat {
        MaterialId::Bedrock => "bedrock",
        MaterialId::Stone => "stone",
        MaterialId::Limestone => "limestone",
        MaterialId::LooseRock => "looserock",
        MaterialId::Gravel => "gravel",
        MaterialId::Sand => "sand",
        MaterialId::Clay => "clay",
        MaterialId::Organic => "organic",
        MaterialId::Water => "water",
        MaterialId::Air => "air",
        MaterialId::Snow => "snow",
        MaterialId::Ice => "ice",
    }
}

/// Map °C → RGBA for the temperature overlay (cold blue → hot red).
fn temperature_overlay_color(temp_c: f32) -> Color {
    let t = ((temp_c - 0.0) / 55.0).clamp(0.0, 1.0);
    let r = (t * 255.0) as u8;
    let g = ((1.0 - (t - 0.5).abs() * 2.0).max(0.0) * 180.0) as u8;
    let b = ((1.0 - t) * 255.0) as u8;
    Color::from_rgba(r, g, b, 220)
}

/// Map relative humidity → RGBA (dry brown → wet cyan).
fn humidity_overlay_color(rh: f32) -> Color {
    let t = rh.clamp(0.0, 1.0);
    let r = ((1.0 - t) * 160.0) as u8;
    let g = (120.0 + t * 100.0) as u8;
    let b = (40.0 + t * 200.0) as u8;
    Color::from_rgba(r, g, b, 220)
}

fn draw_inspector(snap: &RenderSnapshot, world_x: i32, sw: f32) {
    let Some(col) = snap.columns.iter().find(|c| c.world_x == world_x) else {
        return;
    };

    let panel_w = 280.0;
    let panel_h = 240.0;
    let x0 = sw - panel_w - 8.0;
    let y0 = 8.0;

    draw_rectangle(x0, y0, panel_w, panel_h, Color::from_rgba(0, 0, 0, 220));
    draw_rectangle_lines(x0, y0, panel_w, panel_h, 1.0, Color::from_rgba(200, 200, 200, 255));

    let mut lines = vec![
        format!("Column x={}", col.world_x),
        format!("surface_y={:.2} m  bedrock={:.0}m", col.surface_y, col.bedrock_y),
        format!(
            "temp={:.1}C  RH={:.0}%  biome={}",
            col.temperature_c,
            col.humidity_rh * 100.0,
            col.biome.name()
        ),
        format!(
            "water={} kg  moisture={} kg  sat={:.0}%",
            col.surface_water,
            col.moisture,
            col.saturation * 100.0
        ),
        format!("ice={} kg  snow={} kg", col.ice, col.snow),
        format!(
            "sediment={} kg ({})",
            col.sediment.total,
            material_name(col.sediment.dominant)
        ),
        format!("flux w={} erode={}", col.water_flux, col.erosion_flux),
        "--- layers ---".to_string(),
    ];

    for (i, &(mat, thickness, age_start, age_end)) in col.layers.iter().enumerate().take(6) {
        lines.push(format!(
            "  [{i}] {} {}kg age {}-{}",
            material_name(mat),
            thickness,
            age_start,
            age_end
        ));
    }

    for (i, line) in lines.iter().enumerate().take(12) {
        draw_text(line, x0 + 8.0, y0 + 16.0 + i as f32 * 14.0, 14.0, WHITE);
    }
}
