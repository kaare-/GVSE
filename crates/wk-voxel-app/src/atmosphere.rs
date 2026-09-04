//! Sim-linked sky / weather / far-ridge drawing for wk-voxel-app.
//!
//! Design: `docs/SKY.md`. Isolation: wk-voxel + wk-material + macroquad only.

use std::collections::HashMap;

use macroquad::prelude::*;
use wk_material::MaterialId;
use wk_voxel::{
    airborne_loose_at, build_canopy_index_posed, carbon_ratio, celestial_moon_screen_pos_cfg,
    celestial_sun_screen_pos_cfg, cloud_floor_y, cloud_sky_transmit, continental_surface_y,
    day_night_factor_cfg, falls_through_empty_air, humidity_mean_norm, is_standing_water,
    precip_cover_fraction, resolve_organism_draw_cells, shade_transmit_column,
    sky_rgb_at_height_weather, CanopyIndex, CarbonBudget, ClimateConfig, Humidity, ModuleId,
    OrganismStore, PosedModule, SkyWeatherParams, Temperature, Wind, World, CHUNK_CELLS_H,
    CHUNK_CELLS_W, GRAIN_REPOSE_HAZE_MAX, LIVE_SURFACE_DESCENT_MAX, LIVE_SURFACE_SEARCH,
};

pub const FAR_RIDGE_PARALLAX: f32 = 0.12;
pub const NEAR_RIDGE_PARALLAX: f32 = 0.32;
const RIDGE_REFRESH_TICKS: u64 = 30;
const MILD_TEMP_C: f32 = 18.0;

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
    /// Strength of sun-angled cast shadows from plants/creatures (0..1).
    pub cast_shadow_strength: f32,
    /// Mild column dim under wet humidity (0..1).
    pub cloud_shade_strength: f32,
    /// `H` overlay: bilinear per-cell sample (on) vs flat 4×4 tiles (off).
    pub haze_resample: bool,
    /// `H` overlay: seats / samples below this mass do not paint.
    pub haze_min_mass: f32,
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
            cast_shadow_strength: 0.85,
            cloud_shade_strength: 0.35,
            haze_resample: true,
            haze_min_mass: 0.0,
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
    /// Drop the cached plates so the next `ensure` resamples live columns.
    /// F3 erase must call this — otherwise the ghost hill stays for
    /// [`RIDGE_REFRESH_TICKS`] and the walk used to keep the seed crest anyway.
    pub fn invalidate(&mut self) {
        self.far.clear();
        self.near.clear();
        self.last_tick = 0;
    }

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
    if falls_through_empty_air(c.material) || airborne_loose_at(world, gx, y, c) {
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

fn live_surface_y_ground(world: &World, gx: i32, hint: i32, search: i32, sea_level: i32) -> i32 {
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
        // Same extra climb as `live_surface_y`: an F3 tower can rise
        // farther than the local window.
        let extra = LIVE_SURFACE_DESCENT_MAX.saturating_sub(search);
        for _ in 0..extra {
            if world.get_cell(jx, y + 1).is_none() || !ground(y + 1) {
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
    // Same extra walk as `live_surface_y`: a wiped hill is farther than
    // the local window. Keep the seed only when the column is loaded air
    // with no bed (an unstamped neighbor, not an F3 wipe).
    let extra = LIVE_SURFACE_DESCENT_MAX.saturating_sub(search);
    for _ in 0..extra {
        y -= 1;
        if y < 0 {
            break;
        }
        if world.get_cell(jx, y).is_none() {
            break;
        }
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

/// True when this cell is falling **rain**. Snow floats through the wash
/// and must not carve the 1-wide shaft.
fn haze_cell_is_drop_cell(c: wk_voxel::Cell) -> bool {
    c.material == MaterialId::Air && c.sat.0 > GRAIN_REPOSE_HAZE_MAX
}

fn haze_cell_is_drop(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy) {
        Some(c) => haze_cell_is_drop_cell(c),
        None => false,
    }
}

/// Highest drop in each column. Haze below that cell is the open path.
fn collect_drop_tops(world: &World) -> HashMap<i32, i32> {
    collect_drop_tops_where(world, |_, _| true)
}

/// [`collect_drop_tops`] restricted to chunks that can affect the
/// viewport shaft. Off-screen rain still falls in the sim; it cannot
/// carve an on-screen wash. Full column height stays — a drop above
/// the camera still opens the path below it. Chunks whose entire
/// 64-high band sits **below** the camera are leftover (shafts only
/// go down).
fn collect_drop_tops_where(
    world: &World,
    mut keep_chunk: impl FnMut(i32, i32) -> bool,
) -> HashMap<i32, i32> {
    let mut tops: HashMap<i32, i32> = HashMap::new();
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    for chunk in world.chunks.values() {
        if !keep_chunk(chunk.coord.cx, chunk.coord.cy) {
            continue;
        }
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
                tops.entry(gx)
                    .and_modify(|y| *y = (*y).max(gy))
                    .or_insert(gy);
            }
        }
    }
    tops
}

/// True when a humidity tile's 4×4 can produce a pixel in the viewport
/// (including ring `x` copies). Pad one tile so panning does not pop.
fn humidity_tile_touches_view(
    hx: i32,
    hy: i32,
    tc: i32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
) -> bool {
    if cell_px <= 0.0 {
        return false;
    }
    let tc = tc.max(1);
    let pad = tc as f32 * cell_px;
    let gx0 = hx * tc;
    let gy0 = hy * tc;
    let tile_px = tc as f32 * cell_px;
    // Same `top_sy` as [`draw_haze_and_wind`]: top of cell `gy` is
    // `origin_y - (gy + 1 - bedrock) * cell_px`.
    let top_sy = origin_y - (gy0 + tc - bedrock_floor_y) as f32 * cell_px;
    let bot_sy = origin_y - (gy0 - bedrock_floor_y) as f32 * cell_px;
    if bot_sy < -pad || top_sy > sh + pad {
        return false;
    }
    let copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    for &copy in copies {
        let sx = origin_x + (gx0 + copy * width_cols) as f32 * cell_px;
        if sx + tile_px >= -pad && sx <= sw + pad {
            return true;
        }
    }
    false
}

/// Chunk `cx` whose 64-wide x band can touch the viewport (ring copies).
fn chunk_x_touches_view(
    cx: i32,
    origin_x: f32,
    cell_px: f32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
) -> bool {
    if cell_px <= 0.0 {
        return false;
    }
    let pad = CHUNK_CELLS_W as f32 * cell_px;
    let gx0 = cx * CHUNK_CELLS_W as i32;
    let copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    for &copy in copies {
        let sx = origin_x + (gx0 + copy * width_cols) as f32 * cell_px;
        if sx + pad >= 0.0 && sx <= sw {
            return true;
        }
    }
    false
}

/// Inclusive tile box that can produce a haze / overlay pixel.
///
/// Built from the camera so paint can probe O(view) instead of walking
/// every humidity key (55k+ after a soak). One-tile pad matches
/// [`humidity_tile_touches_view`] so bilinear neighbours stay.
#[derive(Clone, Debug)]
pub(crate) struct ViewTileBox {
    pub hy_lo: i32,
    pub hy_hi: i32,
    hx_ranges: [(i32, i32); 2],
    n_hx: u8,
}

impl ViewTileBox {
    pub(crate) fn contains(&self, hx: i32, hy: i32) -> bool {
        hy >= self.hy_lo && hy <= self.hy_hi && self.contains_hx(hx)
    }

    fn contains_hx(&self, hx: i32) -> bool {
        for i in 0..self.n_hx as usize {
            let (a, b) = self.hx_ranges[i];
            if hx >= a && hx <= b {
                return true;
            }
        }
        false
    }

    pub(crate) fn tile_count(&self) -> usize {
        let h = (self.hy_hi - self.hy_lo + 1).max(0) as usize;
        let mut w = 0usize;
        for i in 0..self.n_hx as usize {
            let (a, b) = self.hx_ranges[i];
            w += (b - a + 1).max(0) as usize;
        }
        h.saturating_mul(w)
    }

    pub(crate) fn for_each_hx(&self, mut f: impl FnMut(i32)) {
        for i in 0..self.n_hx as usize {
            let (a, b) = self.hx_ranges[i];
            for hx in a..=b {
                f(hx);
            }
        }
    }
}

/// World-x intervals (inclusive) visible in camera space, wrapped.
pub(crate) fn wrap_world_x_ranges(
    gx_lo: i32,
    gx_hi: i32,
    width_cols: i32,
) -> ([(i32, i32); 2], u8) {
    if width_cols <= 0 || gx_hi < gx_lo {
        return ([(0, -1), (0, -1)], 0);
    }
    if gx_hi - gx_lo >= width_cols {
        return ([(0, width_cols - 1), (0, -1)], 1);
    }
    let a = gx_lo.rem_euclid(width_cols);
    let b = a + (gx_hi - gx_lo);
    if b < width_cols {
        ([(a, b), (0, -1)], 1)
    } else {
        ([(a, width_cols - 1), (0, b - width_cols)], 2)
    }
}

/// Inclusive world-x ranges that can produce a cell-sized overlay pixel.
///
/// U / M / G used to scan `0..width_cols` and skip off-screen `sx`.
/// Same leftover as H walking every humidity key — probe the camera.
pub(crate) fn view_cell_x_ranges(
    origin_x: f32,
    cell_px: f32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
) -> ([(i32, i32); 2], u8) {
    if cell_px <= 0.0 || sw <= 0.0 {
        return ([(0, -1), (0, -1)], 0);
    }
    let gx_lo = ((-cell_px - origin_x) / cell_px).floor() as i32;
    let gx_hi = ((sw + cell_px - origin_x) / cell_px).ceil() as i32;
    if !wrap_x || width_cols <= 0 {
        let a = gx_lo.max(0);
        let b = if width_cols > 0 {
            gx_hi.min(width_cols - 1)
        } else {
            gx_hi
        };
        if b < a {
            return ([(0, -1), (0, -1)], 0);
        }
        return ([(a, b), (0, -1)], 1);
    }
    wrap_world_x_ranges(gx_lo, gx_hi, width_cols)
}

pub(crate) fn gx_in_ranges(gx: i32, ranges: &[(i32, i32); 2], n: u8) -> bool {
    for i in 0..n as usize {
        let (a, b) = ranges[i];
        if gx >= a && gx <= b {
            return true;
        }
    }
    false
}

pub(crate) fn view_tile_box(
    tc: i32,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
) -> Option<ViewTileBox> {
    if cell_px <= 0.0 || sw <= 0.0 || sh <= 0.0 {
        return None;
    }
    let tc = tc.max(1);
    let pad = tc as f32 * cell_px;
    // Expand one tile so float rounding cannot drop a seat that
    // [`humidity_tile_touches_view`] would keep.
    let hy_hi =
        ((bedrock_floor_y as f32 + (origin_y + pad) / cell_px) / tc as f32).floor() as i32 + 1;
    let hy_lo = {
        let raw =
            (bedrock_floor_y as f32 - tc as f32 + (origin_y - sh - pad) / cell_px) / tc as f32;
        raw.ceil() as i32 - 1
    };
    let gx_lo = ((-pad - origin_x) / cell_px).floor() as i32 - tc;
    let gx_hi = ((sw + pad - origin_x) / cell_px).ceil() as i32;
    let mut hx_ranges = [(0, -1), (0, -1)];
    let n_hx;
    if !wrap_x || width_cols <= 0 {
        hx_ranges[0] = (gx_lo.div_euclid(tc), gx_hi.div_euclid(tc));
        n_hx = 1;
    } else {
        let (wx, n) = wrap_world_x_ranges(gx_lo, gx_hi, width_cols);
        n_hx = n;
        for i in 0..n as usize {
            let (a, b) = wx[i];
            hx_ranges[i] = (a.div_euclid(tc), b.div_euclid(tc));
        }
    }
    Some(ViewTileBox {
        hy_lo,
        hy_hi,
        hx_ranges,
        n_hx,
    })
}

/// True unless this chunk's 64-high band sits entirely below the
/// viewport. High sky stays — a drop above the camera still carves.
fn chunk_not_below_view(
    cy: i32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    sh: f32,
) -> bool {
    if cell_px <= 0.0 {
        return true;
    }
    let oy = cy * CHUNK_CELLS_H as i32;
    let top_sy = origin_y - (oy + CHUNK_CELLS_H as i32 - bedrock_floor_y) as f32 * cell_px;
    top_sy <= sh + cell_px
}

/// On-screen tile is a seat when it holds mass or a cardinal neighbour
/// does (off-screen mass still seeds bilinear).
fn haze_view_seat_due(humidity: &Humidity, hx: i32, hy: i32) -> bool {
    if humidity.at_tile(hx, hy) > 0.0 {
        return true;
    }
    for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
        let Some(nx) = humidity.wrap_tile_x(hx + dx) else {
            continue;
        };
        let ny = hy + dy;
        if let Some(b) = humidity.bounds {
            if !b.contains(nx, ny) {
                continue;
            }
        }
        if humidity.at_tile(nx, ny) > 0.0 {
            return true;
        }
    }
    false
}

/// Occupied + neighbour seats that can touch `box` — probe the view
/// when it is smaller than the vapour map, otherwise walk keys.
fn haze_paint_seats_in_box(humidity: &Humidity, box_: &ViewTileBox) -> Vec<(i32, i32)> {
    if box_.tile_count() >= humidity.cells.len().max(1) {
        return haze_paint_seats_where(humidity, |hx, hy| box_.contains(hx, hy));
    }
    let mut seats = Vec::new();
    for hy in box_.hy_lo..=box_.hy_hi {
        for i in 0..box_.n_hx as usize {
            let (hx0, hx1) = box_.hx_ranges[i];
            for hx in hx0..=hx1 {
                let Some(hx) = humidity.wrap_tile_x(hx) else {
                    continue;
                };
                if haze_view_seat_due(humidity, hx, hy) {
                    seats.push((hx, hy));
                }
            }
        }
    }
    seats.sort_unstable();
    seats.dedup();
    seats
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
fn haze_paint_seats(humidity: &Humidity) -> Vec<(i32, i32)> {
    haze_paint_seats_where(humidity, |_, _| true)
}

/// Occupied tiles plus cardinal neighbours, then drop seats that cannot
/// touch the viewport. Off-screen mass still seeds an on-screen neighbour
/// (bilinear). The sim field is unchanged — coarsening off-screen
/// advect / lottery would change weather on a ring world.
fn haze_paint_seats_where(
    humidity: &Humidity,
    mut keep: impl FnMut(i32, i32) -> bool,
) -> Vec<(i32, i32)> {
    let mut seats = std::collections::HashSet::new();
    for (&(hx, hy), &mass) in &humidity.cells {
        if mass <= 0.0 {
            continue;
        }
        if keep(hx, hy) {
            seats.insert((hx, hy));
        }
        for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let Some(nx) = humidity.wrap_tile_x(hx + dx) else {
                continue;
            };
            let ny = hy + dy;
            if let Some(b) = humidity.bounds {
                if !b.contains(nx, ny) {
                    continue;
                }
            }
            if keep(nx, ny) {
                seats.insert((nx, ny));
            }
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
    resample: bool,
) -> Vec<(i32, i32, f32)> {
    let tc = humidity.tile_cols.max(1);
    let base_gx = hx * tc;
    let base_gy = hy * tc;
    let y1 = base_gy + tc;
    let tile_mass = humidity.at_tile(hx, hy);
    let mut out = Vec::new();
    for col in 0..tc {
        let wx = wrap_x(base_gx + col);
        let y0 = (floor_y(wx) + 1).max(base_gy);
        let Some(col_y0) = haze_column_y0(y0, y1, drop_tops.get(&wx).copied()) else {
            continue;
        };
        for gy in col_y0..y1 {
            let sampled = if resample {
                humidity.sample_bilinear(wx as f32 + 0.5, gy as f32 + 0.5)
            } else {
                tile_mass
            };
            out.push((wx, gy, sampled));
        }
    }
    out
}

/// Soft white vapor haze alpha (legacy helper for tests / diagnostics).
pub fn humidity_haze_alpha(mass: f32, max_mass: f32) -> u8 {
    humidity_haze_alpha_cell(mass, max_mass, 0.12)
}

/// Per-cell wash. Soft floor — the 12% live-max cut ate neighbour
/// columns around an emptied tile and turned rain into a 4-wide hole.
fn humidity_haze_alpha_cell(mass: f32, max_mass: f32, floor: f32) -> u8 {
    if mass <= 0.0 {
        return 0;
    }
    let norm = (mass / max_mass.max(1.0)).clamp(0.0, 1.0);
    if norm < floor {
        return 0;
    }
    (18.0 + norm * 42.0) as u8
}

/// `H` wash after the Tab min-mass cutoff. `floor` is leftover live-max
/// fraction (tests / thin-vapour helper); play uses `min_mass` as the gate.
fn humidity_haze_alpha_gated(mass: f32, max_mass: f32, min_mass: f32) -> u8 {
    if mass < min_mass.max(0.0) {
        return 0;
    }
    humidity_haze_alpha_cell(mass, max_mass, 0.0)
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
    humidity: &Humidity,
    temperature: &Temperature,
    carbon: &CarbonBudget,
    width_cols: i32,
    snow_bias: f32,
) -> SkyWeatherParams {
    let precip_cover = precip_cover_fraction(humidity, 0, width_cols);
    // Mean the tiles that actually hold vapour — not a global sea+4 cut
    // that pinned haze / sky tint to the lake line.
    let humidity_mean = humidity_mean_norm(humidity, i32::MIN);
    let mut t_sum = 0.0f32;
    let mut t_n = 0u32;
    for (&(hx, hy), &mass) in &humidity.cells {
        if mass <= 0.0 {
            continue;
        }
        t_sum += temperature.at_tile(hx, hy);
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
            let y_vis = bedrock_floor_y + ((surf_soft - bedrock_floor_y) as f32 * y_squash) as i32;
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
                    let b_y =
                        bedrock_floor_y + ((b_surf - bedrock_floor_y) as f32 * b.y_squash) as i32;
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
/// Wind streaks are a separate overlay ([`draw_wind_streaks`], `V`).
pub fn draw_haze_and_wind(
    humidity: &Humidity,
    world: &World,
    wind: &Wind,
    look: &AtmosphereLookConfig,
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
    if humidity.cells.is_empty() || cell_px <= 0.0 {
        return;
    }
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };
    let max_mass = humidity
        .cells
        .values()
        .copied()
        .fold(0.0f32, f32::max)
        .max(1.0);
    let min_mass = look.haze_min_mass.max(0.0);
    let resample = look.haze_resample;
    let _ = sea_level_y;
    let tc = humidity.tile_cols.max(1);
    let drop_tops = collect_drop_tops_where(world, |cx, cy| {
        chunk_x_touches_view(cx, origin_x, cell_px, wrap_x, width_cols, sw)
            && chunk_not_below_view(cy, origin_y, cell_px, bedrock_floor_y, sh)
    });
    // No global y-cut. Per-column `cloud_floor_y` already clips buried cells.
    // Probe the viewport tile box so a soaked sky does not walk 55k keys
    // to paint a few thousand on-screen seats. Sim field unchanged.
    let seats = match view_tile_box(
        tc,
        origin_x,
        origin_y,
        cell_px,
        bedrock_floor_y,
        wrap_x,
        width_cols,
        sw,
        sh,
    ) {
        Some(box_) => haze_paint_seats_in_box(humidity, &box_),
        None => haze_paint_seats_where(humidity, |hx, hy| {
            humidity_tile_touches_view(
                hx,
                hy,
                tc,
                origin_x,
                origin_y,
                cell_px,
                bedrock_floor_y,
                wrap_x,
                width_cols,
                sw,
                sh,
            )
        }),
    };
    let mut floor_cache: HashMap<i32, i32> = HashMap::new();

    for (hx, hy) in seats {
        for (wx, gy, sampled) in haze_resampled_cells(
            humidity,
            hx,
            hy,
            &drop_tops,
            |x| world.wrap_x(x),
            |wx| {
                *floor_cache
                    .entry(wx)
                    .or_insert_with(|| cloud_floor_y(world, wind, wx as f32).round() as i32)
            },
            resample,
        ) {
            let cell_alpha = humidity_haze_alpha_gated(sampled, max_mass, min_mass);
            if cell_alpha == 0 {
                continue;
            }
            for &x_copy in x_copies {
                let sx = origin_x + (wx + x_copy * width_cols) as f32 * cell_px;
                let top_sy = origin_y - (gy + 1 - bedrock_floor_y) as f32 * cell_px;
                if sx + cell_px < 0.0 || sx > sw || top_sy > sh || top_sy + cell_px < 0.0 {
                    continue;
                }
                draw_rectangle(
                    sx,
                    top_sy,
                    cell_px + 0.5,
                    cell_px + 0.5,
                    Color::from_rgba(255, 255, 255, cell_alpha),
                );
            }
        }
    }
}

/// Map a climate-scale vector onto a readable overlay stroke.
///
/// Sim wind is ~0.05 tiles/tick by default. Using `vx` as a pixel delta
/// made arrows sub-pixel until the Tab slider was cranked. Direction is
/// unit-length; length and alpha encode speed with a floor so the default
/// breeze still reads. Kept short so neighbouring lattice points do not
/// overlap into a scribble.
fn wind_streak_geom(vx: f32, vy: f32, tile_px: f32) -> Option<(f32, f32, f32, u8)> {
    let speed = vx.hypot(vy);
    if speed < 0.0015 {
        return None;
    }
    let ux = vx / speed;
    let uy = vy / speed;
    // 0.05 (Tab default) → vis 1.0. About one tile at the breeze; gusts ~2.
    let vis = (speed / 0.05).clamp(0.40, 3.2);
    let len = ((0.55 + 0.40 * vis) * tile_px).max(6.0);
    let alpha = (140.0 + 80.0 * ((vis - 0.40) / 2.8).clamp(0.0, 1.0)) as u8;
    Some((ux, uy, len, alpha))
}

/// Overlay lattice in tile coords. Zoomed-out views skip more so the
/// screen does not fill with overlapping stems. The sim field is unchanged.
fn wind_streak_stride(cell_px: f32) -> i32 {
    if cell_px < 2.0 {
        3
    } else {
        2
    }
}

fn wind_streak_on_lattice(hx: i32, hy: i32, stride: i32) -> bool {
    let s = stride.max(1);
    hx.rem_euclid(s) == 0 && hy.rem_euclid(s) == 0
}

fn wind_tile_center_is_solid(world: Option<&World>, tc: i32, hx: i32, hy: i32) -> bool {
    let Some(w) = world else {
        return false;
    };
    let gx = w.wrap_x(hx * tc + tc / 2);
    let gy = hy * tc + tc / 2;
    matches!(w.get_cell(gx, gy), Some(c) if c.material.is_solid())
}

/// World-space wind strokes (`V` overlay) from the local heatmap.
///
/// A coarse lattice of short arrows — not one stroke per rebuilt tile —
/// so heading and force read on the terrain without a hair on every seat.
pub fn draw_wind_streaks(
    wind: &Wind,
    world: Option<&World>,
    tick: u64,
    origin_x: f32,
    origin_y: f32,
    cell_px: f32,
    bedrock_floor_y: i32,
    wrap_x: bool,
    width_cols: i32,
    sw: f32,
    sh: f32,
) {
    if cell_px <= 0.0 {
        return;
    }
    let tc = wind.tile_cols.max(1);
    let evx = wind.effective_vx(tick);
    let evy = wind.effective_vy(tick);
    let x_copies: &[i32] = if wrap_x { &[-1, 0, 1] } else { &[0] };

    let stride = wind_streak_stride(cell_px);
    let view = view_tile_box(
        tc,
        origin_x,
        origin_y,
        cell_px,
        bedrock_floor_y,
        wrap_x,
        width_cols,
        sw,
        sh,
    );
    let mut samples: Vec<(i32, i32, f32, f32)> = Vec::new();
    if !wind.field.is_empty() {
        for (&(hx, hy), &(vx, vy)) in &wind.field {
            if !wind_streak_on_lattice(hx, hy, stride) {
                continue;
            }
            if view.as_ref().is_some_and(|b| !b.contains(hx, hy)) {
                continue;
            }
            if !humidity_tile_touches_view(
                hx,
                hy,
                tc,
                origin_x,
                origin_y,
                cell_px,
                bedrock_floor_y,
                wrap_x,
                width_cols,
                sw,
                sh,
            ) {
                continue;
            }
            if wind_tile_center_is_solid(world, tc, hx, hy) {
                continue;
            }
            samples.push((hx, hy, vx, vy));
        }
    } else {
        // Field not rebuilt yet — still show climate wind on a viewport grid.
        if evx.abs() + evy.abs() < 0.0015 {
            return;
        }
        let gx0 = ((-origin_x) / cell_px).floor() as i32 - tc;
        let gx1 = gx0 + (sw / cell_px).ceil() as i32 + tc * 2;
        let gy_hi = bedrock_floor_y + (origin_y / cell_px).ceil() as i32 + 2;
        let gy_lo = gy_hi - (sh / cell_px).ceil() as i32 - 2;
        let mut hx = gx0.div_euclid(tc);
        let hx1 = gx1.div_euclid(tc);
        while hx <= hx1 {
            let mut hy = gy_lo.div_euclid(tc);
            let hy1 = gy_hi.div_euclid(tc);
            while hy <= hy1 {
                if wind_streak_on_lattice(hx, hy, stride)
                    && !wind_tile_center_is_solid(world, tc, hx, hy)
                {
                    let (vx, vy) = wind.vector_at(world, hx, hy);
                    samples.push((hx, hy, vx, vy));
                }
                hy += stride;
            }
            hx += stride;
        }
    }

    let tile_px = tc as f32 * cell_px;
    for (hx, hy, vx, vy) in samples {
        let cx = hx * tc + tc / 2;
        let cy = hy * tc + tc / 2;
        let Some((ux, uy, len, alpha)) = wind_streak_geom(vx, vy, tile_px) else {
            continue;
        };
        let color = Color::from_rgba(220, 234, 248, alpha);
        for &x_copy in x_copies {
            let sx = origin_x + (cx + x_copy * width_cols) as f32 * cell_px;
            let sy = origin_y - (cy as f32 + 0.5 - bedrock_floor_y as f32) * cell_px;
            if sx < -len || sx > sw + len || sy < -len || sy > sh + len {
                continue;
            }
            let x2 = sx + ux * len;
            let y2 = sy - uy * len;
            // One stroke + two barbs. The old dark/light pair was six
            // draw_line calls per tile and read as a scribble once dense.
            draw_line(sx, sy, x2, y2, 1.35, color);
            let hx_n = ux * (0.22 * len).min(5.5);
            let hy_n = -uy * (0.22 * len).min(5.5);
            draw_line(
                x2,
                y2,
                x2 - hx_n + hy_n * 0.45,
                y2 - hy_n - hx_n * 0.45,
                1.2,
                color,
            );
            draw_line(
                x2,
                y2,
                x2 - hx_n - hy_n * 0.45,
                y2 - hy_n + hx_n * 0.45,
                1.2,
                color,
            );
        }
    }
}

/// Surface dim under foliage + diagonal celestial cast on the lee (+ mild cloud dim).
///
/// Day: sun cast (call before organisms). Night: moon cast, near-black lee
/// (call after organisms). Cast direction is **viewport-relative** to the
/// on-screen sun/moon so shadows never fall toward the light.
pub fn draw_canopy_air_dim(
    world: &World,
    organisms: &OrganismStore,
    humidity: &Humidity,
    tick: u64,
    wind_vx: f32,
    celestial_local: f32,
    celestial_sx: f32,
    _celestial_sy: f32,
    is_day: bool,
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

    // 3) Mild vapour dim on exposed surface (day + night).
    if cloud_k > 0.02 && !humidity.cells.is_empty() {
        for x in 0..width_cols {
            let Some(y) = top_shadow_receiver_y(world, x, y_min_vis, y_max_vis) else {
                continue;
            };
            let ct = cloud_sky_transmit(humidity, x, y);
            let dim = ((1.0 - ct) * cloud_k * 0.50).clamp(0.0, 0.35);
            if dim < 0.04 {
                continue;
            }
            let e = shade.entry((x, y)).or_insert(0.0);
            *e = (*e + dim).min(0.70);
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

/// Build weather params + snow bias for the current scene view.
pub fn sky_weather_for_scene(
    tick: u64,
    climate: &ClimateConfig,
    humidity: &Humidity,
    temperature: &Temperature,
    carbon: &CarbonBudget,
    width_cols: i32,
    snow_bias: f32,
) -> SkyWeatherParams {
    sample_sky_weather(
        tick,
        climate,
        humidity,
        temperature,
        carbon,
        width_cols,
        snow_bias,
    )
}

/// Snow bias from wet tiles that sit at or below freeze (0..1).
pub fn estimate_snow_bias(humidity: &Humidity, temperature: &Temperature, freeze_c: f32) -> f32 {
    let mut wet = 0u32;
    let mut snow = 0u32;
    for (&(hx, hy), &mass) in &humidity.cells {
        if mass <= 0.0 {
            continue;
        }
        wet += 1;
        if temperature.at_tile(hx, hy) <= freeze_c {
            snow += 1;
        }
    }
    if wet == 0 {
        0.0
    } else {
        snow as f32 / wet as f32
    }
}

#[cfg(test)]
mod tests {
    use super::{
        chunk_not_below_view, collect_drop_tops, gx_in_ranges, haze_cell_is_drop, haze_column_y0,
        haze_paint_seats, haze_paint_seats_in_box, haze_paint_seats_where, haze_resampled_cells,
        humidity_haze_alpha, humidity_haze_alpha_gated, humidity_tile_touches_view,
        view_cell_x_ranges, view_tile_box,
    };
    use std::collections::HashMap;
    use wk_voxel::Humidity;

    #[test]
    fn airborne_snow_does_not_spike_the_ridge() {
        // The old scan started at the sky ceiling and treated any non-Air
        // as the crest. Snow is a solid, so a flake became a needle; the
        // 30-tick cache then made the spike linger until the sky cleared.
        use super::{column_surface_y, ridge_ground_at};
        use wk_material::MaterialId;
        use wk_voxel::{continental_surface_y, Cell, ChunkCoord, World, CHUNK_CELLS_H};

        let seed = 1u64;
        let width = 64;
        let sea = 8;
        // Plains band (~0.30–0.40 of the ring). Abyss columns sit
        // below y=0 and the clamp would hide the stone from the walk.
        let gx = 22;
        let hint = continental_surface_y(seed, gx, sea, width);
        assert!(hint > sea, "test column should be land (hint={hint})");
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
    fn built_tower_lifts_the_ridge_past_the_search_window() {
        use super::column_surface_y;
        use wk_material::MaterialId;
        use wk_voxel::{continental_surface_y, Cell, ChunkCoord, World, CHUNK_CELLS_H};

        let seed = 1u64;
        let width = 64;
        let sea = 20;
        let gx = 22;
        let hint = continental_surface_y(seed, gx, sea, width);
        let crest = hint + 120;
        assert!(hint > 0, "need a loaded seed hint (hint={hint})");
        let mut w = World::new(seed);
        for y in [0, hint, crest] {
            w.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(64),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        for y in 0..=crest {
            w.set_cell(gx, y, Cell::solid(MaterialId::Stone));
        }
        let sky = crest + 40;
        assert_eq!(
            column_surface_y(&w, gx, 0, sky, sea, seed, width),
            crest,
            "ridge must climb an F3 tower past the 64-cell window (hint={hint})"
        );
    }

    #[test]
    fn erased_hill_drops_the_ridge_off_the_seed_crest() {
        use super::column_surface_y;
        use wk_material::MaterialId;
        use wk_voxel::{continental_surface_y, Cell, ChunkCoord, World, CHUNK_CELLS_H};

        let seed = 1u64;
        let width = 64;
        let sea = 20;
        let gx = 22;
        let hint = continental_surface_y(seed, gx, sea, width);
        assert!(
            hint > sea + 8,
            "need a seed crest well above the leftover bed (hint={hint})"
        );
        let bed = (hint - 90).max(4);
        let mut w = World::new(seed);
        for y in [0, bed, hint] {
            w.ensure_chunk(ChunkCoord::new(
                gx.div_euclid(64),
                y.div_euclid(CHUNK_CELLS_H as i32),
            ));
        }
        w.set_cell(gx, 0, Cell::solid(MaterialId::Bedrock));
        for y in 1..=bed {
            w.set_cell(gx, y, Cell::solid(MaterialId::Sand));
        }
        for y in (bed + 1)..=hint {
            w.set_cell(gx, y, Cell::air());
        }
        w.set_cell(gx, hint - 6, Cell::solid(MaterialId::LooseLimestone));
        let sky = hint + 40;
        assert_eq!(
            column_surface_y(&w, gx, 0, sky, sea, seed, width),
            bed,
            "ridge must follow the erased hill, not the seed crest or a scrap"
        );
    }

    #[test]
    fn wet_air_above_the_sea_is_not_a_ridge_crest() {
        use super::column_surface_y;
        use wk_material::MaterialId;
        use wk_voxel::{continental_surface_y, Cell, ChunkCoord, Sat, World, CHUNK_CELLS_H};

        let seed = 1u64;
        let width = 64;
        let sea = 8;
        let gx = 23;
        let hint = continental_surface_y(seed, gx, sea, width);
        assert!(hint > sea, "test column should be land (hint={hint})");
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
        assert_eq!(humidity_haze_alpha(0.0, 100.0), 0);
        assert_eq!(humidity_haze_alpha(5.0, 100.0), 0); // below 12% floor
        assert!(humidity_haze_alpha(50.0, 100.0) >= 18);
        assert!(humidity_haze_alpha(100.0, 100.0) <= 70);
    }

    #[test]
    fn haze_min_mass_slider_gates_the_wash() {
        assert_eq!(humidity_haze_alpha_gated(10.0, 400.0, 50.0), 0);
        assert!(humidity_haze_alpha_gated(80.0, 400.0, 50.0) > 0);
    }

    #[test]
    fn haze_without_resample_paints_flat_tile_mass() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 32, 128);
        h.cells.insert((1, 5), 200.0);
        h.cells.insert((2, 5), 800.0);
        let drop_tops = HashMap::new();
        let cells = haze_resampled_cells(&h, 1, 5, &drop_tops, |x| x, |_| 0, false);
        assert!(!cells.is_empty());
        assert!(
            cells.iter().all(|&(_, _, m)| (m - 200.0).abs() < 1e-3),
            "off-resample must use the seat mass, not bilinear neighbours"
        );
    }

    #[test]
    fn haze_does_not_carve_under_snow() {
        use wk_voxel::{Cell, ChunkCoord};

        let mut w = wk_voxel::World::new(2);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.set_cell(5, 20, Cell::solid(wk_material::MaterialId::Snow));
        let tops = collect_drop_tops(&w);
        assert!(
            tops.get(&5).is_none(),
            "snow floats through the wash — it must not open a rain shaft"
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
        let seats = haze_paint_seats(&h);
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
            .flat_map(|&(hx, hy)| haze_resampled_cells(&h, hx, hy, &drop_tops, |x| x, |_| 0, true))
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
    fn default_climate_wind_makes_a_readable_streak() {
        // Tab default is 0.05 tiles/tick. The first V overlay used vx as a
        // pixel delta, so that breeze was ~0.3 px long. The next pass made
        // 16 px spears that overlapped into a scribble.
        let near_surface = 0.05 * 0.20; // height shear at the ground
        let (ux, _uy, len, alpha) =
            super::wind_streak_geom(near_surface, 0.0, 4.0).expect("default breeze");
        assert!((ux - 1.0).abs() < 1e-5);
        assert!(
            (6.0..14.0).contains(&len),
            "default breeze is a short hatch, not a 16px spear (len={len})"
        );
        assert!(
            alpha >= 130,
            "default breeze must not be a 16-alpha hairline (alpha={alpha})"
        );
        assert!(super::wind_streak_geom(0.0, 0.0, 4.0).is_none());
    }

    #[test]
    fn wind_overlay_skips_most_tiles() {
        assert_eq!(super::wind_streak_stride(4.0), 2);
        assert_eq!(super::wind_streak_stride(1.0), 3);
        let stride = 2;
        let keep = (0..8)
            .flat_map(|hx| (0..8).map(move |hy| (hx, hy)))
            .filter(|&(hx, hy)| super::wind_streak_on_lattice(hx, hy, stride))
            .count();
        assert_eq!(keep, 16, "2×2 lattice keeps a quarter of a 8×8 block");
        assert!(super::wind_streak_on_lattice(-2, 0, 2));
        assert!(!super::wind_streak_on_lattice(-1, 0, 2));
    }

    #[test]
    fn haze_seats_include_lake_level_tiles() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 32, 128);
        // Tile hy=20 → world y 80..84. A global sea+4 cut (sea=80 → hy>=21)
        // used to drop this seat and lock the wash to a shelf above the lake.
        h.cells.insert((2, 20), 180.0);
        let seats = haze_paint_seats(&h);
        assert!(
            seats.contains(&(2, 20)),
            "humidity on the live waterline must stay a seat"
        );
    }

    #[test]
    fn resample_wraps_neighbour_seats_at_the_ring() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 16, 64);
        h.wrap_x = true;
        h.cells.insert((0, 5), 400.0);
        let seats = haze_paint_seats(&h);
        assert!(seats.contains(&(3, 5)), "left neighbour wraps to hx_max");
        assert!(
            !seats.iter().any(|&(hx, _)| hx < 0 || hx > 3),
            "raw hx-1 at the ring was the bright seam"
        );
    }

    #[test]
    fn humidity_tile_view_cull_keeps_the_camera_tile() {
        // origin_y such that world y=0 sits at the bottom of a 64px window.
        let origin_x = 0.0;
        let origin_y = 64.0;
        let cell = 4.0;
        assert!(humidity_tile_touches_view(
            0, 0, 4, origin_x, origin_y, cell, 0, false, 64, 128.0, 64.0
        ));
        assert!(
            !humidity_tile_touches_view(
                0, 40, 4, origin_x, origin_y, cell, 0, false, 64, 128.0, 64.0
            ),
            "hy=40 is world y 160 — far above a 64px window"
        );
        assert!(
            !humidity_tile_touches_view(
                30, 0, 4, origin_x, origin_y, cell, 0, false, 256, 128.0, 64.0
            ),
            "hx=30 is world x 120 — past a 128px window"
        );
    }

    #[test]
    fn haze_view_keeps_on_screen_neighbour_of_off_screen_mass() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 64, 128);
        h.cells.insert((20, 0), 180.0);
        let seats = haze_paint_seats_where(&h, |hx, hy| hx == 0 && hy == 0);
        assert!(
            seats.is_empty(),
            "far mass must not invent an on-screen seat without a neighbour"
        );
        h.cells.insert((1, 0), 180.0);
        let seats = haze_paint_seats_where(&h, |hx, hy| hx == 0 && hy == 0);
        assert!(
            seats.contains(&(0, 0)),
            "on-screen neighbour of visible mass stays a bilinear seat"
        );
        assert!(!seats.contains(&(1, 0)));
        assert!(!seats.contains(&(20, 0)));
    }

    #[test]
    fn view_tile_box_covers_every_tile_that_touches_the_camera() {
        let origin_x = 12.0;
        let origin_y = 80.0;
        let cell = 3.0;
        let tc = 4;
        let bedrock = 0;
        let wrap = true;
        let width = 256;
        let sw = 160.0;
        let sh = 90.0;
        let box_ =
            view_tile_box(tc, origin_x, origin_y, cell, bedrock, wrap, width, sw, sh).expect("box");
        let hx_span = width / tc;
        for hx in -4..70 {
            for hy in -4..40 {
                if humidity_tile_touches_view(
                    hx, hy, tc, origin_x, origin_y, cell, bedrock, wrap, width, sw, sh,
                ) {
                    let whx = hx.rem_euclid(hx_span);
                    assert!(
                        box_.contains(whx, hy),
                        "view box dropped a tile the pixel test keeps ({hx},{hy} → {whx})"
                    );
                }
            }
        }
    }

    #[test]
    fn haze_view_box_keeps_the_same_on_screen_seats() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 256, 128);
        h.wrap_x = true;
        h.cells.insert((2, 1), 180.0);
        h.cells.insert((40, 1), 180.0);
        h.cells.insert((2, 20), 180.0);
        let origin_x = 0.0;
        let origin_y = 64.0;
        let cell = 4.0;
        let keep = |hx: i32, hy: i32| {
            humidity_tile_touches_view(
                hx, hy, 4, origin_x, origin_y, cell, 0, true, 256, 128.0, 64.0,
            )
        };
        let old: std::collections::HashSet<_> =
            haze_paint_seats_where(&h, keep).into_iter().collect();
        let box_ =
            view_tile_box(4, origin_x, origin_y, cell, 0, true, 256, 128.0, 64.0).expect("box");
        let new: std::collections::HashSet<_> =
            haze_paint_seats_in_box(&h, &box_).into_iter().collect();
        for seat in &old {
            assert!(
                new.contains(seat),
                "viewport probe dropped seat {seat:?} the key walk kept"
            );
        }
        assert!(new.contains(&(2, 1)));
        assert!(!old.contains(&(40, 1)));
        assert!(!old.contains(&(2, 20)));
    }

    #[test]
    fn drop_top_chunks_below_the_camera_are_leftover() {
        // origin_y=64, cell=4, sh=64 → world y 0 sits at the bottom.
        // cy=-1 tops out on the HUD line (keep). cy=-2 is world y
        // [-128, -65], entirely below that window.
        assert!(
            !chunk_not_below_view(-2, 64.0, 4.0, 0, 64.0),
            "chunk under the HUD must not scan for shafts"
        );
        assert!(
            chunk_not_below_view(-1, 64.0, 4.0, 0, 64.0),
            "the HUD-line chunk can still sliver into view"
        );
        assert!(
            chunk_not_below_view(0, 64.0, 4.0, 0, 64.0),
            "the camera chunk stays"
        );
        assert!(
            chunk_not_below_view(3, 64.0, 4.0, 0, 64.0),
            "high sky above the camera still carves"
        );
    }

    #[test]
    fn view_cell_x_ranges_cover_every_column_that_touches_the_camera() {
        let origin_x = -80.0;
        let cell = 4.0;
        let width = 256;
        let sw = 128.0;
        let (ranges, n) = view_cell_x_ranges(origin_x, cell, true, width, sw);
        for x in 0..width {
            let mut on_screen = false;
            for &copy in &[-1, 0, 1] {
                let sx = origin_x + (x + copy * width) as f32 * cell;
                if sx + cell >= 0.0 && sx <= sw {
                    on_screen = true;
                    break;
                }
            }
            if on_screen {
                assert!(
                    gx_in_ranges(x, &ranges, n),
                    "visible column {x} missing from {ranges:?}"
                );
            }
        }
    }
}
