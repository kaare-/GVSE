//! Temperature overlay (T) colour scale.
//!
//! Three live stops — cold / mid / hot — remap the authored palette onto a
//! band you can zoom. Defaults match the old fixed climate window
//! (−40 → 18 → 36 °C) so T looks the same until you tune it.

use macroquad::color::Color;

/// Authored palette ends (used after remapping the live window).
pub const PALETTE_LO: f32 = -40.0;
pub const PALETTE_MID: f32 = 18.0;
pub const PALETTE_HI: f32 = 36.0;

/// Slider travel. Wide enough for ice and a hot boiler.
pub const SLIDER_MIN: f32 = -80.0;
pub const SLIDER_MAX: f32 = 200.0;

const MIN_GAP: f32 = 0.5;

/// Default climate band (same as the old fixed stops).
pub const DEFAULT_LO: f32 = PALETTE_LO;
pub const DEFAULT_MID: f32 = PALETTE_MID;
pub const DEFAULT_HI: f32 = PALETTE_HI;

/// Tight band around the default boil point for geothermal soaks.
pub const BOIL_LO: f32 = 80.0;
pub const BOIL_MID: f32 = 100.0;
pub const BOIL_HI: f32 = 140.0;

const STOPS: &[(f32, u8, u8, u8)] = &[
    (PALETTE_LO, 230, 240, 255),
    (-20.0, 70, 90, 200),
    (0.0, 40, 190, 230),
    (12.0, 70, 200, 120),
    (PALETTE_MID, 210, 215, 70),
    (28.0, 235, 140, 35),
    (PALETTE_HI, 220, 40, 30),
];

fn lerp_u8(a: u8, b: u8, t: f32) -> u8 {
    (a as f32 + (b as f32 - a as f32) * t.clamp(0.0, 1.0)).round() as u8
}

/// Keep `lo ≤ mid ≤ hi` with a small gap so the ramp never collapses.
pub fn order_stops(lo: &mut f32, mid: &mut f32, hi: &mut f32) {
    *lo = lo.clamp(SLIDER_MIN, SLIDER_MAX);
    *mid = mid.clamp(SLIDER_MIN, SLIDER_MAX);
    *hi = hi.clamp(SLIDER_MIN, SLIDER_MAX);
    if *lo > *mid {
        *mid = *lo;
    }
    if *mid > *hi {
        *hi = *mid;
    }
    if *mid - *lo < MIN_GAP {
        *mid = (*lo + MIN_GAP).min(SLIDER_MAX);
        if *hi < *mid {
            *hi = *mid;
        }
    }
    if *hi - *mid < MIN_GAP {
        *hi = (*mid + MIN_GAP).min(SLIDER_MAX);
        if *hi - *mid < MIN_GAP {
            *mid = (*hi - MIN_GAP).max(SLIDER_MIN);
            if *mid < *lo {
                *lo = *mid;
            }
        }
    }
}

/// Map a live °C through the three-point window onto the authored palette.
///
/// Below mid uses the cold half (−40…18); above mid uses the warm half
/// (18…36). Pulling the window in stretches contrast in that band.
pub fn remap_temp(temp_c: f32, lo: f32, mid: f32, hi: f32) -> f32 {
    let mut lo = lo;
    let mut mid = mid;
    let mut hi = hi;
    order_stops(&mut lo, &mut mid, &mut hi);
    if temp_c <= mid {
        let span = (mid - lo).max(MIN_GAP);
        let u = ((temp_c - lo) / span).clamp(0.0, 1.0);
        PALETTE_LO + u * (PALETTE_MID - PALETTE_LO)
    } else {
        let span = (hi - mid).max(MIN_GAP);
        let u = ((temp_c - mid) / span).clamp(0.0, 1.0);
        PALETTE_MID + u * (PALETTE_HI - PALETTE_MID)
    }
}

fn color_at_palette_c(temp_c: f32) -> Color {
    let t = temp_c.clamp(STOPS[0].0, STOPS[STOPS.len() - 1].0);
    let mut i = 0;
    while i + 1 < STOPS.len() && t > STOPS[i + 1].0 {
        i += 1;
    }
    let (t0, r0, g0, b0) = STOPS[i];
    let (t1, r1, g1, b1) = STOPS[(i + 1).min(STOPS.len() - 1)];
    let u = if (t1 - t0).abs() < 1e-3 {
        0.0
    } else {
        ((t - t0) / (t1 - t0)).clamp(0.0, 1.0)
    };
    Color::from_rgba(lerp_u8(r0, r1, u), lerp_u8(g0, g1, u), lerp_u8(b0, b1, u), 135)
}

/// Overlay colour for `temp_c` given the live three-point window.
pub fn color(temp_c: f32, lo: f32, mid: f32, hi: f32) -> Color {
    color_at_palette_c(remap_temp(temp_c, lo, mid, hi))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_window_is_identity_on_the_palette() {
        assert!((remap_temp(-40.0, DEFAULT_LO, DEFAULT_MID, DEFAULT_HI) - PALETTE_LO).abs() < 1e-3);
        assert!((remap_temp(18.0, DEFAULT_LO, DEFAULT_MID, DEFAULT_HI) - PALETTE_MID).abs() < 1e-3);
        assert!((remap_temp(36.0, DEFAULT_LO, DEFAULT_MID, DEFAULT_HI) - PALETTE_HI).abs() < 1e-3);
        assert!((remap_temp(0.0, DEFAULT_LO, DEFAULT_MID, DEFAULT_HI) - 0.0).abs() < 0.4);
    }

    #[test]
    fn boil_window_puts_100c_on_the_mid_yellow() {
        let t = remap_temp(100.0, BOIL_LO, BOIL_MID, BOIL_HI);
        assert!(
            (t - PALETTE_MID).abs() < 1e-3,
            "100 °C should sit on the mid stop, got {t}"
        );
        let cold = remap_temp(80.0, BOIL_LO, BOIL_MID, BOIL_HI);
        let hot = remap_temp(140.0, BOIL_LO, BOIL_MID, BOIL_HI);
        assert!((cold - PALETTE_LO).abs() < 1e-3);
        assert!((hot - PALETTE_HI).abs() < 1e-3);
        // 90 °C is halfway through the cold half of a 80–100 window.
        let mid_cold = remap_temp(90.0, BOIL_LO, BOIL_MID, BOIL_HI);
        assert!(
            (mid_cold - (PALETTE_LO + PALETTE_MID) * 0.5).abs() < 0.2,
            "narrow band must stretch contrast (got {mid_cold})"
        );
    }

    #[test]
    fn order_stops_keeps_lo_mid_hi() {
        let mut lo = 40.0;
        let mut mid = 10.0;
        let mut hi = 20.0;
        order_stops(&mut lo, &mut mid, &mut hi);
        assert!(lo <= mid && mid <= hi);
        assert!(mid - lo >= MIN_GAP - 1e-3);
        assert!(hi - mid >= MIN_GAP - 1e-3);
    }

    #[test]
    fn values_outside_the_window_clamp_to_the_ends() {
        let t = remap_temp(-20.0, BOIL_LO, BOIL_MID, BOIL_HI);
        assert!((t - PALETTE_LO).abs() < 1e-3);
        let t = remap_temp(180.0, BOIL_LO, BOIL_MID, BOIL_HI);
        assert!((t - PALETTE_HI).abs() < 1e-3);
    }
}
