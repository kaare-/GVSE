//! Grain fall and repose.
//!
//! A mobile grain swaps with the air (wet or dry) directly below, so
//! sand sinks through water and the water is displaced upward. Bedrock
//! never moves. Stone and loose rock fall, slump, and turn as they go.
//!
//! Repose: a supported grain slides one cell down-diagonal when the
//! neighbour column drops by more than [`crate::cell::stable_step`].
//! When both directions are open, `hash_u64` picks the side.

use crate::cell::{is_mobile_grain, stable_step};
use crate::grid::PhysicsGrid;
use crate::hash::hash_u64;
use crate::rules::plan::ActiveRect;

pub fn fall(grid: &mut PhysicsGrid, active: &ActiveRect) {
    for y in active.y0..=active.y1 {
        for x in active.x0..=active.x1 {
            let cell = grid.get(x, y);
            if !is_mobile_grain(cell.id()) || cell.is_bound() || cell.is_rigid() || cell.is_silhouette() {
                continue;
            }
            let below = grid.get(x, y - 1);
            if !below.is_air() {
                continue;
            }
            let mut moving = cell;
            if moving.tumbles() {
                moving.spin();
            }
            grid.set(x, y - 1, moving);
            grid.set(x, y, below);
        }
    }
}

pub fn repose(grid: &mut PhysicsGrid, active: &ActiveRect, seed: u64, tick: u64, tilt_x: f32) {
    for y in active.y0..=active.y1 {
        for x in active.x0..=active.x1 {
            let cell = grid.get(x, y);
            if !is_mobile_grain(cell.id()) || cell.is_bound() || cell.is_rigid() || cell.is_silhouette() {
                continue;
            }
            if grid.get(x, y - 1).is_air() {
                continue;
            }
            let left = can_slide(grid, x, y, -1, active);
            let right = can_slide(grid, x, y, 1, active);
            let dx = match (left, right) {
                (true, true) => {
                    if tilt_x.abs() > 0.08 {
                        if tilt_x > 0.0 {
                            1
                        } else {
                            -1
                        }
                    } else if hash_u64(seed, x as u64, y as u64, tick) & 1 == 0 {
                        -1
                    } else {
                        1
                    }
                }
                (true, false) => -1,
                (false, true) => 1,
                (false, false) => 0,
            };
            if dx == 0 {
                continue;
            }
            let nx = x + dx;
            let ny = y - 1;
            let dest = grid.get(nx, ny);
            let mut moving = cell;
            if moving.tumbles() {
                moving.spin();
            }
            grid.set(nx, ny, moving);
            grid.set(x, y, dest);
        }
    }
}

fn can_slide(grid: &PhysicsGrid, x: i32, y: i32, dx: i32, active: &ActiveRect) -> bool {
    let nx = x + dx;
    let ny = y - 1;
    if nx < active.x0 || nx > active.x1 || ny < active.y0 {
        return false;
    }
    if !grid.get(nx, ny).is_air() {
        return false;
    }
    let step = stable_step(grid.get(x, y).id());
    let mut drop = 0i32;
    let mut yy = ny;
    while yy >= active.y0 && grid.get(nx, yy).is_air() {
        drop += 1;
        yy -= 1;
        if drop > step + 8 {
            break;
        }
    }
    drop > step
}
