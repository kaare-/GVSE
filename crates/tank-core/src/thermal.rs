//! Phase 4 — the thermal cycle (`evolution-system.md` §4) and the coarse
//! `T_local` field that THERMO cells heat (`cell-language.md` §a.7).
//!
//! ```text
//! L        = day_factor · light_slider
//! T_direct = 10 °C · L
//! S       += (T_direct − S) / τ          one scalar per tank
//! τ        = day · (0.02·air + 0.08·water + 0.12·sand + 0.20·rock + 0.10·bio)
//! T_tank   = T_base + 0.4·T_direct + 0.6·S
//! T(x, y)  = T_tank + T_local / 4        T_local: quarter degrees, THERMO only
//! ```
//!
//! The sun heats, the flashlight does not (K4#7). "Always day" holds the
//! cycle at midday.

use crate::cell::Cell;
use crate::consts::{
    CYCLE_TICKS, DT_LIGHT_C, TAU_AIR, TAU_BIO, TAU_ROCK, TAU_SAND, TAU_WATER, T_DIRECT_WEIGHT,
    T_LOCAL_CAP_Q2, T_LOCAL_DIFFUSE_EVERY, T_LOCAL_RELAX_DIV, T_LOCAL_RELAX_EVERY, T_STORED_WEIGHT,
    WATER_SAT_RENDER,
};
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::pixels::CoarseField;
use wk_material::MaterialId;

/// Volume fractions of the tank by thermal class.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct ThermalMass {
    pub air: f32,
    pub water: f32,
    pub sand: f32,
    pub rock: f32,
    pub bio: f32,
}

impl ThermalMass {
    /// Count the visible physics cells; `live_cells` biomass is one pixel
    /// each (a ninth of a physics cell).
    pub fn measure(grid: &PhysicsGrid, live_cells: usize) -> Self {
        let w = grid.visible_w();
        let h = grid.visible_h();
        let total = (w as f32 * h as f32).max(1.0);
        let (mut air, mut water, mut sand, mut rock) = (0u32, 0u32, 0u32, 0u32);
        for vy in 0..h {
            for vx in 0..w {
                let c = grid.get_visible(vx, vy);
                if c.is_open() {
                    if c.sat >= WATER_SAT_RENDER {
                        water += 1;
                    } else {
                        air += 1;
                    }
                } else if c.is_glass() {
                    rock += 1;
                } else {
                    match c.id() {
                        MaterialId::Sand
                        | MaterialId::Soil
                        | MaterialId::Gravel
                        | MaterialId::Organic => sand += 1,
                        _ => rock += 1,
                    }
                }
            }
        }
        let bio = (live_cells as f32 / 9.0 / total).min(0.5);
        Self {
            air: air as f32 / total,
            water: water as f32 / total,
            sand: sand as f32 / total,
            rock: rock as f32 / total,
            bio,
        }
    }

    /// Stored-heat time constant in ticks (never under one second).
    pub fn tau_ticks(&self) -> f32 {
        let f = TAU_AIR * self.air
            + TAU_WATER * self.water
            + TAU_SAND * self.sand
            + TAU_ROCK * self.rock
            + TAU_BIO * self.bio;
        (CYCLE_TICKS as f32 * f).max(30.0)
    }
}

/// Solar heating for a light level `l` (day factor × slider, 0..1).
pub fn t_direct(l: f32) -> f32 {
    DT_LIGHT_C * l.clamp(0.0, 1.0)
}

/// One tick of stored-heat relaxation toward `t_direct`.
pub fn relax_stored(stored: f32, t_direct: f32, tau_ticks: f32) -> f32 {
    stored + (t_direct - stored) / tau_ticks.max(1.0)
}

/// `T_tank` from the three terms.
pub fn t_tank(t_base: f32, t_direct: f32, stored: f32) -> f32 {
    (t_base + T_DIRECT_WEIGHT * t_direct + T_STORED_WEIGHT * stored).clamp(-10.0, 60.0)
}

/// Degrees above ambient at a coarse index.
pub fn local_c(coarse: &CoarseField, ci: usize) -> f32 {
    coarse
        .t_local
        .get(ci)
        .map(|&q| q as f32 / 4.0)
        .unwrap_or(0.0)
}

/// THERMO heat: add `q2` quarter degrees at a coarse index, capped.
pub fn inject(coarse: &mut CoarseField, ci: usize, q2: u8) {
    if let Some(v) = coarse.t_local.get_mut(ci) {
        *v = (*v as i16 + q2 as i16).min(T_LOCAL_CAP_Q2 as i16) as i8;
    }
}

/// Leak toward ambient and spread to the four neighbours. Integer and
/// in-place-deterministic (a scratch copy drives the diffusion).
pub fn step_local(coarse: &mut CoarseField, tick: u64) {
    if tick % T_LOCAL_RELAX_EVERY == 0 {
        for (i, v) in coarse.t_local.iter_mut().enumerate() {
            let q = *v as i16;
            if q <= 0 {
                *v = 0;
                continue;
            }
            // Proportional leak with hash dithering for the fraction,
            // so small halos fade at the same τ as big ones.
            let frac = (q % T_LOCAL_RELAX_DIV) as u64;
            let dither = hash_u64(0x7E_C0, tick, i as u64, 0) % T_LOCAL_RELAX_DIV as u64;
            let drop = q / T_LOCAL_RELAX_DIV + u8::from(dither < frac) as i16;
            *v = (q - drop).max(0) as i8;
        }
    }
    if tick % T_LOCAL_DIFFUSE_EVERY != 0 {
        return;
    }
    let w = coarse.w as usize;
    let h = coarse.h as usize;
    if w == 0 || h == 0 || coarse.t_local.iter().all(|&v| v == 0) {
        return;
    }
    let src = coarse.t_local.clone();
    for y in 0..h {
        for x in 0..w {
            let i = y * w + x;
            let here = src[i] as i16;
            let mut sum = 0i16;
            if x > 0 {
                sum += src[i - 1] as i16 - here;
            }
            if x + 1 < w {
                sum += src[i + 1] as i16 - here;
            }
            if y > 0 {
                sum += src[i - w] as i16 - here;
            }
            if y + 1 < h {
                sum += src[i + w] as i16 - here;
            }
            // A sixteenth of each gradient per pass: stable, conservative
            // up to rounding, and the centre keeps most of its heat.
            let next = (here + sum / 16).clamp(0, T_LOCAL_CAP_Q2 as i16);
            coarse.t_local[i] = next as i8;
        }
    }
}

/// Warm-haze tint for an open pixel: coral weight 0..=255 from `q2`.
pub fn haze_weight(q2: i8) -> u8 {
    (q2.max(0) as u32 * 3).min(96) as u8
}

/// Whether a material counts as "rock" for the thermal mass (also the
/// condensation surfaces of Phase 4's humidity field).
pub fn is_rock(cell: Cell) -> bool {
    cell.is_glass() || matches!(cell.id(), MaterialId::Stone | MaterialId::Bedrock)
}
