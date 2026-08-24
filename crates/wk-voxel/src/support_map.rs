//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Canonical **surface** and **grounded** maps for landscape fall.
//!
//! - **Surface** — solid cell with an open (Air / missing) 4-neighbour.
//! - **Grounded** — solid reachable from Bedrock through solid cells.
//!
//! "Ungrounded" is the canonical floater test: a solid that cannot reach
//! bedrock through solids is hanging, no matter how it is welded sideways.
//!
//! Both layers are **per-chunk bitsets**, not `HashSet<(i32, i32)>`. One bit
//! per cell (512 B per 64×64 chunk) turns a per-cell hash into a chunk lookup
//! plus a bit test, which is what makes a full rebuild affordable.

use std::collections::HashMap;

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

const CELLS_PER_CHUNK: usize = CHUNK_CELLS_W * CHUNK_CELLS_H;
const WORDS_PER_CHUNK: usize = (CELLS_PER_CHUNK + 63) / 64;

/// One bit per cell in a chunk.
#[derive(Debug, Clone)]
pub struct ChunkMask {
  words: [u64; WORDS_PER_CHUNK],
}

impl Default for ChunkMask {
  fn default() -> Self {
    Self {
      words: [0; WORDS_PER_CHUNK],
    }
  }
}

impl ChunkMask {
  #[inline]
  fn idx(lx: usize, ly: usize) -> usize {
    ly * CHUNK_CELLS_W + lx
  }

  #[inline]
  pub fn get(&self, lx: usize, ly: usize) -> bool {
    let i = Self::idx(lx, ly);
    self.words[i >> 6] & (1u64 << (i & 63)) != 0
  }

  #[inline]
  pub fn set(&mut self, lx: usize, ly: usize) {
    let i = Self::idx(lx, ly);
    self.words[i >> 6] |= 1u64 << (i & 63);
  }

  #[inline]
  pub fn unset(&mut self, lx: usize, ly: usize) {
    let i = Self::idx(lx, ly);
    self.words[i >> 6] &= !(1u64 << (i & 63));
  }

  #[inline]
  pub fn clear_all(&mut self) {
    self.words = [0; WORDS_PER_CHUNK];
  }

  #[inline]
  pub fn is_empty(&self) -> bool {
    self.words.iter().all(|w| *w == 0)
  }

  pub fn count(&self) -> usize {
    self.words.iter().map(|w| w.count_ones() as usize).sum()
  }
}

#[inline]
fn is_open_material(mat: MaterialId) -> bool {
  mat == MaterialId::Air
}

#[inline]
fn is_support_solid(mat: MaterialId) -> bool {
  mat.is_solid()
}

/// Sparse surface + grounded overlays, keyed by chunk with bitset payloads.
#[derive(Debug, Clone, Default)]
pub struct SupportMap {
  pub surface: HashMap<ChunkCoord, ChunkMask>,
  pub grounded: HashMap<ChunkCoord, ChunkMask>,
  /// Tick of last successful rebuild (`u64::MAX` = never).
  pub last_rebuild_tick: u64,
}

impl SupportMap {
  pub fn new() -> Self {
    Self {
      surface: HashMap::new(),
      grounded: HashMap::new(),
      last_rebuild_tick: u64::MAX,
    }
  }

  pub fn is_surface(&self, gx: i32, gy: i32) -> bool {
    let (coord, lx, ly) = World::split(gx, gy);
    self.surface.get(&coord).is_some_and(|m| m.get(lx, ly))
  }

  pub fn is_grounded(&self, gx: i32, gy: i32) -> bool {
    let (coord, lx, ly) = World::split(gx, gy);
    self.grounded.get(&coord).is_some_and(|m| m.get(lx, ly))
  }

  /// True when the map has any data (rebuilt at least once).
  pub fn is_ready(&self) -> bool {
    self.last_rebuild_tick != u64::MAX && !self.grounded.is_empty()
  }

  pub fn surface_count(&self) -> usize {
    self.surface.values().map(|m| m.count()).sum()
  }

  pub fn grounded_count(&self) -> usize {
    self.grounded.values().map(|m| m.count()).sum()
  }

  /// Full rebuild over all loaded chunks.
  ///
  /// Surface is a local 4-neighbour scan; grounded is one BFS from every
  /// Bedrock cell through solids. Both write bitsets in place, so repeated
  /// rebuilds reuse allocations instead of rehashing millions of tuples.
  pub fn rebuild(&mut self, world: &World) {
    for mask in self.surface.values_mut() {
      mask.clear_all();
    }
    for mask in self.grounded.values_mut() {
      mask.clear_all();
    }
    for &coord in world.chunks.keys() {
      self.surface.entry(coord).or_default();
      self.grounded.entry(coord).or_default();
    }
    self.surface.retain(|c, _| world.chunks.contains_key(c));
    self.grounded.retain(|c, _| world.chunks.contains_key(c));

    let cw = CHUNK_CELLS_W as i32;
    let ch = CHUNK_CELLS_H as i32;

    // --- Surface: solid with an open 4-neighbour.
    for (&coord, chunk) in &world.chunks {
      let base_gx = coord.cx * cw;
      let base_gy = coord.cy * ch;
      let Some(mask) = self.surface.get_mut(&coord) else {
        continue;
      };
      for ly in 0..CHUNK_CELLS_H {
        for lx in 0..CHUNK_CELLS_W {
          if !is_support_solid(chunk.get(lx, ly).material) {
            continue;
          }
          let gx = base_gx + lx as i32;
          let gy = base_gy + ly as i32;
          // In-chunk fast path avoids World::split for interior cells.
          let mut open = false;
          for (dx, dy) in [(-1_i32, 0_i32), (1, 0), (0, -1), (0, 1)] {
            let nlx = lx as i32 + dx;
            let nly = ly as i32 + dy;
            if nlx >= 0 && nlx < cw && nly >= 0 && nly < ch {
              if is_open_material(chunk.get(nlx as usize, nly as usize).material) {
                open = true;
                break;
              }
            } else {
              match world.get_cell(world.wrap_x(gx + dx), gy + dy) {
                None => {
                  open = true;
                  break;
                }
                Some(c) if is_open_material(c.material) => {
                  open = true;
                  break;
                }
                _ => {}
              }
            }
          }
          if open {
            mask.set(lx, ly);
          }
        }
      }
    }

    // --- Grounded: BFS from Bedrock through solids (crosses chunk seams).
    let mut queue: Vec<(i32, i32)> = Vec::new();
    for (&coord, chunk) in &world.chunks {
      let base_gx = coord.cx * cw;
      let base_gy = coord.cy * ch;
      let Some(mask) = self.grounded.get_mut(&coord) else {
        continue;
      };
      for ly in 0..CHUNK_CELLS_H {
        for lx in 0..CHUNK_CELLS_W {
          if chunk.get(lx, ly).material != MaterialId::Bedrock {
            continue;
          }
          mask.set(lx, ly);
          queue.push((base_gx + lx as i32, base_gy + ly as i32));
        }
      }
    }
    while let Some((x, y)) = queue.pop() {
      for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
        let nx = world.wrap_x(x + dx);
        let ny = y + dy;
        let (coord, lx, ly) = World::split(nx, ny);
        let Some(chunk) = world.chunks.get(&coord) else {
          continue;
        };
        if !is_support_solid(chunk.get(lx, ly).material) {
          continue;
        }
        let Some(mask) = self.grounded.get_mut(&coord) else {
          continue;
        };
        if mask.get(lx, ly) {
          continue;
        }
        mask.set(lx, ly);
        queue.push((nx, ny));
      }
    }
    self.last_rebuild_tick = world.tick;
  }
}

/// Max cells to walk when testing column support without a built map.
pub const COLUMN_SUPPORT_MAX_WALK: i32 = 64;

/// Fallback support test: walk down through solids looking for Bedrock.
///
/// Bounded — a column this deep is load-bearing terrain for our purposes, and
/// an unbounded walk made seed scanning O(cells × depth).
pub fn column_supported(world: &World, gx: i32, gy: i32) -> bool {
  let mut y = gy;
  for _ in 0..COLUMN_SUPPORT_MAX_WALK {
    let by = y - 1;
    match world.get_cell(gx, by) {
      None => return false,
      Some(c) if c.material == MaterialId::Air => return false,
      Some(c) if c.material == MaterialId::Bedrock => return true,
      Some(c) if is_support_solid(c.material) => y = by,
      Some(_) => return false,
    }
  }
  // Deep solid column — treat as supported terrain.
  true
}

impl SupportMap {
  /// Support test preferring the built grounded map, falling back to a walk.
  pub fn cell_supported(&self, world: &World, gx: i32, gy: i32) -> bool {
    if self.is_ready() {
      return self.is_grounded(gx, gy);
    }
    column_supported(world, gx, gy)
  }
}

/// Load-bearing test for landscape detach: is there a **vertical** load path
/// down through solids to bedrock?
///
/// Deliberately *not* [`SupportMap::is_grounded`]. A carved arch is still
/// grounded through its legs, but it is hanging and must fall. Lateral welds
/// do not carry a slab in this model. The grounded map is used only as a fast
/// positive: rock that cannot reach bedrock at all is certainly hanging.
#[inline]
fn landscape_supported(world: &World, support: Option<&SupportMap>, x: i32, y: i32) -> bool {
  if let Some(m) = support {
    if m.is_ready() && !m.is_grounded(x, y) {
      // Fully disconnected from bedrock — hanging, skip the walk.
      return false;
    }
  }
  column_supported(world, x, y)
}

/// Flood a hanging competent mass that has no vertical load path to bedrock.
///
/// Stops at column-supported competent rock so hill mass and pillar legs stay.
pub fn hanging_landscape_cluster_with(
  world: &World,
  support: Option<&SupportMap>,
  seed_x: i32,
  seed_y: i32,
  max_cells: usize,
) -> Vec<(i32, i32)> {
  let supported = |x: i32, y: i32| landscape_supported(world, support, x, y);
  let Some(seed) = world.get_cell(seed_x, seed_y) else {
    return Vec::new();
  };
  if !is_competent_rock(seed.material) {
    return Vec::new();
  }
  if supported(seed_x, seed_y) {
    return Vec::new();
  }
  let mut out = Vec::new();
  let mut seen = std::collections::HashSet::new();
  let mut q = std::collections::VecDeque::new();
  q.push_back((seed_x, seed_y));
  seen.insert((seed_x, seed_y));
  while let Some((x, y)) = q.pop_front() {
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
      if supported(nx, ny) {
        continue;
      }
      q.push_back((nx, ny));
    }
  }
  out
}

/// [`hanging_landscape_cluster_with`] without a prebuilt support map.
pub fn hanging_landscape_cluster(
  world: &World,
  seed_x: i32,
  seed_y: i32,
  max_cells: usize,
) -> Vec<(i32, i32)> {
  hanging_landscape_cluster_with(world, None, seed_x, seed_y, max_cells)
}

/// Collect competent seeds that sit over void and carry no vertical load path.
///
/// Ordering matters for cost: the in-chunk "air directly below" test is a few
/// array reads and rejects almost every terrain cell, so it runs before the
/// bounded column walk.
pub fn void_below_competent_seeds_with(
  world: &World,
  support: Option<&SupportMap>,
  coords: &[ChunkCoord],
) -> Vec<(i32, i32)> {
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
        // Air directly below — chunk-local when possible.
        let open_below = if ly > 0 {
          is_open_material(chunk.get(lx, ly - 1).material)
        } else {
          match world.get_cell(base_gx + lx as i32, base_gy - 1) {
            None => true,
            Some(c) => is_open_material(c.material),
          }
        };
        if !open_below {
          continue;
        }
        let gx = world.wrap_x(base_gx + lx as i32);
        let gy = base_gy + ly as i32;
        if landscape_supported(world, support, gx, gy) {
          continue;
        }
        seeds.push((gx, gy));
      }
    }
  }
  seeds
}

/// [`void_below_competent_seeds_with`] without a prebuilt support map.
pub fn void_below_competent_seeds(world: &World, coords: &[ChunkCoord]) -> Vec<(i32, i32)> {
  void_below_competent_seeds_with(world, None, coords)
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
    assert!(map.is_ready());
    assert!(map.is_grounded(5, 5), "pillar rooted in bedrock");
    assert!(!map.is_grounded(15, 32), "sky slab not grounded");
    assert!(map.is_surface(15, 30), "slab underside is surface");
    assert!(column_supported(&w, 5, 10));
    assert!(!column_supported(&w, 15, 30));
  }

  #[test]
  fn grounded_map_matches_column_walk_on_terrain() {
    let mut w = World::new(48);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..48 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 1 + (x % 7);
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mut map = SupportMap::new();
    map.rebuild(&w);
    for x in 0..48 {
      let h = 1 + (x % 7);
      assert!(
        map.is_grounded(x, h),
        "stacked terrain must be grounded at ({x},{h})"
      );
    }
  }

  #[test]
  fn lateral_weld_to_hill_is_still_ungrounded_when_carved_under() {
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
    for x in 6..30 {
      for y in 1..18 {
        w.set_cell(x, y, Cell::air());
      }
    }
    // Legs still reach bedrock, so the span IS grounded through them.
    let mut map = SupportMap::new();
    map.rebuild(&w);
    assert!(map.is_grounded(18, 20), "span is welded to grounded legs");
    // Column walk (used for detach) treats it as hanging.
    assert!(!column_supported(&w, 18, 20));
    let cluster = hanging_landscape_cluster(&w, 18, 18, 4096);
    assert!(
      cluster.len() >= 40,
      "span must detach via column support (got {})",
      cluster.len()
    );
  }

  #[test]
  fn mask_bits_roundtrip() {
    let mut m = ChunkMask::default();
    assert!(!m.get(0, 0));
    m.set(0, 0);
    m.set(63, 63);
    m.set(17, 5);
    assert!(m.get(0, 0) && m.get(63, 63) && m.get(17, 5));
    assert!(!m.get(1, 0));
    assert_eq!(m.count(), 3);
    m.clear_all();
    assert_eq!(m.count(), 0);
  }
}
