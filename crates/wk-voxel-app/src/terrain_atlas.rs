//! Viewport terrain atlas fill — chunk-aware + parallel columns.
//!
//! Hot path for interactive FPS: cost must track *occupied* on-screen cells,
//! not `sw×sh` HashMap probes through empty sky.

use rayon::prelude::*;
use wk_material::MaterialId;
use wk_voxel::{
    apply_weather_rgb, continental_surface_y, is_standing_water, ChunkCoord, SkyWeatherParams,
    World, CHUNK_CELLS_H, CHUNK_CELLS_W, GRAIN_REPOSE_HAZE_MAX,
};

use crate::atmosphere::{
    apply_celestial_key_rgb, celestial_exposure, is_exposed_surface_top, terrain_key_falloff,
};
use crate::palette::cell_color;

/// Disjoint per-column writes into a row-major atlas (safe under rayon).
#[derive(Clone, Copy)]
struct PixelBuf {
    ptr: *mut [u8; 4],
    w: usize,
    h: usize,
}

// SAFETY: each task writes only its `ax` column; indices never alias.
unsafe impl Send for PixelBuf {}
unsafe impl Sync for PixelBuf {}

impl PixelBuf {
    #[inline]
    fn set(self, ax: usize, img_y: usize, rgba: [u8; 4]) {
        debug_assert!(ax < self.w && img_y < self.h);
        unsafe {
            *self.ptr.add(img_y * self.w + ax) = rgba;
        }
    }
}

/// Fill `pixels` (row-major, `w×h`) for unwrapped viewport columns `x0..x0+w`.
///
/// Skips missing chunks (empty sky) in 64-row bands and paints columns on
/// rayon so large windows stay near the 16.6 ms frame budget.
pub fn fill_terrain_atlas(
    pixels: &mut [[u8; 4]],
    w: usize,
    h: usize,
    world: &World,
    x0: i32,
    y_min: i32,
    y_max: i32,
    wrap_x: bool,
    width_cols: i32,
    sea_level_y: i32,
    dn_fg: f32,
    sky_weather: &SkyWeatherParams,
    sun_local: f32,
    sun_day: bool,
) {
    assert_eq!(pixels.len(), w.saturating_mul(h));
    if w == 0 || h == 0 || y_max <= y_min {
        return;
    }
    // Transparent clear — sky / ridges show through.
    pixels.fill([0, 0, 0, 0]);

    let buf = PixelBuf {
        ptr: pixels.as_mut_ptr(),
        w,
        h,
    };
    let weather = *sky_weather;

    (0..w).into_par_iter().for_each(|ax| {
        let x_unwrapped = x0 + ax as i32;
        let x = if wrap_x {
            world.wrap_x(x_unwrapped)
        } else if x_unwrapped < 0 || x_unwrapped >= width_cols {
            return;
        } else {
            x_unwrapped
        };
        fill_column(
            buf,
            ax,
            world,
            x,
            y_min,
            y_max,
            width_cols,
            sea_level_y,
            dn_fg,
            &weather,
            sun_local,
            sun_day,
        );
    });
}

fn fill_column(
    buf: PixelBuf,
    ax: usize,
    world: &World,
    x: i32,
    y_min: i32,
    y_max: i32,
    width_cols: i32,
    sea_level_y: i32,
    dn_fg: f32,
    sky_weather: &SkyWeatherParams,
    sun_local: f32,
    sun_day: bool,
) {
    // Stamped sky is full of Air chunks — don't walk empty sky above the
    // worldgen crest / sea (tall sky_ceiling used to dominate fill cost).
    let width = width_cols.max(1);
    let paint_ceiling = continental_surface_y(world.seed.0, x, sea_level_y, width)
        .max(sea_level_y)
        + 6;
    let y_scan_max = y_max.min(paint_ceiling + 1);
    if y_scan_max <= y_min {
        return;
    }

    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;
    let cx = x.div_euclid(cw);
    let lx = x.rem_euclid(cw) as usize;
    let cy_max = (y_scan_max - 1).div_euclid(ch);
    let cy_min = y_min.div_euclid(ch);

    let mut stack_exposure = 0.0f32;
    let mut stack_depth = -1i32;
    let mut stack_water = false;

    for cy in (cy_min..=cy_max).rev() {
        let Some(chunk) = world.chunks.get(&ChunkCoord::new(cx, cy)) else {
            // Missing chunk = empty air band — no per-cell HashMap probes.
            stack_depth = -1;
            continue;
        };
        let chunk_y0 = cy * ch;
        let y_hi = (chunk_y0 + ch).min(y_scan_max);
        let y_lo = chunk_y0.max(y_min);
        if y_hi <= y_lo {
            continue;
        }

        for y in (y_lo..y_hi).rev() {
            let ly = (y - chunk_y0) as usize;
            let cell = chunk.get(lx, ly);
            let img_y = (y_max - 1 - y) as usize;

            if cell.material == MaterialId::Air {
                if cell.sat.is_empty()
                    || cell.sat.0 <= GRAIN_REPOSE_HAZE_MAX
                    || (y > sea_level_y && !is_standing_water(world, x, y))
                {
                    stack_depth = -1;
                    continue;
                }
            }

            let waterish = cell.material == MaterialId::Water
                || (cell.material == MaterialId::Air && is_standing_water(world, x, y));
            if stack_depth < 0 {
                if is_exposed_surface_top(world, x, y) {
                    stack_exposure = celestial_exposure(world, x, y, sun_local);
                    stack_water = waterish;
                    stack_depth = 0;
                } else {
                    stack_exposure = 0.0;
                    stack_water = waterish;
                    stack_depth = 0;
                }
            } else {
                stack_depth += 1;
                stack_water = stack_water || waterish;
            }

            let [r0, g0, b0] = cell_color(cell);
            let [mut r, mut g, mut b] = apply_weather_rgb([r0, g0, b0], dn_fg, sky_weather);
            let falloff = terrain_key_falloff(stack_depth, stack_water, sun_day);
            let key = stack_exposure * falloff;
            if key > 0.03 {
                let lit = apply_celestial_key_rgb([r, g, b], key, sun_local, sun_day);
                r = lit[0];
                g = lit[1];
                b = lit[2];
            }
            if img_y < buf.h {
                buf.set(ax, img_y, [r, g, b, 255]);
            }
        }
    }
}
