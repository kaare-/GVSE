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

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

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
}

impl Humidity {
    pub fn new(tile_cols: i32) -> Self {
        Self {
            tile_cols: tile_cols.max(1),
            cells: HashMap::new(),
        }
    }

    /// Tile coord for a world cell.
    pub fn tile_of(&self, gx: i32, gy: i32) -> (i32, i32) {
        (gx.div_euclid(self.tile_cols), gy.div_euclid(self.tile_cols))
    }

    /// Deposit `mass` at world cell `(gx, gy)`.
    pub fn add(&mut self, gx: i32, gy: i32, mass: f32) {
        if mass == 0.0 {
            return;
        }
        let key = self.tile_of(gx, gy);
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

    /// Explicit 4-neighbour diffusion step.
    ///
    /// `alpha` is the fraction of each pairwise head difference
    /// transferred per pass. Von Neumann stability for the
    /// 4-neighbour stencil requires `alpha ≤ 0.25`; we clamp to that.
    /// Compute-then-apply from a snapshot so the result is
    /// independent of iteration order; pruning removes near-zero
    /// tiles so the sparse map doesn't grow without bound.
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
            sources.push((hx + 1, hy));
            sources.push((hx - 1, hy));
            sources.push((hx, hy + 1));
            sources.push((hx, hy - 1));
        }
        sources.sort_unstable();
        sources.dedup();

        let mut deltas: HashMap<(i32, i32), f32> = HashMap::new();
        for &(hx, hy) in &sources {
            let val = *snap.get(&(hx, hy)).unwrap_or(&0.0);
            for &(dx, dy) in &[(1, 0), (0, 1)] {
                let n_key = (hx + dx, hy + dy);
                let n_val = *snap.get(&n_key).unwrap_or(&0.0);
                let flow = (val - n_val) * alpha;
                if flow.abs() < 1e-9 {
                    continue;
                }
                *deltas.entry((hx, hy)).or_insert(0.0) -= flow;
                *deltas.entry(n_key).or_insert(0.0) += flow;
            }
        }
        for (k, d) in deltas {
            *self.cells.entry(k).or_insert(0.0) += d;
        }
        self.cells.retain(|_, v| v.abs() > 1e-6);
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
}
