//! Shared fixtures for wk-voxel scenario ports of legacy E-series intents.
//!
//! These exercise the same *product intents* as `tests/scenarios/` in the
//! column stack, but use only the greenfield voxel APIs. See
//! `docs/VOXEL_MIGRATION.md` § Isolation Guardrails.

use wk_material::MaterialId;
use wk_voxel::{Cell, ChunkCoord, World, CHUNK_CELLS_W};

/// Sum of `sat` over an inclusive world rectangle.
pub fn sat_sum(world: &World, x0: i32, x1: i32, y0: i32, y1: i32) -> i64 {
    let mut total = 0i64;
    for x in x0..=x1 {
        for y in y0..=y1 {
            if let Some(c) = world.get_cell(x, y) {
                total += c.sat.0 as i64;
            }
        }
    }
    total
}

/// Count cells of `material` in an inclusive rectangle.
pub fn count_material(
    world: &World,
    material: MaterialId,
    x0: i32,
    x1: i32,
    y0: i32,
    y1: i32,
) -> usize {
    let mut n = 0usize;
    for x in x0..=x1 {
        for y in y0..=y1 {
            if let Some(c) = world.get_cell(x, y) {
                if c.material == material {
                    n += 1;
                }
            }
        }
    }
    n
}

/// Impermeable floor across `width` columns at y=0.
pub fn lay_bedrock_floor(world: &mut World, width: i32) {
    for x in 0..width {
        world.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
}

/// Symmetric stone hill: peak at `center`, height `peak_h` (above floor).
///
/// Surface elevation at column `x` is `1 + max(0, peak_h - |x - center|)`.
pub fn setup_hill_world(seed: u64, width: i32, center: i32, peak_h: i32) -> World {
    let mut w = World::new(seed);
    // Ensure chunks covering the transect (and sky headroom for rain).
    let chunks_x = (width + CHUNK_CELLS_W as i32 - 1) / CHUNK_CELLS_W as i32;
    for cx in 0..chunks_x {
        w.ensure_chunk(ChunkCoord::new(cx, 0));
    }
    lay_bedrock_floor(&mut w, width);
    for x in 0..width {
        let h = 1 + (peak_h - (x - center).abs()).max(0);
        for y in 1..=h {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
    }
    w
}

/// Closed stone basin: floor + walls, open top. Interior is empty Air.
///
/// Layout (world-x / world-y):
/// ```text
///   y=wall_h .. walls at x=x0 and x=x1
///   y=1      .. stone floor between walls
///   y=0      .. bedrock
/// ```
pub fn setup_basin_world(seed: u64, x0: i32, x1: i32, wall_h: i32) -> World {
    let mut w = World::new(seed);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut w, x1 + 4);
    for x in x0..=x1 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
    }
    for y in 2..=wall_h {
        w.set_cell(x0, y, Cell::solid(MaterialId::Stone));
        w.set_cell(x1, y, Cell::solid(MaterialId::Stone));
    }
    w
}

/// Flat stone terrace spanning two chunks so water can cross x=64.
pub fn setup_seam_terrace(seed: u64) -> World {
    let mut w = World::new(seed);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(1, 0));
    lay_bedrock_floor(&mut w, 128);
    for x in 0..128 {
        w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
    }
    w
}

/// Flat stone plain with an open surface pit (sinkhole) at `[pit_x0, pit_x1]`.
///
/// Pit columns have Air from y=1..=pit_depth (open to sky). Neighbouring
/// columns have stone at y=1 so surface water can pond on the rim.
pub fn setup_sinkhole_world(
    seed: u64,
    width: i32,
    pit_x0: i32,
    pit_x1: i32,
    pit_depth: i32,
) -> World {
    let mut w = World::new(seed);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    lay_bedrock_floor(&mut w, width);
    for x in 0..width {
        if x >= pit_x0 && x <= pit_x1 {
            continue;
        }
        w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
    }
    for x in pit_x0..=pit_x1 {
        for y in 1..=pit_depth {
            w.set_cell(x, y, Cell::air());
        }
    }
    w
}

