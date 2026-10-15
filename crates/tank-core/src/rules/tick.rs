//! Tick order: plan, water (gravity, flow, head), seepage, moisture film,
//! then grain fall and repose.

use crate::consts::{GRAIN_PASSES, WATER_SUBSTEPS};
use crate::grid::PhysicsGrid;

use super::grain;
use super::gravity;
use super::head;
use super::moisture;
use super::plan;
use super::seepage;
use super::water_flow;

pub fn step(grid: &mut PhysicsGrid, seed: u64, tick: u64, tilt_x: f32) {
    let active = plan::plan_active(grid);
    if active.x1 < active.x0 || active.y1 < active.y0 {
        return;
    }
    for s in 0..WATER_SUBSTEPS {
        gravity::apply(grid, &active);
        water_flow::apply(grid, &active, tick.wrapping_add(s as u64), tilt_x);
        head::apply(grid, &active);
    }
    seepage::apply(grid, &active);
    moisture::apply(grid, &active);
    for p in 0..GRAIN_PASSES {
        grain::fall(grid, &active);
        grain::repose(grid, &active, seed, tick.wrapping_add(p as u64), tilt_x);
    }
}
