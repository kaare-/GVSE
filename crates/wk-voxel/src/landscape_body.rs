//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Landscape falling bodies: detach an **massive** ungrounded competent
//! cluster (≥ [`MIN_LANDSCAPE_BODY_CELLS`]) from the grid into a temporary
//! rigid entity, translate **straight down** under gravity, then rematerialize.
//!
//! Boulder-sized and mid chunks stay in the grid and use competent CA
//! tip / roll — landscape entities must not steal tumble from rocks.

use std::collections::VecDeque;

use crate::fasthash::FxHashSet as HashSet;
use crate::displace::{
  deposit_free_water, deposit_shifted_cells, take_free_water, take_soft_cell, ShiftedCell,
};

use wk_material::MaterialId;

use crate::cell::{is_competent_rock, Cell, CellFlags};
use crate::chunk::ChunkCoord;
use crate::grid::World;
use crate::support_map::{
  hanging_landscape_cluster_with, void_below_competent_seeds_with, SupportMap,
};

/// Only truly massive hang clusters become landscape entities.
/// Everything smaller stays on competent CA (tip / roll / shatter).
pub const MIN_LANDSCAPE_BODY_CELLS: usize = 200;
/// All landscape bodies are gravity-only (alias of min size).
pub const LANDSCAPE_GRAVITY_ONLY_MIN: usize = MIN_LANDSCAPE_BODY_CELLS;
/// Hard cap on cells in one landscape body (FPS / stamp safety).
pub const MAX_LANDSCAPE_BODY_CELLS: usize = 2048;
/// Live landscape entities at once.
pub const MAX_LANDSCAPE_BODIES: usize = 8;
/// Detach attempts per tick.
pub const MAX_LANDSCAPE_DETACH_PER_TICK: usize = 2;
/// Max free-fall cells per body per tick.
pub const LANDSCAPE_FALL_CELLS: i32 = 48;
/// Ticks with zero drop while still airborne before forced impact stamp.
pub const LANDSCAPE_STUCK_STAMP_TICKS: u32 = 6;
/// Max bottom-face cells converted to loose on hard impact.
pub const LANDSCAPE_IMPACT_SHATTER_MAX: u32 = 16;

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
  /// Consecutive ticks with no downward motion while airborne.
  pub stuck_ticks: u32,
}

impl LandscapeBody {
  pub fn rock_cell_count(&self) -> usize {
    self.cells.len()
  }

  pub fn world_cells(&self) -> Vec<(i32, i32, Cell)> {
    self.all_world_cells()
  }

  fn all_world_cells(&self) -> Vec<(i32, i32, Cell)> {
    let mut out = Vec::with_capacity(self.cells.len() + self.cargo.len());
    for &(lx, ly, c) in self.cells.iter().chain(self.cargo.iter()) {
      out.push((self.ox + lx, self.oy + ly, c));
    }
    out
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
  let mut seen = HashSet::default();
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

/// Write body cells back into the grid, displacing (never deleting) any water
/// they land in. Returns water units that need re-homing.
fn stamp_cells_displacing(world: &mut World, cells: &[(i32, i32, Cell)]) -> u32 {
  let mut water = 0u32;
  for &(x, y, _) in cells {
    water += take_free_water(world, x, y);
  }
  for &(x, y, mut c) in cells {
    if is_competent_rock(c.material) {
      c.flags.set(CellFlags::MOBILE_ROCK);
    }
    world.set_cell(x, y, c);
  }
  water
}

fn stamp_cells(world: &mut World, cells: &[(i32, i32, Cell)]) {
  let water = stamp_cells_displacing(world, cells);
  if water > 0 {
    let occupied: HashSet<(i32, i32)> = cells
      .iter()
      .map(|&(x, y, _)| (world.wrap_x(x), y))
      .collect();
    // Push the lake up around the body rather than through it.
    let above: Vec<(i32, i32)> = cells
      .iter()
      .map(|&(x, y, _)| (world.wrap_x(x), y + 1))
      .filter(|p| !occupied.contains(p))
      .collect();
    let _ = deposit_free_water(world, water, &above, &occupied);
  }
}

fn is_crushable_bed(material: MaterialId) -> bool {
  matches!(
    material,
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

fn blocks_fall(_world: &World, gx: i32, gy: i32, mover_cells: usize) -> bool {
  let Some(c) = _world.get_cell(gx, gy) else {
    return true;
  };
  if c.material == MaterialId::Air {
    return false;
  }
  if is_crushable_bed(c.material) {
    return false;
  }
  let _ = (gx, gy, mover_cells);
  true
}

fn can_place_fall(
  world: &World,
  cells: &[(i32, i32, Cell)],
  self_set: &HashSet<(i32, i32)>,
) -> bool {
  let n = cells.len();
  for &(x, y, _) in cells {
    if y < 0 {
      return false;
    }
    let wx = world.wrap_x(x);
    if self_set.contains(&(wx, y)) {
      continue;
    }
    if blocks_fall(world, wx, y, n) {
      return false;
    }
  }
  true
}

fn translate_body(body: &mut LandscapeBody, dx: i32, dy: i32) {
  body.ox += dx;
  body.oy += dy;
}

/// Lift soft beds out of the slab's path so they can be re-homed after the move.
///
/// A landscape slab shoves sand, soil, gravel and loose rock aside; it must not
/// delete them. The cells come back in [`apply_drop`], preferring the volume the
/// slab vacates.
fn take_obstacles(world: &mut World, cells: &[(i32, i32, Cell)]) -> Vec<ShiftedCell> {
  let mut out = Vec::new();
  for &(x, y, _) in cells {
    if let Some(taken) = take_soft_cell(world, x, y, is_crushable_bed) {
      out.push(taken);
    }
  }
  out
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

fn max_drop(world: &World, body: &LandscapeBody, max_dy: i32) -> i32 {
  if max_dy <= 0 {
    return 0;
  }
  let cells = body.all_world_cells();
  let empty = HashSet::default();
  let mut lo = 0;
  let mut hi = max_dy;
  while lo < hi {
    let mid = (lo + hi + 1) / 2;
    let shifted: Vec<_> = cells
      .iter()
      .map(|(x, y, c)| (world.wrap_x(*x), *y - mid, *c))
      .collect();
    if can_place_fall(world, &shifted, &empty) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  lo
}

fn apply_drop(world: &mut World, body: &mut LandscapeBody, drop: i32) {
  if drop <= 0 {
    return;
  }
  let cells = body.all_world_cells();
  let shifted: Vec<_> = cells
    .iter()
    .map(|(x, y, c)| (world.wrap_x(*x), *y - drop, *c))
    .collect();
  // Everything the slab sweeps through is shifted, never consumed: loose beds
  // are lifted out and lake water is displaced upward.
  let moved_beds = take_obstacles(world, &shifted);
  let mut water = 0u32;
  for &(x, y, _) in &shifted {
    water += take_free_water(world, x, y);
  }
  translate_body(body, 0, -drop);
  if water > 0 || !moved_beds.is_empty() {
    let occupied: HashSet<(i32, i32)> = shifted.iter().map(|&(x, y, _)| (x, y)).collect();
    // Prefer the column the slab just left — that is the volume it swapped.
    let vacated: Vec<(i32, i32)> = cells
      .iter()
      .map(|&(x, y, _)| (world.wrap_x(x), y))
      .filter(|p| !occupied.contains(p))
      .collect();
    if !moved_beds.is_empty() {
      let _ = deposit_shifted_cells(world, moved_beds, &vacated, &occupied);
    }
    if water > 0 {
      let _ = deposit_free_water(world, water, &vacated, &occupied);
    }
  }
}

fn footprint_overlaps_store(store: &LandscapeBodyStore, cells: &[(i32, i32)]) -> bool {
  let mut claimed = HashSet::default();
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
  detach_landscape_bodies_with(world, store, None, coords)
}

/// [`detach_landscape_bodies`] using a prebuilt support map when available.
pub fn detach_landscape_bodies_with(
  world: &mut World,
  store: &mut LandscapeBodyStore,
  support: Option<&SupportMap>,
  coords: &[ChunkCoord],
) -> u32 {
  if store.len() >= MAX_LANDSCAPE_BODIES {
    return 0;
  }
  let seeds = void_below_competent_seeds_with(world, support, coords);
  if seeds.is_empty() {
    return 0;
  }
  let mut detached = 0u32;
  let mut used = HashSet::default();
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
    let cluster =
      hanging_landscape_cluster_with(world, support, sx, sy, MAX_LANDSCAPE_BODY_CELLS);
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
      stuck_ticks: 0,
    });
    detached += 1;
  }
  detached
}

/// Step all landscape bodies: gravity fall only, stamp when seated or jammed.
pub fn step_landscape_bodies(
  world: &mut World,
  store: &mut LandscapeBodyStore,
) -> LandscapeFallStats {
  let mut stats = LandscapeFallStats::default();
  let mut stamped = Vec::new();
  for (i, body) in store.bodies.iter_mut().enumerate() {
    let drop = max_drop(world, body, LANDSCAPE_FALL_CELLS);
    if drop > 0 {
      apply_drop(world, body, drop);
      body.fall_streak = body.fall_streak.saturating_add(drop as u32);
      body.stuck_ticks = 0;
      stats.fall_moves += 1;
      continue;
    }
    let airborne = is_fully_airborne(world, body);
    if airborne {
      body.stuck_ticks = body.stuck_ticks.saturating_add(1);
    } else {
      body.stuck_ticks = 0;
    }
    let force_impact = airborne && body.stuck_ticks >= LANDSCAPE_STUCK_STAMP_TICKS;
    // Seated, wedged, or stuck-in-air too long — rematerialize.
    if !airborne || force_impact {
      let cells = body.all_world_cells();
      stamp_cells(world, &cells);
      // Light impact shatter only after a real fall — keep the slab mostly rock.
      if body.fall_streak >= 8 || force_impact {
        let set: HashSet<_> = cells.iter().map(|(x, y, _)| (*x, *y)).collect();
        let mut shattered = 0u32;
        for &(x, y, c) in &cells {
          if shattered >= LANDSCAPE_IMPACT_SHATTER_MAX {
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
///
/// `coords` is the detach scan. **Empty means skip detach** (still step
/// any live bodies). Pass loaded chunk keys for a full hanger sweep.
pub fn apply_landscape_fall(
  world: &mut World,
  store: &mut LandscapeBodyStore,
  support: &SupportMap,
  coords: &[ChunkCoord],
) -> LandscapeFallStats {
  let mut stats = LandscapeFallStats::default();
  stats.detached = detach_landscape_bodies_with(world, store, Some(support), coords);
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
  use crate::rules::{apply_competent_fall_regions, CompetentFallConfig};

  fn detach_all(w: &mut World, store: &mut LandscapeBodyStore) -> u32 {
    let coords: Vec<_> = w.chunks.keys().copied().collect();
    detach_landscape_bodies(w, store, &coords)
  }

  #[test]
  fn floating_slab_detaches_and_falls() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // 25×10 = 250 cells — landscape-sized.
    for x in 5..30 {
      for y in 40..50 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mut store = LandscapeBodyStore::new();
    let n = detach_all(&mut w, &mut store);
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
      .flat_map(|x| (0..55).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .max()
      .unwrap_or(0);
    assert!(max_y < 20, "stamped slab must be near bed (max_y={max_y})");
  }

  #[test]
  fn carved_arch_detaches_span_keeps_legs() {
    let mut w = World::new(48);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..48 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=28 {
      w.set_cell(5, y, Cell::solid(MaterialId::Stone));
      w.set_cell(42, y, Cell::solid(MaterialId::Stone));
    }
    // Thick span ≥200 cells (38×6 = 228).
    for x in 5..=42 {
      for y in 22..=27 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    for x in 6..42 {
      for y in 1..22 {
        w.set_cell(x, y, Cell::air());
      }
    }
    let mut store = LandscapeBodyStore::new();
    let n = detach_all(&mut w, &mut store);
    assert_eq!(n, 1, "arch span must detach");
    assert!(
      w.get_cell(5, 10).map(|c| c.material) == Some(MaterialId::Stone),
      "legs stay in grid"
    );
    assert!(
      w.get_cell(24, 24).map(|c| c.material) == Some(MaterialId::Air),
      "span leaves the grid"
    );
    for _ in 0..20 {
      step_landscape_bodies(&mut w, &mut store);
    }
    let mid_air = w.get_cell(24, 24).map(|c| c.material) == Some(MaterialId::Stone);
    assert!(!mid_air, "fallen span must leave the arch perch");
  }

  #[test]
  fn boulder_sized_chunk_stays_for_competent_tumble() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Sand ramp for roll.
    for x in 0..32 {
      let h = 2 + x / 2;
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    // Round-ish boulder ~r=5 → ~80 cells — under landscape min.
    let cx = 20i32;
    let cy = 28i32;
    for x in cx - 5..=cx + 5 {
      for y in cy - 5..=cy + 5 {
        if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= 25 {
          w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
      }
    }
    let mut store = LandscapeBodyStore::new();
    assert_eq!(
      detach_all(&mut w, &mut store),
      0,
      "boulder must not become a landscape entity"
    );
    assert!(
      w.get_cell(cx, cy).map(|c| c.material) == Some(MaterialId::Stone),
      "boulder stays in the grid for CA tumble"
    );
    let cfg = CompetentFallConfig {
      max_roll_events: 24,
      min_impact_fall_cells: 99,
      ..CompetentFallConfig::default()
    };
    let x0 = (cx - 5..=cx + 5)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(x, _)| x)
      .min()
      .unwrap_or(cx);
    for _ in 0..24 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let x1 = (0..40)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(x, _)| x)
      .min()
      .unwrap_or(99);
    let stone_n = (0..40)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .count();
    assert!(
      stone_n >= 40,
      "boulder must stay mostly rock, not disintegrate (n={stone_n})"
    );
    assert!(
      x1 < x0,
      "boulder should roll downhill (start_min_x={x0}, end_min_x={x1})"
    );
  }

  #[test]
  fn mid_chunk_under_200_does_not_detach() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // 12×12 = 144 < 200.
    for x in 10..22 {
      for y in 40..52 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mut store = LandscapeBodyStore::new();
    assert_eq!(detach_all(&mut w, &mut store), 0);
  }

  #[test]
  fn landscape_slab_shifts_loose_beds_instead_of_eating_them() {
    let mut w = World::new(40);
    for cy in 0..2 {
      w.ensure_chunk(ChunkCoord::new(0, cy));
    }
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Thick mixed loose bed the slab ploughs straight through.
    for x in 0..40 {
      for y in 1..14 {
        let mat = match y % 4 {
          0 => MaterialId::Sand,
          1 => MaterialId::Soil,
          2 => MaterialId::Gravel,
          _ => MaterialId::Clay,
        };
        w.set_cell(x, y, Cell::solid(mat));
      }
    }
    // Landscape-sized slab (25x10 = 250 cells) above the bed.
    for x in 5..30 {
      for y in 45..55 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let mats = [
      MaterialId::Sand,
      MaterialId::Soil,
      MaterialId::Gravel,
      MaterialId::Clay,
    ];
    let count = |w: &World| -> Vec<usize> {
      mats
        .iter()
        .map(|&m| {
          (0..40)
            .flat_map(|x| (0..70).map(move |y| (x, y)))
            .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(m))
            .count()
        })
        .collect()
    };
    let before = count(&w);
    let mut store = LandscapeBodyStore::new();
    assert_eq!(detach_all(&mut w, &mut store), 1);
    for _ in 0..24 {
      step_landscape_bodies(&mut w, &mut store);
    }
    let after = count(&w);
    assert_eq!(
      after, before,
      "slab must shove loose beds aside, not consume them \
       (before={before:?}, after={after:?})"
    );
  }

  #[test]
  fn landscape_slab_dropped_in_lake_displaces_water() {
    let mut w = World::new(40);
    for cx in 0..1 {
      for cy in 0..2 {
        w.ensure_chunk(ChunkCoord::new(cx, cy));
      }
    }
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Deep lake.
    for x in 0..40 {
      for y in 1..20 {
        w.set_cell(
          x,
          y,
          Cell {
            material: MaterialId::Air,
            sat: crate::cell::Sat(255),
            ..Cell::default()
          },
        );
      }
    }
    // Landscape-sized slab (25x10 = 250 cells) above the lake.
    for x in 5..30 {
      for y in 45..55 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let water = |w: &World| -> u32 {
      let mut n = 0;
      for x in 0..40 {
        for y in 0..70 {
          if let Some(c) = w.get_cell(x, y) {
            n += c.sat.0 as u32;
          }
        }
      }
      n
    };
    let before = water(&w);
    let mut store = LandscapeBodyStore::new();
    assert_eq!(detach_all(&mut w, &mut store), 1);
    for _ in 0..24 {
      step_landscape_bodies(&mut w, &mut store);
    }
    let submerged = (5..30)
      .flat_map(|x| (1..20).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .count();
    assert!(
      submerged > 0,
      "slab must end up in the lake for this test to mean anything"
    );
    assert_eq!(
      water(&w),
      before,
      "landscape slab must displace lake water, not consume it (submerged={submerged})"
    );
  }

  #[test]
  fn sand_cap_rides_massive_landscape_body() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for x in 5..30 {
      for y in 40..50 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
      for y in 50..53 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    let mut store = LandscapeBodyStore::new();
    assert_eq!(detach_all(&mut w, &mut store), 1);
    assert!(!store.bodies[0].cargo.is_empty(), "sand must ride");
    for _ in 0..16 {
      step_landscape_bodies(&mut w, &mut store);
    }
    let sand_min = (5..30)
      .flat_map(|x| (0..55).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
      .map(|(_, y)| y)
      .min()
      .unwrap_or(99);
    let stone_max = (5..30)
      .flat_map(|x| (0..55).map(move |y| (x, y)))
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
