//! Finite-difference stencil primitives over [`FieldPatch`].

use crate::patch::FieldPatch;

/// Five-point Laplacian: `(n + e + s + w - 4·c) / Δx²`.
///
/// Reads halo values when a neighbour is outside the interior.
pub fn laplacian_5point(field: &FieldPatch, cx: usize, cy: usize) -> f32 {
    let dx2 = field.cell_size_m * field.cell_size_m;
    if dx2 <= 0.0 {
        return 0.0;
    }
    let x = cx as i32;
    let y = cy as i32;
    let c = field.value_with_halo(x, y);
    let e = field.value_with_halo(x + 1, y);
    let w = field.value_with_halo(x - 1, y);
    let n = field.value_with_halo(x, y + 1);
    let s = field.value_with_halo(x, y - 1);
    (e + w + n + s - 4.0 * c) / dx2
}

/// Central-difference gradient `(∂φ/∂x, ∂φ/∂y)`.
pub fn gradient(field: &FieldPatch, cx: usize, cy: usize) -> (f32, f32) {
    let dx = field.cell_size_m;
    if dx <= 0.0 {
        return (0.0, 0.0);
    }
    let x = cx as i32;
    let y = cy as i32;
    let e = field.value_with_halo(x + 1, y);
    let w = field.value_with_halo(x - 1, y);
    let n = field.value_with_halo(x, y + 1);
    let s = field.value_with_halo(x, y - 1);
    ((e - w) / (2.0 * dx), (n - s) / (2.0 * dx))
}

/// Divergence of a vector field `(vx, vy)` at cell `(cx, cy)`.
pub fn divergence(vx: &FieldPatch, vy: &FieldPatch, cx: usize, cy: usize) -> f32 {
    debug_assert_eq!(vx.width_cells, vy.width_cells);
    debug_assert_eq!(vx.height_cells, vy.height_cells);
    debug_assert!((vx.cell_size_m - vy.cell_size_m).abs() < 1e-6);
    let dx = vx.cell_size_m;
    if dx <= 0.0 {
        return 0.0;
    }
    let x = cx as i32;
    let y = cy as i32;
    let dve_dx = (vx.value_with_halo(x + 1, y) - vx.value_with_halo(x - 1, y)) / (2.0 * dx);
    let dvy_dy = (vy.value_with_halo(x, y + 1) - vy.value_with_halo(x, y - 1)) / (2.0 * dx);
    dve_dx + dvy_dy
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::patch::FieldPatch;

    #[test]
    fn laplacian_of_constant_is_zero() {
        let f = FieldPatch::new(8, 8, 0.5, 0.0, 0.0, 7.0);
        for cy in 0..8 {
            for cx in 0..8 {
                assert!(
                    laplacian_5point(&f, cx, cy).abs() < 1e-5,
                    "lap at ({cx},{cy})"
                );
            }
        }
    }

    #[test]
    fn laplacian_of_linear_is_zero() {
        // φ = x + 2y in world metres, sampled at cell centres.
        let mut f = FieldPatch::new(6, 6, 1.0, 0.0, 0.0, 0.0);
        for cy in 0..6 {
            for cx in 0..6 {
                let (x, y) = f.cell_center(cx, cy);
                f.set_cell(cx, cy, x + 2.0 * y);
            }
        }
        // Match interior to halo so the stencil sees the linear field.
        for cy in 0..6 {
            let (x0, y0) = f.cell_center(0, cy);
            let (x1, y1) = f.cell_center(5, cy);
            f.halo.left[cy] = (x0 - 1.0) + 2.0 * y0;
            f.halo.right[cy] = (x1 + 1.0) + 2.0 * y1;
        }
        for cx in 0..6 {
            let (x0, y0) = f.cell_center(cx, 0);
            let (x1, y1) = f.cell_center(cx, 5);
            f.halo.bottom[cx] = x0 + 2.0 * (y0 - 1.0);
            f.halo.top[cx] = x1 + 2.0 * (y1 + 1.0);
        }
        for cy in 1..5 {
            for cx in 1..5 {
                assert!(
                    laplacian_5point(&f, cx, cy).abs() < 1e-4,
                    "lap at ({cx},{cy}) = {}",
                    laplacian_5point(&f, cx, cy)
                );
            }
        }
    }

    #[test]
    fn gradient_of_linear_field() {
        let mut f = FieldPatch::new(5, 5, 1.0, 0.0, 0.0, 0.0);
        for cy in 0..5 {
            for cx in 0..5 {
                let (x, y) = f.cell_center(cx, cy);
                f.set_cell(cx, cy, 3.0 * x - y);
            }
        }
        for cy in 0..5 {
            let (x0, y0) = f.cell_center(0, cy);
            let (x1, y1) = f.cell_center(4, cy);
            f.halo.left[cy] = 3.0 * (x0 - 1.0) - y0;
            f.halo.right[cy] = 3.0 * (x1 + 1.0) - y1;
        }
        for cx in 0..5 {
            let (x0, y0) = f.cell_center(cx, 0);
            let (x1, y1) = f.cell_center(cx, 4);
            f.halo.bottom[cx] = 3.0 * x0 - (y0 - 1.0);
            f.halo.top[cx] = 3.0 * x1 - (y1 + 1.0);
        }
        let (gx, gy) = gradient(&f, 2, 2);
        assert!((gx - 3.0).abs() < 1e-4);
        assert!((gy - (-1.0)).abs() < 1e-4);
    }
}
