//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Shared day/night clock for light, temperature, and sky drawing.
//!
//! Phase convention matches the original Set A `day_factor`:
//! **tick 0 ≈ noon**, tick `DEMO_DAY_TICKS/2` ≈ midnight.

/// Demo day length in ticks (shorter than column-GVSE).
pub const DEMO_DAY_TICKS: u64 = 1_200;

/// Phase in `[0, 1)` — 0 ≈ noon, 0.5 ≈ midnight.
pub fn phase_fraction(tick: u64) -> f32 {
    (tick % DEMO_DAY_TICKS) as f32 / DEMO_DAY_TICKS as f32
}

/// Raised cosine for organism light / upkeep: ~1 at noon, floor 0.08 at night.
pub fn day_factor(tick: u64) -> f32 {
    let t = phase_fraction(tick);
    let angle = t * std::f32::consts::TAU;
    (angle.cos() * 0.5 + 0.5).clamp(0.08, 1.0)
}

/// Signed day/night drive: +1 noon, −1 midnight, 0 at dawn/dusk.
pub fn day_night_factor(tick: u64) -> f32 {
    let t = phase_fraction(tick);
    (t * std::f32::consts::TAU).cos()
}

pub fn is_daytime(tick: u64) -> bool {
    day_night_factor(tick) >= 0.0
}

/// Progress 0→1 along the current body's sky arc (rise→set).
///
/// Day spans phase `0.75 → 1.0 → 0.0 → 0.25` (dawn→noon→dusk).
/// Night spans `0.25 → 0.75` (dusk→midnight→dawn).
pub fn celestial_local(tick: u64) -> f32 {
    let phase = phase_fraction(tick);
    if is_daytime(tick) {
        if phase >= 0.75 {
            (phase - 0.75) / 0.5
        } else {
            // phase in [0, 0.25]
            (phase + 0.25) / 0.5
        }
        .clamp(0.0, 1.0)
    } else {
        ((phase - 0.25) / 0.5).clamp(0.0, 1.0)
    }
}

/// Screen-space arc for the active celestial body.
pub fn celestial_screen_pos(tick: u64, sw: f32, sh: f32) -> (f32, f32) {
    let local = celestial_local(tick);
    let x = 0.08 * sw + local * 0.84 * sw;
    let y = 0.10 * sh + 0.26 * sh * (1.0 - (local * std::f32::consts::PI).sin());
    (x, y)
}

/// Sky RGB for a day/night factor in `[-1, 1]`.
pub fn sky_rgb(day_night: f32) -> [u8; 3] {
    let day = [0x87u8, 0xCE, 0xEB];
    let dusk = [0xC4u8, 0x6A, 0x3A];
    let night = [0x0Bu8, 0x10, 0x28];
    let t = day_night.clamp(-1.0, 1.0);
    if t >= 0.25 {
        day
    } else if t >= -0.25 {
        let u = (t + 0.25) / 0.5;
        lerp_rgb(dusk, day, u)
    } else {
        let u = ((t + 1.0) / 0.75).clamp(0.0, 1.0);
        lerp_rgb(night, dusk, u)
    }
}

/// Vertical sky sample: `height_01` = 0 at zenith (top), 1 at horizon.
pub fn sky_rgb_at_height(day_night: f32, height_01: f32) -> [u8; 3] {
    let base = sky_rgb(day_night);
    let h = height_01.clamp(0.0, 1.0);
    let zenith_darken = if day_night >= 0.0 {
        1.0 - 0.10 * (1.0 - h)
    } else {
        1.0 - 0.40 * (1.0 - h)
    };
    [
        (base[0] as f32 * zenith_darken) as u8,
        (base[1] as f32 * zenith_darken) as u8,
        (base[2] as f32 * zenith_darken) as u8,
    ]
}

fn lerp_rgb(a: [u8; 3], b: [u8; 3], t: f32) -> [u8; 3] {
    let t = t.clamp(0.0, 1.0);
    [
        (a[0] as f32 + (b[0] as f32 - a[0] as f32) * t) as u8,
        (a[1] as f32 + (b[1] as f32 - a[1] as f32) * t) as u8,
        (a[2] as f32 + (b[2] as f32 - a[2] as f32) * t) as u8,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn noon_is_bright_midnight_is_dim() {
        let noon = day_factor(0);
        let midnight = day_factor(DEMO_DAY_TICKS / 2);
        assert!(noon > 0.9, "noon={noon}");
        assert!(midnight < 0.2, "midnight={midnight}");
        assert!(day_night_factor(0) > 0.9);
        assert!(day_night_factor(DEMO_DAY_TICKS / 2) < -0.9);
    }

    #[test]
    fn celestial_local_covers_arc() {
        // Dawn ≈ phase 0.75 → local ~0; noon phase 0 → local ~0.5.
        let dawn = celestial_local((DEMO_DAY_TICKS as f32 * 0.75) as u64);
        let noon = celestial_local(0);
        assert!(dawn < 0.1, "dawn={dawn}");
        assert!((noon - 0.5).abs() < 0.05, "noon local={noon}");
    }
}
