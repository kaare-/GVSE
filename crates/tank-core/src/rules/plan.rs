//! Active rectangle for a tick. The tank is one screen, so Phase 0
//! plans the whole interior. The rect is the same shape later phases
//! will shrink to a dirty set.

use crate::grid::PhysicsGrid;

#[derive(Debug, Clone, Copy)]
pub struct ActiveRect {
    pub x0: i32,
    pub y0: i32,
    pub x1: i32,
    pub y1: i32,
}

pub fn plan_active(grid: &PhysicsGrid) -> ActiveRect {
    ActiveRect {
        x0: 1,
        y0: 1,
        x1: grid.width() - 2,
        y1: grid.height() - 2,
    }
}
