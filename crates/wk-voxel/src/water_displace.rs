//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app.
//!
//! Water displacement bookkeeping for solids that move into wet cells.
//!
//! Free water lives as `sat` on `Air` cells, so any rule that overwrites an Air
//! cell with a solid **destroys** that water unless the units are carried over.
//! Rock dropped in a lake must raise the level, not drink it.
//!
//! Usage: [`take_free_water`] every cell the body will occupy, then
//! [`deposit_free_water`] once the write is done, preferring the cells the body
//! vacated.

use std::collections::VecDeque;

use wk_material::MaterialId;

use crate::cell::{Cell, Sat};
use crate::fasthash::FxHashSet as HashSet;
use crate::grid::World;

/// Cells visited when searching for somewhere to put displaced water.
pub const WATER_SPREAD_MAX_VISIT: usize = 4096;

/// Remove and return the free water in a cell (0 when dry or not `Air`).
#[inline]
pub fn take_free_water(world: &mut World, gx: i32, gy: i32) -> u32 {
  let wx = world.wrap_x(gx);
  let Some(cur) = world.get_cell(wx, gy) else {
    return 0;
  };
  if cur.material != MaterialId::Air || cur.sat.0 == 0 {
    return 0;
  }
  let units = cur.sat.0 as u32;
  world.set_cell(wx, gy, Cell::air());
  units
}

/// Water a solid cell was holding in its pores, as free-water units.
#[inline]
pub fn pore_water_of(cell: &Cell) -> u32 {
  cell.sat.0 as u32
}

fn fill_cell(world: &mut World, x: i32, y: i32, units: &mut u32) {
  if *units == 0 {
    return;
  }
  let Some(cur) = world.get_cell(x, y) else {
    return;
  };
  if cur.material != MaterialId::Air {
    return;
  }
  let cap = world.water_capacity(MaterialId::Air) as u32;
  let room = cap.saturating_sub(cur.sat.0 as u32);
  if room == 0 {
    return;
  }
  let put = room.min(*units);
  let mut next = cur;
  next.sat = Sat((cur.sat.0 as u32 + put).min(u8::MAX as u32) as u8);
  world.set_cell(x, y, next);
  world.touch_dirty(x, y);
  *units -= put;
}

/// Pour displaced water back into the world.
///
/// `prefer` is tried in order first (normally the cells the body vacated, which
/// is exactly the volume it swapped out of the lake). Remaining units spread by
/// a bounded flood biased upward, since displaced water rises. Returns units
/// that found no capacity — nonzero means genuine loss.
pub fn deposit_free_water(
  world: &mut World,
  mut units: u32,
  prefer: &[(i32, i32)],
  blocked: &HashSet<(i32, i32)>,
) -> u32 {
  if units == 0 {
    return 0;
  }
  for &(x, y) in prefer {
    let wx = world.wrap_x(x);
    if blocked.contains(&(wx, y)) {
      continue;
    }
    fill_cell(world, wx, y, &mut units);
    if units == 0 {
      return 0;
    }
  }

  let mut seen: HashSet<(i32, i32)> = prefer
    .iter()
    .map(|&(x, y)| (world.wrap_x(x), y))
    .collect();
  let mut q: VecDeque<(i32, i32)> = seen.iter().copied().collect();
  if q.is_empty() {
    return units;
  }
  let mut visited = 0usize;
  while let Some((x, y)) = q.pop_front() {
    if units == 0 || visited >= WATER_SPREAD_MAX_VISIT {
      break;
    }
    visited += 1;
    // Upward first: a submerged body raises the surface above it.
    for (dx, dy) in [(0, 1), (1, 0), (-1, 0), (0, -1)] {
      let nx = world.wrap_x(x + dx);
      let ny = y + dy;
      if ny < 0 || !seen.insert((nx, ny)) {
        continue;
      }
      match world.get_cell(nx, ny) {
        Some(c) if c.material == MaterialId::Air => {
          if !blocked.contains(&(nx, ny)) {
            fill_cell(world, nx, ny, &mut units);
          }
          q.push_back((nx, ny));
        }
        // Keep expanding past solids — water routes around them.
        Some(_) => q.push_back((nx, ny)),
        None => {}
      }
      if units == 0 {
        break;
      }
    }
  }
  units
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::chunk::ChunkCoord;

  fn wet(sat: u8) -> Cell {
    Cell {
      material: MaterialId::Air,
      sat: Sat(sat),
      ..Cell::default()
    }
  }

  #[test]
  fn take_then_deposit_conserves_units() {
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.set_cell(5, 5, wet(200));
    let got = take_free_water(&mut w, 5, 5);
    assert_eq!(got, 200);
    assert_eq!(w.get_cell(5, 5).unwrap().sat.0, 0);
    let left = deposit_free_water(&mut w, got, &[(5, 6)], &HashSet::default());
    assert_eq!(left, 0);
    assert_eq!(w.get_cell(5, 6).unwrap().sat.0, 200);
  }

  #[test]
  fn deposit_overflows_into_neighbours() {
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    // Only 255 fits per cell, so 400 units need two cells.
    let left = deposit_free_water(&mut w, 400, &[(5, 5)], &HashSet::default());
    assert_eq!(left, 0, "overflow must find another cell");
    let total: u32 = (0..12)
      .flat_map(|x| (0..12).map(move |y| (x, y)))
      .filter_map(|(x, y)| w.get_cell(x, y))
      .filter(|c| c.material == MaterialId::Air)
      .map(|c| c.sat.0 as u32)
      .sum();
    assert_eq!(total, 400);
  }

  #[test]
  fn deposit_skips_blocked_cells() {
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    let mut blocked = HashSet::default();
    blocked.insert((5, 5));
    let left = deposit_free_water(&mut w, 100, &[(5, 5)], &blocked);
    assert_eq!(left, 0);
    assert_eq!(
      w.get_cell(5, 5).unwrap().sat.0,
      0,
      "blocked cell must stay dry"
    );
  }

  #[test]
  fn solid_cells_never_take_free_water() {
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.set_cell(5, 5, Cell::solid(MaterialId::Stone));
    assert_eq!(take_free_water(&mut w, 5, 5), 0);
    assert_eq!(w.get_cell(5, 5).unwrap().material, MaterialId::Stone);
  }
}
