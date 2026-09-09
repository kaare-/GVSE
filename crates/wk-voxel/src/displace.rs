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

use crate::cell::{water_capacity_cell, Cell, Sat};
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

/// Park orphan water near `(gx, gy)` so a take-then-partial-place path never
/// silently deletes the remainder.
///
/// Order: free Air sat upward → lateral Air / porous room → cave humidity.
/// Returns units still unplaced (should be rare; only when the neighbourhood
/// and the cave-humidity map are both full).
pub fn park_orphan_water(world: &mut World, gx: i32, gy: i32, mut units: u32) -> u32 {
  if units == 0 {
    return 0;
  }
  let gx = world.wrap_x(gx);
  for dy in 0..16 {
    if units == 0 {
      return 0;
    }
    let y = gy + dy;
    let Some(mut c) = world.get_cell(gx, y) else {
      break;
    };
    if c.material != MaterialId::Air {
      continue;
    }
    let room = (u8::MAX - c.sat.0) as u32;
    let put = room.min(units);
    if put > 0 {
      c.sat = Sat(c.sat.0 + put as u8);
      world.set_cell(gx, y, c);
      units -= put;
    }
  }
  if units > 0 {
    for (dx, dy) in [
      (-1, 0),
      (1, 0),
      (-1, 1),
      (1, 1),
      (0, 1),
      (-2, 0),
      (2, 0),
      (0, 2),
      (-1, 2),
      (1, 2),
    ] {
      if units == 0 {
        break;
      }
      let x = world.wrap_x(gx + dx);
      let y = gy + dy;
      let Some(mut c) = world.get_cell(x, y) else {
        continue;
      };
      let cap = water_capacity_cell(c, &world.hydro) as u32;
      if cap == 0 {
        continue;
      }
      let room = cap.saturating_sub(c.sat.0 as u32);
      let put = room.min(units);
      if put == 0 {
        continue;
      }
      c.sat = Sat(c.sat.0 + put as u8);
      world.set_cell(x, y, c);
      units -= put;
    }
  }
  if units > 0 {
    for (dx, dy) in [
      (0, 0),
      (0, 1),
      (-1, 0),
      (1, 0),
      (0, 2),
      (-1, 1),
      (1, 1),
      (0, 3),
    ] {
      if units == 0 {
        break;
      }
      let x = world.wrap_x(gx + dx);
      let y = gy + dy;
      if !world
        .get_cell(x, y)
        .is_some_and(|c| c.material == MaterialId::Air)
      {
        continue;
      }
      while units > 0 {
        let chunk = units.min(255) as u8;
        let put = crate::cave_humidity::try_add_cave_humidity(world, x, y, chunk);
        if put == 0 {
          break;
        }
        units -= put as u32;
      }
    }
  }
  units
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
    if let Some(&(sx, sy)) = prefer.first() {
      return park_orphan_water(world, sx, sy, units);
    }
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
  if units > 0 {
    let seed = prefer
      .first()
      .copied()
      .or_else(|| seen.iter().next().copied())
      .unwrap_or((0, 0));
    units = park_orphan_water(world, seed.0, seed.1, units);
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
    if dst.sat.0 > 0 && crate::cell::water_capacity_cell(cell, &world.hydro) > 0 {
      // Wet slot: let the grain soak up what it can hold.
      let cap = crate::cell::water_capacity_cell(cell, &world.hydro) as u32;
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
  fn park_orphan_water_under_lid_uses_cave_humidity() {
    let mut w = World::new(1);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    // Sealed stone box with one Air cell already full of sat.
    for x in 3..8 {
      for y in 1..6 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mut full = Cell::air();
    full.sat = Sat(255);
    w.set_cell(5, 2, full);
    w.set_cell(5, 3, Cell::air()); // dry neighbour for cave humidity
    w.set_cell(5, 4, Cell::solid(MaterialId::Stone)); // roof
    let before = crate::audit::sat_totals(&w).cell_total;
    let left = park_orphan_water(&mut w, 5, 2, 40);
    assert_eq!(left, 0, "must park under a lid");
    let after = crate::audit::sat_totals(&w).cell_total;
    assert_eq!(after, before + 40, "parked units must stay in the cell budget");
    assert!(
      crate::cave_humidity::cave_humidity_at(&w, 5, 3) > 0
        || w.get_cell(5, 3).unwrap().sat.0 > 0,
      "units must land as sat or cave humidity"
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
