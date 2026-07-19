//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Atmospheric water heatmap.
//!
//! Coarse-resolution sparse map of "water mass currently in the air"
//! per `tile_cols × tile_cols` block of world cells. Evaporation
//! routes removed cell saturation into this heatmap so the sim stays
//! mass-conservative even when water leaves the ground.
//!
//! The heatmap is intentionally decoupled from the cell grid so it
//! can run at its own resolution — coarser than cells, matching the
//! design doc's "temperature/humidity/wind sampled at 4×4 tiles"
//! plan. Diffusion is a straight two-neighbour (right + up) pairwise
//! filter; combined with symmetric application that's the standard
//! isotropic diffusion stencil in a form that trivially conserves
//! mass across a snapshot pass.
//!
//! Callers should set [`Humidity::bounds`] to the stamped world so
//! diffusion cannot grow an unbounded sparse haze outside the map.
//! Diffusion itself is also meant to run on a schedule (see
//! [`humidity_diffuse_due`]) — matching column-GVSE's `HumidityField`
//! period — not every physics tick.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// Inclusive tile-coordinate rectangle.
///
/// When set on a [`Humidity`], diffusion stays inside the box.
/// Vertical edges are Neumann (no-flux). Horizontal edges wrap when
/// [`Humidity::wrap_x`] is set (ring worlds).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct TileBounds {
    pub hx_min: i32,
    pub hx_max: i32,
    pub hy_min: i32,
    pub hy_max: i32,
}

impl TileBounds {
    /// Tile box covering world cells `[x0, x1) × [y0, y1)`.
    pub fn from_world_cells(tile_cols: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let tc = tile_cols.max(1);
        let x_lo = x0.min(x1 - 1);
        let x_hi = (x1 - 1).max(x0);
        let y_lo = y0.min(y1 - 1);
        let y_hi = (y1 - 1).max(y0);
        Self {
            hx_min: x_lo.div_euclid(tc),
            hx_max: x_hi.div_euclid(tc),
            hy_min: y_lo.div_euclid(tc),
            hy_max: y_hi.div_euclid(tc),
        }
    }

    pub fn contains(self, hx: i32, hy: i32) -> bool {
        hx >= self.hx_min && hx <= self.hx_max && hy >= self.hy_min && hy <= self.hy_max
    }

    pub fn tile_capacity(self) -> usize {
        let w = (self.hx_max - self.hx_min + 1).max(0) as usize;
        let h = (self.hy_max - self.hy_min + 1).max(0) as usize;
        w.saturating_mul(h)
    }
}

/// Cadence for atmospheric diffusion — same numbers as column-GVSE
/// `SubsystemId::HumidityField` (`period: 20`, `phase: 3`).
pub const HUMIDITY_DIFFUSE_PERIOD: u64 = 20;
pub const HUMIDITY_DIFFUSE_PHASE: u64 = 3;

/// True on ticks when humidity diffusion should run.
pub fn humidity_diffuse_due(tick: u64) -> bool {
    tick % HUMIDITY_DIFFUSE_PERIOD == HUMIDITY_DIFFUSE_PHASE
}

/// A sparse 2D heatmap keyed by tile coordinates. Each tile covers
/// `tile_cols` × `tile_cols` world cells. Missing keys are implicit
/// zero — a fresh atmosphere is dry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Humidity {
    /// World cells per tile side. Must be ≥ 1. Typical value: 4.
    pub tile_cols: i32,
    /// Water mass per tile. Same units as [`crate::cell::Sat`] (a `u8`
    /// on 0..255) but stored as `f32` so diffusion can accumulate
    /// fractional deltas without quantisation error.
    pub cells: HashMap<(i32, i32), f32>,
    /// Optional hard clamp on tile keys. `None` leaves diffusion
    /// unbounded (unit-test convenience only — production worlds
    /// should always set this).
    pub bounds: Option<TileBounds>,
    /// When true (and [`Self::bounds`] is set), horizontal diffusion
    /// wraps at `hx_min`/`hx_max` so the atmosphere joins on a ring.
    pub wrap_x: bool,
}

impl Humidity {
    pub fn new(tile_cols: i32) -> Self {
        Self {
            tile_cols: tile_cols.max(1),
            cells: HashMap::new(),
            bounds: None,
            wrap_x: false,
        }
    }

    /// Convenience: humidity map pre-clamped to a stamped world's
    /// cell rectangle `[x0, x1) × [y0, y1)`.
    pub fn with_world_bounds(tile_cols: i32, x0: i32, y0: i32, x1: i32, y1: i32) -> Self {
        let mut h = Self::new(tile_cols);
        h.bounds = Some(TileBounds::from_world_cells(h.tile_cols, x0, y0, x1, y1));
        h
    }

    /// Tile coord for a world cell.
    pub fn tile_of(&self, gx: i32, gy: i32) -> (i32, i32) {
        (gx.div_euclid(self.tile_cols), gy.div_euclid(self.tile_cols))
    }

    fn accepts(&self, hx: i32, hy: i32) -> bool {
        self.bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
    }

    /// Neighbour tile in +x / −x, wrapping horizontally on ring maps.
    fn wrap_hx(&self, hx: i32) -> Option<i32> {
        match self.bounds {
            Some(b) if self.wrap_x => {
                let w = b.hx_max - b.hx_min + 1;
                if w <= 0 {
                    return None;
                }
                Some(b.hx_min + (hx - b.hx_min).rem_euclid(w))
            }
            Some(b) => {
                if hx >= b.hx_min && hx <= b.hx_max {
                    Some(hx)
                } else {
                    None
                }
            }
            None => Some(hx),
        }
    }

    /// Deposit `mass` at world cell `(gx, gy)`.
    ///
    /// Deposits outside [`Self::bounds`] are dropped (the cell grid
    /// should not evaporate outside the stamped world).
    pub fn add(&mut self, gx: i32, gy: i32, mass: f32) {
        if mass == 0.0 {
            return;
        }
        let key = self.tile_of(gx, gy);
        if !self.accepts(key.0, key.1) {
            return;
        }
        *self.cells.entry(key).or_insert(0.0) += mass;
    }

    /// Humidity mass at world cell `(gx, gy)`. Missing tile → 0.
    pub fn at_cell(&self, gx: i32, gy: i32) -> f32 {
        let key = self.tile_of(gx, gy);
        *self.cells.get(&key).unwrap_or(&0.0)
    }

    /// Humidity mass at tile coord `(hx, hy)`. Missing → 0.
    pub fn at_tile(&self, hx: i32, hy: i32) -> f32 {
        *self.cells.get(&(hx, hy)).unwrap_or(&0.0)
    }

    /// Total humidity mass across all tiles. Useful for
    /// mass-conservation assertions in tests and for HUD summaries.
    pub fn total_mass(&self) -> f32 {
        self.cells.values().copied().sum()
    }

    /// Drop any sparse keys outside [`Self::bounds`].
    pub fn clamp_to_bounds(&mut self) {
        let Some(b) = self.bounds else {
            return;
        };
        self.cells.retain(|&(hx, hy), _| b.contains(hx, hy));
    }

    /// Explicit 4-neighbour diffusion step.
    ///
    /// `alpha` is the fraction of each pairwise head difference
    /// transferred per pass. Von Neumann stability for the
    /// 4-neighbour stencil requires `alpha ≤ 0.25`; we clamp to that.
    /// Compute-then-apply from a snapshot so the result is
    /// independent of iteration order; pruning removes near-zero
    /// tiles so the sparse map doesn't grow without bound.
    ///
    /// When [`Self::bounds`] is set without [`Self::wrap_x`], horizontal
    /// out-of-box neighbours are Neumann walls. With `wrap_x`, the left
    /// and right tile edges join (ring atmosphere).
    pub fn diffuse(&mut self, alpha: f32) {
        let alpha = alpha.clamp(0.0, 0.25);
        if alpha == 0.0 || self.cells.is_empty() {
            return;
        }
        // Snapshot the current state so we don't chase deltas across
        // the pass.
        let snap: HashMap<(i32, i32), f32> = self.cells.clone();

        // Build the iteration set: every mapped tile *plus* each of
        // its four neighbours (so a lone spike still spreads to its
        // -x/-y sides, which would never be sources otherwise). We
        // then walk this set and only look at (+x, +y) direction
        // pairs so every undirected pair is visited exactly once.
        let mut sources: Vec<(i32, i32)> = Vec::with_capacity(snap.len() * 5);
        for &(hx, hy) in snap.keys() {
            sources.push((hx, hy));
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) {
                    sources.push((nx, hy));
                }
            }
            if let Some(nx) = self.wrap_hx(hx - 1) {
                if self.accepts(nx, hy) {
                    sources.push((nx, hy));
                }
            }
            if self.accepts(hx, hy + 1) {
                sources.push((hx, hy + 1));
            }
            if self.accepts(hx, hy - 1) {
                sources.push((hx, hy - 1));
            }
        }
        sources.sort_unstable();
        sources.dedup();

        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&0.0);
            // +x neighbour (possibly wrapped).
            if let Some(nx) = self.wrap_hx(hx + 1) {
                if self.accepts(nx, hy) && nx != hx {
                    let n_val = *snap.get(&(nx, hy)).unwrap_or(&0.0);
                    let flow = (val - n_val) * alpha;
                    if flow.abs() >= 1e-9 {
                        *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                        *deltas.entry((nx, hy)).or_insert(0.0) += flow;
                    }
                }
            }
            // +y neighbour (never wraps).
            let n_key = (hx, hy + 1);
            if self.accepts(n_key.0, n_key.1) {
                let n_val = *snap.get(&n_key).unwrap_or(&0.0);
                let flow = (val - n_val) * alpha;
                if flow.abs() >= 1e-9 {
                    *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                    *deltas.entry(n_key).or_insert(0.0) += flow;
                }
            }
        }
        for (k, d) in deltas {
            if !self.accepts(k.0, k.1) {
                continue;
            }
            *self.cells.entry(k).or_insert(0.0) += d;
        }
        let bounds = self.bounds;
        self.cells.retain(|&(hx, hy), v| {
            v.abs() > 1e-6 && bounds.map(|b| b.contains(hx, hy)).unwrap_or(true)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn add_accumulates_at_the_same_tile() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 10.0);
        h.add(1, 3, 5.0); // same tile as (0,0)
        assert_eq!(h.at_cell(2, 2), 15.0);
    }

    #[test]
    fn tile_boundary_is_exclusive_on_upper_edge() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 1.0);
        h.add(4, 0, 2.0); // next tile over
        assert_eq!(h.at_cell(0, 0), 1.0);
        assert_eq!(h.at_cell(4, 0), 2.0);
        assert_eq!(h.at_tile(0, 0), 1.0);
        assert_eq!(h.at_tile(1, 0), 2.0);
    }

    #[test]
    fn diffusion_conserves_total_mass() {
        let mut h = Humidity::new(2);
        h.add(0, 0, 100.0);
        h.add(20, 0, 50.0);
        h.add(-4, -4, 25.0);
        let before = h.total_mass();
        for _ in 0..20 {
            h.diffuse(0.2);
        }
        let after = h.total_mass();
        assert!(
            (before - after).abs() < 1e-3,
            "diffusion must be mass-conservative: before={before}, after={after}"
        );
    }

    #[test]
    fn diffusion_spreads_a_spike() {
        let mut h = Humidity::new(1);
        h.add(0, 0, 100.0);
        assert_eq!(h.at_cell(1, 0), 0.0);
        h.diffuse(0.25);
        assert!(h.at_cell(1, 0) > 0.0, "mass should have flowed right");
        assert!(h.at_cell(-1, 0) > 0.0, "mass should have flowed left");
        assert!(h.at_cell(0, 1) > 0.0, "mass should have flowed up");
        assert!(h.at_cell(0, -1) > 0.0, "mass should have flowed down");
    }

    #[test]
    fn diffusion_with_alpha_zero_is_a_noop() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 42.0);
        let before: Vec<((i32, i32), f32)> =
            h.cells.iter().map(|(&k, &v)| (k, v)).collect();
        h.diffuse(0.0);
        let after: Vec<((i32, i32), f32)> =
            h.cells.iter().map(|(&k, &v)| (k, v)).collect();
        assert_eq!(before, after);
    }

    #[test]
    fn empty_humidity_stays_empty_under_diffusion() {
        let mut h = Humidity::new(4);
        h.diffuse(0.2);
        assert_eq!(h.cells.len(), 0);
        assert_eq!(h.total_mass(), 0.0);
    }

    #[test]
    fn zero_add_does_not_create_an_entry() {
        let mut h = Humidity::new(4);
        h.add(0, 0, 0.0);
        assert!(h.cells.is_empty());
    }

    #[test]
    fn bounds_block_out_of_world_deposits() {
        let mut h = Humidity::with_world_bounds(4, 0, 0, 16, 16);
        h.add(2, 2, 10.0);
        h.add(100, 100, 50.0); // outside
        assert_eq!(h.total_mass(), 10.0);
        assert_eq!(h.cells.len(), 1);
    }

    #[test]
    fn diffuse_with_bounds_stays_inside_and_conserves() {
        let mut h = Humidity::with_world_bounds(1, 0, 0, 4, 4);
        // Capacity = 4×4 = 16 tiles.
        h.add(1, 1, 100.0);
        let before = h.total_mass();
        for _ in 0..80 {
            h.diffuse(0.25);
        }
        let after = h.total_mass();
        assert!(
            (before - after).abs() < 1e-3,
            "bounded diffusion must conserve: before={before}, after={after}"
        );
        assert!(
            h.cells.len() <= h.bounds.unwrap().tile_capacity(),
            "tile count {} exceeded capacity",
            h.cells.len()
        );
        for &(hx, hy) in h.cells.keys() {
            assert!(h.bounds.unwrap().contains(hx, hy), "oob tile ({hx},{hy})");
        }
        // Edge mass should remain (Neumann) — centre spike spreads but
        // does not vanish out the sides.
        assert!(after > 99.0);
    }

    #[test]
    fn diffuse_does_not_create_keys_outside_bounds() {
        let mut h = Humidity::with_world_bounds(1, 0, 0, 2, 2);
        h.add(0, 0, 100.0);
        h.diffuse(0.25);
        for &(hx, hy) in h.cells.keys() {
            assert!(
                (0..=1).contains(&hx) && (0..=1).contains(&hy),
                "created oob key ({hx},{hy})"
            );
        }
    }

    #[test]
    fn humidity_diffuse_due_matches_column_schedule() {
        assert!(!humidity_diffuse_due(0));
        assert!(!humidity_diffuse_due(1));
        assert!(humidity_diffuse_due(3));
        assert!(!humidity_diffuse_due(4));
        assert!(humidity_diffuse_due(23));
        assert!(humidity_diffuse_due(43));
    }

    #[test]
    fn diffuse_wraps_horizontally_on_ring() {
        let mut h = Humidity::with_world_bounds(1, 0, 0, 4, 2);
        h.wrap_x = true;
        // Spike on the rightmost tile; after one pass some mass must
        // appear on the leftmost tile (the ring neighbour).
        h.add(3, 0, 100.0);
        let before = h.total_mass();
        h.diffuse(0.25);
        assert!((h.total_mass() - before).abs() < 1e-3);
        assert!(
            h.at_tile(0, 0) > 0.0,
            "mass should wrap from hx=3 to hx=0"
        );
    }
}
