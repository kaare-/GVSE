//! Sim-linked sky / weather / far-ridge drawing for wk-voxel-app.
//!
//! Design: `docs/SKY.md`. Isolation: wk-voxel + wk-material + macroquad only.

use std::collections::HashMap;

use macroquad::prelude::*;
use wk_material::MaterialId;
use wk_voxel::{
    build_canopy_index_posed, carbon_ratio, celestial_moon_screen_pos_cfg,
    celestial_sun_screen_pos_cfg, cloud_floor_y, cloud_sky_transmit, continental_surface_y,
    day_night_factor_cfg, falls_through_empty_air, humidity_mean_norm, is_standing_water,
    precip_cover_fraction, resolve_organism_draw_cells, shade_transmit_column,
    sky_rgb_at_height_weather, CanopyIndex, CarbonBudget, ClimateConfig, CloudStore, Humidity,
    ModuleId, OrganismStore, PosedModule, SkyWeatherParams, Temperature, Wind, World,
    CHUNK_CELLS_H, CHUNK_CELLS_W, GRAIN_REPOSE_HAZE_MAX, LIVE_SURFACE_SEARCH,
};

/// Active-layer parcel parallax (1 = locked to terrain; lower = farther).
pub const CLOUD_PARALLAX: f32 = 0.78;
pub const FAR_RIDGE_PARALLAX: f32 = 0.12;
pub const NEAR_RIDGE_PARALLAX: f32 = 0.32;
/// Soft cloud bank parallax for depth echoes (humidity-driven, not tile paint).
pub const CLOUD_FAR_PARALLAX: f32 = 0.20;
pub const CLOUD_MID_PARALLAX: f32 = 0.48;
pub const CLOUD_FRONT_PARALLAX: f32 = 1.08;
const RIDGE_REFRESH_TICKS: u64 = 30;
const MILD_TEMP_C: f32 = 18.0;
/// Depth pass for soft cloud banks (see `docs/SKY.md`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloudDepthLayer {
    Far,
    Mid,
    /// Visual humidity echo on the playfield.
    Active,
    Front,
}

/// One soft lobe bank to stamp (parcel echo or humidity-peak ghost).
#[derive(Debug, Clone, Copy)]
struct SoftCloudSrc {
    fx: f32,
    fy: f32,
    wet: f32,
    shape_seed: u32,
    deform: f32,
    radius_cells: f32,
}

/// Live-tunable sky / ridge / cloud cosmetics (Tab → Climate → Sky look).
///
/// Sun/moon radii are **screen pixels** (finer than world cells), not voxel cells.
#[derive(Debug, Clone)]
pub struct AtmosphereLookConfig {
    /// Sun body radius in screen pixels.
    pub sun_radius: f32,
    /// Outer glow radius in screen pixels.
    pub sun_glow_radius: f32,
    /// Moon disk radius in screen pixels.
    pub moon_radius: f32,
    /// Crescent bite offset (pixels, along +x).
    pub moon_bite_offset: f32,
    /// Crescent bite radius (pixels).
    pub moon_bite_radius: f32,
    pub ridge_sky_mix_near: f32,
    pub ridge_sky_mix_far: f32,
    pub ridge_desat_near: f32,
    pub ridge_desat_far: f32,
    pub ridge_feather_near: f32,
    pub ridge_feather_far: f32,
    /// How hard the mid crest tips toward sky / far plate (0..1).
    pub ridge_crest_blend: f32,
    /// Extra far-plate mix into mid crest when far sits behind it (0..1).
    pub ridge_far_into_crest: f32,
    /// 0 = grey banks, 1 = bright white cores.
    pub cloud_whiteness: f32,
    /// Strength of sun-angled cast shadows from plants/creatures (0..1).
    pub cast_shadow_strength: f32,
    /// Mild column dim under cloud cover (0..1).
    pub cloud_shade_strength: f32,
    /// Humidity vapour layer strengths (0..1).
    pub vapour_far: f32,
    pub vapour_mid: f32,
    pub vapour_active: f32,
    pub vapour_front: f32,
}

impl Default for AtmosphereLookConfig {
    fn default() -> Self {
        Self {
            sun_radius: 28.0,
            sun_glow_radius: 58.0,
            moon_radius: 22.0,
            moon_bite_offset: 11.0,
            moon_bite_radius: 18.0,
            ridge_sky_mix_near: 0.72,
            ridge_sky_mix_far: 0.88,
            ridge_desat_near: 0.28,
            ridge_desat_far: 0.38,
            ridge_feather_near: 4.0,
            ridge_feather_far: 5.0,
            ridge_crest_blend: 0.55,
            ridge_far_into_crest: 0.70,
            cloud_whiteness: 0.62,
            cast_shadow_strength: 0.85,
            cloud_shade_strength: 0.35,
            vapour_far: 0.55,
            vapour_mid: 0.70,
            vapour_active: 0.85,
            vapour_front: 0.65,
        }
    }
}

/// Cached dual-parallax silhouettes derived from live surface heights.
#[derive(Debug, Clone, Default)]
pub struct RidgeSilhouette {
    pub width_cols: i32,
    pub far: Vec<i32>,
    pub near: Vec<i32>,
    last_tick: u64,
    last_seed: u64,
}

impl RidgeSilhouette {
    pub fn ensure(
        &mut self,
        world: &World,
        width_cols: i32,
        bedrock_floor_y: i32,
        sky_ceiling_y: i32,
        sea_level_y: i32,
        tick: u64,
        seed: u64,
    ) {
        let due = self.far.is_empty()
            || self.width_cols != width_cols
            || self.last_seed != seed
            || tick.saturating_sub(self.last_tick) >= RIDGE_REFRESH_TICKS;
        if !due {
            return;
        }
        let mut raw = Vec::with_capacity(width_cols as usize);
        for x in 0..width_cols {
            raw.push(column_surface_y(
                world,
                x,
                bedrock_floor_y,
                sky_ceiling_y,
                sea_level_y,
                seed,
                width_cols,
            ));
        }
        self.far = lowpass_wrap(&raw, 8);
        self.near = lowpass_wrap(&raw, 3);
        self.width_cols = width_cols;
        self.last_tick = tick;
        self.last_seed = seed;
    }
}

fn column_surface_y(
    world: &World,
    gx: i32,
    bedrock: i32,
    sky_ceiling: i32,
    sea_level: i32,
    seed: u64,
    width_cols: i32,
) -> i32 {
    // Walk from the procedural profile, not from the sky ceiling.
    // Snow is a solid, so a ceiling scan treats every falling flake as the
    // crest — that is the needle-spike on the far/near plates. Humidity is
    // a tile field and never lives in these cells; wet Air only counts when
    // it is a lake or the sea.
    let hint = continental_surface_y(seed, gx, sea_level, width_cols)
        .clamp(bedrock, sky_ceiling.saturating_sub(1));
    live_surface_y_ground(world, gx, hint, LIVE_SURFACE_SEARCH, sea_level)
}

fn ridge_ground_at(world: &World, gx: i32, y: i32, sea_level: i32) -> bool {
    let Some(c) = world.get_cell(gx, y) else {
        return false;
    };
    if falls_through_empty_air(c.material) {
        return false;
    }
    if c.material != MaterialId::Air {
        return true;
    }
    if c.sat.is_empty() {
        return false;
    }
    y <= sea_level || is_standing_water(world, gx, y)
}

fn live_surface_y_ground(
    world: &World,
    gx: i32,
    hint: i32,
    search: i32,
    sea_level: i32,
) -> i32 {
    let jx = world.wrap_x(gx);
    let ground = |y: i32| ridge_ground_at(world, jx, y, sea_level);
    if world.get_cell(jx, hint).is_none() {
        return hint;
    }
    if ground(hint) {
        let mut y = hint;
        for _ in 0..search {
            if !ground(y + 1) {
                return y;
            }
            y += 1;
        }
        return y;
    }
    let mut y = hint;
    for _ in 0..search {
        y -= 1;
        if ground(y) {
            return y;
        }
    }
    hint
}

fn lowpass_wrap(raw: &[i32], half_window: i32) -> Vec<i32> {
    let n = raw.len() as i32;
    if n == 0 {
        return Vec::new();
    }
    let w = half_window.max(0);
    let mut out = Vec::with_capacity(raw.len());
    for i in 0..n {
        let mut sum = 0i64;
        let mut count = 0i64;
        for d in -w..=w {
            let j = (i + d).rem_euclid(n) as usize;
            sum += raw[j] as i64;
            count += 1;
        }
        out.push((sum / count) as i32);
    }
    out
}

/// True when this cell is the falling drop (1-wide water / flake), not vapour.
fn haze_cell_is_drop_cell(c: wk_voxel::Cell) -> bool {
    if c.material == MaterialId::Air && c.sat.0 > GRAIN_REPOSE_HAZE_MAX {
        return true;
    }
    falls_through_empty_air(c.material)
}

fn haze_cell_is_drop(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy) {
        Some(c) => haze_cell_is_drop_cell(c),
        None => false,
    }
}

/// Highest drop in each column. Haze below that cell is the open path.
fn collect_drop_tops(world: &World) -> HashMap<i32, i32> {
    let mut tops: HashMap<i32, i32> = HashMap::new();
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    for chunk in world.chunks.values() {
        if !chunk.has_wet_air && !chunk.has_snow && !chunk.has_buoyant {
            continue;
        }
        let ox = chunk.coord.cx * cw;
        let oy = chunk.coord.cy * ch;
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                if !haze_cell_is_drop_cell(chunk.get(lx, ly)) {
                    continue;
                }
                let gx = world.wrap_x(ox + lx as i32);
                let gy = oy + ly as i32;
                tops.entry(gx).and_modify(|y| *y = (*y).max(gy)).or_insert(gy);
            }
        }
    }
    tops
}

/// Inclusive bottom of the 4×4 column still painted. Everything at or
/// under the drop is left open — the 1-wide path through the field.
fn haze_column_y0(y0: i32, y1: i32, drop_y: Option<i32>) -> Option<i32> {
    let y0 = match drop_y {
        Some(d) => (d + 1).max(y0),
        None => y0,
    };
    if y0 >= y1 {
        None
    } else {
        Some(y0)
    }
}

/// Occupied tiles plus their cardinal neighbours (wrapped).
///
/// An emptied nucleating tile must stay a seat so bilinear can paint
/// the three sibling columns. Skipping it is the 4-wide hole. Neighbour
/// keys go through [`Humidity::wrap_tile_x`] — raw `hx-1` was the ring
/// seam.
fn haze_paint_seats(humidity: &Humidity, sky_hy_min: i32) -> Vec<(i32, i32)> {
    let mut seats = std::collections::HashSet::new();
    for (&(hx, hy), &mass) in &humidity.cells {
        if mass <= 0.0 || hy < sky_hy_min {
            continue;
        }
        seats.insert((hx, hy));
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let Some(nx) = humidity.wrap_tile_x(hx + dx) else {
                continue;
            };
            let ny = hy + dy;
            if ny < sky_hy_min {
                continue;
            }
            if let Some(b) = humidity.bounds {
                if !b.contains(nx, ny) {
                    continue;
                }
            }
            seats.insert((nx, ny));
        }
    }
    let mut out: Vec<_> = seats.into_iter().collect();
    out.sort_unstable();
    out
}

/// Cells of one seat that still get haze after the shaft, with bilinear mass.
///
/// Shaft is the mask (1-wide). Resample is alpha only. Per-column floor
/// so the clip does not step 4-wide at a tile edge (the 127/128 shelf).
fn haze_resampled_cells(
    humidity: &Humidity,
    hx: i32,
    hy: i32,
    drop_tops: &HashMap<i32, i32>,
    wrap_x: impl Fn(i32) -> i32,
    mut floor_y: impl FnMut(i32) -> i32,
) -> Vec<(i32, i32, f32)> {
    let tc = humidity.tile_cols.max(1);
    let base_gx = hx * tc;
    let base_gy = hy * tc;
    let y1 = base_gy + tc;
    let mut out = Vec::new();
    for col in 0..tc {
        let wx = wrap_x(base_gx + col);
        let y0 = (floor_y(wx) + 1).max(base_gy);
        let Some(col_y0) = haze_column_y0(y0, y1, drop_tops.get(&wx).copied()) else {
            continue;
        };
        for gy in col_y0..y1 {
            let sampled = humidity.sample_haze(wx as f32 + 0.5, gy as f32 + 0.5);
            out.push((wx, gy, sampled));
        }
    }
    out
}

/// Soft white vapor haze alpha (legacy helper for tests / diagnostics).
pub fn humidity_haze_alpha(mass: f32, max_mass: f32) -> u8 {
    humidity_haze_style(mass, max_mass, HAZE_MASS_FLOOR / HAZE_MASS_FULL.max(1.0)).3
}

/// Absolute humidity mass below which the H wash stays clear.
///
/// This is **10% of the 255 humidity scale** (`0.10 × 255 = 25.5`), not
/// 25.5% of saturation-at-T — that mistake wiped the whole field.
pub const HAZE_MASS_FLOOR: f32 = 25.5;
/// Humidity mass that paints as full wash opacity (the 255 cell scale).
pub const HAZE_MASS_FULL: f32 = 255.0;
/// Lowest painted alpha — the mass-floor edge must read as haze.
pub const HAZE_ALPHA_MIN: u8 = 36;
pub const HAZE_ALPHA_MAX: u8 = 210;
/// `< 1` lifts mid values so moist air reads as a field, not a film.
const HAZE_ALPHA_GAMMA: f32 = 0.72;

/// `(r, g, b, a)` for one haze cell. Wetter seats go denser *and* less
/// blue-grey so high humidity reads as fuller white.
pub fn humidity_haze_style(mass: f32, max_mass: f32, floor: f32) -> (u8, u8, u8, u8) {
    let a = humidity_haze_alpha_cell(mass, max_mass, floor);
    if a == 0 {
        return (255, 255, 255, 0);
    }
    let u = ((a as f32 - HAZE_ALPHA_MIN as f32)
        / (HAZE_ALPHA_MAX - HAZE_ALPHA_MIN).max(1) as f32)
        .clamp(0.0, 1.0);
    let r = (235.0 + u * 20.0).round() as u8;
    let g = (240.0 + u * 15.0).round() as u8;
    let b = (255.0 - u * 35.0).round() as u8;
    (r, g, b, a)
}

/// Per-cell wash. Absolute mass floor cuts dry air; the span up to the
/// opacity reference remaps to the full alpha range.
///
/// Continuous alpha (not hard steps): step edges used to flicker every
/// tick as mass crossed a bucket boundary under a pumping live-max.
fn humidity_haze_alpha_cell(mass: f32, max_mass: f32, floor: f32) -> u8 {
    if mass <= HAZE_MASS_FLOOR {
        return 0;
    }
    let ref_mass = max_mass.max(HAZE_MASS_FULL);
    let norm = (mass / ref_mass).clamp(0.0, 1.0);
    // `floor` is the relative cut matching HAZE_MASS_FLOOR / ref when the
    // caller passes that; also enforce the absolute mass gate above.
    if norm <= floor {
        return 0;
    }
    let span = (1.0 - floor).max(1e-6);
    let t = ((norm - floor) / span).clamp(0.0, 1.0);
    let shaped = t.powf(HAZE_ALPHA_GAMMA);
    (HAZE_ALPHA_MIN as f32 + shaped * (HAZE_ALPHA_MAX - HAZE_ALPHA_MIN) as f32).round() as u8
}

/// Target opacity reference for the H overlay.
///
/// Fixed **255 humidity scale** (where a 10% floor is mass 25.5). Using
/// saturation-at-T (~2000–2500) as the ceiling made ordinary moist air
/// look empty; live-max used to pump the whole sky when a peak drained.
pub fn haze_alpha_ref_target(live_max: f32, temp_c: f32) -> f32 {
    let _ = (live_max, temp_c);
    HAZE_MASS_FULL
}

/// Asymmetric EMA: follow rises quickly, falls slowly — a draining peak
/// must not suddenly brighten the rest of the sky.
pub fn haze_alpha_ref_step(prev: f32, target: f32) -> f32 {
    if prev < 1.0 {
        return target;
    }
    let k = if target > prev { 0.18 } else { 0.035 };
    prev * (1.0 - k) + target * k
}

fn cloud_layer_strength(look: &AtmosphereLookConfig, layer: CloudDepthLayer) -> f32 {
    match layer {
        CloudDepthLayer::Far => look.vapour_far,
        CloudDepthLayer::Mid => look.vapour_mid,
        CloudDepthLayer::Active => look.vapour_active,
        CloudDepthLayer::Front => look.vapour_front,
    }
    .clamp(0.0, 1.0)
}

/// Gather soft lobe sources from the visual humidity echo.
///
/// Far / mid / front are parallax echoes of the same `CloudStore` — humidity
/// already owns the water; we do not paint the humidity tile raster here.
fn gather_soft_cloud_srcs(
    clouds: &CloudStore,
    _humidity: &Humidity,
    layer: CloudDepthLayer,
    _sea_level_y: i32,
    _world_seed: u64,
    downpour_mass: f32,
) -> Vec<SoftCloudSrc> {
    let (size_k, wet_k, y_bias, seed_xor, skip_mod, skip_lt) = match layer {
        CloudDepthLayer::Far => (0.72, 0.55, 14.0, 0xA11Fu32, 5u32, 2u32),
        CloudDepthLayer::Mid => (0.90, 0.75, 6.0, 0xB22E, 4, 1),
        CloudDepthLayer::Active => (1.0, 1.0, 0.0, 0, 1, 0),
        CloudDepthLayer::Front => (1.15, 0.65, -4.0, 0xC33D, 3, 1),
    };
    let mut out = Vec::with_capacity(clouds.parcels.len());

    if layer == CloudDepthLayer::Active {
        for p in &clouds.parcels {
            out.push(SoftCloudSrc {
                fx: p.fx,
                fy: p.fy,
                wet: p.wetness_with(downpour_mass),
                shape_seed: p.shape_seed,
                deform: p.deform,
                radius_cells: p.radius(),
            });
        }
        return out;
    }

    for p in &clouds.parcels {
        let seed = p.shape_seed ^ seed_xor;
        if skip_mod > 1 && (seed % skip_mod) < skip_lt {
            continue;
        }
        let phase = (seed & 0xFFFF) as f32 / 65535.0;
        out.push(SoftCloudSrc {
            fx: p.fx + (phase - 0.5) * 28.0,
            fy: p.fy + y_bias + (phase - 0.5) * 10.0,
            wet: (p.wetness_with(downpour_mass) * wet_k).clamp(0.0, 1.0),
            shape_seed: seed,
            deform: (p.deform * 0.45).clamp(0.0, 1.0),
            radius_cells: (p.radius() * size_k).clamp(5.0, 22.0),
        });
    }
    out
}

fn paint_soft_cloud_mask(
    mask: &HashMap<(i32, i32), f32>,
    cell_px: f32,
    look: &AtmosphereLookConfig,
    alpha_scale: f32,
) {
    let alpha_scale = alpha_scale.clamp(0.0, 1.0);
    if alpha_scale < 0.02 || mask.is_empty() {
        return;
    }
    let white = look.cloud_whiteness.clamp(0.0, 1.0);
    for (&(ix, iy), &wet) in mask {
        let mut n = 0u8;
        for (dx, dy) in [
            (-1, 0),
            (1, 0),
            (0, -1),
            (0, 1),
            (-1, -1),
            (1, -1),
            (-1, 1),
            (1, 1),
        ] {
            if mask.contains_key(&(ix + dx, iy + dy)) {
                n += 1;
            }
        }
        let edge = (n as f32 / 8.0).clamp(0.0, 1.0);
        let base = 165.0 + white * 70.0;
        let shade = (base - wet * (28.0 + white * 12.0)) as u8;
        let alpha =
            ((120.0 + wet * 70.0) * (0.22 + 0.78 * edge) * alpha_scale).min(210.0) as u8;
        if alpha < 10 {
            continue;
        }
        let lift = (4.0 + white * 10.0) as u8;
        draw_rectangle(
            ix as f32 * cell_px,
            iy as f32 * cell_px,
            cell_px,
            cell_px,
            Color::from_rgba(
                shade.saturating_add(lift),
                shade.saturating_add(lift.saturating_add(2)),
                shade.saturating_add(lift.saturating_add(4)),
                alpha,
            ),
        );
    }
}

/// Soft lobe cloud banks for one depth layer (never paints the humidity tile raster).
pub fn draw_depth_cloud_layer(
    clouds: &CloudStore,
    humidity: &Humidity,
    wind: &Wind,
    tick: u64,
    layer: CloudDepthLayer,
    look: &AtmosphereLookConfig,
    world_seed: u64,
    sea_level_y: i32,
    downpour_mass: f32,
    cam_x: f32,
    cam_y: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
) {
    let strength = cloud_layer_strength(look, layer);
    if strength <= 0.02 || cell_px <= 0.0 {
        return;
    }
    let srcs = gather_soft_cloud_srcs(
        clouds,
        humidity,
        layer,
        sea_level_y,
        world_seed,
        downpour_mass,
    );
    if srcs.is_empty() {
        return;
    }
    let (parallax, y_lag_scale, scroll_k) = match layer {
        CloudDepthLayer::Far => (CLOUD_FAR_PARALLAX, 0.40, 0.0),
        CloudDepthLayer::Mid => (CLOUD_MID_PARALLAX, 0.50, 0.0),
        CloudDepthLayer::Active => (CLOUD_PARALLAX, 0.55, 0.0),
        CloudDepthLayer::Front => (CLOUD_FRONT_PARALLAX, 0.20, 0.18),
    };
    let lag_x = cam_x * (1.0 - parallax);
    let lag_y = cam_y * (1.0 - parallax) * y_lag_scale;
    let scroll_x = if scroll_k > 0.0 {
        let vx = wind.effective_vx(tick);
        (get_time() as f32 * vx * cell_px * scroll_k).rem_euclid(sw + 80.0)
    } else {
        0.0
    };
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    let mut mask: HashMap<(i32, i32), f32> = HashMap::with_capacity(srcs.len() * 64);
    for src in &srcs {
        let r = src.radius_cells * cell_px;
        for &x_copy in x_copies {
            let sx = origin_x
                + (src.fx + (x_copy * width_cols) as f32) * cell_px
                + lag_x
                + scroll_x;
            let sy = origin_y - (src.fy - bedrock_floor_y as f32) * cell_px + lag_y;
            if sx + r * 2.0 < 0.0 || sx - r * 2.0 > sw || sy + r < 0.0 || sy - r > sh {
                continue;
            }
            stamp_pixel_cloud_mask(
                &mut mask,
                sx,
                sy,
                r,
                src.wet,
                src.shape_seed,
                src.deform,
                cell_px,
            );
        }
    }
    paint_soft_cloud_mask(&mask, cell_px, look, strength);
}

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    let t = t.clamp(0.0, 1.0);
    (a as f32 + (b as f32 - a as f32) * t).round() as u8
}

fn desaturate_rgb(rgb: [u8; 3], amount: f32) -> [u8; 3] {
    let amount = amount.clamp(0.0, 1.0);
    let grey = 0.30 * rgb[0] as f32 + 0.59 * rgb[1] as f32 + 0.11 * rgb[2] as f32;
    [
        lerp_u8(rgb[0], grey as u8, amount),
        lerp_u8(rgb[1], grey as u8, amount),
        lerp_u8(rgb[2], grey as u8, amount),
    ]
}

/// Distant ridge fill + sky sample for crest feathering.
///
/// Keeps sky hue (teal/warm) so plates read as atmosphere, not flat grey.
/// Body stays opaque; crest softens separately.
fn ridge_palette(
    day_night: f32,
    weather: &SkyWeatherParams,
    look: &AtmosphereLookConfig,
    near: bool,
) -> (Color, [u8; 3]) {
    // day_night: +1 noon … −1 midnight
    let day = ((day_night + 1.0) * 0.5).clamp(0.0, 1.0);
    let h = if near { 0.58 } else { 0.78 };
    let sky = sky_rgb_at_height_weather(day_night, h, weather);
    // Soft land undertone — mostly buried under sky wash.
    let land = if near {
        [
            lerp_u8(88, 118, day),
            lerp_u8(92, 122, day),
            lerp_u8(98, 128, day),
        ]
    } else {
        [
            lerp_u8(108, 142, day),
            lerp_u8(112, 146, day),
            lerp_u8(118, 152, day),
        ]
    };
    let sky_mix = if near {
        look.ridge_sky_mix_near
    } else {
        look.ridge_sky_mix_far
    }
    .clamp(0.0, 1.0);
    let mixed = [
        lerp_u8(land[0], sky[0], sky_mix),
        lerp_u8(land[1], sky[1], sky_mix),
        lerp_u8(land[2], sky[2], sky_mix),
    ];
    let sat_cut = if near {
        look.ridge_desat_near
    } else {
        look.ridge_desat_far
    }
    .clamp(0.0, 1.0);
    let [r, g, b] = desaturate_rgb(mixed, sat_cut);
    (Color::from_rgba(r, g, b, 255), sky)
}

fn sample_sky_weather(
    _tick: u64,
    _climate: &ClimateConfig,
    clouds: &CloudStore,
    humidity: &Humidity,
    temperature: &Temperature,
    carbon: &CarbonBudget,
    width_cols: i32,
    wrap_x: bool,
    sea_level_y: i32,
    downpour_mass: f32,
    snow_bias: f32,
) -> SkyWeatherParams {
    let wrap = if wrap_x { Some(width_cols) } else { None };
    let precip_cover = precip_cover_fraction(clouds, 0, width_cols, wrap, downpour_mass);
    // Sky stats: all free-air tiles, not "above sea" — dug basins count.
    let sky_hy_min = humidity.bounds.map(|b| b.hy_min).unwrap_or(0);
    let _ = sea_level_y;
    let humidity_mean = humidity_mean_norm(humidity, sky_hy_min);
    let mut t_sum = 0.0f32;
    let mut t_n = 0u32;
    let hy_min = sky_hy_min;
    for (&(_hx, hy), &temp_c) in &temperature.cells {
        if hy < hy_min {
            continue;
        }
        t_sum += temp_c;
        t_n += 1;
    }
    let mean_t = if t_n > 0 {
        t_sum / t_n as f32
    } else {
        MILD_TEMP_C
    };
    SkyWeatherParams {
        precip_cover,
        humidity_mean,
        temp_bias_c: mean_t - MILD_TEMP_C,
        carbon_ratio: carbon_ratio(carbon.atmosphere),
        snow_bias,
    }
}

/// Day/night sky gradient + weather tint (celestials drawn separately).
pub fn draw_sky(
    tick: u64,
    sw: f32,
    sh: f32,
    climate: &ClimateConfig,
    weather: &SkyWeatherParams,
    _look: &AtmosphereLookConfig,
) {
    let dn = day_night_factor_cfg(tick, climate);
    const BANDS: i32 = 36;
    for i in 0..BANDS {
        let y0 = sh * (i as f32) / BANDS as f32;
        let h = y0 + sh / BANDS as f32;
        let height_01 = (i as f32 + 0.5) / BANDS as f32;
        let [r, g, b] = sky_rgb_at_height_weather(dn, height_01, weather);
        draw_rectangle(0.0, y0, sw, h - y0 + 1.0, Color::from_rgba(r, g, b, 255));
    }

    // Soft vapour veil — humidity-led mood wash (kept light to avoid blue cast).
    let veil = (weather.humidity_mean * 0.45 + weather.precip_cover * 0.18).clamp(0.0, 0.50);
    if veil > 0.02 {
        let a = (veil * 32.0) as u8;
        draw_rectangle(0.0, 0.0, sw, sh * 0.50, Color::from_rgba(90, 96, 108, a));
    }
}

/// Soft fine-resolution sun/moon — drawn **before** ridges (behind background).
///
/// Reveal is a soft 0→1 fade as the body clears the far-ridge crest, so
/// rise/set read as gradual, not a hard pop.
pub fn draw_celestials(
    tick: u64,
    sw: f32,
    sh: f32,
    climate: &ClimateConfig,
    weather: &SkyWeatherParams,
    look: &AtmosphereLookConfig,
    ridges: &RidgeSilhouette,
    cam_x: f32,
    cam_y: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
) {
    let dn = day_night_factor_cfg(tick, climate);
    // Coarse overcast — humidity noise must not pulse celestials.
    let overcast = ((weather.humidity_mean * 0.55 + weather.precip_cover * 0.35).clamp(0.0, 1.0)
        * 4.0)
        .round()
        / 4.0;

    // Smooth sun↔moon handoff around dusk/dawn (dn ≈ 0). Separate arcs.
    let sun_w = ((dn + 0.18) / 0.36).clamp(0.0, 1.0);
    let moon_w = ((0.18 - dn) / 0.36).clamp(0.0, 1.0);

    if sun_w > 0.02 {
        let (cx, cy) = celestial_sun_screen_pos_cfg(tick, sw, sh, climate);
        let body_r = look.sun_radius.clamp(12.0, 80.0);
        // Glow must be strictly larger than the body or the slider is a no-op.
        let glow_r = look
            .sun_glow_radius
            .max(body_r + 12.0)
            .clamp(body_r + 12.0, 140.0);
        let reveal = celestial_far_reveal(
            cx,
            cy,
            body_r,
            ridges,
            cam_x,
            cam_y,
            origin_x,
            origin_y,
            cell_px,
            bedrock_floor_y,
            wrap_x,
            width_cols,
            sh,
        );
        let w = sun_w * reveal;
        if w > 0.02 {
            let core_a = (255.0 * w * (1.0 - 0.45 * overcast)) as u8;
            let glow_peak = (110.0 * w * (1.0 - 0.50 * overcast)) as u8;
            // Amber halo only outside the body — this is what glow radius controls.
            draw_sun_glow(cx, cy, body_r, glow_r, glow_peak);
            if core_a > 16 {
                draw_circle(cx, cy, body_r, Color::from_rgba(255, 214, 90, core_a));
                draw_circle(
                    cx,
                    cy,
                    body_r * 0.45,
                    Color::from_rgba(255, 248, 210, core_a),
                );
            }
        }
    }

    if moon_w > 0.02 {
        let (cx, cy) = celestial_moon_screen_pos_cfg(tick, sw, sh, climate);
        let moon_r = look.moon_radius.clamp(10.0, 72.0);
        let reveal = celestial_far_reveal(
            cx,
            cy,
            moon_r,
            ridges,
            cam_x,
            cam_y,
            origin_x,
            origin_y,
            cell_px,
            bedrock_floor_y,
            wrap_x,
            width_cols,
            sh,
        );
        // Snap reveal so ridge-cache jitter can't pulse the moon.
        let reveal_q = ((reveal * 10.0).round() / 10.0).clamp(0.0, 1.0);
        let w = moon_w * reveal_q;
        if w > 0.02 {
            // Flat alpha — no overcast/glow disk (that was the dark “bubble”).
            let a = (255.0 * w).round() as u8;
            let bite_dx = look.moon_bite_offset.clamp(4.0, 40.0);
            let bite_r = look.moon_bite_radius.clamp(8.0, 64.0);
            draw_solid_crescent(
                cx,
                cy,
                moon_r,
                bite_dx,
                0.0,
                bite_r,
                Color::from_rgba(232, 236, 245, a),
            );
        }
    }
}

/// Warm amber halo from `body_r` out to `glow_r` (glow slider actually changes size).
fn draw_sun_glow(cx: f32, cy: f32, body_r: f32, glow_r: f32, peak_a: u8) {
    if glow_r <= body_r + 1.0 || peak_a < 4 {
        return;
    }
    const RINGS: i32 = 5;
    let span = glow_r - body_r;
    for i in 0..RINGS {
        let t = i as f32 / (RINGS - 1) as f32; // 0 outer → 1 near body
        let rr = glow_r - span * t;
        // Outer faint, denser near the limb.
        let a = (peak_a as f32 * (0.12 + 0.55 * t)).round() as u8;
        if a < 3 {
            continue;
        }
        draw_circle(cx, cy, rr, Color::from_rgba(255, 160, 50, a));
    }
}

/// 0 = fully behind far ridge, 1 = clear above it (soft band ~ body radius).
fn celestial_far_reveal(
    cx: f32,
    cy: f32,
    body_r: f32,
    ridges: &RidgeSilhouette,
    cam_x: f32,
    cam_y: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sh: f32,
) -> f32 {
    let band = (body_r * 1.6).max(10.0);
    let crest_sy = if ridges.far.is_empty() || cell_px <= 0.0 || width_cols <= 0 {
        sh * 0.72
    } else {
        let lag_x = cam_x * (1.0 - FAR_RIDGE_PARALLAX);
        let lag_y = cam_y * (1.0 - FAR_RIDGE_PARALLAX);
        let col = ((cx - origin_x - lag_x) / cell_px).floor() as i32;
        let idx = if wrap_x {
            col.rem_euclid(width_cols) as usize
        } else if col >= 0 && col < width_cols {
            col as usize
        } else {
            return if cy > sh * 0.78 { 0.0 } else { 1.0 };
        };
        let surf = ridges.far[idx.min(ridges.far.len() - 1)];
        let y_vis = bedrock_floor_y + ((surf - bedrock_floor_y) as f32 * 0.78) as i32;
        origin_y - (y_vis - bedrock_floor_y) as f32 * cell_px + lag_y
    };
    // Positive clearance = centre above crest (smaller screen y).
    let clearance = crest_sy - cy;
    // Fade in across `band` pixels as the body rises clear of the ridge.
    smooth01((clearance + band * 0.15) / band)
}

fn smooth01(t: f32) -> f32 {
    let t = t.clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// Flat-alpha crescent (no soft rim / no under-disk — those pulsed and stained the sky).
fn draw_solid_crescent(
    cx: f32,
    cy: f32,
    radius: f32,
    bite_dx: f32,
    bite_dy: f32,
    bite_radius: f32,
    color: Color,
) {
    if radius <= 0.5 || color.a <= 0.0 {
        return;
    }
    let step = 1.0_f32;
    let r2 = (radius + 0.35).powi(2);
    let b2 = (bite_radius + 0.15).powi(2);
    let a = (color.a * 255.0) as u8;
    let r = (color.r * 255.0) as u8;
    let g = (color.g * 255.0) as u8;
    let b = (color.b * 255.0) as u8;
    let n = (radius.ceil() as i32) + 1;
    for dy in -n..=n {
        for dx in -n..=n {
            let fx = dx as f32 * step;
            let fy = dy as f32 * step;
            if fx * fx + fy * fy > r2 {
                continue;
            }
            let bx = fx - bite_dx;
            let by = fy - bite_dy;
            if bx * bx + by * by <= b2 {
                continue;
            }
            draw_rectangle(
                cx + fx - 0.5,
                cy + fy - 0.5,
                1.0,
                1.0,
                Color::from_rgba(r, g, b, a),
            );
        }
    }
}

/// Draw far then near surface-echo ridges behind terrain.
///
/// Mid-ground (`near`) body is opaque so the far plate cannot show through.
/// Crest is an opaque color fade — toward sky, or toward the far plate when
/// that plate sits behind the mid crest (dusk/dawn polish).
pub fn draw_ridge_silhouettes(
    ridges: &RidgeSilhouette,
    day_night: f32,
    weather: &SkyWeatherParams,
    look: &AtmosphereLookConfig,
    cam_x: f32,
    cam_y: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    sea_level_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
) {
    if ridges.far.is_empty() {
        return;
    }
    let (far_fill, far_sky) = ridge_palette(day_night, weather, look, false);
    let (near_fill, near_sky) = ridge_palette(day_night, weather, look, true);
    let far_feather = look.ridge_feather_far.round().clamp(2.0, 10.0) as i32;
    let near_feather = look.ridge_feather_near.round().clamp(2.0, 10.0) as i32;
    draw_ridge_band(
        &ridges.far,
        None,
        cam_x,
        cam_y,
        FAR_RIDGE_PARALLAX,
        origin_x,
        origin_y,
        cell_px,
        bedrock_floor_y,
        sea_level_y,
        wrap_x,
        width_cols,
        sw,
        sh,
        far_fill,
        far_sky,
        0.78,
        far_feather,
        look.ridge_crest_blend,
        0.0,
    );
    let far_rgb = [
        (far_fill.r * 255.0) as u8,
        (far_fill.g * 255.0) as u8,
        (far_fill.b * 255.0) as u8,
    ];
    draw_ridge_band(
        &ridges.near,
        Some(RidgeBehind {
            profile: &ridges.far,
            parallax: FAR_RIDGE_PARALLAX,
            y_squash: 0.78,
            fill_rgb: far_rgb,
        }),
        cam_x,
        cam_y,
        NEAR_RIDGE_PARALLAX,
        origin_x,
        origin_y,
        cell_px,
        bedrock_floor_y,
        sea_level_y,
        wrap_x,
        width_cols,
        sw,
        sh,
        near_fill,
        near_sky,
        0.90,
        near_feather,
        look.ridge_crest_blend,
        look.ridge_far_into_crest,
    );
}

struct RidgeBehind<'a> {
    profile: &'a [i32],
    parallax: f32,
    y_squash: f32,
    fill_rgb: [u8; 3],
}

fn draw_ridge_band(
    profile: &[i32],
    behind: Option<RidgeBehind<'_>>,
    cam_x: f32,
    cam_y: f32,
    parallax: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    sea_level_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
    fill: Color,
    sky_rgb: [u8; 3],
    y_squash: f32,
    feather_cells: i32,
    crest_blend: f32,
    far_into_crest: f32,
) {
    // Lag opposite camera so distant layers move slower in X and Y.
    let lag_x = cam_x * (1.0 - parallax);
    let lag_y = cam_y * (1.0 - parallax);
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    let _ = sea_level_y;
    let n = profile.len() as i32;
    if n <= 0 {
        return;
    }
    let feather = feather_cells.max(2);
    let feather_px = feather as f32 * cell_px;
    let crest_w = crest_blend.clamp(0.0, 1.0);
    let far_w = far_into_crest.clamp(0.0, 1.0);

    for &x_copy in x_copies {
        for (i, &surf_y) in profile.iter().enumerate() {
            let x = i as i32 + x_copy * width_cols;
            let sx = origin_x + x as f32 * cell_px + lag_x;
            if sx + cell_px < 0.0 || sx > sw {
                continue;
            }
            // Neighbour blend softens the jagged crest line.
            let i0 = i as i32;
            let yl = profile[((i0 - 1).rem_euclid(n)) as usize];
            let yr = profile[((i0 + 1).rem_euclid(n)) as usize];
            let surf_soft = ((surf_y as i64 + yl as i64 + yr as i64) / 3) as i32;
            let y_vis = bedrock_floor_y
                + ((surf_soft - bedrock_floor_y) as f32 * y_squash) as i32;
            let top_sy = origin_y - (y_vis - bedrock_floor_y) as f32 * cell_px + lag_y;
            // Extend solid fill to the bottom of the viewport. Stopping at
            // `origin_y + lag_y` (parallax-lagged bedrock line) left a hard
            // transparency shelf — visible through dug holes, a dropped
            // ocean, or heatmap-only / no-landscape.
            let bottom_sy = sh;
            if top_sy > sh {
                continue;
            }

            // Opaque body below the feather zone (mid-ground stays solid).
            let body_top = (top_sy + feather_px).min(bottom_sy);
            let body_h = (bottom_sy - body_top).max(0.0);
            if body_h > 0.5 {
                draw_rectangle(sx, body_top, cell_px + 0.5, body_h, fill);
            }

            // Crest target: sky, or far-plate color when that plate rises behind us.
            let mut edge_rgb = sky_rgb;
            if let Some(ref b) = behind {
                let bn = b.profile.len() as i32;
                if bn > 0 {
                    let bi = i0.rem_euclid(bn) as usize;
                    let b_surf = b.profile[bi];
                    let b_y = bedrock_floor_y
                        + ((b_surf - bedrock_floor_y) as f32 * b.y_squash) as i32;
                    let b_lag_y = cam_y * (1.0 - b.parallax);
                    let b_top = origin_y - (b_y - bedrock_floor_y) as f32 * cell_px + b_lag_y;
                    // Far crest higher on screen (smaller sy) → far body is behind mid tip.
                    if b_top + cell_px < top_sy + feather_px {
                        edge_rgb = [
                            lerp_u8(sky_rgb[0], b.fill_rgb[0], far_w),
                            lerp_u8(sky_rgb[1], b.fill_rgb[1], far_w),
                            lerp_u8(sky_rgb[2], b.fill_rgb[2], far_w),
                        ];
                    }
                }
            }

            let fr = (fill.r * 255.0) as u8;
            let fg = (fill.g * 255.0) as u8;
            let fb = (fill.b * 255.0) as u8;
            for k in 0..feather {
                let t = (k as f32 + 0.5) / feather as f32; // 0 tip → 1 body
                let w = (1.0 - t) * crest_w;
                let rgb = [
                    lerp_u8(fr, edge_rgb[0], w),
                    lerp_u8(fg, edge_rgb[1], w),
                    lerp_u8(fb, edge_rgb[2], w),
                ];
                let y0 = top_sy + k as f32 * cell_px;
                if y0 > sh || y0 + cell_px < 0.0 {
                    continue;
                }
                draw_rectangle(
                    sx,
                    y0,
                    cell_px + 0.5,
                    cell_px + 0.5,
                    Color::from_rgba(rgb[0], rgb[1], rgb[2], 255),
                );
            }
        }
    }
}

/// Humidity tile diagnostic (front of terrain).
///
/// Occupied 4×4 seats plus a one-tile neighbour halo (wrapped). A drop
/// masks that column from itself downward (1-wide). Cells that remain
/// bilinear-sample the store so tile edges do not read as a clamp.
/// Soft cloud banks are separate (`N` / [`draw_depth_cloud_layer`]).
/// Wind streaks are a separate overlay ([`draw_wind_streaks`], `V`).
pub fn draw_haze_and_wind(
    humidity: &Humidity,
    world: &World,
    wind: &Wind,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    sea_level_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
    alpha_ref: f32,
) {
    if humidity.cells.is_empty() || cell_px <= 0.0 {
        return;
    }
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    let ref_mass = alpha_ref.max(1.0);
    let tc = humidity.tile_cols.max(1);
    // Do **not** clamp seats to sea level — that painted a hard horizontal
    // shelf and hid vapor over dug lakes below sea. Per-column cloud floor
    // is the only bottom clip.
    let sky_hy_min = humidity.bounds.map(|b| b.hy_min).unwrap_or(0);
    let _ = (tc, sea_level_y);
    let drop_tops = collect_drop_tops(world);
    let seats = haze_paint_seats(humidity, sky_hy_min);
    let mut floor_cache: HashMap<i32, i32> = HashMap::new();

    // Dedupe world cells first. Semi-transparent `cell_px + 0.5` overdraw
    // used to stack at every column edge and read as vertical corduroy
    // banding across an otherwise smooth field.
    let mut cells: HashMap<(i32, i32), f32> = HashMap::new();
    for (hx, hy) in seats {
        for (wx, gy, sampled) in haze_resampled_cells(
            humidity,
            hx,
            hy,
            &drop_tops,
            |x| world.wrap_x(x),
            |wx| {
                *floor_cache.entry(wx).or_insert_with(|| {
                    cloud_floor_y(world, wind, wx as f32).round() as i32
                })
            },
        ) {
            cells
                .entry((wx, gy))
                .and_modify(|m| *m = (*m).max(sampled))
                .or_insert(sampled);
        }
    }

    let floor = HAZE_MASS_FLOOR / HAZE_MASS_FULL;
    for (&(wx, gy), &sampled) in &cells {
        let (hr, hg, hb, cell_alpha) = humidity_haze_style(sampled, ref_mass, floor);
        if cell_alpha == 0 {
            continue;
        }
        for &x_copy in x_copies {
            // Pixel-snapped edges so neighbouring cells share a boundary
            // without translucent overlap (the corduroy bands).
            let x0 = origin_x + (wx + x_copy * width_cols) as f32 * cell_px;
            let x1 = origin_x + (wx + 1 + x_copy * width_cols) as f32 * cell_px;
            let y1 = origin_y - (gy - bedrock_floor_y) as f32 * cell_px;
            let y0 = origin_y - (gy + 1 - bedrock_floor_y) as f32 * cell_px;
            let sx = x0.floor();
            let sy = y0.floor();
            let sw_cell = (x1.floor() - sx).max(1.0);
            let sh_cell = (y1.floor() - sy).max(1.0);
            if sx + sw_cell < 0.0 || sx > sw || sy > sh || sy + sh_cell < 0.0 {
                continue;
            }
            draw_rectangle(
                sx,
                sy,
                sw_cell,
                sh_cell,
                Color::from_rgba(hr, hg, hb, cell_alpha),
            );
        }
    }
}

/// Sparse screen-space wind strokes (`V` overlay).
///
/// Placeholder: 1-px hairlines that do not read speed or direction well.
/// Kept off `H` so the humidity field stays clean. Needs a real visual.
pub fn draw_wind_streaks(wind: &Wind, tick: u64, sw: f32, sh: f32) {
    let vx = wind.effective_vx(tick);
    if vx.abs() < 0.008 {
        return;
    }
    let t = get_time() as f32;
    let n = ((sw / 36.0).ceil() as i32).clamp(8, 28);
    let drift = (t * vx * 40.0).rem_euclid(sw + 40.0);
    for i in 0..n {
        let base = (i as f32 / n as f32) * (sw + 40.0) - 20.0;
        let x = (base + drift).rem_euclid(sw + 40.0) - 20.0;
        let y = sh * (0.14 + 0.32 * ((i as f32 * 0.37) % 1.0));
        let len = 12.0 + (i % 5) as f32 * 4.0;
        draw_rectangle(x, y, len, 1.0, Color::from_rgba(230, 236, 245, 16));
    }
}

pub fn draw_canopy_air_dim(
    world: &World,
    organisms: &OrganismStore,
    clouds: &CloudStore,
    tick: u64,
    wind_vx: f32,
    celestial_local: f32,
    celestial_sx: f32,
    _celestial_sy: f32,
    is_day: bool,
    downpour_mass: f32,
    look: &AtmosphereLookConfig,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    y_min_vis: i32,
    y_max_vis: i32,
    sw: f32,
    sh: f32,
) {
    let _ = (sw, sh); // frustum already applied by caller via y_min/y_max
    let posed = resolve_organism_draw_cells(world, &organisms.atoms, tick, wind_vx);
    let canopy = build_canopy_index_posed(&organisms.atoms, &posed);
    let wrap = if wrap_x { Some(width_cols) } else { None };
    let cast_k = look.cast_shadow_strength.clamp(0.0, 1.0);
    let cloud_k = look.cloud_shade_strength.clamp(0.0, 1.0);

    let mut shade: HashMap<(i32, i32), f32> = HashMap::new();

    // 1) Day: mild ground/water dim under foliage.
    if is_day && !canopy.is_empty() {
        stamp_under_canopy_surface(world, &canopy, width_cols, y_min_vis, y_max_vis, &mut shade);
    }

    // 2) Time-of-day cast (pan-invariant): soft air corridor + exposed ground.
    if cast_k > 0.02 && !posed.is_empty() {
        let _ = (celestial_sx, origin_x, cell_px);
        let strength = if is_day {
            (cast_k * 0.90).clamp(0.0, 1.0)
        } else {
            (cast_k * 1.10).clamp(0.0, 1.0)
        };
        stamp_celestial_cast_shadows(
            world,
            &posed,
            strength,
            is_day,
            y_min_vis,
            y_max_vis,
            &mut shade,
            celestial_local,
        );
    }

    // 3) Mild cloud dim on exposed surface (day + night).
    if cloud_k > 0.02 && !clouds.is_empty() {
        for x in 0..width_cols {
            let ct = cloud_sky_transmit(clouds, x, wrap, downpour_mass);
            let dim = ((1.0 - ct) * cloud_k * 0.50).clamp(0.0, 0.35);
            if dim < 0.04 {
                continue;
            }
            if let Some(y) = top_shadow_receiver_y(world, x, y_min_vis, y_max_vis) {
                let e = shade.entry((x, y)).or_insert(0.0);
                *e = (*e + dim).min(0.70);
            }
        }
    }

    if shade.is_empty() {
        return;
    }
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    // Keep ground lee readable; air stamps are already capped softer upstream.
    let alpha_k = if is_day { 100.0 } else { 190.0 };
    let alpha_max = if is_day { 125.0 } else { 220.0 };
    for (&(x, y), &dim) in &shade {
        if dim < 0.04 {
            continue;
        }
        let a = (dim * alpha_k).clamp(0.0, alpha_max) as u8;
        for &x_copy in x_copies {
            let sx = origin_x + (x + x_copy * width_cols) as f32 * cell_px;
            let sy = origin_y - (y - bedrock_floor_y) as f32 * cell_px;
            if sx + cell_px < 0.0 || sx > sw || sy + cell_px < 0.0 || sy > sh {
                continue;
            }
            draw_rectangle(
                sx,
                sy - cell_px,
                cell_px,
                cell_px,
                Color::from_rgba(6, 8, 14, a),
            );
        }
    }
}

fn is_shadow_receiver(world: &World, x: i32, y: i32) -> bool {
    let Some(cell) = world.get_cell(x, y) else {
        return false;
    };
    if cell.material == MaterialId::Water {
        return true;
    }
    if cell.material != MaterialId::Air {
        return true;
    }
    is_standing_water(world, x, y)
}

/// How open a cell is toward the active celestial (1 = lit, 0 = occluded).
///
/// Short diagonal probe toward sun/moon — used for terrain-top and plant key.
pub fn celestial_exposure(world: &World, x: i32, y: i32, celestial_local: f32) -> f32 {
    let local = celestial_local.clamp(0.0, 1.0);
    let elev = (local * std::f32::consts::PI).sin().clamp(0.05, 1.0);
    let toward = if local < 0.5 { -1.0 } else { 1.0 };
    let horiz = ((1.0 - elev) / elev).clamp(0.0, 1.45);
    let max_steps = 7i32;
    let mut fx = x as f32;
    let mut yy = y;
    for step in 1..=max_steps {
        yy += 1;
        fx += toward * horiz;
        let ix = world.wrap_x(fx.round() as i32);
        if is_shadow_receiver(world, ix, yy) {
            // Soft occlusion — near blockers still leak a little ambient.
            return ((step as f32 - 1.0) / max_steps as f32 * 0.20).clamp(0.0, 0.20);
        }
    }
    1.0
}

/// Warm sun highlight or cool moon glow mixed into an RGB sample.
pub fn apply_celestial_key_rgb(
    rgb: [u8; 3],
    exposure: f32,
    celestial_local: f32,
    is_day: bool,
) -> [u8; 3] {
    let elev = (celestial_local.clamp(0.0, 1.0) * std::f32::consts::PI)
        .sin()
        .clamp(0.0, 1.0);
    // Horizon bodies contribute less key; apex more.
    let elev_k = (0.35 + 0.65 * elev).clamp(0.35, 1.0);
    let e = (exposure.clamp(0.0, 1.0) * elev_k).clamp(0.0, 1.0);
    if e < 0.03 {
        return rgb;
    }
    if is_day {
        // Warm sun highlight — readable on dark stems.
        let k = 0.38 * e;
        let warm = [255.0f32, 228.0, 150.0];
        let lift = 14.0 * e;
        [
            (rgb[0] as f32 * (1.0 - k) + warm[0] * k + lift).clamp(0.0, 255.0) as u8,
            (rgb[1] as f32 * (1.0 - k) + warm[1] * k + lift * 0.85).clamp(0.0, 255.0) as u8,
            (rgb[2] as f32 * (1.0 - k) + warm[2] * k + lift * 0.45).clamp(0.0, 255.0) as u8,
        ]
    } else {
        // Soft cool moon glow on rims / light-facing edges.
        let k = 0.30 * e;
        let cool = [155.0f32, 180.0, 245.0];
        let lift = 12.0 * e;
        [
            (rgb[0] as f32 * (1.0 - k) + cool[0] * k + lift).clamp(0.0, 255.0) as u8,
            (rgb[1] as f32 * (1.0 - k) + cool[1] * k + lift).clamp(0.0, 255.0) as u8,
            (rgb[2] as f32 * (1.0 - k) + cool[2] * k + lift * 1.15).clamp(0.0, 255.0) as u8,
        ]
    }
}

/// True when this cell is an exposed surface top (air / empty above).
pub fn is_exposed_surface_top(world: &World, x: i32, y: i32) -> bool {
    if !is_shadow_receiver(world, x, y) {
        return false;
    }
    match world.get_cell(x, y + 1) {
        None => true,
        Some(c) => c.material == MaterialId::Air && !is_standing_water(world, x, y + 1),
    }
}

fn is_waterish(world: &World, x: i32, y: i32) -> bool {
    let Some(cell) = world.get_cell(x, y) else {
        return false;
    };
    cell.material == MaterialId::Water || is_standing_water(world, x, y)
}

/// Key strength for terrain/water: full on the lit crest, soft bleed a few
/// cells into the layer beneath (deeper into water than rock).
pub fn terrain_celestial_key_strength(
    world: &World,
    x: i32,
    y: i32,
    celestial_local: f32,
    is_day: bool,
) -> f32 {
    if !is_shadow_receiver(world, x, y) {
        return 0.0;
    }
    // Climb to the top of this solid/water stack.
    //
    // Stop one past the deepest bleed any material gets (`max_bleed` below is
    // never more than 3): if the stack continues that far above, this cell is
    // buried and the answer is 0 regardless of where the real top is. Buried
    // cells are the overwhelming majority of a drawn frame, and this runs per
    // cell per frame, so the old 10-cell climb plus surface/water probes was
    // ~14 `get_cell` calls each to return 0.
    const MAX_BLEED_ANY: i32 = 3;
    let mut top = y;
    for _ in 0..=MAX_BLEED_ANY {
        if is_shadow_receiver(world, x, top + 1) {
            top += 1;
        } else {
            break;
        }
    }
    let depth = top - y;
    if depth > MAX_BLEED_ANY {
        return 0.0;
    }
    if !is_exposed_surface_top(world, x, top) {
        return 0.0;
    }
    let water = is_waterish(world, x, top) || is_waterish(world, x, y);
    let max_bleed = if water {
        if is_day {
            2
        } else {
            3
        }
    } else if is_day {
        1
    } else {
        2
    };
    if depth < 0 || depth > max_bleed {
        return 0.0;
    }
    let falloff = if water {
        match depth {
            0 => 1.0,
            1 => 0.55,
            2 => 0.30,
            3 => 0.14,
            _ => 0.0,
        }
    } else {
        match depth {
            0 => 1.0,
            1 => {
                if is_day {
                    0.32
                } else {
                    0.48
                }
            }
            2 => 0.20,
            _ => 0.0,
        }
    };
    celestial_exposure(world, x, top, celestial_local) * falloff
}

/// True when an organism draw cell sits in free air above the local surface.
pub fn is_organism_aboveground(world: &World, x: i32, y: i32) -> bool {
    match world.get_cell(x, y) {
        Some(c) if c.material == MaterialId::Air && !is_standing_water(world, x, y) => {}
        None => {}
        _ => return false, // solid / water = buried (roots)
    }
    // Nearest solid/water at or below this cell — must be strictly below.
    for sy in (y.saturating_sub(64)..=y).rev() {
        if is_shadow_receiver(world, x, sy) {
            return y > sy;
        }
    }
    true
}

/// World −1/+1 toward the active celestial from time-of-day (pan-invariant).
///
/// Morning (`local < 0.5`): light on the left → −1. Evening: light on the right → +1.
/// Do **not** compare plant screen-X to the decorative sun — that flips when panning.
pub fn toward_light_celestial(celestial_local: f32) -> i32 {
    if celestial_local < 0.5 {
        -1
    } else {
        1
    }
}

/// Organism rim facing the sun/moon (`toward_light` from [`toward_light_celestial`]).
///
/// Thin 1-wide stems have empty neighbors on both sides — do **not** treat that
/// as a lit face (it washed the whole stalk). Key only crowns / true lit edges.
pub fn organism_celestial_rim(
    occupied: &std::collections::HashSet<(i32, i32)>,
    x: i32,
    y: i32,
    toward_light: i32,
    celestial_local: f32,
    is_day: bool,
) -> f32 {
    let elev = (celestial_local.clamp(0.0, 1.0) * std::f32::consts::PI)
        .sin()
        .clamp(0.0, 1.0);
    let low_angle = (1.0 - elev).clamp(0.0, 1.0);
    let tl = if toward_light >= 0 { 1 } else { -1 };
    let open_up = !occupied.contains(&(x, y + 1));
    let open_light_diag = !occupied.contains(&(x + tl, y + 1));
    let open_light_side = !occupied.contains(&(x + tl, y));
    // Lit face of a wider silhouette (body continues on the lee side).
    let lit_edge = open_light_side && occupied.contains(&(x - tl, y));

    let mut best = 0.0f32;
    if is_day {
        if open_up {
            // Crown / tip — strongest when sky is open toward the sun.
            best = best.max(if open_light_diag { 1.0 } else { 0.70 });
        }
        if lit_edge {
            best = best.max((0.35 + 0.55 * low_angle).clamp(0.35, 0.90));
        }
    } else {
        if open_up {
            best = best.max(if open_light_diag { 0.90 } else { 0.50 });
        }
        if lit_edge {
            best = best.max((0.30 + 0.50 * low_angle).clamp(0.30, 0.80));
        }
    }
    best
}

/// Warm/cool key for organism rims (tips + lit edges only).
pub fn apply_organism_celestial_key_rgb(
    rgb: [u8; 3],
    exposure: f32,
    celestial_local: f32,
    is_day: bool,
) -> [u8; 3] {
    let elev = (celestial_local.clamp(0.0, 1.0) * std::f32::consts::PI)
        .sin()
        .clamp(0.0, 1.0);
    let elev_k = (0.40 + 0.60 * elev).clamp(0.40, 1.0);
    let e = (exposure.clamp(0.0, 1.0) * elev_k).clamp(0.0, 1.0);
    if e < 0.03 {
        return rgb;
    }
    if is_day {
        let k = 0.40 * e;
        let warm = [255.0f32, 228.0, 150.0];
        let lift = 20.0 * e;
        [
            (rgb[0] as f32 * (1.0 - k) + warm[0] * k + lift).clamp(0.0, 255.0) as u8,
            (rgb[1] as f32 * (1.0 - k) + warm[1] * k + lift * 0.85).clamp(0.0, 255.0) as u8,
            (rgb[2] as f32 * (1.0 - k) + warm[2] * k + lift * 0.40).clamp(0.0, 255.0) as u8,
        ]
    } else {
        let k = 0.34 * e;
        let cool = [160.0f32, 185.0, 245.0];
        let lift = 16.0 * e;
        [
            (rgb[0] as f32 * (1.0 - k) + cool[0] * k + lift).clamp(0.0, 255.0) as u8,
            (rgb[1] as f32 * (1.0 - k) + cool[1] * k + lift).clamp(0.0, 255.0) as u8,
            (rgb[2] as f32 * (1.0 - k) + cool[2] * k + lift * 1.15).clamp(0.0, 255.0) as u8,
        ]
    }
}

fn top_shadow_receiver_y(world: &World, x: i32, y_min: i32, y_max: i32) -> Option<i32> {
    for y in (y_min..y_max).rev() {
        if is_shadow_receiver(world, x, y) {
            return Some(y);
        }
    }
    None
}

/// Dim terrain/water directly under canopy columns.
fn stamp_under_canopy_surface(
    world: &World,
    canopy: &CanopyIndex,
    width_cols: i32,
    y_min_vis: i32,
    y_max_vis: i32,
    shade: &mut HashMap<(i32, i32), f32>,
) {
    for x in 0..width_cols {
        let Some(y) = top_shadow_receiver_y(world, x, y_min_vis, y_max_vis) else {
            continue;
        };
        let t = shade_transmit_column(canopy, x, y);
        // Contact dim under foliage (zenith relies on this when casts are short).
        let dim = ((1.0 - t) * 0.40).clamp(0.0, 0.32);
        if dim >= 0.04 {
            let e = shade.entry((x, y)).or_insert(0.0);
            *e = (*e).max(dim);
        }
    }
}

fn is_dry_air(world: &World, x: i32, y: i32) -> bool {
    match world.get_cell(x, y) {
        Some(c) => c.material == MaterialId::Air && !is_standing_water(world, x, y),
        None => false,
    }
}

/// Project foliage away from the sun/moon using **time-of-day**, not screen X.
///
/// Pan-invariant: decorative sun stays fixed on screen while terrain scrolls, so
/// comparing plant_sx to celestial_sx flipped lees when the camera moved.
///
/// Leaves stamp a soft air corridor + ground lee; stems stamp ground only.
fn stamp_celestial_cast_shadows(
    world: &World,
    posed: &[PosedModule],
    strength: f32,
    is_day: bool,
    y_min_vis: i32,
    y_max_vis: i32,
    shade: &mut HashMap<(i32, i32), f32>,
    celestial_local: f32,
) {
    let local = celestial_local.clamp(0.0, 1.0);
    let elev = (local * std::f32::consts::PI).sin().clamp(0.05, 1.0);
    let slant = (1.0 - elev).clamp(0.0, 1.0);
    // −1 morning (light left) … +1 evening (light right).
    let azimuth = (local - 0.5) * 2.0;
    let shadow_dir = if azimuth >= 0.0 { -1.0 } else { 1.0 };
    let mut horiz = if elev < 0.08 {
        1.45
    } else {
        ((1.0 - elev) / elev).clamp(0.0, 1.55)
    };
    // Keep a readable lean when the body is clearly off-zenith (high afternoon).
    horiz = horiz.max(azimuth.abs() * 0.65).min(1.70);
    // Soft, light air corridor — ground lee stays stronger.
    let air_k = if is_day { 0.22 } else { 0.30 };
    let air_cap = if is_day { 0.24 } else { 0.32 };

    for p in posed {
        if !matches!(p.mid, ModuleId::Photosystem | ModuleId::Stem) {
            continue;
        }
        let is_leaf = p.mid == ModuleId::Photosystem;
        let caster = if is_leaf { 1.0 } else { 0.62 };

        // Reach must cover plant height — slant-only caps left tall plants unshadowed.
        let local_surf = top_shadow_receiver_y(world, p.wx, y_min_vis, y_max_vis)
            .unwrap_or(p.wy.saturating_sub(1));
        let height = (p.wy - local_surf).max(0);
        if height == 0 {
            if let Some(surf) = top_shadow_receiver_y(world, p.wx, y_min_vis, y_max_vis) {
                if is_exposed_surface_top(world, p.wx, surf) {
                    let dim = (strength * caster * 0.40).clamp(0.0, 0.70);
                    let e = shade.entry((p.wx, surf)).or_insert(0.0);
                    *e = (*e).max(dim);
                }
            }
            continue;
        }
        let max_steps = (height + 2 + (slant * 14.0).round() as i32).clamp(4, 40);
        let noon_floor = if elev > 0.85 {
            strength * caster * 0.28
        } else {
            0.0
        };

        for step in 1..=max_steps {
            let ray_y = p.wy - step;
            if ray_y < y_min_vis {
                break;
            }
            let ix = world.wrap_x((p.wx as f32 + shadow_dir * horiz * step as f32).round() as i32);
            let Some(surf) = top_shadow_receiver_y(world, ix, y_min_vis, y_max_vis) else {
                continue;
            };
            let falloff = 1.0 - (step as f32 - 1.0) / max_steps as f32;
            let dim = (strength * caster * (0.40 + 0.60 * falloff))
                .max(noon_floor)
                .clamp(0.0, 0.90);

            // Soft air corridor (leaves only) — core + 1-cell penumbra.
            if ray_y > surf {
                if is_leaf && is_dry_air(world, ix, ray_y) {
                    let air_dim = (dim * air_k).clamp(0.0, air_cap);
                    if air_dim >= 0.03 {
                        let e = shade.entry((ix, ray_y)).or_insert(0.0);
                        *e = (*e).max(air_dim);
                        for &ddx in &[-1i32, 1] {
                            let nx = world.wrap_x(ix + ddx);
                            if let Some(ns) = top_shadow_receiver_y(world, nx, y_min_vis, y_max_vis)
                            {
                                if ray_y > ns && is_dry_air(world, nx, ray_y) {
                                    let e = shade.entry((nx, ray_y)).or_insert(0.0);
                                    *e = (*e).max(air_dim * 0.38);
                                }
                            }
                        }
                    }
                }
                continue;
            }

            // Ground lee, then stop (never stamp underground).
            if is_exposed_surface_top(world, ix, surf) {
                let e = shade.entry((ix, surf)).or_insert(0.0);
                *e = (*e).max(dim);
                let nx = world.wrap_x(ix + shadow_dir as i32);
                if let Some(ns) = top_shadow_receiver_y(world, nx, y_min_vis, y_max_vis) {
                    if is_exposed_surface_top(world, nx, ns) {
                        let e = shade.entry((nx, ns)).or_insert(0.0);
                        *e = (*e).max(dim * 0.50);
                    }
                }
            }
            break;
        }
    }
}

/// Active-layer soft parcel banks + precip streaks (humidity echo in CloudStore).
///
/// Banks use lobe masks; precip streaks are world-aligned so they clip on ground.
pub fn draw_clouds(
    clouds: &CloudStore,
    humidity: &Humidity,
    world: &World,
    wind: &Wind,
    tick: u64,
    cam_x: f32,
    cam_y: f32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    sea_level_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
    downpour_mass: f32,
    look: &AtmosphereLookConfig,
    world_seed: u64,
    snowing: impl Fn(f32, f32) -> bool,
) {
    if cell_px <= 0.0 {
        return;
    }
    draw_depth_cloud_layer(
        clouds,
        humidity,
        wind,
        tick,
        CloudDepthLayer::Active,
        look,
        world_seed,
        sea_level_y,
        downpour_mass,
        cam_x,
        cam_y,
        origin_x,
        origin_y,
        cell_px,
        bedrock_floor_y,
        wrap_x,
        width_cols,
        sw,
        sh,
    );

    if clouds.is_empty() {
        return;
    }
    let lag_x = cam_x * (1.0 - CLOUD_PARALLAX);
    let lag_y = cam_y * (1.0 - CLOUD_PARALLAX) * 0.55;
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };

    // No precip streaks: rain is real water in the grid now, nucleated in the
    // air and drawn by the terrain pass as it falls. The streaks existed because
    // rain teleported to the ground and something had to stand in for the fall;
    // drawing both would show every shower twice.
}


fn cloud_lobe_layout(shape_seed: u32, deform: f32) -> (f32, f32, Vec<(f32, f32, f32)>) {
    let d = deform.clamp(0.0, 1.0);
    // Ridge scrape: wider, flatter.
    let sx = 1.0 + d * 0.38;
    let sy = 1.0 - d * 0.40;
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
pub fn stamp_pixel_cloud_mask(
    mask: &mut HashMap<(i32, i32), f32>,
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

/// Build weather params + snow bias for the current scene view.
pub fn sky_weather_for_scene(
    tick: u64,
    climate: &ClimateConfig,
    clouds: &CloudStore,
    humidity: &Humidity,
    temperature: &Temperature,
    carbon: &CarbonBudget,
    width_cols: i32,
    wrap_x: bool,
    sea_level_y: i32,
    downpour_mass: f32,
    snow_bias: f32,
) -> SkyWeatherParams {
    sample_sky_weather(
        tick,
        climate,
        clouds,
        humidity,
        temperature,
        carbon,
        width_cols,
        wrap_x,
        sea_level_y,
        downpour_mass,
        snow_bias,
    )
}

/// Helper: estimate snow bias from raining parcels (0..1).
pub fn estimate_snow_bias(
    clouds: &CloudStore,
    snowing: impl Fn(f32, f32) -> bool,
) -> f32 {
    let mut raining = 0u32;
    let mut snow = 0u32;
    for p in &clouds.parcels {
        if !p.raining {
            continue;
        }
        raining += 1;
        if snowing(p.fx, p.fy) {
            snow += 1;
        }
    }
    if raining == 0 {
        0.0
    } else {
        snow as f32 / raining as f32
    }
}

#[cfg(test)]
mod tests {
    use super::{
        collect_drop_tops, gather_soft_cloud_srcs, haze_cell_is_drop, haze_column_y0,
        haze_paint_seats, haze_resampled_cells, humidity_haze_alpha, humidity_haze_style,
        haze_alpha_ref_step, haze_alpha_ref_target, stamp_pixel_cloud_mask, HAZE_ALPHA_MAX,
        HAZE_ALPHA_MIN, HAZE_MASS_FLOOR, HAZE_MASS_FULL,
        CloudDepthLayer,
    };
    use std::collections::HashMap;
    use wk_voxel::{CloudStore, Humidity};

    #[test]
    fn airborne_snow_does_not_spike_the_ridge() {
        // The old scan started at the sky ceiling and treated any non-Air
        // as the crest. Snow is a solid, so a flake became a needle; the
        // 30-tick cache then made the spike linger until the sky cleared.
        use super::{column_surface_y, ridge_ground_at};
        use wk_material::MaterialId;
        use wk_voxel::{continental_surface_y, Cell, ChunkCoord, CHUNK_CELLS_H, World};

        let seed = 1u64;
        let width = 64;
        let sea = 8;
        // Plains band (~0.30–0.40 of the ring). Abyss columns sit
        // below y=0 and the clamp would hide the stone from the walk.
        let gx = 22;
        let hint = continental_surface_y(seed, gx, sea, width);
        assert!(
            hint > sea,
            "test column should be land (hint={hint})"
        );
        let mut w = World::new(seed);
        for y in [hint, hint + 30] {
            w.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(64),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        w.set_cell(gx, hint, Cell::solid(MaterialId::Stone));
        w.set_cell(gx, hint + 30, Cell::solid(MaterialId::Snow));
        assert!(
            !ridge_ground_at(&w, gx, hint + 30, sea),
            "a flake in the sky is not ground"
        );
        let sky = hint + 64;
        let y = column_surface_y(&w, gx, 0, sky, sea, seed, width);
        assert_eq!(
            y, hint,
            "ridge crest must sit on the stone, not the flake (got {y})"
        );
    }

    #[test]
    fn wet_air_above_the_sea_is_not_a_ridge_crest() {
        use super::column_surface_y;
        use wk_material::MaterialId;
        use wk_voxel::{continental_surface_y, Cell, ChunkCoord, Sat, CHUNK_CELLS_H, World};

        let seed = 1u64;
        let width = 64;
        let sea = 8;
        let gx = 23;
        let hint = continental_surface_y(seed, gx, sea, width);
        assert!(
            hint > sea,
            "test column should be land (hint={hint})"
        );
        let mut w = World::new(seed);
        w.ensure_chunk(ChunkCoord::new(
            gx.div_euclid(64),
            hint.div_euclid(CHUNK_CELLS_H as i32),
        ));
        w.ensure_chunk(ChunkCoord::new(
            gx.div_euclid(64),
            (hint + 24).div_euclid(CHUNK_CELLS_H as i32),
        ));
        w.set_cell(gx, hint, Cell::solid(MaterialId::Stone));
        let mut haze = Cell::air();
        haze.sat = Sat(200);
        w.set_cell(gx, hint + 24, haze);
        let sky = hint + 64;
        let y = column_surface_y(&w, gx, 0, sky, sea, seed, width);
        assert_eq!(y, hint, "mid-air wetness is not a mountain (got {y})");
    }

    #[test]
    fn haze_ignores_thin_vapor_and_stays_soft() {
        assert_eq!(humidity_haze_alpha(0.0, HAZE_MASS_FULL), 0);
        assert_eq!(humidity_haze_alpha(10.0, HAZE_MASS_FULL), 0); // ≤25.5 mass floor
        assert_eq!(humidity_haze_alpha(HAZE_MASS_FLOOR, HAZE_MASS_FULL), 0);
        assert!(humidity_haze_alpha(26.0, HAZE_MASS_FULL) >= HAZE_ALPHA_MIN);
        assert_eq!(humidity_haze_alpha(HAZE_MASS_FULL, HAZE_MASS_FULL), HAZE_ALPHA_MAX);
        // Remap above the floor: mid moisture readable, full near-opaque.
        let lo = humidity_haze_alpha(40.0, HAZE_MASS_FULL);
        let mid = humidity_haze_alpha(120.0, HAZE_MASS_FULL);
        let hi = humidity_haze_alpha(HAZE_MASS_FULL, HAZE_MASS_FULL);
        assert!(lo >= HAZE_ALPHA_MIN, "just-above-floor must paint (got {lo})");
        assert!(hi >= 180, "full-scale humidity should read dense (got {hi})");
        assert!(
            lo < mid && mid < hi,
            "haze alpha should rise with moisture ({lo} < {mid} < {hi})"
        );
        let floor = HAZE_MASS_FLOOR / HAZE_MASS_FULL;
        let (_, _, b_lo, _) = humidity_haze_style(40.0, HAZE_MASS_FULL, floor);
        let (_, _, b_hi, _) = humidity_haze_style(HAZE_MASS_FULL, HAZE_MASS_FULL, floor);
        assert!(
            b_hi < b_lo,
            "wetter haze should be less blue-grey ({b_hi} vs {b_lo})"
        );
    }

    #[test]
    fn haze_ref_does_not_pump_when_a_peak_drains() {
        // Fixed 255 display scale — draining a wet peak must not rescale
        // the wash, and supersat must not raise the ceiling.
        let before = haze_alpha_ref_target(400.0, 18.0);
        let after = haze_alpha_ref_target(80.0, 18.0);
        let spiked = haze_alpha_ref_target(3_000.0, 18.0);
        assert!(
            (before - after).abs() < 1.0,
            "draining the peak must not rescale the wash ({before} → {after})"
        );
        assert!(
            (spiked - before).abs() < 1.0,
            "supersat must not raise the ceiling ({spiked} vs {before})"
        );
        assert!(
            (before - HAZE_MASS_FULL).abs() < 1.0,
            "ref stays on the 255 humidity scale (got {before})"
        );
        let mid = humidity_haze_alpha(80.0, before);
        let mid2 = humidity_haze_alpha(80.0, after);
        assert_eq!(mid, mid2, "same mass must keep the same alpha");
        let stepped = haze_alpha_ref_step(before, 80.0);
        assert!(
            stepped > before * 0.9,
            "ref should fall slowly (got {stepped} from {before})"
        );
    }

    #[test]
    fn haze_seats_include_tiles_below_sea_level() {
        // A dug lake below sea must still paint. Sea was a hard sky_hy_min.
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 320);
        h.cells.insert((2, 5), 80.0); // y~20 — well below default sea 80
        let seats = haze_paint_seats(&h, 0);
        assert!(
            seats.iter().any(|&(hx, hy)| hx == 2 && hy == 5),
            "below-sea vapor must remain a paint seat ({seats:?})"
        );
    }

    #[test]
    fn haze_carves_only_the_drop_cell() {
        use wk_voxel::{Cell, ChunkCoord, Sat, GRAIN_REPOSE_HAZE_MAX};

        let mut w = wk_voxel::World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(4, 10, Cell::air());
        let mut drop = Cell::air();
        drop.sat = Sat(GRAIN_REPOSE_HAZE_MAX.saturating_add(8));
        w.set_cell(5, 10, drop);
        w.set_cell(6, 10, Cell::air());
        assert!(!haze_cell_is_drop(&w, 4, 10));
        assert!(haze_cell_is_drop(&w, 5, 10), "the droplet is one cell");
        assert!(!haze_cell_is_drop(&w, 6, 10));
    }

    #[test]
    fn a_drop_opens_the_column_under_it() {
        use wk_voxel::{Cell, ChunkCoord, Sat, GRAIN_REPOSE_HAZE_MAX};

        let mut w = wk_voxel::World::new(1);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        let mut drop = Cell::air();
        drop.sat = Sat(GRAIN_REPOSE_HAZE_MAX.saturating_add(8));
        w.set_cell(5, 20, drop);
        let tops = collect_drop_tops(&w);
        assert_eq!(tops.get(&5).copied(), Some(20));
        assert_eq!(haze_column_y0(8, 24, Some(20)), Some(21));
        assert_eq!(haze_column_y0(8, 20, Some(20)), None);
        assert_eq!(haze_column_y0(8, 24, None), Some(8));
        assert_eq!(
            haze_column_y0(8, 24, Some(20)),
            Some(21),
            "blocks at and under the drop stay open"
        );
    }

    #[test]
    fn resample_keeps_a_one_wide_path_through_an_emptied_tile() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 32, 128);
        h.cells.insert((1, 5), 400.0);
        h.cells.insert((0, 4), 400.0);
        h.cells.insert((0, 6), 400.0);
        let seats = haze_paint_seats(&h, 0);
        assert!(
            seats.contains(&(0, 5)),
            "emptied nucleating tile must stay a seat or rain is 4-wide"
        );
        assert!(
            seats.iter().all(|&(hx, _)| hx >= 0),
            "neighbour seats wrap; they must not use raw hx-1"
        );

        let mut drop_tops = HashMap::new();
        drop_tops.insert(2, 21);
        let painted: std::collections::HashSet<_> = seats
            .iter()
            .flat_map(|&(hx, hy)| {
                haze_resampled_cells(&h, hx, hy, &drop_tops, |x| x, |_| 0)
            })
            .filter(|&(_, _, m)| m > 0.0)
            .map(|(x, y, _)| (x, y))
            .collect();

        for gy in 16..=21 {
            assert!(
                !painted.contains(&(2, gy)),
                "shaft must stay open under the drop (2,{gy})"
            );
        }
        for gx in [0, 1, 3] {
            assert!(
                (20..24).any(|gy| painted.contains(&(gx, gy))),
                "sibling column {gx} of the punched tile must still paint"
            );
        }
        let open: Vec<i32> = (0..4)
            .filter(|&gx| !(20..=21).any(|gy| painted.contains(&(gx, gy))))
            .collect();
        assert_eq!(
            open,
            vec![2],
            "only the drop column stays open through the punched band ({open:?})"
        );
    }

    #[test]
    fn resample_wraps_neighbour_seats_at_the_ring() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 16, 64);
        h.wrap_x = true;
        h.cells.insert((0, 5), 400.0);
        let seats = haze_paint_seats(&h, 0);
        assert!(seats.contains(&(3, 5)), "left neighbour wraps to hx_max");
        assert!(
            !seats.iter().any(|&(hx, _)| hx < 0 || hx > 3),
            "raw hx-1 at the ring was the bright seam"
        );
    }

    #[test]
    fn depth_echoes_come_from_active_parcels() {
        let mut clouds = CloudStore::new();
        clouds.parcels.push(wk_voxel::CloudParcel {
            fx: 10.0,
            fy: 40.0,
            mass: 80.0,
            raining: false,
            on_ridge: false,
            shape_seed: 7,
            cruise_fy: 40.0,
            vis_mass: 80.0,
            deform: 0.0,
        });
        let hum = Humidity::new(4);
        let active = gather_soft_cloud_srcs(&clouds, &hum, CloudDepthLayer::Active, 20, 1, 200.0);
        let mid = gather_soft_cloud_srcs(&clouds, &hum, CloudDepthLayer::Mid, 20, 1, 200.0);
        assert_eq!(active.len(), 1);
        assert_eq!(mid.len(), 1, "mid should echo the active parcel");
        assert!((active[0].fx - 10.0).abs() < 1e-3);
        assert!((mid[0].fx - active[0].fx).abs() > 0.01 || (mid[0].fy - active[0].fy).abs() > 0.01);
    }

    #[test]
    fn overlapping_parcels_union_into_one_mask() {
        let cell = 3.0_f32;
        let mut mask = HashMap::new();
        stamp_pixel_cloud_mask(&mut mask, 100.0, 80.0, 36.0, 0.4, 1, 0.0, cell);
        let alone = mask.len();
        stamp_pixel_cloud_mask(&mut mask, 118.0, 82.0, 36.0, 0.9, 2, 0.0, cell);
        assert!(alone > 20, "first parcel should stamp a real footprint");
        assert!(
            mask.len() < alone * 2,
            "overlap must share cells (alone={alone} union={})",
            mask.len()
        );
        let wet_max = mask.values().cloned().fold(0.0_f32, f32::max);
        assert!((wet_max - 0.9).abs() < 1e-5);
    }
}
