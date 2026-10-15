//! Hydrostatic head. Column surfaces are measured once per pass, then
//! a higher surface pours into the air beside it. Gravity on the next
//! substep drops that water. Open lakes level; a gap at the bottom of
//! a wall still equalises because lateral flow runs in the same tick.

use crate::consts::STANDING_AIR_SAT;
use crate::grid::PhysicsGrid;
use crate::rules::plan::ActiveRect;

pub fn apply(grid: &mut PhysicsGrid, active: &ActiveRect) {
    // Visible width is at most 256, plus the border cell we index by
    // absolute x. 260 covers the cap in `consts::MAX_VISIBLE_W`.
    let mut surf = [i32::MIN; 260];
    let floor = active.y0 - 1;
    for x in active.x0..=active.x1 {
        surf[x as usize] = surface_y(grid, x, active).unwrap_or(floor);
    }
    for x in active.x0..active.x1 {
        let left = surf[x as usize];
        let right = surf[(x + 1) as usize];
        let poured = if left > right + 1 {
            pour(grid, x, x + 1, left, right, active)
        } else if right > left + 1 {
            pour(grid, x + 1, x, right, left, active)
        } else {
            false
        };
        if poured {
            surf[x as usize] = surface_y(grid, x, active).unwrap_or(floor);
            surf[(x + 1) as usize] = surface_y(grid, x + 1, active).unwrap_or(floor);
        }
    }
}

fn surface_y(grid: &PhysicsGrid, x: i32, active: &ActiveRect) -> Option<i32> {
    for y in (active.y0..=active.y1).rev() {
        let cell = grid.get(x, y);
        if cell.is_air() {
            if cell.sat >= STANDING_AIR_SAT {
                return Some(y);
            }
            continue;
        }
        return None;
    }
    None
}

fn pour(
    grid: &mut PhysicsGrid,
    src_x: i32,
    dst_x: i32,
    ys: i32,
    yd: i32,
    active: &ActiveRect,
) -> bool {
    if ys < active.y0 || ys > active.y1 {
        return false;
    }
    let dest = grid.get(dst_x, ys);
    if !dest.is_air() {
        return false;
    }
    let src = grid.get(src_x, ys);
    if !src.is_air() || src.sat == 0 {
        return false;
    }
    let room = 255 - dest.sat as i32;
    if room <= 0 {
        return false;
    }
    let want = ((ys - yd).max(1) * 8).clamp(1, 48);
    let mv = want.min(src.sat as i32).min(room) as u8;
    if mv == 0 {
        return false;
    }
    let mut s = src;
    let mut d = dest;
    s.sat -= mv;
    d.sat += mv;
    grid.set(src_x, ys, s);
    grid.set(dst_x, ys, d);
    true
}
