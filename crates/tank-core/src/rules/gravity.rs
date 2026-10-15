//! Bottom-up gravity for free water. Each air cell pulls saturation
//! from the air cell above, one step. Pore water does not fall; that
//! is seepage. Same pull direction as GvsE `apply_gravity_fall`.

use crate::grid::PhysicsGrid;
use crate::rules::plan::ActiveRect;

pub fn apply(grid: &mut PhysicsGrid, active: &ActiveRect) {
    for y in active.y0..=active.y1 {
        for x in active.x0..=active.x1 {
            let mut dst = grid.get(x, y);
            let mut src = grid.get(x, y + 1);
            if !dst.is_air() || !src.is_air() {
                continue;
            }
            let room = 255 - dst.sat as i32;
            if room <= 0 || src.sat == 0 {
                continue;
            }
            let mv = room.min(src.sat as i32) as u8;
            dst.sat += mv;
            src.sat -= mv;
            grid.set(x, y, dst);
            grid.set(x, y + 1, src);
        }
    }
}
