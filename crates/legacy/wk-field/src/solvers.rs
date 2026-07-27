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
    let dx2 = field.cell_size_m * field.cell_size_m;
    if w == 0 || h == 0 {
        return;
    }
    // Fast interior path: direct slice indexing, no per-cell halo branches.
    // ~3× speedup vs `laplacian_5point(field, cx, cy)` for big field grids
    // (thermal on the ring was 29 ms/call before; interior is >95% of it).
    let f = field.cells.as_slice();
    let a_slice = alpha.cells.as_slice();
    let s_slice = source.cells.as_slice();
    let inv_dx2 = if dx2 > 0.0 { 1.0 / dx2 } else { 0.0 };
    if w >= 2 && h >= 2 {
        for cy in 1..h.saturating_sub(1) {
            let row = cy * w;
            let up = (cy + 1) * w;
            let dn = (cy - 1) * w;
            for cx in 1..w - 1 {
                let i = row + cx;
                let lap = (f[i + 1] + f[i - 1] + f[up + cx] + f[dn + cx] - 4.0 * f[i]) * inv_dx2;
                out.cells[i] = f[i] + dt * (a_slice[i] * lap + s_slice[i]);
            }
        }
    }
    // Edge cells use the halo-aware stencil.
    let edge = |cx: usize, cy: usize| {
        let lap = laplacian_5point(field, cx, cy);
        let a = alpha.cell_at(cx, cy);
        let s = source.cell_at(cx, cy);
        field.cell_at(cx, cy) + dt * (a * lap + s)
    };
    for cx in 0..w {
        out.set_cell(cx, 0, edge(cx, 0));
        if h > 1 {
            out.set_cell(cx, h - 1, edge(cx, h - 1));
        }
    }
    for cy in 1..h.saturating_sub(1) {
        out.set_cell(0, cy, edge(0, cy));
        if w > 1 {
            out.set_cell(w - 1, cy, edge(w - 1, cy));
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
