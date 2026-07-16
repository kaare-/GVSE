//! Explicit stencil solvers over [`FieldPatch`].
//!
//! These are the composition primitives field subsystems call. They do
//! not own world state — callers supply coefficient / source patches
//! and an output buffer of matching shape.

use crate::patch::FieldPatch;
use crate::stencil::laplacian_5point;

/// Forward-Euler diffusion: `out = field + dt * (α ∇²field + source)`.
///
/// `alpha` and `source` must match `field`'s geometry. Stability for
/// constant α requires `dt ≤ Δx² / (4α)` in 2D.
pub fn explicit_diffusion(
    field: &FieldPatch,
    alpha: &FieldPatch,
    source: &FieldPatch,
    dt: f32,
    out: &mut FieldPatch,
) {
    debug_assert_eq!(field.width_cells, alpha.width_cells);
    debug_assert_eq!(field.height_cells, alpha.height_cells);
    debug_assert_eq!(field.width_cells, source.width_cells);
    debug_assert_eq!(field.height_cells, source.height_cells);
    debug_assert_eq!(field.width_cells, out.width_cells);
    debug_assert_eq!(field.height_cells, out.height_cells);

    let w = field.width_cells as usize;
    let h = field.height_cells as usize;
    for cy in 0..h {
        for cx in 0..w {
            let lap = laplacian_5point(field, cx, cy);
            let a = alpha.cell_at(cx, cy);
            let s = source.cell_at(cx, cy);
            let next = field.cell_at(cx, cy) + dt * (a * lap + s);
            out.set_cell(cx, cy, next);
        }
    }
    // Interior updated; halo on `out` is left for the caller / barrier
    // exchange to refresh from neighbours.
}

/// Semi-Lagrangian advection: sample `field` at the back-traced
/// position `x − v·dt` and write into `out`.
///
/// First-order (linear back-trace + bilinear sample). Stable for large
/// CFL but dissipative — fine for humidity / concentration at this
/// fidelity. `vx`/`vy` are velocities in m per unit-`dt`.
pub fn semi_lagrangian_advect(
    field: &FieldPatch,
    vx: &FieldPatch,
    vy: &FieldPatch,
    dt: f32,
    out: &mut FieldPatch,
) {
    debug_assert_eq!(field.width_cells, vx.width_cells);
    debug_assert_eq!(field.height_cells, vx.height_cells);
    debug_assert_eq!(field.width_cells, vy.width_cells);
    debug_assert_eq!(field.height_cells, vy.height_cells);
    debug_assert_eq!(field.width_cells, out.width_cells);
    debug_assert_eq!(field.height_cells, out.height_cells);

    let w = field.width_cells as usize;
    let h = field.height_cells as usize;
    for cy in 0..h {
        for cx in 0..w {
            let (x, y) = field.cell_center(cx, cy);
            let u = vx.cell_at(cx, cy);
            let v = vy.cell_at(cx, cy);
            let sx = x - u * dt;
            let sy = y - v * dt;
            out.set_cell(cx, cy, field.sample_bilinear(sx, sy));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::FieldPatch;

    #[test]
    fn diffusion_of_constant_field_is_noop() {
        let field = FieldPatch::new(4, 4, 0.5, 0.0, 0.0, 5.0);
        let alpha = FieldPatch::new(4, 4, 0.5, 0.0, 0.0, 1.0);
        let source = FieldPatch::new(4, 4, 0.5, 0.0, 0.0, 0.0);
        let mut out = field.zeros_like();
        explicit_diffusion(&field, &alpha, &source, 0.01, &mut out);
        for cy in 0..4 {
            for cx in 0..4 {
                assert!((out.cell_at(cx, cy) - 5.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn diffusion_with_source_raises_uniformly() {
        let field = FieldPatch::new(3, 3, 1.0, 0.0, 0.0, 0.0);
        let alpha = FieldPatch::new(3, 3, 1.0, 0.0, 0.0, 0.0); // no diffusion
        let source = FieldPatch::new(3, 3, 1.0, 0.0, 0.0, 2.0);
        let mut out = field.zeros_like();
        explicit_diffusion(&field, &alpha, &source, 0.5, &mut out);
        for cy in 0..3 {
            for cx in 0..3 {
                assert!((out.cell_at(cx, cy) - 1.0).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn advection_shifts_pulse_left_when_vx_positive() {
        // Pulse at cx=2; wind to +x means back-trace comes from -x,
        // so the pulse moves to the right in the forward sense —
        // sample at cx=3 should pick up the pulse after one step when
        // v*dt = one cell.
        let mut field = FieldPatch::new(5, 1, 1.0, 0.0, 0.0, 0.0);
        field.set_cell(2, 0, 1.0);
        let vx = FieldPatch::new(5, 1, 1.0, 0.0, 0.0, 1.0);
        let vy = FieldPatch::new(5, 1, 1.0, 0.0, 0.0, 0.0);
        let mut out = field.zeros_like();
        semi_lagrangian_advect(&field, &vx, &vy, 1.0, &mut out);
        // Cell centre of cx=3 back-traces to centre of cx=2.
        assert!((out.cell_at(3, 0) - 1.0).abs() < 1e-4);
        assert!(out.cell_at(2, 0).abs() < 1e-4);
    }
}
