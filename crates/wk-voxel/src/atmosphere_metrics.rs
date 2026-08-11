//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Shared atmosphere metrics for sky draw and organism light.
//! Spec: `docs/organism/LIGHT.md` (`sky_transmit`) and `docs/SKY.md`.

use wk_material::MaterialId;

use crate::carbon::AMBIENT_ATM_C;
use crate::climate::sky_rgb_at_height;
use crate::clouds::CloudStore;
use crate::grid::World;
use crate::humidity::Humidity;

/// Clear-sky cloud transmit floor so night under storms is not pure black.
pub const CLOUD_TRANSMIT_FLOOR: f32 = 0.28;
/// Max light cut from a single wet raining parcel disk (kept mild — vapour carries mood).
pub const CLOUD_COVER_MAX: f32 = 0.55;
/// Humidity haze term in [`sky_transmit`] (LIGHT.md).
pub const HUMIDITY_SKY_ATTEN: f32 = 0.10;
/// Floor for cheap sun-angle occlusion (never fully black).
pub const SUN_TRANSMIT_FLOOR: f32 = 0.38;

/// Viewport / global weather knobs for sky colour (render + tests).
#[derive(Debug, Clone, Copy, Default)]
pub struct SkyWeatherParams {
    /// 0 = clear, 1 = view fully under raining / thick parcels.
    pub precip_cover: f32,
    /// 0..1 mean humidity mass / tile cap in the sky band.
    pub humidity_mean: f32,
    /// Air temperature bias vs mild reference (°C). Negative = colder.
    pub temp_bias_c: f32,
    /// Atmosphere carbon / ambient (1.0 = default start).
    pub carbon_ratio: f32,
    /// Extra cold bias when precip forms snow in the view (0..1).
    pub snow_bias: f32,
}

/// 1 = clear column, lower under overlapping cloud parcels.
pub fn cloud_sky_transmit(
    clouds: &CloudStore,
    gx: i32,
    wrap_width: Option<i32>,
    downpour_mass: f32,
) -> f32 {
    let mut cover = 0.0f32;
    let x = gx as f32;
    for p in &clouds.parcels {
        let dx = wrapped_dx(x, p.fx, wrap_width);
        let r = p.radius().max(1.0);
        if dx.abs() > r {
            continue;
        }
        let edge = 1.0 - dx.abs() / r;
        let wet = p.wetness_with(downpour_mass);
        // Soft bank, not a hard eclipse — humidity/vapour does the heavy mood work.
        let mut strength = (0.12 + 0.28 * wet) * edge;
        if p.raining {
            strength *= 1.15;
        }
        cover = cover.max(strength.min(CLOUD_COVER_MAX));
    }
    (1.0 - cover).clamp(CLOUD_TRANSMIT_FLOOR, 1.0)
}

/// Cheap sun-angle occlusion: short diagonal probes toward the sun.
///
/// `sun_local` is [`crate::climate::celestial_local_cfg`] (0 dawn left → 1 dusk right).
/// At noon probes are nearly vertical (legacy top-down); at rise/set they lean
/// so cliffs cast a readable lee shadow that crawls as the day advances.
/// Night returns 1 — night dimming stays on `day_factor`.
pub fn sun_sky_transmit(world: &World, gx: i32, gy: i32, sun_local: f32, is_day: bool) -> f32 {
    if !is_day {
        return 1.0;
    }
    let local = sun_local.clamp(0.0, 1.0);
    let slant = (local - 0.5).abs() * 2.0; // 0 noon, 1 at horizon
    let toward = if local < 0.5 { -1 } else { 1 };
    let max_steps = (1 + (slant * 7.0).round() as i32).clamp(1, 8);
    let mut transmit = 1.0f32;
    for s in 1..=max_steps {
        let ox = world.wrap_x(gx + toward * s);
        let rise = ((s as f32) * (1.0 - 0.40 * slant)).max(1.0).round() as i32;
        let oy = gy + rise;
        let blocked = matches!(
            world.get_cell(ox, oy),
            Some(c) if c.material != MaterialId::Air
        ) || matches!(
            world.get_cell(ox, oy + 1),
            Some(c) if c.material != MaterialId::Air
        );
        if blocked {
            // Soft hit — longer slant = deeper lee.
            transmit *= (0.50 - 0.12 * slant).clamp(0.32, 0.55);
            break;
        }
    }
    transmit.clamp(SUN_TRANSMIT_FLOOR, 1.0)
}

/// Per-column surface lee from taller land toward the sun (crawling shadows).
/// `surface_y[i]` = top solid/water y for column `i` in `[0, width)`.
pub fn column_surface_lee(
    surface_y: &[i32],
    gx: i32,
    sun_local: f32,
    is_day: bool,
    wrap: bool,
) -> f32 {
    if !is_day || surface_y.is_empty() {
        return 1.0;
    }
    let n = surface_y.len() as i32;
    let local = sun_local.clamp(0.0, 1.0);
    let slant = (local - 0.5).abs() * 2.0;
    if slant < 0.08 {
        return 1.0; // noon — mostly overhead
    }
    // Occluder lies toward the sun.
    let toward = if local < 0.5 { -1 } else { 1 };
    let max_span = (2 + (slant * 14.0) as i32).clamp(2, 16);
    let my = {
        let i = if wrap {
            gx.rem_euclid(n) as usize
        } else if gx >= 0 && gx < n {
            gx as usize
        } else {
            return 1.0;
        };
        surface_y[i]
    };
    let mut shade = 1.0f32;
    for s in 1..=max_span {
        let ox = gx + toward * s;
        let oi = if wrap {
            ox.rem_euclid(n) as usize
        } else if ox >= 0 && ox < n {
            ox as usize
        } else {
            break;
        };
        let other = surface_y[oi];
        // Taller land toward the sun casts a lee of roughly (height gap) cells.
        let rise = other - my;
        if rise > s / 2 {
            let hit = ((rise - s / 2) as f32 / 10.0).clamp(0.0, 1.0);
            shade *= 1.0 - hit * (0.35 + 0.35 * slant);
        }
    }
    shade.clamp(0.42, 1.0)
}

/// Humidity mass at cell normalized by per-tile cap (0..1).
pub fn humidity_norm_at(humidity: &Humidity, gx: i32, gy: i32) -> f32 {
    (humidity.at_cell(gx, gy) / Humidity::MAX_MASS_PER_TILE).clamp(0.0, 1.0)
}

/// LIGHT.md sky transmit: day × cloud × (1 − 0.1 · humidity).
pub fn sky_transmit(day_factor: f32, cloud_transmit: f32, humidity_norm: f32) -> f32 {
    let day = day_factor.clamp(0.0, 1.0);
    let cloud = cloud_transmit.clamp(CLOUD_TRANSMIT_FLOOR, 1.0);
    let haze = (1.0 - HUMIDITY_SKY_ATTEN * humidity_norm.clamp(0.0, 1.0)).clamp(0.85, 1.0);
    (day * cloud * haze).clamp(0.0, 1.0)
}

/// Convenience: transmit at world column `gx` (sample humidity at `gy`).
pub fn sky_transmit_at(
    day_factor: f32,
    clouds: Option<&CloudStore>,
    humidity: Option<&Humidity>,
    gx: i32,
    gy: i32,
    wrap_width: Option<i32>,
    downpour_mass: f32,
) -> f32 {
    let cloud = match clouds {
        Some(c) => cloud_sky_transmit(c, gx, wrap_width, downpour_mass),
        None => 1.0,
    };
    let hum = match humidity {
        Some(h) => humidity_norm_at(h, gx, gy),
        None => 0.0,
    };
    sky_transmit(day_factor, cloud, hum)
}

/// Fraction of `[x0, x1)` columns under meaningful cloud cover.
///
/// Samples a fixed column budget (not every world column) so max-width
/// worlds stay O(samples × parcels) instead of O(width × parcels).
pub fn precip_cover_fraction(
    clouds: &CloudStore,
    x0: i32,
    x1: i32,
    wrap_width: Option<i32>,
    downpour_mass: f32,
) -> f32 {
    let width = (x1 - x0).max(1);
    if clouds.is_empty() {
        return 0.0;
    }
    // ≤64 probes across the span — enough for sky tint, cheap at width=4096.
    const MAX_SAMPLES: i32 = 64;
    let samples = width.min(MAX_SAMPLES);
    let mut sum = 0.0f32;
    for i in 0..samples {
        let gx = x0 + (i * width) / samples;
        let t = cloud_sky_transmit(clouds, gx, wrap_width, downpour_mass);
        sum += (1.0 - t) / (1.0 - CLOUD_TRANSMIT_FLOOR).max(1e-3);
    }
    (sum / samples as f32).clamp(0.0, 1.0)
}

/// Mean humidity norm in tiles with `hy >= sky_hy_min`.
///
/// Strides large sparse maps so max-width × tall-sky worlds stay cheap.
pub fn humidity_mean_norm(humidity: &Humidity, sky_hy_min: i32) -> f32 {
    if humidity.cells.is_empty() {
        return 0.0;
    }
    let stride = (humidity.cells.len() / 96).max(1);
    let mut sum = 0.0f32;
    let mut n = 0u32;
    for (i, (&(_hx, hy), &mass)) in humidity.cells.iter().enumerate() {
        if i % stride != 0 || hy < sky_hy_min || mass <= 0.0 {
            continue;
        }
        sum += (mass / Humidity::MAX_MASS_PER_TILE).clamp(0.0, 1.0);
        n += 1;
    }
    if n == 0 {
        0.0
    } else {
        (sum / n as f32).clamp(0.0, 1.0)
    }
}

/// Carbon ratio for tint (`atmosphere / ambient`).
pub fn carbon_ratio(atmosphere: f32) -> f32 {
    (atmosphere / AMBIENT_ATM_C.max(1.0)).clamp(0.0, 4.0)
}

/// Weather-modulated sky sample. `height_01`: 0 zenith, 1 horizon.
pub fn sky_rgb_at_height_weather(
    day_night: f32,
    height_01: f32,
    weather: &SkyWeatherParams,
) -> [u8; 3] {
    let mut rgb = sky_rgb_at_height(day_night, height_01);
    let cover = weather.precip_cover.clamp(0.0, 1.0);
    let hum = weather.humidity_mean.clamp(0.0, 1.0);
    let snow = weather.snow_bias.clamp(0.0, 1.0);
    let temp = weather.temp_bias_c.clamp(-20.0, 20.0);
    let carbon = weather.carbon_ratio.clamp(0.0, 4.0);

    // Overcast: prefer vapour/humidity over hard precip eclipse.
    let overcast = (cover * 0.35 + hum * 0.65).clamp(0.0, 1.0);
    if overcast > 0.01 {
        let grey = ((rgb[0] as f32 + rgb[1] as f32 + rgb[2] as f32) / 3.0) as u8;
        let dark = 1.0 - 0.28 * overcast;
        for c in &mut rgb {
            let g = grey as f32;
            let v = *c as f32;
            let mixed = v * (1.0 - 0.45 * overcast) + g * (0.45 * overcast);
            *c = (mixed * dark) as u8;
        }
    }

    // Temperature: cool → mild blue lift (kept modest — avoid underwater cast),
    // warm → amber on day/dusk.
    if temp < -0.5 {
        let t = ((-temp) / 16.0).clamp(0.0, 1.0) * (1.0 - overcast * 0.5);
        rgb[2] = rgb[2].saturating_add((12.0 * t) as u8);
        rgb[0] = (rgb[0] as f32 * (1.0 - 0.06 * t)) as u8;
        rgb[1] = (rgb[1] as f32 * (1.0 - 0.03 * t)) as u8;
    } else if temp > 0.5 && day_night > -0.2 {
        let t = (temp / 16.0).clamp(0.0, 1.0) * (1.0 - overcast * 0.6);
        rgb[0] = rgb[0].saturating_add((22.0 * t) as u8);
        rgb[1] = rgb[1].saturating_add((10.0 * t) as u8);
        rgb[2] = (rgb[2] as f32 * (1.0 - 0.10 * t)) as u8;
    }

    // Snowing air: soft cool grey, not a hard blue wash.
    if snow > 0.05 {
        let t = snow * (0.55 + 0.45 * cover);
        rgb[0] = (rgb[0] as f32 * (1.0 - 0.06 * t)) as u8;
        rgb[1] = (rgb[1] as f32 * (1.0 - 0.02 * t)) as u8;
        rgb[2] = rgb[2].saturating_add((8.0 * t) as u8);
        let dark = 1.0 - 0.06 * t;
        for c in &mut rgb {
            *c = (*c as f32 * dark) as u8;
        }
    }

    // High atmosphere C: slight thick / grey air (subtle).
    if carbon > 1.05 || carbon < 0.85 {
        let thick = if carbon > 1.0 {
            ((carbon - 1.0) / 2.0).clamp(0.0, 1.0) * 0.08
        } else {
            ((1.0 - carbon) / 1.0).clamp(0.0, 1.0) * 0.04
        };
        let grey = ((rgb[0] as f32 + rgb[1] as f32 + rgb[2] as f32) / 3.0) as f32;
        for c in &mut rgb {
            let v = *c as f32;
            *c = (v * (1.0 - thick) + grey * thick) as u8;
        }
    }

    rgb
}

/// Mild weather / colour-temp shift for foreground cells (terrain, water).
///
/// Kept light and partially desaturated so midday sky blue does not paint
/// the whole landscape “underwater”. At night the landscape drops to a deep
/// cool floor with a weak moon ambient so open ground stays readable; lee
/// darkness comes from the moon cast overlay in the app.
pub fn apply_weather_rgb(rgb: [u8; 3], day_night: f32, weather: &SkyWeatherParams) -> [u8; 3] {
    let sky = sky_rgb_at_height_weather(day_night, 0.72, weather);
    let grey = (0.30 * sky[0] as f32 + 0.59 * sky[1] as f32 + 0.11 * sky[2] as f32) as u8;
    // Pull chroma out of the sky sample before mixing into terrain.
    let soft = [
        ((sky[0] as f32) * 0.45 + grey as f32 * 0.55) as u8,
        ((sky[1] as f32) * 0.45 + grey as f32 * 0.55) as u8,
        ((sky[2] as f32) * 0.45 + grey as f32 * 0.55) as u8,
    ];
    // Day gets an even lighter hand; dusk/night can borrow a bit more mood.
    let day = ((day_night + 1.0) * 0.5).clamp(0.0, 1.0);
    let t = 0.12 + 0.14 * (1.0 - day);
    let mut out = [
        (rgb[0] as f32 * (1.0 - t) + soft[0] as f32 * t),
        (rgb[1] as f32 * (1.0 - t) + soft[1] as f32 * t),
        (rgb[2] as f32 * (1.0 - t) + soft[2] as f32 * t),
    ];
    let night = (-day_night).clamp(0.0, 1.0);
    if night > 0.01 {
        // Midnight ≈ 0.15 of daytime; cool moon fill keeps open ground readable.
        let keep = (0.15 + 0.85 * (1.0 - night)).clamp(0.15, 1.0);
        let amb = 0.10 * night;
        let cool = [0.55f32, 0.64, 0.92];
        for i in 0..3 {
            let v = out[i] * keep;
            let moon = v * cool[i] + 22.0 * cool[i];
            out[i] = (v * (1.0 - amb) + moon * amb).clamp(0.0, 255.0);
        }
    }
    [out[0] as u8, out[1] as u8, out[2] as u8]
}

/// Combine column water attenuation with weather + cheap sun lee.
pub fn lit_sky_at(
    world: &World,
    gx: i32,
    gy: i32,
    day_factor: f32,
    clouds: Option<&CloudStore>,
    humidity: Option<&Humidity>,
    wrap_width: Option<i32>,
    downpour_mass: f32,
    sun_local: f32,
    is_day: bool,
    column_sky: f32,
) -> f32 {
    let weather = sky_transmit_at(
        day_factor,
        clouds,
        humidity,
        gx,
        gy,
        wrap_width,
        downpour_mass,
    );
    let sun = sun_sky_transmit(world, gx, gy, sun_local, is_day);
    (column_sky * weather * sun).clamp(0.0, 1.0)
}

fn wrapped_dx(a: f32, b: f32, wrap_width: Option<i32>) -> f32 {
    let mut d = a - b;
    if let Some(w) = wrap_width {
        let w = w as f32;
        if w > 0.0 {
            d = (d + w * 0.5).rem_euclid(w) - w * 0.5;
        }
    }
    d
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::clouds::{CloudParcel, CloudStore, DOWNPOUR_MASS};

    fn raining_parcel(fx: f32, mass: f32) -> CloudParcel {
        CloudParcel {
            fx,
            fy: 100.0,
            mass,
            raining: true,
            on_ridge: false,
            shape_seed: 1,
            cruise_fy: 100.0,
            vis_mass: mass,
            deform: 0.0,
        }
    }

    #[test]
    fn sky_transmit_clear_noon_is_near_one() {
        let t = sky_transmit(1.0, 1.0, 0.0);
        assert!(t > 0.98, "clear noon transmit={t}");
    }

    #[test]
    fn sky_transmit_drops_under_wet_parcel_and_humidity() {
        let mut clouds = CloudStore::new();
        clouds.parcels.push(raining_parcel(10.0, DOWNPOUR_MASS * 1.4));
        let cloud_t = cloud_sky_transmit(&clouds, 10, None, DOWNPOUR_MASS);
        assert!(cloud_t < 0.95, "cloud transmit should drop, got {cloud_t}");
        assert!(cloud_t > CLOUD_TRANSMIT_FLOOR - 0.01);
        let t = sky_transmit(1.0, cloud_t, 0.8);
        assert!(t < cloud_t, "humidity should cut further, t={t} cloud={cloud_t}");
    }

    #[test]
    fn precip_cover_high_when_parcel_spans_view() {
        let mut clouds = CloudStore::new();
        clouds.parcels.push(raining_parcel(16.0, DOWNPOUR_MASS * 1.5));
        let cover = precip_cover_fraction(&clouds, 0, 32, None, DOWNPOUR_MASS);
        assert!(cover > 0.15, "cover={cover}");
    }

    #[test]
    fn sky_transmit_at_clear_vs_storm_column() {
        let mut clouds = CloudStore::new();
        let clear = sky_transmit_at(1.0, Some(&clouds), None, 10, 80, None, DOWNPOUR_MASS);
        clouds.parcels.push(raining_parcel(10.0, DOWNPOUR_MASS * 1.5));
        let storm = sky_transmit_at(1.0, Some(&clouds), None, 10, 80, None, DOWNPOUR_MASS);
        assert!(clear > 0.98);
        assert!(storm < clear, "clear={clear} storm={storm}");
    }

    #[test]
    fn sun_transmit_lower_in_lee_at_low_sun() {
        use crate::cell::Cell;
        use crate::grid::World;
        let mut w = World::new(1);
        // Wall at x=5; sample to the right. Dawn sun is left → probes toward −x.
        for y in 10..20 {
            w.set_cell(5, y, Cell::solid(MaterialId::Stone));
        }
        w.set_cell(8, 12, Cell::air());
        let noon = sun_sky_transmit(&w, 8, 12, 0.5, true);
        let dawn = sun_sky_transmit(&w, 8, 12, 0.05, true);
        assert!(dawn < noon, "dawn lee={dawn} noon={noon}");
    }

    #[test]
    fn weather_sky_darker_under_high_precip_cover() {
        let clear = sky_rgb_at_height_weather(
            1.0,
            0.5,
            &SkyWeatherParams {
                precip_cover: 0.0,
                ..Default::default()
            },
        );
        let storm = sky_rgb_at_height_weather(
            1.0,
            0.5,
            &SkyWeatherParams {
                precip_cover: 0.9,
                humidity_mean: 0.5,
                ..Default::default()
            },
        );
        let clear_l = clear[0] as u32 + clear[1] as u32 + clear[2] as u32;
        let storm_l = storm[0] as u32 + storm[1] as u32 + storm[2] as u32;
        assert!(storm_l < clear_l, "storm={storm:?} clear={clear:?}");
    }
}
