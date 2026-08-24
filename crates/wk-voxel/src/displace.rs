//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app.
//!
//! Displacement bookkeeping for solids that move into occupied cells.
//!
//! A moving body must **shift** what is in its way, never consume it:
//!
//! - Free water lives as `sat` on `Air` cells, so overwriting an Air cell with a
//!   solid destroys that water unless the units are carried over. Rock dropped
//!   in a lake raises the level, it does not drink it.
//! - Loose material (sand, soil, clay, gravel, loose rock, snow, litter) is a
//!   whole cell. Overwriting it deletes real mass, so it is relocated instead.
//!
//! Both follow the same shape: take from every cell the body will occupy, write
//! the body, then deposit into the cells the body **vacated**. A body vacates
//! exactly as many cells as it occupies, so the volume it swaps out always has
//! room for what it pushed aside.

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

/// A loose cell lifted out of a body's path, waiting to be re-homed.
#[derive(Debug, Clone, Copy)]
pub struct ShiftedCell {
  pub cell: Cell,
  /// Where it came from, so deposits can prefer staying nearby.
  pub from: (i32, i32),
}

/// Remove a loose cell from a body's path, keeping its material and pore water.
///
/// Leaves behind any free water that was sharing the space (there is none for a
/// solid, but the destination swap in [`deposit_shifted_cells`] relies on the
/// same convention).
pub fn take_soft_cell(
  world: &mut World,
  gx: i32,
  gy: i32,
  is_soft: impl Fn(MaterialId) -> bool,
) -> Option<ShiftedCell> {
  let wx = world.wrap_x(gx);
  let cur = world.get_cell(wx, gy)?;
  if !is_soft(cur.material) {
    return None;
  }
  world.set_cell(wx, gy, Cell::air());
  Some(ShiftedCell {
    cell: cur,
    from: (wx, gy),
  })
}

/// Re-home loose cells the body shoved aside.
///
/// Tries `prefer` first (normally the cells the body vacated), then a bounded
/// search outward from each cell's origin, biased upward — material shoved by a
/// sinking rock piles up beside and above it. Returns any cells that found no
/// space; grain settling will tidy the resulting heap on later ticks.
pub fn deposit_shifted_cells(
  world: &mut World,
  mut shifted: Vec<ShiftedCell>,
  prefer: &[(i32, i32)],
  blocked: &HashSet<(i32, i32)>,
) -> Vec<ShiftedCell> {
  if shifted.is_empty() {
    return shifted;
  }
  let mut open: VecDeque<(i32, i32)> = prefer
    .iter()
    .map(|&(x, y)| (world.wrap_x(x), y))
    .filter(|p| !blocked.contains(p))
    .collect();
  let mut leftover = Vec::new();

  while let Some(item) = shifted.pop() {
    // Preferred (vacated) slots first — they are guaranteed-sized for the swap.
    let mut placed = false;
    while let Some((x, y)) = open.pop_front() {
      if matches!(world.get_cell(x, y), Some(c) if c.material == MaterialId::Air) {
        place_soft(world, x, y, item.cell);
        placed = true;
        break;
      }
    }
    if placed {
      continue;
    }
    if let Some((x, y)) = find_open_near(world, item.from, blocked) {
      place_soft(world, x, y, item.cell);
    } else {
      leftover.push(item);
    }
  }
  leftover
}

/// Write a loose cell into an Air slot, keeping any free water that was there.
fn place_soft(world: &mut World, x: i32, y: i32, mut cell: Cell) {
  if let Some(dst) = world.get_cell(x, y) {
    if dst.sat.0 > 0 && crate::cell::water_capacity(cell.material) > 0 {
      // Wet slot: let the grain soak up what it can hold.
      let cap = crate::cell::water_capacity(cell.material) as u32;
      let take = cap.saturating_sub(cell.sat.0 as u32).min(dst.sat.0 as u32);
      cell.sat = Sat((cell.sat.0 as u32 + take) as u8);
    }
  }
  world.set_cell(x, y, cell);
  world.touch_dirty(x, y);
}

/// Nearest Air cell to `from`, searched upward-first within a bounded radius.
fn find_open_near(
  world: &World,
  from: (i32, i32),
  blocked: &HashSet<(i32, i32)>,
) -> Option<(i32, i32)> {
  const MAX_VISIT: usize = 512;
  let mut seen: HashSet<(i32, i32)> = HashSet::default();
  let mut q = VecDeque::new();
  seen.insert(from);
  q.push_back(from);
  let mut visited = 0usize;
  while let Some((x, y)) = q.pop_front() {
    if visited >= MAX_VISIT {
      break;
    }
    visited += 1;
    // Up first, then sideways, then down: shoved material heaps upward.
    for (dx, dy) in [(0, 1), (1, 0), (-1, 0), (0, -1)] {
      let nx = world.wrap_x(x + dx);
      let ny = y + dy;
      if ny < 0 || !seen.insert((nx, ny)) {
        continue;
      }
      match world.get_cell(nx, ny) {
        Some(c) if c.material == MaterialId::Air => {
          if !blocked.contains(&(nx, ny)) {
            return Some((nx, ny));
          }
          q.push_back((nx, ny));
        }
        Some(_) => q.push_back((nx, ny)),
        None => {}
      }
    }
  }
  None
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
