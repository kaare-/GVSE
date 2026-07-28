//! Chunk-local field patch with edge halos.

use serde::{Deserialize, Serialize};

/// Edge samples from neighbouring patches. Lengths match the adjacent
/// edge of the interior grid (`height_cells` for left/right,
/// `width_cells` for top/bottom).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldHalo {
    pub left: Vec<f32>,
    pub right: Vec<f32>,
    pub top: Vec<f32>,
    pub bottom: Vec<f32>,
}

impl FieldHalo {
    pub fn zeros(width_cells: u16, height_cells: u16) -> Self {
        Self {
            left: vec![0.0; height_cells as usize],
            right: vec![0.0; height_cells as usize],
            top: vec![0.0; width_cells as usize],
            bottom: vec![0.0; width_cells as usize],
        }
    }

    pub fn filled(width_cells: u16, height_cells: u16, value: f32) -> Self {
        Self {
            left: vec![value; height_cells as usize],
            right: vec![value; height_cells as usize],
            top: vec![value; width_cells as usize],
            bottom: vec![value; width_cells as usize],
        }
    }
}

/// A rectangular scalar field covering one chunk's spatial extent.
///
/// Cells are stored row-major: index = `cy * width_cells + cx`, with
/// `cx` increasing east and `cy` increasing *up* (world +y).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FieldPatch {
    pub cells: Vec<f32>,
    pub width_cells: u16,
    pub height_cells: u16,
    pub cell_size_m: f32,
    /// World-space origin of the lower-left corner of cell (0, 0).
    pub origin_x_m: f32,
    pub origin_y_m: f32,
    pub halo: FieldHalo,
}

impl FieldPatch {
    pub fn new(
        width_cells: u16,
        height_cells: u16,
        cell_size_m: f32,
        origin_x_m: f32,
        origin_y_m: f32,
        fill: f32,
    ) -> Self {
        let n = width_cells as usize * height_cells as usize;
        Self {
            cells: vec![fill; n],
            width_cells,
            height_cells,
            cell_size_m,
            origin_x_m,
            origin_y_m,
            halo: FieldHalo::filled(width_cells, height_cells, fill),
        }
    }

    /// Empty-shaped clone (same geometry, zeros) for scratch buffers.
    pub fn zeros_like(&self) -> Self {
        Self::new(
            self.width_cells,
            self.height_cells,
            self.cell_size_m,
            self.origin_x_m,
            self.origin_y_m,
            0.0,
        )
    }

    pub fn cell_count(&self) -> usize {
        self.width_cells as usize * self.height_cells as usize
    }

    #[inline]
    pub fn index(&self, cx: usize, cy: usize) -> usize {
        debug_assert!(cx < self.width_cells as usize);
        debug_assert!(cy < self.height_cells as usize);
        cy * self.width_cells as usize + cx
    }

    #[inline]
    pub fn cell_at(&self, cx: usize, cy: usize) -> f32 {
        self.cells[self.index(cx, cy)]
    }

    #[inline]
    pub fn set_cell(&mut self, cx: usize, cy: usize, value: f32) {
        let i = self.index(cx, cy);
        self.cells[i] = value;
    }

    /// World-space centre of cell `(cx, cy)`.
    pub fn cell_center(&self, cx: usize, cy: usize) -> (f32, f32) {
        let x = self.origin_x_m + (cx as f32 + 0.5) * self.cell_size_m;
        let y = self.origin_y_m + (cy as f32 + 0.5) * self.cell_size_m;
        (x, y)
    }

    /// Nearest-neighbour sample in world metres. Out of interior range
    /// clamps to the edge cell (halo is for stencils, not sampling).
    pub fn sample(&self, x_m: f32, y_m: f32) -> f32 {
        let (cx, cy) = self.world_to_cell(x_m, y_m);
        self.cell_at(cx, cy)
    }

    /// Bilinear sample in world metres. Clamps to the interior.
    pub fn sample_bilinear(&self, x_m: f32, y_m: f32) -> f32 {
        let w = self.width_cells as usize;
        let h = self.height_cells as usize;
        if w == 0 || h == 0 {
            return 0.0;
        }
        let fx = ((x_m - self.origin_x_m) / self.cell_size_m - 0.5)
            .clamp(0.0, (w - 1) as f32);
        let fy = ((y_m - self.origin_y_m) / self.cell_size_m - 0.5)
            .clamp(0.0, (h - 1) as f32);
        let x0 = fx.floor() as usize;
        let y0 = fy.floor() as usize;
        let x1 = (x0 + 1).min(w - 1);
        let y1 = (y0 + 1).min(h - 1);
        let tx = fx - x0 as f32;
        let ty = fy - y0 as f32;
        let v00 = self.cell_at(x0, y0);
        let v10 = self.cell_at(x1, y0);
        let v01 = self.cell_at(x0, y1);
        let v11 = self.cell_at(x1, y1);
        let v0 = v00 + (v10 - v00) * tx;
        let v1 = v01 + (v11 - v01) * tx;
        v0 + (v1 - v0) * ty
    }

    pub fn world_to_cell(&self, x_m: f32, y_m: f32) -> (usize, usize) {
        let w = self.width_cells as usize;
        let h = self.height_cells as usize;
        if w == 0 || h == 0 {
            return (0, 0);
        }
        let cx = ((x_m - self.origin_x_m) / self.cell_size_m)
            .floor()
            .clamp(0.0, (w - 1) as f32) as usize;
        let cy = ((y_m - self.origin_y_m) / self.cell_size_m)
            .floor()
            .clamp(0.0, (h - 1) as f32) as usize;
        (cx, cy)
    }

    /// Value at `(cx, cy)`, reading from halo when the neighbour is
    /// outside the interior. Used by stencil ops.
    pub fn value_with_halo(&self, cx: i32, cy: i32) -> f32 {
        let w = self.width_cells as i32;
        let h = self.height_cells as i32;
        if cx >= 0 && cx < w && cy >= 0 && cy < h {
            return self.cell_at(cx as usize, cy as usize);
        }
        if cx < 0 {
            let row = cy.clamp(0, h - 1) as usize;
            return self.halo.left[row];
        }
        if cx >= w {
            let row = cy.clamp(0, h - 1) as usize;
            return self.halo.right[row];
        }
        if cy < 0 {
            let col = cx.clamp(0, w - 1) as usize;
            return self.halo.bottom[col];
        }
        // cy >= h
        let col = cx.clamp(0, w - 1) as usize;
        self.halo.top[col]
    }

    /// Copy left/right edge columns into a neighbour's opposite halo
    /// slots. Callers still own vertical (top/bottom) boundary policy.
    pub fn copy_edge_into_neighbor_halos(
        &self,
        left_neighbor: Option<&mut FieldHalo>,
        right_neighbor: Option<&mut FieldHalo>,
    ) {
        let w = self.width_cells as usize;
        let h = self.height_cells as usize;
        if let Some(halo) = left_neighbor {
            // Our left edge becomes the right halo of the left neighbour.
            for cy in 0..h {
                halo.right[cy] = self.cell_at(0, cy);
            }
        }
        if let Some(halo) = right_neighbor {
            for cy in 0..h {
                halo.left[cy] = self.cell_at(w - 1, cy);
            }
        }
    }

    pub fn sum(&self) -> f32 {
        self.cells.iter().sum()
    }

    pub fn fill(&mut self, value: f32) {
        for c in &mut self.cells {
            *c = value;
        }
        self.halo = FieldHalo::filled(self.width_cells, self.height_cells, value);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_fills_interior_and_halo() {
        let f = FieldPatch::new(4, 3, 0.5, 0.0, 0.0, 2.5);
        assert_eq!(f.cell_count(), 12);
        assert!(f.cells.iter().all(|&v| v == 2.5));
        assert!(f.halo.left.iter().all(|&v| v == 2.5));
        assert_eq!(f.halo.left.len(), 3);
        assert_eq!(f.halo.top.len(), 4);
    }

    #[test]
    fn sample_at_cell_center_matches_cell() {
        let mut f = FieldPatch::new(4, 4, 1.0, 0.0, 0.0, 0.0);
        f.set_cell(2, 1, 9.0);
        let (x, y) = f.cell_center(2, 1);
        assert!((f.sample(x, y) - 9.0).abs() < 1e-5);
        assert!((f.sample_bilinear(x, y) - 9.0).abs() < 1e-5);
    }

    #[test]
    fn value_with_halo_reads_edges() {
        let mut f = FieldPatch::new(2, 2, 1.0, 0.0, 0.0, 0.0);
        f.halo.left[0] = 3.0;
        f.halo.right[1] = 4.0;
        f.halo.bottom[0] = 5.0;
        f.halo.top[1] = 6.0;
        assert_eq!(f.value_with_halo(-1, 0), 3.0);
        assert_eq!(f.value_with_halo(2, 1), 4.0);
        assert_eq!(f.value_with_halo(0, -1), 5.0);
        assert_eq!(f.value_with_halo(1, 2), 6.0);
    }
}
