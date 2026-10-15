//! Tank clock. Noon is tick 0. Light is a cosine over 20 minutes,
//! then a flat 0.1 floor for 20 minutes of night. The clock advances
//! only when the sim ticks; nothing here reads the wall clock.

use crate::consts::{CYCLE_TICKS, NIGHT_FLOOR};

/// Phase in `[0, 1)`: 0 = noon, 0.25 = dusk, 0.5 = midnight, 0.75 = dawn.
pub fn phase(tick: u64) -> f32 {
    (tick % CYCLE_TICKS) as f32 / CYCLE_TICKS as f32
}

/// `true` during the 20-minute light half (dawn inclusive, dusk exclusive).
pub fn is_day(tick: u64) -> bool {
    let p = phase(tick);
    !(0.25..0.75).contains(&p)
}

/// Organism light in `(NIGHT_FLOOR..=1]`. 1 at noon, floor all night.
pub fn day_factor(tick: u64) -> f32 {
    let p = phase(tick);
    const HALF: f32 = 0.25;
    if p < HALF {
        let t = p / HALF;
        (t * std::f32::consts::FRAC_PI_2).cos().max(NIGHT_FLOOR)
    } else if p >= 1.0 - HALF {
        let t = (p - (1.0 - HALF)) / HALF;
        (t * std::f32::consts::FRAC_PI_2).sin().max(NIGHT_FLOOR)
    } else {
        NIGHT_FLOOR
    }
}

/// `"day"` or `"night"`, for the harness.
pub fn phase_name(tick: u64) -> &'static str {
    if is_day(tick) {
        "day"
    } else {
        "night"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::{CYCLE_TICKS, DAY_TICKS, NIGHT_FLOOR};

    #[test]
    fn noon_is_full_light_and_midnight_is_the_floor() {
        assert!((day_factor(0) - 1.0).abs() < 1e-4);
        assert!(is_day(0));
        assert_eq!(phase_name(0), "day");

        let midnight = DAY_TICKS;
        assert!((phase(midnight) - 0.5).abs() < 1e-4);
        assert!((day_factor(midnight) - NIGHT_FLOOR).abs() < 1e-4);
        assert!(!is_day(midnight));
        assert_eq!(phase_name(midnight), "night");
    }

    #[test]
    fn cosine_is_about_root_two_at_mid_afternoon() {
        // phase 0.125 → halfway from noon to dusk → cos(π/4).
        let mid = CYCLE_TICKS / 8;
        let got = day_factor(mid);
        assert!(
            (got - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.02,
            "{got}"
        );
    }

    #[test]
    fn cycle_wraps_back_to_noon() {
        assert!((day_factor(CYCLE_TICKS) - 1.0).abs() < 1e-4);
        assert!(is_day(CYCLE_TICKS));
    }

    #[test]
    fn lengths_are_twenty_minutes_at_30_hz() {
        assert_eq!(DAY_TICKS, 20 * 60 * 30);
        assert_eq!(CYCLE_TICKS, DAY_TICKS * 2);
    }
}
