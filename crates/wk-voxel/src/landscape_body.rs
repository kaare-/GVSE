//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Landscape falling bodies: detach an ungrounded competent cluster from
//! the grid into a temporary rigid entity, translate / tip / slide it as
//! one piece, then rematerialize into cells on seat or impact.

use std::collections::{HashSet, VecDeque};

use wk_material::MaterialId;

use crate::cell::{is_competent_rock, Cell, CellFlags};
use crate::chunk::ChunkCoord;
use crate::grid::World;
use crate::support_map::{
  hanging_landscape_cluster, void_below_competent_seeds, SupportMap,
};

/// Smallest hang cluster promoted to a landscape entity (else CA path).
pub const MIN_LANDSCAPE_BODY_CELLS: usize = 24;
/// Hard cap on cells in one landscape body (FPS / stamp safety).
pub const MAX_LANDSCAPE_BODY_CELLS: usize = 2048;
/// Live landscape entities at once.
pub const MAX_LANDSCAPE_BODIES: usize = 8;
/// Detach attempts per tick.
pub const MAX_LANDSCAPE_DETACH_PER_TICK: usize = 2;
/// Max free-fall cells per body per tick.
pub const LANDSCAPE_FALL_CELLS: i32 = 48;
/// Tip when COM overhangs support by this many cells.
pub const LANDSCAPE_TIP_OVERHANG: i32 = 2;

#[derive(Debug, Clone)]
pub struct LandscapeBody {
  pub id: u64,
  /// Cell offsets relative to `ox`,`oy` plus material snapshot.
  pub cells: Vec<(i32, i32, Cell)>,
  /// Soft / organic cargo riding on top (same relative frame).
  pub cargo: Vec<(i32, i32, Cell)>,
  /// World origin of the local frame.
  pub ox: i32,
  pub oy: i32,
  /// Accumulated free-fall cells since last seat.
  pub fall_streak: u32,
  /// 0 = upright; ±1 = tipped 90° (swap / flip local axes).
  pub tip_quarter: i8,
}

impl LandscapeBody {
  pub fn world_cells(&self) -> Vec<(i32, i32, Cell)> {
    self.all_world_cells()
  }

  fn all_world_cells(&self) -> Vec<(i32, i32, Cell)> {
    let mut out = Vec::with_capacity(self.cells.len() + self.cargo.len());
    for &(lx, ly, c) in self.cells.iter().chain(self.cargo.iter()) {
      let (wx, wy) = self.local_to_world(lx, ly);
      out.push((wx, wy, c));
    }
    out
  }

  fn local_to_world(&self, lx: i32, ly: i32) -> (i32, i32) {
    match self.tip_quarter.rem_euclid(4) {
      0 => (self.ox + lx, self.oy + ly),
      1 => (self.ox + ly, self.oy - lx),
      2 => (self.ox - lx, self.oy - ly),
      _ => (self.ox - ly, self.oy + lx),
    }
  }

  fn occupied_set(&self) -> HashSet<(i32, i32)> {
    self.all_world_cells()
      .into_iter()
      .map(|(x, y, _)| (x, y))
      .collect()
  }
}

#[derive(Debug, Clone, Default)]
pub struct LandscapeBodyStore {
  pub bodies: Vec<LandscapeBody>,
  next_id: u64,
}

impl LandscapeBodyStore {
  pub fn new() -> Self {
    Self::default()
  }

  pub fn is_empty(&self) -> bool {
    self.bodies.is_empty()
  }

  pub fn len(&self) -> usize {
    self.bodies.len()
  }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LandscapeFallStats {
  pub detached: u32,
  pub fall_moves: u32,
  pub tips: u32,
  pub stamped: u32,
}

fn is_cargo_material(mat: MaterialId) -> bool {
  matches!(
    mat,
    MaterialId::Sand
      | MaterialId::Soil
      | MaterialId::Clay
      | MaterialId::Gravel
      | MaterialId::Organic
      | MaterialId::Snow
      | MaterialId::LooseRock
      | MaterialId::LooseLimestone
  )
}

fn gather_cargo(world: &World, rock: &HashSet<(i32, i32)>) -> Vec<(i32, i32, Cell)> {
  let mut cargo = Vec::new();
  let mut seen = HashSet::new();
  let mut q = VecDeque::new();
  for &(x, y) in rock {
    let above = (world.wrap_x(x), y + 1);
    if rock.contains(&above) || !seen.insert(above) {
      continue;
    }
    let Some(c) = world.get_cell(above.0, above.1) else {
      continue;
    };
    if !is_cargo_material(c.material) {
      continue;
    }
    q.push_back(above);
    cargo.push((above.0, above.1, c));
  }
  while let Some((x, y)) = q.pop_front() {
    let above = (world.wrap_x(x), y + 1);
    if rock.contains(&above) || !seen.insert(above) {
      continue;
    }
    let Some(c) = world.get_cell(above.0, above.1) else {
      continue;
    };
    if !is_cargo_material(c.material) {
      continue;
    }
    q.push_back(above);
    cargo.push((above.0, above.1, c));
  }
  cargo
}

fn clear_cells(world: &mut World, cells: &[(i32, i32, Cell)]) {
  for &(x, y, _) in cells {
    world.set_cell(x, y, Cell::air());
  }
}

fn stamp_cells(world: &mut World, cells: &[(i32, i32, Cell)]) {
  for &(x, y, mut c) in cells {
    if is_competent_rock(c.material) {
      c.flags.set(CellFlags::MOBILE_ROCK);
    }
    world.set_cell(x, y, c);
  }
}

fn can_place(world: &World, cells: &[(i32, i32, Cell)], self_set: &HashSet<(i32, i32)>) -> bool {
  for &(x, y, _) in cells {
    if y < 0 {
      return false;
    }
    let wx = world.wrap_x(x);
    if self_set.contains(&(wx, y)) {
      continue;
    }
    match world.get_cell(wx, y) {
      None => return false,
      Some(c) if c.material == MaterialId::Air => {}
      Some(_) => return false,
    }
  }
  true
}

fn translate_body(body: &mut LandscapeBody, dx: i32, dy: i32) {
  body.ox += dx;
  body.oy += dy;
}

fn body_com(body: &LandscapeBody) -> (i32, i32) {
  let cells = body.all_world_cells();
  if cells.is_empty() {
    return (body.ox, body.oy);
  }
  let n = cells.len() as i64;
  let sx: i64 = cells.iter().map(|(x, _, _)| *x as i64).sum();
  let sy: i64 = cells.iter().map(|(_, y, _)| *y as i64).sum();
  ((sx / n) as i32, (sy / n) as i32)
}

fn bottom_face(body: &LandscapeBody) -> Vec<(i32, i32)> {
  let set = body.occupied_set();
  set.iter()
    .copied()
    .filter(|&(x, y)| !set.contains(&(x, y - 1)))
    .collect()
}

fn is_fully_airborne(world: &World, body: &LandscapeBody) -> bool {
  let face = bottom_face(body);
  !face.is_empty()
    && face.iter().all(|&(x, y)| match world.get_cell(world.wrap_x(x), y - 1) {
      None => true,
      Some(c) => c.material == MaterialId::Air,
    })
}

fn support_drop(world: &World, body: &LandscapeBody) -> Option<(i32, i32)> {
  // Returns (pivot_x, drop) of deepest unsupported overhang sense.
  let face = bottom_face(body);
  if face.is_empty() {
    return None;
  }
  let (cx, _) = body_com(body);
  let mut left_support = i32::MAX;
  let mut right_support = i32::MIN;
  let mut any = false;
  for &(x, y) in &face {
    match world.get_cell(world.wrap_x(x), y - 1) {
      Some(c) if c.material != MaterialId::Air => {
        any = true;
        left_support = left_support.min(x);
        right_support = right_support.max(x);
      }
      _ => {}
    }
  }
  if !any {
    return None;
  }
  if cx > right_support + LANDSCAPE_TIP_OVERHANG {
    Some((right_support, 1))
  } else if cx < left_support - LANDSCAPE_TIP_OVERHANG {
    Some((left_support, -1))
  } else {
    None
  }
}

fn try_tip(world: &World, body: &mut LandscapeBody, dir: i8) -> bool {
  let old = body.tip_quarter;
  body.tip_quarter = body.tip_quarter.wrapping_add(dir);
  let cells = body.all_world_cells();
  // Collision check against world (body cells are already absent from grid).
  let empty = HashSet::new();
  if can_place(world, &cells, &empty) {
    true
  } else {
    body.tip_quarter = old;
    false
  }
}

fn max_drop(world: &World, body: &LandscapeBody, max_dy: i32) -> i32 {
  if max_dy <= 0 {
    return 0;
  }
  let cells = body.all_world_cells();
  let empty = HashSet::new();
  let mut lo = 0;
  let mut hi = max_dy;
  while lo < hi {
    let mid = (lo + hi + 1) / 2;
    let shifted: Vec<_> = cells
      .iter()
      .map(|(x, y, c)| (*x, *y - mid, *c))
      .collect();
    if can_place(world, &shifted, &empty) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  lo
}

fn footprint_overlaps_store(store: &LandscapeBodyStore, cells: &[(i32, i32)]) -> bool {
  let mut claimed = HashSet::new();
  for b in &store.bodies {
    for p in b.occupied_set() {
      claimed.insert(p);
    }
  }
  cells.iter().any(|p| claimed.contains(p))
}

/// Detach hanging landscape clusters into entities (cells leave the grid).
pub fn detach_landscape_bodies(
  world: &mut World,
  store: &mut LandscapeBodyStore,
  coords: &[ChunkCoord],
) -> u32 {
  if store.len() >= MAX_LANDSCAPE_BODIES {
    return 0;
  }
  let seeds = void_below_competent_seeds(world, coords);
  let mut detached = 0u32;
  let mut used = HashSet::new();
  for (sx, sy) in seeds {
    if detached as usize >= MAX_LANDSCAPE_DETACH_PER_TICK {
      break;
    }
    if store.len() >= MAX_LANDSCAPE_BODIES {
      break;
    }
    if used.contains(&(sx, sy)) {
      continue;
    }
    let cluster = hanging_landscape_cluster(world, sx, sy, MAX_LANDSCAPE_BODY_CELLS);
    if cluster.len() < MIN_LANDSCAPE_BODY_CELLS {
      continue;
    }
    if footprint_overlaps_store(store, &cluster) {
      continue;
    }
    for &p in &cluster {
      used.insert(p);
    }
    let rock_set: HashSet<_> = cluster.iter().copied().collect();
    let mut rock_cells = Vec::with_capacity(cluster.len());
    for &(x, y) in &cluster {
      if let Some(c) = world.get_cell(x, y) {
        rock_cells.push((x, y, c));
      }
    }
    let cargo_world = gather_cargo(world, &rock_set);
    // Origin = min corner for stable local frame.
    let min_x = rock_cells.iter().map(|(x, _, _)| *x).min().unwrap_or(0);
    let min_y = rock_cells.iter().map(|(_, y, _)| *y).min().unwrap_or(0);
    let local_rock: Vec<_> = rock_cells
      .iter()
      .map(|(x, y, c)| (*x - min_x, *y - min_y, *c))
      .collect();
    let local_cargo: Vec<_> = cargo_world
      .iter()
      .map(|(x, y, c)| (*x - min_x, *y - min_y, *c))
      .collect();
    clear_cells(world, &rock_cells);
    clear_cells(world, &cargo_world);
    store.next_id = store.next_id.wrapping_add(1);
    store.bodies.push(LandscapeBody {
      id: store.next_id,
      cells: local_rock,
      cargo: local_cargo,
      ox: min_x,
      oy: min_y,
      fall_streak: 0,
      tip_quarter: 0,
    });
    detached += 1;
  }
  detached
}

/// Step all landscape bodies: fall, tip, stamp when seated.
pub fn step_landscape_bodies(
  world: &mut World,
  store: &mut LandscapeBodyStore,
) -> LandscapeFallStats {
  let mut stats = LandscapeFallStats::default();
  let mut stamped = Vec::new();
  for (i, body) in store.bodies.iter_mut().enumerate() {
    // Free fall.
    let drop = max_drop(world, body, LANDSCAPE_FALL_CELLS);
    if drop > 0 {
      translate_body(body, 0, -drop);
      body.fall_streak = body.fall_streak.saturating_add(drop as u32);
      stats.fall_moves += 1;
      continue;
    }
    // Tip when COM overhangs.
    if let Some((_, dir)) = support_drop(world, body) {
      if try_tip(world, body, dir as i8) {
        stats.tips += 1;
        // Small settle after tip.
        let d2 = max_drop(world, body, 4);
        if d2 > 0 {
          translate_body(body, 0, -d2);
          body.fall_streak = body.fall_streak.saturating_add(d2 as u32);
          stats.fall_moves += 1;
        }
        continue;
      }
    }
    // Seated (or jammed) — rematerialize.
    if !is_fully_airborne(world, body) {
      let cells = body.all_world_cells();
      stamp_cells(world, &cells);
      // Impact shatter: convert a few bottom-face competent cells after a fall.
      if body.fall_streak >= 4 {
        let set: HashSet<_> = cells.iter().map(|(x, y, _)| (*x, *y)).collect();
        let mut shattered = 0u32;
        for &(x, y, c) in &cells {
          if shattered >= 24 {
            break;
          }
          if !is_competent_rock(c.material) {
            continue;
          }
          if set.contains(&(x, y - 1)) {
            continue;
          }
          match world.get_cell(world.wrap_x(x), y - 1) {
            Some(b) if b.material != MaterialId::Air && b.material.is_solid() => {
              let loose = if c.material == MaterialId::Limestone {
                MaterialId::LooseLimestone
              } else {
                MaterialId::LooseRock
              };
              world.set_cell(x, y, Cell::solid(loose));
              shattered += 1;
            }
            _ => {}
          }
        }
      }
      stamped.push(i);
      stats.stamped += 1;
    }
  }
  for i in stamped.into_iter().rev() {
    store.bodies.swap_remove(i);
  }
  stats
}

/// Detach + step in one call (tick integration).
pub fn apply_landscape_fall(
  world: &mut World,
  store: &mut LandscapeBodyStore,
  _support: &SupportMap,
  coords: &[ChunkCoord],
) -> LandscapeFallStats {
  let mut stats = LandscapeFallStats::default();
  stats.detached = detach_landscape_bodies(world, store, coords);
  let step = step_landscape_bodies(world, store);
  stats.fall_moves = step.fall_moves;
  stats.tips = step.tips;
  stats.stamped = step.stamped;
  stats
}

/// Stamp any live bodies back into the grid (save / teardown).
pub fn force_stamp_all(world: &mut World, store: &mut LandscapeBodyStore) {
  for body in store.bodies.drain(..) {
    stamp_cells(world, &body.all_world_cells());
  }
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::cell::Cell;
  use crate::chunk::ChunkCoord;

  #[test]
  fn floating_slab_detaches_and_falls() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 8..28 {
      for y in 40..48 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mut store = LandscapeBodyStore::new();
    let n = detach_landscape_bodies(&mut w, &mut store, &[]);
    assert_eq!(n, 1, "slab must detach as one body");
    assert_eq!(store.len(), 1);
    assert!(
      w.get_cell(18, 44).map(|c| c.material) == Some(MaterialId::Air),
      "detached cells leave the grid"
    );
    for _ in 0..16 {
      step_landscape_bodies(&mut w, &mut store);
    }
    assert!(store.is_empty(), "body must stamp after seating");
    let max_y = (0..40)
      .flat_map(|x| (0..50).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .max()
      .unwrap_or(0);
    assert!(max_y < 20, "stamped slab must be near bed (max_y={max_y})");
  }

  #[test]
  fn carved_arch_detaches_span_keeps_legs() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=22 {
      w.set_cell(5, y, Cell::solid(MaterialId::Stone));
      w.set_cell(34, y, Cell::solid(MaterialId::Stone));
    }
    for x in 5..=34 {
      for y in 18..=22 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    for x in 6..34 {
      for y in 1..18 {
        w.set_cell(x, y, Cell::air());
      }
    }
    let mut store = LandscapeBodyStore::new();
    let n = detach_landscape_bodies(&mut w, &mut store, &[]);
    assert_eq!(n, 1, "arch span must detach");
    assert!(
      w.get_cell(5, 10).map(|c| c.material) == Some(MaterialId::Stone),
      "legs stay in grid"
    );
    assert!(
      w.get_cell(20, 20).map(|c| c.material) == Some(MaterialId::Air),
      "span leaves the grid"
    );
    for _ in 0..20 {
      step_landscape_bodies(&mut w, &mut store);
    }
    assert!(
      w.get_cell(20, 20).map(|c| c.material) != Some(MaterialId::Stone)
        || store.is_empty(),
      "span must not remain mid-air in the grid"
    );
    let mid_air = w.get_cell(20, 20).map(|c| c.material) == Some(MaterialId::Stone);
    assert!(!mid_air, "fallen span must leave the arch perch");
  }

  #[test]
  fn sand_cap_rides_landscape_body() {
    let mut w = World::new(24);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..24 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 6..18 {
      for y in 40..46 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
      for y in 46..49 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    let mut store = LandscapeBodyStore::new();
    assert_eq!(detach_landscape_bodies(&mut w, &mut store, &[]), 1);
    assert!(!store.bodies[0].cargo.is_empty(), "sand must ride");
    for _ in 0..16 {
      step_landscape_bodies(&mut w, &mut store);
    }
    let sand_min = (6..18)
      .flat_map(|x| (0..50).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
      .map(|(_, y)| y)
      .min()
      .unwrap_or(99);
    let stone_max = (6..18)
      .flat_map(|x| (0..50).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .max()
      .unwrap_or(0);
    assert!(
      sand_min <= stone_max + 4,
      "sand stays on rock (sand_min={sand_min}, stone_max={stone_max})"
    );
  }
}
