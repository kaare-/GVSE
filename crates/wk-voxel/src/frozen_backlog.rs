//! Water + load backlog for chunks outside loaded RAM (Phase 4 streaming).
//!
//! When active-edge flow targets a missing (evicted) neighbour, sat (and a
//! pro-rata share of suspended / dissolved load) is booked here instead of
//! reflecting at a wall. [`apply_frozen_backlog`] paints water onto edge Air
//! and dumps loads onto wet seats when the chunk reloads via
//! [`crate::chunk_store::sync_residency`].

use crate::cell::{Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fasthash::FxHashMap;
use crate::grid::World;
use crate::mineral::{add_dissolved, dissolved_at, take_dissolved};
use crate::sediment::{add_suspended, suspended_at, take_suspended};
use wk_material::MaterialId;

/// Per-chunk backlog waiting for thaw (WORLDGEN Rule 1, water + loads).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct FrozenBacklog {
    /// Inflow onto local `lx == 0` (from the west neighbour).
    pub water_in_left: i64,
    /// Inflow onto local `lx == CHUNK_CELLS_W - 1` (from the east).
    pub water_in_right: i64,
    /// Vertical / interior of a missing slab (same `cx`, other `cy`).
    pub water_other: i64,
    /// Suspended fine sediment (clay) units riding the absorbed water.
    pub sediment_in: i64,
    /// Dissolved mineral load units riding the absorbed water.
    pub dissolved_in: i64,
}

impl FrozenBacklog {
    pub fn water_total(self) -> i64 {
        self.water_in_left + self.water_in_right + self.water_other
    }

    pub fn total(self) -> i64 {
        self.water_total()
    }

    pub fn is_empty(self) -> bool {
        self.water_total() == 0 && self.sediment_in == 0 && self.dissolved_in == 0
    }
}

/// Sum of all parked backlog water (for [`crate::audit::sat_totals`]).
pub fn frozen_backlog_total(world: &World) -> i64 {
    world
        .frozen_backlog
        .values()
        .map(|b| b.water_total())
        .sum()
}

/// Sum of parked suspended sediment in the backlog.
pub fn frozen_backlog_sediment_total(world: &World) -> i64 {
    world
        .frozen_backlog
        .values()
        .map(|b| b.sediment_in)
        .sum()
}

/// Sum of parked dissolved mineral in the backlog.
pub fn frozen_backlog_dissolved_total(world: &World) -> i64 {
    world
        .frozen_backlog
        .values()
        .map(|b| b.dissolved_in)
        .sum()
}

/// Book `amt` sat (+ pro-rata loads from `from`) into the dest chunk backlog.
pub fn absorb_water(
    world: &mut World,
    from: (i32, i32),
    to_gx: i32,
    to_gy: i32,
    amt: i32,
    donor_sat_before: u8,
) {
    if amt <= 0 {
        return;
    }
    let gx = world.wrap_x(to_gx);
    let (coord, lx, _ly) = World::split(gx, to_gy);
    let mut sed_moved = 0u16;
    let mut diss_moved = 0u16;
    if donor_sat_before > 0 {
        let sed = suspended_at(world, from.0, from.1);
        if sed > 0 {
            let share = (sed as u32 * amt as u32 / donor_sat_before as u32) as u16;
            sed_moved = take_suspended(world, from.0, from.1, share);
        }
        let diss = dissolved_at(world, from.0, from.1);
        if diss > 0 {
            let share = ((diss as u32 * amt as u32) / donor_sat_before as u32) as u16;
            let share = share.max(1);
            diss_moved = take_dissolved(world, from.0, from.1, share);
        }
    }
    let entry = world.frozen_backlog.entry(coord).or_default();
    if lx == 0 {
        entry.water_in_left += amt as i64;
    } else if lx as usize == CHUNK_CELLS_W - 1 {
        entry.water_in_right += amt as i64;
    } else {
        entry.water_other += amt as i64;
    }
    entry.sediment_in += sed_moved as i64;
    entry.dissolved_in += diss_moved as i64;
}

/// Paint backlog onto Air cells of a loaded chunk; clear the entry.
///
/// Remainder that does not fit stays booked on the coord.
pub fn apply_frozen_backlog(world: &mut World, coord: ChunkCoord) {
    let Some(mut bl) = world.frozen_backlog.remove(&coord) else {
        return;
    };
    if bl.is_empty() {
        return;
    }
    if !world.chunks.contains_key(&coord) {
        world.frozen_backlog.insert(coord, bl);
        return;
    }
    bl.water_in_left = fill_edge_column(world, coord, 0, bl.water_in_left);
    bl.water_in_right = fill_edge_column(
        world,
        coord,
        (CHUNK_CELLS_W - 1) as i32,
        bl.water_in_right,
    );
    bl.water_other = fill_chunk_air(world, coord, bl.water_other);
    dump_loads(world, coord, &mut bl);
    if !bl.is_empty() {
        world.frozen_backlog.insert(coord, bl);
    }
}

fn dump_loads(world: &mut World, coord: ChunkCoord, bl: &mut FrozenBacklog) {
    if bl.sediment_in == 0 && bl.dissolved_in == 0 {
        return;
    }
    let base_x = coord.cx * CHUNK_CELLS_W as i32;
    let base_y = coord.cy * CHUNK_CELLS_H as i32;
    for ly in 0..CHUNK_CELLS_H as i32 {
        for lx in 0..CHUNK_CELLS_W as i32 {
            if bl.sediment_in == 0 && bl.dissolved_in == 0 {
                return;
            }
            let gx = base_x + lx;
            let gy = base_y + ly;
            let Some(cell) = world.get_cell(gx, gy) else {
                continue;
            };
            if cell.material != MaterialId::Air || cell.sat.is_empty() {
                continue;
            }
            if bl.sediment_in > 0 {
                let give = bl.sediment_in.min(u16::MAX as i64) as u16;
                add_suspended(world, gx, gy, give);
                bl.sediment_in -= give as i64;
            }
            if bl.dissolved_in > 0 {
                let give = bl.dissolved_in.min(u16::MAX as i64) as u16;
                add_dissolved(world, gx, gy, give);
                bl.dissolved_in -= give as i64;
            }
        }
    }
}

fn fill_edge_column(world: &mut World, coord: ChunkCoord, lx: i32, mut amt: i64) -> i64 {
    if amt <= 0 {
        return 0;
    }
    let base_x = coord.cx * CHUNK_CELLS_W as i32 + lx;
    let base_y = coord.cy * CHUNK_CELLS_H as i32;
    for ly in 0..CHUNK_CELLS_H as i32 {
        if amt <= 0 {
            break;
        }
        let gx = base_x;
        let gy = base_y + ly;
        let Some(cell) = world.get_cell(gx, gy) else {
            continue;
        };
        if cell.material != MaterialId::Air {
            continue;
        }
        let free = (u8::MAX - cell.sat.0) as i64;
        if free <= 0 {
            continue;
        }
        let give = amt.min(free) as u8;
        world.set_cell(
            gx,
            gy,
            Cell {
                sat: Sat(cell.sat.0 + give),
                ..cell
            },
        );
        amt -= give as i64;
    }
    amt
}

fn fill_chunk_air(world: &mut World, coord: ChunkCoord, mut amt: i64) -> i64 {
    if amt <= 0 {
        return 0;
    }
    let base_x = coord.cx * CHUNK_CELLS_W as i32;
    let base_y = coord.cy * CHUNK_CELLS_H as i32;
    for ly in 0..CHUNK_CELLS_H as i32 {
        for lx in 0..CHUNK_CELLS_W as i32 {
            if amt <= 0 {
                return 0;
            }
            let gx = base_x + lx;
            let gy = base_y + ly;
            let Some(cell) = world.get_cell(gx, gy) else {
                continue;
            };
            if cell.material != MaterialId::Air {
                continue;
            }
            let free = (u8::MAX - cell.sat.0) as i64;
            if free <= 0 {
                continue;
            }
            let give = amt.min(free) as u8;
            world.set_cell(
                gx,
                gy,
                Cell {
                    sat: Sat(cell.sat.0 + give),
                    ..cell
                },
            );
            amt -= give as i64;
        }
    }
    amt
}

/// True when streaming is on and the destination cell's chunk is not loaded.
pub fn should_absorb_missing(world: &World, to_gx: i32, to_gy: i32) -> bool {
    if world.stream_window.is_none() {
        return false;
    }
    let gx = world.wrap_x(to_gx);
    let (coord, _, _) = World::split(gx, to_gy);
    !world.chunks.contains_key(&coord)
}

/// Helper for tests / diagnostics.
pub fn backlog_map(world: &World) -> &FxHashMap<ChunkCoord, FrozenBacklog> {
    &world.frozen_backlog
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audit::{mineral_total, sat_totals, sediment_total};
    use crate::streaming::StreamWindow;

    #[test]
    fn absorb_then_apply_preserves_sat() {
        let mut w = World::new(1);
        w.wrap_width = Some(64 * CHUNK_CELLS_W as i32);
        let near_x = CHUNK_CELLS_W as i32 - 1;
        let far_x = CHUNK_CELLS_W as i32; // lx=0 of cx=1
        for x in 0..(2 * CHUNK_CELLS_W as i32) {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
        }
        w.set_cell(near_x, 1, Cell::water());
        let water0 = sat_totals(&w).cell_total;

        // Drop cx=1 from RAM while streaming.
        let drop: Vec<_> = w
            .chunks
            .keys()
            .copied()
            .filter(|c| c.cx == 1)
            .collect();
        for c in drop {
            w.chunks.remove(&c);
        }
        w.invalidate_chunk_cache_pub();
        w.stream_window = Some(StreamWindow::new(0, 128));
        assert!(should_absorb_missing(&w, far_x, 1));

        let src = w.get_cell(near_x, 1).unwrap();
        let amt = src.sat.0 as i32;
        let before = src.sat.0;
        w.set_cell(
            near_x,
            1,
            Cell {
                sat: Sat(0),
                ..src
            },
        );
        absorb_water(&mut w, (near_x, 1), far_x, 1, amt, before);
        assert_eq!(frozen_backlog_total(&w), amt as i64);
        assert_eq!(sat_totals(&w).cell_total, water0);

        // Remint blank slab (Air) and thaw.
        w.ensure_chunk(ChunkCoord::new(1, 0));
        apply_frozen_backlog(&mut w, ChunkCoord::new(1, 0));
        assert_eq!(frozen_backlog_total(&w), 0);
        assert_eq!(sat_totals(&w).cell_total, water0);
        let edge_sat: i64 = (0..CHUNK_CELLS_H as i32)
            .filter_map(|y| w.get_cell(far_x, y).map(|c| c.sat.0 as i64))
            .sum();
        assert_eq!(edge_sat, water0);
    }

    #[test]
    fn absorb_carries_suspended_and_dissolved() {
        let mut w = World::new(1);
        w.wrap_width = Some(64 * CHUNK_CELLS_W as i32);
        let near_x = CHUNK_CELLS_W as i32 - 1;
        let far_x = CHUNK_CELLS_W as i32;
        w.set_cell(near_x, 1, Cell::water());
        add_suspended(&mut w, near_x, 1, 40);
        add_dissolved(&mut w, near_x, 1, 25);
        let sed0 = sediment_total(&w);
        let min0 = mineral_total(&w);

        let drop: Vec<_> = w
            .chunks
            .keys()
            .copied()
            .filter(|c| c.cx == 1)
            .collect();
        for c in drop {
            w.chunks.remove(&c);
        }
        w.invalidate_chunk_cache_pub();
        w.stream_window = Some(StreamWindow::new(0, 128));

        let src = w.get_cell(near_x, 1).unwrap();
        let before = src.sat.0;
        let amt = before as i32;
        w.set_cell(
            near_x,
            1,
            Cell {
                sat: Sat(0),
                ..src
            },
        );
        absorb_water(&mut w, (near_x, 1), far_x, 1, amt, before);
        assert!(frozen_backlog_sediment_total(&w) > 0);
        assert!(frozen_backlog_dissolved_total(&w) > 0);
        assert_eq!(sediment_total(&w), sed0);
        assert_eq!(mineral_total(&w), min0);

        w.ensure_chunk(ChunkCoord::new(1, 0));
        apply_frozen_backlog(&mut w, ChunkCoord::new(1, 0));
        assert_eq!(frozen_backlog_sediment_total(&w), 0);
        assert_eq!(frozen_backlog_dissolved_total(&w), 0);
        assert_eq!(sediment_total(&w), sed0);
        assert_eq!(mineral_total(&w), min0);
    }

    #[test]
    fn no_absorb_without_stream_window() {
        let mut w = World::new(1);
        assert!(!should_absorb_missing(&w, 64, 1));
    }
}
