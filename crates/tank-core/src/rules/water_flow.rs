//! Lateral equalisation of free water, with optional tilt bias.
//!
//! When `tilt_x` is near zero the pass is unbiased (default). A non-zero
//! tilt (phone gyro, Phase 1.5) walks downhill and peels a little extra
//! saturation toward the low side. Sum of saturation is conserved.

use crate::grid::PhysicsGrid;
use crate::rules::plan::ActiveRect;

pub fn apply(grid: &mut PhysicsGrid, active: &ActiveRect, tick: u64, tilt_x: f32) {
    let bias = if tilt_x.abs() < 0.08 {
        0
    } else if tilt_x > 0.0 {
        1
    } else {
        -1
    };
    let left_to_right = match bias {
        n if n > 0 => true,
        n if n < 0 => false,
        _ => tick % 2 == 0,
    };
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
            if (sa - sb).abs() <= 1 && bias == 0 {
                continue;
            }
            let sum = sa + sb;
            if sum == 0 {
                continue;
            }
            let avg = sum / 2;
            let rem = sum % 2;
            let (mut na, mut nb) = if sa >= sb {
                (avg + rem, avg)
            } else {
                (avg, avg + rem)
            };
            if bias > 0 && na > 0 {
                let peel = (na.min(6) as f32 * tilt_x.abs().min(1.0)) as i32;
                let peel = peel.max(1).min(na);
                na -= peel;
                nb += peel;
            } else if bias < 0 && nb > 0 {
                let peel = (nb.min(6) as f32 * tilt_x.abs().min(1.0)) as i32;
                let peel = peel.max(1).min(nb);
                nb -= peel;
                na += peel;
            }
            if na == sa && nb == sb {
                continue;
            }
            let mut ca = a;
            let mut cb = b;
            ca.sat = na.clamp(0, 255) as u8;
            cb.sat = nb.clamp(0, 255) as u8;
            grid.set(x, y, ca);
            grid.set(x + 1, y, cb);
        }
    }
}
