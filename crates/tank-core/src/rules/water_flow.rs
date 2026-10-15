//! Lateral equalisation of free water. One pass walks the row in a
//! direction that alternates each substep, so a puddle spreads without
//! a permanent drift. Sum of saturation is conserved.

use crate::grid::PhysicsGrid;
use crate::rules::plan::ActiveRect;

pub fn apply(grid: &mut PhysicsGrid, active: &ActiveRect, tick: u64) {
    let left_to_right = tick % 2 == 0;
    let n = active.x1 - active.x0;
    for y in active.y0..=active.y1 {
        for i in 0..n {
            let x = if left_to_right {
                active.x0 + i
            } else {
                active.x1 - 1 - i
            };
            let a = grid.get(x, y);
            let b = grid.get(x + 1, y);
            if !a.is_air() || !b.is_air() {
                continue;
            }
            let sa = a.sat as i32;
            let sb = b.sat as i32;
            if (sa - sb).abs() <= 1 {
                continue;
            }
            let sum = sa + sb;
            let avg = sum / 2;
            let rem = sum % 2;
            let (na, nb) = if sa >= sb {
                (avg + rem, avg)
            } else {
                (avg, avg + rem)
            };
            let mut ca = a;
            let mut cb = b;
            ca.sat = na as u8;
            cb.sat = nb as u8;
            grid.set(x, y, ca);
            grid.set(x + 1, y, cb);
        }
    }
}
