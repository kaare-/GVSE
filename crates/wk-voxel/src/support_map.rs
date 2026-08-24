//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Canonical **surface** and **grounded** maps for landscape fall.
//!
//! - **Surface** — solid cell with an open (Air / missing) 4-neighbour.
//! - **Grounded** — solid reachable from Bedrock through solid cells.
//!
//! Landscape detach uses a third derived view: cells that sit on void
//! (air below) and are not *column-supported* down to Bedrock — those
//! form hanging clusters even when laterally welded to a hill.

use std::collections::{HashMap, HashSet, VecDeque};

use wk_material::MaterialId;

use crate::cell::is_competent_rock;
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::grid::World;

/// Rebuild period (ticks) for a quiet full refresh.
pub const SUPPORT_MAP_PERIOD: u64 = 8;
pub const SUPPORT_MAP_PHASE: u64 = 1;

pub fn support_map_due(tick: u64) -> bool {
  tick % SUPPORT_MAP_PERIOD == SUPPORT_MAP_PHASE
}

#[inline]
fn is_open(world: &World, gx: i32, gy: i32) -> bool {
  match world.get_cell(gx, gy) {
    None => true,
    Some(c) => c.material == MaterialId::Air,
  }
}

#[inline]
fn is_support_solid(mat: MaterialId) -> bool {
  mat.is_solid()
}

/// Sparse surface + grounded overlays.
#[derive(Debug, Clone, Default)]
pub struct SupportMap {
  /// Solid cells with at least one open 4-neighbour.
  pub surface: HashSet<(i32, i32)>,
  /// Solids connected to Bedrock through solids.
  pub grounded: HashSet<(i32, i32)>,
  pub last_rebuild_tick: u64,
}

impl SupportMap {
  pub fn new() -> Self {
    Self {
      surface: HashSet::new(),
      grounded: HashSet::new(),
      last_rebuild_tick: u64::MAX,
    }
  }

  pub fn is_surface(&self, gx: i32, gy: i32) -> bool {
    self.surface.contains(&(gx, gy))
  }

  pub fn is_grounded(&self, gx: i32, gy: i32) -> bool {
    self.grounded.contains(&(gx, gy))
  }

  /// Full rebuild over all loaded chunks.
  pub fn rebuild(&mut self, world: &World) {
    self.surface.clear();
    self.grounded.clear();
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    self.rebuild_coords(world, &coords);
    self.last_rebuild_tick = world.tick;
  }

  /// Rebuild only listed chunks (plus a 1-cell halo for surface edges).
  pub fn rebuild_coords(&mut self, world: &World, coords: &[ChunkCoord]) {
    if coords.is_empty() {
      return;
    }
    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;

    // Drop stale entries for these chunks before rescanning.
    for &coord in coords {
      let x0 = coord.cx * cw;
      let y0 = coord.cy * ch;
      self.surface.retain(|&(x, y)| {
        !(x >= x0 && x < x0 + cw && y >= y0 && y < y0 + ch)
      });
      self.grounded.retain(|&(x, y)| {
        !(x >= x0 && x < x0 + cw && y >= y0 && y < y0 + ch)
      });
    }

    // Surface pass.
    for &coord in coords {
      let Some(chunk) = world.chunks.get(&coord) else {
        continue;
      };
      let base_gx = coord.cx * cw;
      let base_gy = coord.cy * ch;
      for ly in 0..CHUNK_CELLS_H {
        for lx in 0..CHUNK_CELLS_W {
          let cell = chunk.get(lx, ly);
          if !is_support_solid(cell.material) {
            continue;
          }
          let gx = world.wrap_x(base_gx + lx as i32);
          let gy = base_gy + ly as i32;
          let open = [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .iter()
            .any(|(dx, dy)| is_open(world, world.wrap_x(gx + dx), gy + dy));
          if open {
            self.surface.insert((gx, gy));
          }
        }
      }
    }

    // Grounded flood from Bedrock across the whole loaded world — support
    // paths routinely cross chunk seams.
    let mut q = VecDeque::new();
    for (&coord, chunk) in &world.chunks {
      let base_gx = coord.cx * cw;
      let base_gy = coord.cy * ch;
      for ly in 0..CHUNK_CELLS_H {
        for lx in 0..CHUNK_CELLS_W {
          let cell = chunk.get(lx, ly);
          if cell.material != MaterialId::Bedrock {
            continue;
          }
          let gx = world.wrap_x(base_gx + lx as i32);
          let gy = base_gy + ly as i32;
          if self.grounded.insert((gx, gy)) {
            q.push_back((gx, gy));
          }
        }
      }
    }
    while let Some((x, y)) = q.pop_front() {
      for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let nx = world.wrap_x(x + dx);
        let ny = y + dy;
        if self.grounded.contains(&(nx, ny)) {
          continue;
        }
        let Some(n) = world.get_cell(nx, ny) else {
          continue;
        };
        if !is_support_solid(n.material) {
          continue;
        }
        if self.grounded.insert((nx, ny)) {
          q.push_back((nx, ny));
        }
      }
    }
  }

  /// True when walking down through solids in this column hits Bedrock
  /// before open air.
  pub fn column_supported(world: &World, gx: i32, gy: i32) -> bool {
    let mut y = gy;
    let mut guard = 0;
    while guard < 512 {
      guard += 1;
      let by = y - 1;
      match world.get_cell(gx, by) {
        None => return false,
        Some(c) if c.material == MaterialId::Air => return false,
        Some(c) if c.material == MaterialId::Bedrock => return true,
        Some(c) if is_support_solid(c.material) => {
          y = by;
        }
        Some(_) => return false,
      }
    }
    false
  }
}

/// Flood a hanging competent mass that sits on void (not column-supported).
/// Stops at column-supported competent (hill / pillar legs stay).
pub fn hanging_landscape_cluster(
  world: &World,
  seed_x: i32,
  seed_y: i32,
  max_cells: usize,
) -> Vec<(i32, i32)> {
  let Some(seed) = world.get_cell(seed_x, seed_y) else {
    return Vec::new();
  };
  if !is_competent_rock(seed.material) {
    return Vec::new();
  }
  if SupportMap::column_supported(world, seed_x, seed_y) {
    return Vec::new();
  }
  // Must sit on void, or connect to a cell that does.
  let mut out = Vec::new();
  let mut seen = HashSet::new();
  let mut q = VecDeque::new();
  q.push_back((seed_x, seed_y));
  seen.insert((seed_x, seed_y));
  while let Some((x, y)) = q.pop_front() {
    if SupportMap::column_supported(world, x, y) {
      continue;
    }
    out.push((x, y));
    if out.len() >= max_cells {
      break;
    }
    for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
      let nx = world.wrap_x(x + dx);
      let ny = y + dy;
      if !seen.insert((nx, ny)) {
        continue;
      }
      let Some(n) = world.get_cell(nx, ny) else {
        continue;
      };
      if !is_competent_rock(n.material) {
        continue;
      }
      if SupportMap::column_supported(world, nx, ny) {
        continue;
      }
      q.push_back((nx, ny));
    }
  }
  out
}

/// Collect void-below competent seeds across loaded chunks (or active set).
pub fn void_below_competent_seeds(world: &World, coords: &[ChunkCoord]) -> Vec<(i32, i32)> {
  let cw = CHUNK_CELLS_W as i32;
  let ch = CHUNK_CELLS_H as i32;
  let mut seeds = Vec::new();
  let scan: Vec<ChunkCoord> = if coords.is_empty() {
    world.chunks.keys().copied().collect()
  } else {
    coords.to_vec()
  };
  for coord in scan {
    let Some(chunk) = world.chunks.get(&coord) else {
      continue;
    };
    let base_gx = coord.cx * cw;
    let base_gy = coord.cy * ch;
    for ly in 0..CHUNK_CELLS_H {
      for lx in 0..CHUNK_CELLS_W {
        let cell = chunk.get(lx, ly);
        if !is_competent_rock(cell.material) {
          continue;
        }
        let gx = world.wrap_x(base_gx + lx as i32);
        let gy = base_gy + ly as i32;
        if !is_open(world, gx, gy - 1) {
          continue;
        }
        if SupportMap::column_supported(world, gx, gy) {
          continue;
        }
        seeds.push((gx, gy));
      }
    }
  }
  seeds
}

/// Debug counts for HUD / tests.
pub fn support_counts(map: &SupportMap) -> (usize, usize) {
  (map.surface.len(), map.grounded.len())
}

/// Occupancy hint used by tests.
pub fn surface_ratio_near(
  map: &SupportMap,
  world: &World,
  solids: &HashMap<(i32, i32), ()>,
) -> f32 {
  if solids.is_empty() {
    return 0.0;
  }
  let mut n = 0usize;
  for &(x, y) in solids.keys() {
    if map.is_surface(x, y) || {
      // Fresh check if map stale.
      let open = [(-1, 0), (1, 0), (0, -1), (0, 1)]
        .iter()
        .any(|(dx, dy)| is_open(world, world.wrap_x(x + dx), y + dy));
      open
    } {
      n += 1;
    }
  }
  n as f32 / solids.len() as f32
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::cell::Cell;
  use crate::chunk::ChunkCoord;

  #[test]
  fn bedrock_column_is_grounded_and_floating_slab_is_not() {
    let mut w = World::new(32);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..32 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=10 {
      w.set_cell(5, y, Cell::solid(MaterialId::Stone));
    }
    for x in 10..20 {
      for y in 30..35 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mut map = SupportMap::new();
    map.rebuild(&w);
    assert!(map.is_grounded(5, 5), "pillar rooted in bedrock");
    assert!(!map.is_grounded(15, 32), "sky slab not grounded");
    assert!(map.is_surface(15, 30), "slab underside is surface");
    assert!(SupportMap::column_supported(&w, 5, 10));
    assert!(!SupportMap::column_supported(&w, 15, 30));
  }

  #[test]
  fn hanging_cluster_stops_at_column_supported_leg() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=20 {
      w.set_cell(5, y, Cell::solid(MaterialId::Stone));
      w.set_cell(30, y, Cell::solid(MaterialId::Stone));
    }
    for x in 5..=30 {
      for y in 18..=22 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    // Carve under the span.
    for x in 6..30 {
      for y in 1..18 {
        w.set_cell(x, y, Cell::air());
      }
    }
    let cluster = hanging_landscape_cluster(&w, 18, 18, 4096);
    assert!(
      cluster.len() >= 40,
      "span must detach (got {})",
      cluster.len()
    );
    assert!(
      !cluster.iter().any(|&(x, _)| x == 5 || x == 30),
      "column-supported legs must stay"
    );
  }
}
