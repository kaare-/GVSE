//! Flat physics grid. The phone tank is one screen (128×256), so the
//! world-scale chunk map is not allocated. A one-cell bedrock frame
//! sits *outside* the visible area: callers pass visible size, and the
//! stored grid is `(visible + 2)` on each axis. `y = 0` is the bottom.

use crate::cell::Cell;

#[derive(Debug, Clone)]
pub struct PhysicsGrid {
    w: i32,
    h: i32,
    cells: Vec<Cell>,
}

impl PhysicsGrid {
    pub fn new(visible_w: u32, visible_h: u32) -> Self {
        let w = visible_w as i32 + 2;
        let h = visible_h as i32 + 2;
        let mut cells = vec![Cell::air(); (w * h) as usize];
        for y in 0..h {
            for x in 0..w {
                if x == 0 || y == 0 || x == w - 1 || y == h - 1 {
                    cells[(y * w + x) as usize] = Cell::bedrock();
                }
            }
        }
        Self { w, h, cells }
    }

    #[inline]
    pub fn width(&self) -> i32 {
        self.w
    }

    #[inline]
    pub fn height(&self) -> i32 {
        self.h
    }

    #[inline]
    pub fn visible_w(&self) -> i32 {
        self.w - 2
    }

    #[inline]
    pub fn visible_h(&self) -> i32 {
        self.h - 2
    }

    #[inline]
    pub fn in_bounds(&self, x: i32, y: i32) -> bool {
        x >= 0 && y >= 0 && x < self.w && y < self.h
    }

    #[inline]
    pub fn is_border(&self, x: i32, y: i32) -> bool {
        x == 0 || y == 0 || x == self.w - 1 || y == self.h - 1
    }

    #[inline]
    fn index(&self, x: i32, y: i32) -> usize {
        (y * self.w + x) as usize
    }

    #[inline]
    pub fn get(&self, x: i32, y: i32) -> Cell {
        if !self.in_bounds(x, y) {
            return Cell::bedrock();
        }
        self.cells[self.index(x, y)]
    }

    /// Writes an interior cell. The bedrock frame is immutable.
    #[inline]
    pub fn set(&mut self, x: i32, y: i32, cell: Cell) {
        if !self.in_bounds(x, y) || self.is_border(x, y) {
            return;
        }
        let idx = self.index(x, y);
        self.cells[idx] = cell;
    }

    /// Visible coordinate: `vx` right, `vy` up from the bottom of the tank.
    #[inline]
    pub fn get_visible(&self, vx: i32, vy: i32) -> Cell {
        self.get(vx + 1, vy + 1)
    }

    #[inline]
    pub fn set_visible(&mut self, vx: i32, vy: i32, cell: Cell) {
        self.set(vx + 1, vy + 1, cell);
    }

    pub fn sat_sum(&self) -> u64 {
        self.cells.iter().map(|c| c.sat as u64).sum()
    }

    pub fn count_material(&self, material: u8) -> usize {
        self.cells.iter().filter(|c| c.material == material).count()
    }

    pub(crate) fn cells(&self) -> &[Cell] {
        &self.cells
    }

    pub(crate) fn cells_mut(&mut self) -> &mut [Cell] {
        &mut self.cells
    }
}
