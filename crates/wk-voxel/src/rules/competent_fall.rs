//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Competent rock (Stone / Limestone) falls as connected rigid bodies:
//! 1. Vertical drop through Air (including lake water — rocks sink).
//! 2. Impact shatter — bottom face converts to fallable debris.
//! 3. Slope roll + soft-bed embed on sand / soil / clay.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry};

use crate::active::ActiveChunk;
use crate::cell::{is_competent_rock, Cell, Sat};
use crate::chunk::{ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::failure::roof_collapse_debris;
use crate::grid::World;

/// Max vertical drop (cells) per body per tick on the full-feel path.
pub const COMPETENT_FALL_PASSES: u32 = 96;
/// FPS path: one rigid drop of this many cells (sky → ground in ~1–2 frames).
pub const COMPETENT_FALL_PASSES_FPS: u32 = 64;
/// Rebuild / impact / roll loops after the free-fall jump.
pub const COMPETENT_TOPOLOGY_PASSES: u32 = 8;

/// Live-tunable competent-body knobs (Tab → Geotech).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompetentFallConfig {
  pub enable: bool,
  /// Max vertical cells a body may fall in one tick.
  pub max_passes: u32,
  /// Bottom-face cells convert after at least this many fall cells.
  pub min_impact_fall_cells: u32,
  /// Heavy impacts peel an extra face row.
  pub heavy_impact_fall_cells: u32,
  /// Max bottom-face cells shattered per impact.
  pub max_impact_cells: u32,
  /// Slope roll when downhill drop ≥ this many cells.
  pub roll_drop_cells: i32,
  /// Max whole-body diagonal rolls per tick.
  pub max_roll_events: u32,
}

impl Default for CompetentFallConfig {
  fn default() -> Self {
    Self {
      enable: true,
      max_passes: COMPETENT_FALL_PASSES_FPS,
      min_impact_fall_cells: 1,
      heavy_impact_fall_cells: 12,
      max_impact_cells: 48,
      roll_drop_cells: 1,
      max_roll_events: 12,
    }
  }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CompetentFallStats {
  pub fall_moves: u32,
  pub impacts: u32,
  pub roll_moves: u32,
  pub embed_cells: u32,
}

/// Dry Air (including wet lake films) — rocks sink through water.
#[inline]
fn body_passable_at(_world: &World, _gx: i32, _gy: i32, cell: &Cell) -> bool {
  cell.material == MaterialId::Air
}

#[inline]
fn passable_for_body_material(material: MaterialId) -> bool {
  material == MaterialId::Air
}

#[inline]
fn is_soft_embed_bed(material: MaterialId) -> bool {
  matches!(
    material,
    MaterialId::Sand | MaterialId::Soil | MaterialId::Clay | MaterialId::Gravel
  )
}

/// Soft beds can be crushed aside during a tumble (not hard rock / bedrock).
#[inline]
fn is_roll_displaceable(material: MaterialId) -> bool {
  is_soft_embed_bed(material)
}

#[inline]
fn roll_destination_ok(world: &World, gx: i32, gy: i32, cell: &Cell, body: &HashSet<(i32, i32)>) -> bool {
  if body.contains(&(gx, gy)) {
    return true;
  }
  body_passable_at(world, gx, gy, cell) || is_roll_displaceable(cell.material)
}

struct Component {
  cells: Vec<(i32, i32, Cell)>,
  set: HashSet<(i32, i32)>,
  min_y: i32,
  max_y: i32,
}

fn build_components(world: &World, active: &[ActiveChunk]) -> Vec<Component> {
  let mut index: HashMap<(i32, i32), usize> = HashMap::new();
  let mut cells: Vec<(i32, i32, Cell)> = Vec::new();
  for ac in active {
    let Some(chunk) = world.chunks.get(&ac.coord) else {
      continue;
    };
    let base_gx = ac.coord.cx * CHUNK_CELLS_W as i32;
    let base_gy = ac.coord.cy * CHUNK_CELLS_H as i32;
    for y in ac.rect.y0..=ac.rect.y1 {
      for x in ac.rect.x0..=ac.rect.x1 {
        let cell = chunk.get(x as usize, y as usize);
        if !is_competent_rock(cell.material) {
          continue;
        }
        let gx = world.wrap_x(base_gx + x as i32);
        let gy = base_gy + y as i32;
        let key = (gx, gy);
        if index.contains_key(&key) {
          continue;
        }
        let idx = cells.len();
        index.insert(key, idx);
        cells.push((gx, gy, cell));
      }
    }
  }
  if cells.is_empty() {
    return Vec::new();
  }

  let mut parent: Vec<usize> = (0..cells.len()).collect();
  fn find(parent: &mut [usize], i: usize) -> usize {
    let mut v = i;
    while parent[v] != v {
      let p = parent[v];
      parent[v] = parent[p];
      v = p;
    }
    v
  }
  fn union(parent: &mut [usize], a: usize, b: usize) {
    let ra = find(parent, a);
    let rb = find(parent, b);
    if ra != rb {
      parent[ra] = rb;
    }
  }

  for i in 0..cells.len() {
    let (gx, gy, _) = cells[i];
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1), (1, 1), (1, -1), (-1, 1), (-1, -1)] {
      let nx = world.wrap_x(gx + dx);
      let ny = gy + dy;
      if let Some(&j) = index.get(&(nx, ny)) {
        union(&mut parent, i, j);
      }
    }
  }

  let mut buckets: HashMap<usize, Vec<(i32, i32, Cell)>> = HashMap::new();
  for i in 0..cells.len() {
    let r = find(&mut parent, i);
    buckets
      .entry(r)
      .or_default()
      .push(cells[i]);
  }

  let mut out: Vec<Component> = buckets
    .into_values()
    .map(|cells| {
      let set: HashSet<_> = cells.iter().map(|(x, y, _)| (*x, *y)).collect();
      let min_y = cells.iter().map(|(_, y, _)| *y).min().unwrap_or(0);
      let max_y = cells.iter().map(|(_, y, _)| *y).max().unwrap_or(0);
      Component {
        cells,
        set,
        min_y,
        max_y,
      }
    })
    .collect();
  out.sort_by_key(|c| c.min_y);
  out
}

fn bottom_face(comp: &Component) -> Vec<(i32, i32, Cell)> {
  comp.cells
    .iter()
    .filter(|(x, y, _)| !comp.set.contains(&(*x, y - 1)))
    .copied()
    .collect()
}

fn can_translate(world: &World, comp: &Component, dx: i32, dy: i32) -> bool {
  for (gx, gy, _) in &comp.cells {
    let tx = world.wrap_x(gx + dx);
    let ty = gy + dy;
    if ty < 0 {
      return false;
    }
    if comp.set.contains(&(tx, ty)) {
      continue;
    }
    let Some(dst) = world.get_cell(tx, ty) else {
      return false;
    };
    if !body_passable_at(world, tx, ty, &dst) {
      return false;
    }
  }
  true
}

/// Largest `drop` in `1..=max_drop` where the body can jump down that far.
fn max_drop_distance(world: &World, comp: &Component, max_drop: i32) -> i32 {
  if max_drop <= 0 || !can_translate(world, comp, 0, -1) {
    return 0;
  }
  let mut lo = 1;
  let mut hi = max_drop;
  while lo < hi {
    let mid = (lo + hi + 1) / 2;
    if can_translate(world, comp, 0, -mid) {
      lo = mid;
    } else {
      hi = mid - 1;
    }
  }
  lo
}

fn translate_component(world: &mut World, comp: &Component, dx: i32, dy: i32) -> bool {
  if dx == 0 && dy == 0 {
    return false;
  }
  if !can_translate(world, comp, dx, dy) {
    return false;
  }
  let moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(x, y, c)| (world.wrap_x(x + dx), y + dy, *c))
    .collect();
  // Snapshot destinations, then clear sources, then write — never read a cell
  // we just cleared while the body is mid-move (fixes top-row separation).
  for (x, y, _) in &comp.cells {
    world.set_cell(*x, *y, Cell::air());
  }
  for (tx, ty, cell) in moves {
    world.set_cell(tx, ty, cell);
    world.touch_dirty(tx, ty);
  }
  true
}

fn is_floating(world: &World, comp: &Component) -> bool {
  let face = bottom_face(comp);
  !face.is_empty()
    && face.iter().all(|(x, y, _)| match world.get_cell(*x, y - 1) {
      None => true,
      Some(c) => body_passable_at(world, *x, y - 1, &c),
    })
}

/// Every bottom-face cell rests on solid support (body landed as a unit).
fn is_fully_supported(world: &World, comp: &Component) -> bool {
  let face = bottom_face(comp);
  !face.is_empty()
    && face.iter().all(|(x, y, _)| {
      matches!(
        support_below(world, *x, *y),
        Some((bed, _)) if bed != MaterialId::Air
      )
    })
}

fn support_below(world: &World, gx: i32, gy: i32) -> Option<(MaterialId, i32)> {
  let by = gy - 1;
  let Some(below) = world.get_cell(gx, by) else {
    return None;
  };
  if below.material == MaterialId::Air {
    return None;
  }
  Some((below.material, by))
}

fn comp_anchor(comp: &Component) -> (i32, i32) {
  let mut v: Vec<(i32, i32)> = comp.cells.iter().map(|(x, y, _)| (*x, *y)).collect();
  v.sort_unstable();
  v[0]
}

fn impact_shatter(
  world: &mut World,
  comp: &Component,
  fall_cells: u32,
  cfg: &CompetentFallConfig,
) -> u32 {
  if fall_cells < cfg.min_impact_fall_cells {
    return 0;
  }
  let layers = if fall_cells >= cfg.heavy_impact_fall_cells {
    2
  } else {
    1
  };
  let mut face = bottom_face(comp);
  face.sort_by_key(|(_, y, _)| *y);
  let mut applied = 0u32;
  let mut hit_cols: HashSet<i32> = HashSet::new();
  for (gx, gy, cell) in face {
    if applied >= cfg.max_impact_cells {
      break;
    }
    if !hit_cols.insert(gx) {
      continue;
    }
    let Some((bed_mat, _)) = support_below(world, gx, gy) else {
      continue;
    };
    if passable_for_body_material(bed_mat) {
      continue;
    }
    let debris = roof_collapse_debris(cell.material);
    let mut yy = gy;
    for _ in 0..layers {
      let Some(cur) = world.get_cell(gx, yy) else {
        break;
      };
      if !is_competent_rock(cur.material) {
        break;
      }
      if let Some(below) = world.get_cell(gx, yy - 1) {
        if below.material == MaterialId::Air {
          world.set_cell(
            gx,
            yy - 1,
            Cell {
              material: debris,
              sat: Sat(cur.sat.0 / 2),
              ..Cell::default()
            },
          );
          world.set_cell(gx, yy, Cell::air());
          applied += 1;
          break;
        }
      }
      world.set_cell(
        gx,
        yy,
        Cell {
          material: debris,
          sat: cur.sat,
          ..cur
        },
      );
      applied += 1;
      if yy <= comp.min_y {
        break;
      }
      yy -= 1;
    }
  }
  applied
}

fn soft_embed(world: &mut World, comp: &Component, cfg: &CompetentFallConfig) -> u32 {
  // Prefer rolling on any slope — only glue when the bed is flat.
  if downhill_roll_dir(world, comp, cfg).is_some() {
    return 0;
  }
  if !is_fully_supported(world, comp) || !rests_on_soft_bed(world, comp) {
    return 0;
  }
  let mut applied = 0u32;
  let mut cols: HashSet<i32> = HashSet::new();
  for (gx, gy, cell) in bottom_face(comp) {
    if !cols.insert(gx) {
      continue;
    }
    let Some((bed, _)) = support_below(world, gx, gy) else {
      continue;
    };
    if !is_soft_embed_bed(bed) {
      continue;
    }
    let cohesion = MaterialRegistry::props(bed).cohesion;
    if cohesion > 60 {
      continue;
    }
    let debris = roof_collapse_debris(cell.material);
    world.set_cell(
      gx,
      gy,
      Cell {
        material: debris,
        sat: Sat(cell.sat.0.saturating_sub(2)),
        ..cell
      },
    );
    world.touch_dirty(gx, gy);
    applied += 1;
  }
  applied
}

fn downhill_roll_dir(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<i32> {
  // Only sample bottom-face columns — overhang cells have no support and must
  // not abort the whole search with `?`.
  let mut cols: HashSet<i32> = HashSet::new();
  for (gx, _, _) in bottom_face(comp) {
    cols.insert(gx);
  }
  if cols.is_empty() {
    return None;
  }
  let mut best_dx = 0_i32;
  let mut best_drop = 0_i32;
  for &gx in &cols {
    let Some(center) = column_top_support_y(world, gx, &comp.set) else {
      continue;
    };
    for dx in [-1_i32, 1] {
      let nx = world.wrap_x(gx + dx);
      if cols.contains(&nx) {
        continue;
      }
      let Some(side) = column_top_support_y(world, nx, &HashSet::new()) else {
        // Open void / unloaded beside us counts as a deep drop.
        if best_drop < 8 {
          best_drop = 8;
          best_dx = dx;
        }
        continue;
      };
      let drop = center - side;
      if drop > best_drop {
        best_drop = drop;
        best_dx = dx;
      }
    }
  }
  if best_drop >= cfg.roll_drop_cells {
    Some(best_dx)
  } else {
    None
  }
}

fn column_top_support_y(world: &World, gx: i32, body: &HashSet<(i32, i32)>) -> Option<i32> {
  let mut top = None;
  for y in 0..512 {
    if body.contains(&(gx, y)) {
      continue;
    }
    let Some(c) = world.get_cell(gx, y) else {
      break;
    };
    if c.material != MaterialId::Air {
      top = Some(y);
    }
  }
  top
}

/// Downhill bottom-corner pivot — tip the body *over* the edge into air.
fn roll_pivot(comp: &Component, dx: i32) -> (i32, i32) {
  let face = bottom_face(comp);
  if face.is_empty() {
    return comp_anchor(comp);
  }
  if dx > 0 {
    // Roll right: tip over the right (downhill) toe.
    face
      .iter()
      .max_by_key(|(x, y, _)| (*x, -*y))
      .map(|(x, y, _)| (*x, *y))
      .unwrap_or(comp_anchor(comp))
  } else {
    // Roll left: tip over the left (downhill) toe.
    face
      .iter()
      .min_by_key(|(x, y, _)| (*x, *y))
      .map(|(x, y, _)| (*x, *y))
      .unwrap_or(comp_anchor(comp))
  }
}

fn rotate_pos(px: i32, py: i32, gx: i32, gy: i32, dx: i32) -> (i32, i32) {
  let rx = gx - px;
  let ry = gy - py;
  if dx > 0 {
    // Clockwise tip over the right downhill edge.
    (px + ry, py - rx)
  } else {
    // Counter-clockwise tip over the left downhill edge.
    (px - ry, py + rx)
  }
}

fn can_pivot_roll(world: &World, comp: &Component, pivot: (i32, i32), dx: i32) -> bool {
  for (gx, gy, _) in &comp.cells {
    let (tx, ty) = rotate_pos(pivot.0, pivot.1, *gx, *gy, dx);
    if ty < 0 {
      return false;
    }
    let tx = world.wrap_x(tx);
    if comp.set.contains(&(tx, ty)) {
      continue;
    }
    let Some(dst) = world.get_cell(tx, ty) else {
      return false;
    };
    if !roll_destination_ok(world, tx, ty, &dst, &comp.set) {
      return false;
    }
  }
  true
}

/// Push soft bed out of a roll destination into nearby air, else crush it.
fn displace_soft_at(world: &mut World, gx: i32, gy: i32) {
  let Some(cur) = world.get_cell(gx, gy) else {
    return;
  };
  if !is_roll_displaceable(cur.material) {
    return;
  }
  // Prefer spilling downhill / down into air.
  for (dx, dy) in [(0, -1), (1, -1), (-1, -1), (1, 0), (-1, 0)] {
    let nx = world.wrap_x(gx + dx);
    let ny = gy + dy;
    if ny < 0 {
      continue;
    }
    if matches!(world.get_cell(nx, ny), Some(c) if c.material == MaterialId::Air) {
      world.set_cell(nx, ny, cur);
      world.touch_dirty(nx, ny);
      world.set_cell(gx, gy, Cell::air());
      world.touch_dirty(gx, gy);
      return;
    }
  }
  world.set_cell(gx, gy, Cell::air());
  world.touch_dirty(gx, gy);
}

fn write_roll_cells(world: &mut World, sources: &[(i32, i32)], moves: Vec<(i32, i32, Cell)>) {
  // Clear soft beds that occupy destinations before clearing sources,
  // so displaced soft can use vacated body cells if needed.
  let src_set: HashSet<(i32, i32)> = sources.iter().copied().collect();
  for (tx, ty, _) in &moves {
    if src_set.contains(&(*tx, *ty)) {
      continue;
    }
    if let Some(dst) = world.get_cell(*tx, *ty) {
      if is_roll_displaceable(dst.material) {
        displace_soft_at(world, *tx, *ty);
      }
    }
  }
  for (x, y) in sources {
    world.set_cell(*x, *y, Cell::air());
  }
  for (tx, ty, cell) in moves {
    world.set_cell(tx, ty, cell);
    world.touch_dirty(tx, ty);
  }
}

fn pivot_roll_component(world: &mut World, comp: &Component, pivot: (i32, i32), dx: i32) -> bool {
  if !can_pivot_roll(world, comp, pivot, dx) {
    return false;
  }
  let moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(gx, gy, c)| {
      let (tx, ty) = rotate_pos(pivot.0, pivot.1, *gx, *gy, dx);
      (world.wrap_x(tx), ty, *c)
    })
    .collect();
  let sources: Vec<(i32, i32)> = comp.cells.iter().map(|(x, y, _)| (*x, *y)).collect();
  write_roll_cells(world, &sources, moves);
  true
}

fn try_pivot_roll(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<i32> {
  if is_floating(world, comp) {
    return None;
  }
  let dx = downhill_roll_dir(world, comp, cfg)?;
  let pivot = roll_pivot(comp, dx);
  if can_pivot_roll(world, comp, pivot, dx) {
    Some(dx)
  } else {
    None
  }
}

fn can_translate_roll(world: &World, comp: &Component, dx: i32, dy: i32) -> bool {
  for (gx, gy, _) in &comp.cells {
    let tx = world.wrap_x(gx + dx);
    let ty = gy + dy;
    if ty < 0 {
      return false;
    }
    if comp.set.contains(&(tx, ty)) {
      continue;
    }
    let Some(dst) = world.get_cell(tx, ty) else {
      return false;
    };
    if !roll_destination_ok(world, tx, ty, &dst, &comp.set) {
      return false;
    }
  }
  true
}

fn translate_roll_component(world: &mut World, comp: &Component, dx: i32, dy: i32) -> bool {
  if dx == 0 && dy == 0 {
    return false;
  }
  if !can_translate_roll(world, comp, dx, dy) {
    return false;
  }
  let moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(x, y, c)| (world.wrap_x(x + dx), y + dy, *c))
    .collect();
  let sources: Vec<(i32, i32)> = comp.cells.iter().map(|(x, y, _)| (*x, *y)).collect();
  write_roll_cells(world, &sources, moves);
  true
}

fn try_roll_translate(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<(i32, i32)> {
  if is_floating(world, comp) {
    return None;
  }
  let dx = downhill_roll_dir(world, comp, cfg)?;
  // Tip-step down the face; sideways slide last (shallow slopes).
  for &(mx, my) in &[(dx, -1), (dx, -2), (dx, 0)] {
    if can_translate_roll(world, comp, mx, my) {
      return Some((mx, my));
    }
  }
  None
}

/// True when a seated body still has a downhill neighbor and should stay awake.
fn body_has_downhill(world: &World, gx: i32, gy: i32) -> bool {
  let Some(below) = world.get_cell(gx, gy - 1) else {
    return true;
  };
  if below.material == MaterialId::Air {
    return true;
  }
  // Only contact cells (bed under us, not another rock cell of the same body).
  if is_competent_rock(below.material) {
    return false;
  }
  let seat_y = gy - 1;
  for dx in [-1_i32, 1] {
    let nx = world.wrap_x(gx + dx);
    // Air beside / below-beside → can tumble that way.
    if matches!(world.get_cell(nx, gy), Some(c) if c.material == MaterialId::Air)
      && matches!(
        world.get_cell(nx, gy - 1),
        Some(c) if c.material == MaterialId::Air || is_roll_displaceable(c.material)
      )
    {
      return true;
    }
    // Neighbor bed surface lower than our seat.
    if let Some(side_top) = column_top_support_y(world, nx, &HashSet::new()) {
      if seat_y - side_top >= 1 {
        return true;
      }
    } else {
      return true;
    }
    if let Some(side) = world.get_cell(nx, seat_y - 1) {
      if side.material != MaterialId::Air {
        if matches!(world.get_cell(nx, seat_y), Some(c) if c.material == MaterialId::Air) {
          return true;
        }
      }
    }
  }
  false
}

/// Re-dirty competent bodies that can still fall or roll.
pub fn wake_competent_bodies(world: &mut World, coords: &[ChunkCoord]) {
  let cw = CHUNK_CELLS_W as i32;
  let ch = CHUNK_CELLS_H as i32;
  let mut touches: Vec<(i32, i32)> = Vec::new();
  for &coord in coords {
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
        let wake = match world.get_cell(gx, gy - 1) {
          None => true,
          Some(b) if body_passable_at(world, gx, gy - 1, &b) => true,
          Some(_) if body_has_downhill(world, gx, gy) => true,
          _ => false,
        };
        if wake {
          touches.push((gx, gy));
        }
      }
    }
  }
  for (gx, gy) in touches {
    world.touch_dirty(gx, gy);
  }
}

/// Wake every loaded chunk (F3 mid-air paint insurance).
pub fn wake_competent_bodies_all(world: &mut World) {
  let coords: Vec<_> = world.chunks.keys().copied().collect();
  wake_competent_bodies(world, &coords);
}

fn region_has_competent(world: &World, ac: &ActiveChunk) -> bool {
  let Some(chunk) = world.chunks.get(&ac.coord) else {
    return false;
  };
  for y in ac.rect.y0..=ac.rect.y1 {
    for x in ac.rect.x0..=ac.rect.x1 {
      if is_competent_rock(chunk.get(x as usize, y as usize).material) {
        return true;
      }
    }
  }
  false
}

/// Inflate dirty rects downward for free-fall and sideways for roll — not full chunks.
fn expand_competent_regions(active: &[ActiveChunk], drop_budget: i32) -> Vec<ActiveChunk> {
  if active.is_empty() {
    return Vec::new();
  }
  let w = CHUNK_CELLS_W as i32;
  let h = CHUNK_CELLS_H as i32;
  let pad_x = 2_i32;
  let drop = drop_budget.max(4);
  let mut map: HashMap<ChunkCoord, Rect> = HashMap::new();
  let absorb = |map: &mut HashMap<ChunkCoord, Rect>, coord: ChunkCoord, x0: i32, y0: i32, x1: i32, y1: i32| {
    if x1 < 0 || y1 < 0 || x0 >= w || y0 >= h {
      return;
    }
    let rx0 = x0.clamp(0, w - 1) as u8;
    let ry0 = y0.clamp(0, h - 1) as u8;
    let rx1 = x1.clamp(0, w - 1) as u8;
    let ry1 = y1.clamp(0, h - 1) as u8;
    map.entry(coord)
      .and_modify(|r| {
        r.expand_to_include(rx0, ry0);
        r.expand_to_include(rx1, ry1);
      })
      .or_insert(Rect {
        x0: rx0,
        y0: ry0,
        x1: rx1,
        y1: ry1,
      });
  };
  for ac in active {
    let x0 = ac.rect.x0 as i32 - pad_x;
    let x1 = ac.rect.x1 as i32 + pad_x;
    let y0 = ac.rect.y0 as i32 - drop;
    let y1 = ac.rect.y1 as i32 + 3;
    // Home chunk.
    absorb(&mut map, ac.coord, x0, y0, x1, y1);
    // Neighbours when the inflate crosses a seam.
    if x0 < 0 {
      absorb(
        &mut map,
        ChunkCoord::new(ac.coord.cx - 1, ac.coord.cy),
        w + x0,
        y0,
        w - 1,
        y1,
      );
    }
    if x1 >= w {
      absorb(
        &mut map,
        ChunkCoord::new(ac.coord.cx + 1, ac.coord.cy),
        0,
        y0,
        x1 - w,
        y1,
      );
    }
    if y0 < 0 {
      absorb(
        &mut map,
        ChunkCoord::new(ac.coord.cx, ac.coord.cy - 1),
        x0,
        h + y0,
        x1,
        h - 1,
      );
    }
    if x0 < 0 && y0 < 0 {
      absorb(
        &mut map,
        ChunkCoord::new(ac.coord.cx - 1, ac.coord.cy - 1),
        w + x0,
        h + y0,
        w - 1,
        h - 1,
      );
    }
    if x1 >= w && y0 < 0 {
      absorb(
        &mut map,
        ChunkCoord::new(ac.coord.cx + 1, ac.coord.cy - 1),
        0,
        h + y0,
        x1 - w,
        h - 1,
      );
    }
  }
  let mut out: Vec<ActiveChunk> = map
    .into_iter()
    .map(|(coord, rect)| ActiveChunk { coord, rect })
    .collect();
  out.sort_by(|a, b| {
    a.coord
      .cy
      .cmp(&b.coord.cy)
      .then(a.coord.cx.cmp(&b.coord.cx))
  });
  out
}

fn expand_regions_to_components(
  world: &World,
  components: &[Component],
  drop_budget: i32,
) -> Vec<ActiveChunk> {
  if components.is_empty() {
    return Vec::new();
  }
  let w = CHUNK_CELLS_W as i32;
  let h = CHUNK_CELLS_H as i32;
  let mut by_chunk: HashMap<ChunkCoord, Rect> = HashMap::new();
  for comp in components {
    for (gx, gy, _) in &comp.cells {
      let cx = gx.div_euclid(w);
      let cy = gy.div_euclid(h);
      let lx = gx.rem_euclid(w) as u8;
      let ly = gy.rem_euclid(h) as u8;
      let coord = ChunkCoord::new(cx, cy);
      by_chunk
        .entry(coord)
        .and_modify(|r| {
          r.expand_to_include(lx, ly);
        })
        .or_insert(Rect {
          x0: lx,
          y0: ly,
          x1: lx,
          y1: ly,
        });
    }
  }
  let seeds: Vec<ActiveChunk> = by_chunk
    .into_iter()
    .map(|(coord, rect)| ActiveChunk { coord, rect })
    .collect();
  let expanded = expand_competent_regions(&seeds, drop_budget);
  expanded
    .into_iter()
    .filter(|ac| world.chunks.contains_key(&ac.coord))
    .collect()
}

fn competent_active_regions(world: &World, active: &[ActiveChunk], drop_budget: i32) -> Vec<ActiveChunk> {
  let seed = if active.is_empty() {
    world
      .chunks
      .keys()
      .copied()
      .map(|coord| ActiveChunk {
        coord,
        rect: Rect::full(),
      })
      .collect()
  } else {
    active
      .iter()
      .filter(|ac| region_has_competent(world, ac))
      .cloned()
      .collect::<Vec<_>>()
  };
  if seed.is_empty() {
    return Vec::new();
  }
  let expanded = expand_competent_regions(&seed, drop_budget);
  // Keep only loaded chunks.
  expanded
    .into_iter()
    .filter(|ac| world.chunks.contains_key(&ac.coord))
    .collect()
}

fn rests_on_soft_bed(world: &World, comp: &Component) -> bool {
  bottom_face(comp).iter().any(|(x, y, _)| {
    matches!(
      support_below(world, *x, *y),
      Some((bed, _)) if is_soft_embed_bed(bed)
    )
  })
}

fn rests_on_hard_bed(world: &World, comp: &Component) -> bool {
  bottom_face(comp).iter().any(|(x, y, _)| {
    matches!(
      support_below(world, *x, *y),
      Some((bed, _)) if bed != MaterialId::Air && !is_soft_embed_bed(bed)
    )
  })
}

fn advance_streak(
  fall_streak: &mut HashMap<(i32, i32), u32>,
  from: (i32, i32),
  dx: i32,
  dy: i32,
) {
  let streak = fall_streak.remove(&from).unwrap_or(0);
  let gained = if dy < 0 { (-dy) as u32 } else { 0 };
  fall_streak.insert((from.0 + dx, from.1 + dy), streak + gained);
}

fn apply_roll(
  world: &mut World,
  regions: &[ActiveChunk],
  comp: &Component,
  anchor: (i32, i32),
  cfg: &CompetentFallConfig,
  fall_streak: &mut HashMap<(i32, i32), u32>,
  stats: &mut CompetentFallStats,
) -> bool {
  if stats.roll_moves >= cfg.max_roll_events {
    return false;
  }
  // Pivot tumble first — whole body rotates around the uphill toe.
  if let Some(dx) = try_pivot_roll(world, comp, cfg) {
    let pivot = roll_pivot(comp, dx);
    if pivot_roll_component(world, comp, pivot, dx) {
      stats.roll_moves += 1;
      let post = build_components(world, regions);
      if let Some(refreshed) = post.iter().find(|c| c.set.contains(&pivot)) {
        if is_floating(world, refreshed) && translate_component(world, refreshed, 0, -1) {
          stats.fall_moves += 1;
          advance_streak(fall_streak, pivot, 0, -1);
        }
      }
      return true;
    }
  }
  if let Some((dx, dy)) = try_roll_translate(world, comp, cfg) {
    if translate_roll_component(world, comp, dx, dy) {
      stats.roll_moves += 1;
      advance_streak(fall_streak, anchor, dx, dy);
      let new_anchor = (anchor.0 + dx, anchor.1 + dy);
      if dy == 0 {
        let post = build_components(world, regions);
        if let Some(refreshed) = post.iter().find(|c| c.set.contains(&new_anchor)) {
          if is_floating(world, refreshed) && translate_component(world, refreshed, 0, -1) {
            stats.fall_moves += 1;
            advance_streak(fall_streak, new_anchor, 0, -1);
          }
        }
      }
      return true;
    }
  }
  false
}

/// Run competent-body physics on the active scan set.
pub fn apply_competent_fall_regions(
  world: &mut World,
  active: &[ActiveChunk],
  cfg: &CompetentFallConfig,
  fps_path: bool,
) -> CompetentFallStats {
  if !cfg.enable {
    return CompetentFallStats::default();
  }
  let max_drop = if fps_path {
    cfg.max_passes.min(COMPETENT_FALL_PASSES_FPS)
  } else {
    cfg.max_passes.max(COMPETENT_FALL_PASSES_FPS)
  } as i32;
  let mut regions = competent_active_regions(world, active, max_drop);
  if regions.is_empty() {
    return CompetentFallStats::default();
  }
  let mut stats = CompetentFallStats::default();
  let mut fall_streak: HashMap<(i32, i32), u32> = HashMap::new();
  // Free-fall jumps once (binary-searched distance), then impact/roll rebuilds —
  // not one rebuild per cell of drop.
  for pass in 0..COMPETENT_TOPOLOGY_PASSES {
    let components = build_components(world, &regions);
    if components.is_empty() {
      break;
    }
    let mut moved = false;
    for comp in components {
      let anchor = comp_anchor(&comp);
      // Pass 0: long free-fall jump; later passes: shorter jumps while still
      // fully airborne (very tall sky paint without waiting extra ticks).
      let drop = if pass == 0 {
        max_drop_distance(world, &comp, max_drop)
      } else if is_floating(world, &comp) {
        max_drop_distance(world, &comp, 16.min(max_drop))
      } else {
        0
      };
      if drop > 0 {
        if translate_component(world, &comp, 0, -drop) {
          stats.fall_moves += 1;
          advance_streak(&mut fall_streak, anchor, 0, -drop);
          moved = true;
        }
        continue;
      }
      if is_floating(world, &comp) {
        continue;
      }
      // Wait until every bottom cell has support — avoids uneven per-column
      // embed/shatter while the body is still bridging a slope or gap.
      if !is_fully_supported(world, &comp) {
        if apply_roll(
          world,
          &regions,
          &comp,
          anchor,
          cfg,
          &mut fall_streak,
          &mut stats,
        ) {
          moved = true;
        }
        continue;
      }
      let streak = *fall_streak.get(&anchor).unwrap_or(&0);
      if rests_on_soft_bed(world, &comp) {
        if apply_roll(
          world,
          &regions,
          &comp,
          anchor,
          cfg,
          &mut fall_streak,
          &mut stats,
        ) {
          moved = true;
          continue;
        }
        let embedded = soft_embed(world, &comp, cfg);
        if embedded > 0 {
          stats.embed_cells += embedded;
          moved = true;
        }
        continue;
      }
      if rests_on_hard_bed(world, &comp) {
        let shattered = impact_shatter(world, &comp, streak, cfg);
        if shattered > 0 {
          stats.impacts += 1;
          fall_streak.remove(&anchor);
          moved = true;
          continue;
        }
      }
      if apply_roll(
        world,
        &regions,
        &comp,
        anchor,
        cfg,
        &mut fall_streak,
        &mut stats,
      ) {
        moved = true;
      }
    }
    if moved {
      let refreshed = expand_regions_to_components(world, &build_components(world, &regions), max_drop);
      if !refreshed.is_empty() {
        regions = refreshed;
      }
    } else {
      break;
    }
  }
  stats
}

/// True when roof collapse should defer to the body fall pass (air below).
pub fn roof_defer_to_competent_fall(
  enable: bool,
  material: MaterialId,
  below: &Cell,
) -> bool {
  enable && is_competent_rock(material) && passable_for_body_material(below.material)
}

#[cfg(test)]
mod tests {
  use super::*;
  use crate::chunk::ChunkCoord;
  use crate::rules::tick_with_perf;
  use crate::rules::PerfConfig;

  fn stamp_blob(w: &mut World, x0: i32, y0: i32, wdt: i32, hgt: i32) {
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in x0..x0 + wdt {
      for y in y0..y0 + hgt {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
  }

  #[test]
  fn tall_blob_stays_connected_after_fall() {
    let mut w = World::new(9);
    stamp_blob(&mut w, 4, 50, 3, 5);
    let cfg = CompetentFallConfig {
      max_passes: 48,
      ..CompetentFallConfig::default()
    };
    apply_competent_fall_regions(&mut w, &[], &cfg, false);
    let stones: Vec<(i32, i32)> = (4..7)
      .flat_map(|x| (0..60).map(move |y| (x, y)))
      .filter(|&(x, y)| {
        w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone)
      })
      .collect();
    assert_eq!(stones.len(), 15, "all 15 stone cells must survive the fall");
    for x in 4..7 {
      let ys: Vec<i32> = stones.iter().filter(|(sx, _)| *sx == x).map(|(_, y)| *y).collect();
      if ys.len() < 2 {
        continue;
      }
      let span = ys.iter().max().unwrap() - ys.iter().min().unwrap() + 1;
      assert_eq!(
        span as usize,
        ys.len(),
        "column {x} must not have internal air gaps (ys={ys:?})"
      );
    }
  }

  #[test]
  fn stone_sky_blob_falls_as_unified_body() {
    let mut w = World::new(9);
    stamp_blob(&mut w, 4, 70, 3, 3);
    let perf = PerfConfig::default();
    for _ in 0..4 {
      tick_with_perf(&mut w, &perf);
    }
    let y_top = (0..128)
      .filter(|&y| w.get_cell(5, y).map(|c| c.material) == Some(MaterialId::Stone))
      .max()
      .unwrap_or(128);
    assert!(
      y_top < 68,
      "3×3 stone blob must fall as a body (top y={y_top}, expected < 68)"
    );
  }

  #[test]
  fn stone_blob_shatters_on_bedrock_impact() {
    let mut w = World::new(9);
    stamp_blob(&mut w, 4, 8, 3, 3);
    for x in 3..=7 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let cfg = CompetentFallConfig {
      max_passes: 16,
      min_impact_fall_cells: 1,
      ..CompetentFallConfig::default()
    };
    apply_competent_fall_regions(
      &mut w,
      &[],
      &cfg,
      false,
    );
    let loose: u32 = (4..7)
      .flat_map(|x| (1..=8).map(move |y| (x, y)))
      .filter(|&(x, y)| {
        w.get_cell(x, y)
          .map(|c| c.material == MaterialId::LooseRock)
          .unwrap_or(false)
      })
      .count() as u32;
    assert!(loose > 0, "impact must spawn LooseRock debris (loose={loose})");
  }

  #[test]
  fn limestone_blob_shatters_on_bedrock_impact() {
    let mut w = World::new(9);
    for x in 4..7 {
      for y in 8..11 {
        w.set_cell(x, y, Cell::solid(MaterialId::Limestone));
      }
    }
    for x in 3..=7 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    let cfg = CompetentFallConfig {
      max_passes: 16,
      min_impact_fall_cells: 1,
      ..CompetentFallConfig::default()
    };
    apply_competent_fall_regions(&mut w, &[], &cfg, false);
    let loose: u32 = (4..7)
      .flat_map(|x| (1..=10).map(move |y| (x, y)))
      .filter(|&(x, y)| {
        w.get_cell(x, y)
          .map(|c| c.material == MaterialId::LooseLimestone)
          .unwrap_or(false)
      })
      .count() as u32;
    assert!(loose > 0, "limestone impact must spawn LooseLimestone (loose={loose})");
  }

  #[test]
  fn stone_blob_sinks_through_lake() {
    let mut w = World::new(9);
    // Wide sand floor so landing does not tip off a tiny pad edge.
    for x in 0..16 {
      for y in 0..=2 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    for x in 3..=7 {
      for y in 3..=12 {
        w.set_cell(
          x,
          y,
          Cell {
            material: MaterialId::Air,
            sat: Sat(255),
            ..Cell::default()
          },
        );
      }
    }
    stamp_blob(&mut w, 4, 18, 3, 3);
    let cfg = CompetentFallConfig {
      max_passes: 24,
      min_impact_fall_cells: 99,
      max_roll_events: 0, // sink test only — no tip after landing
      ..CompetentFallConfig::default()
    };
    apply_competent_fall_regions(&mut w, &[], &cfg, false);
    let min_stone_y = (0..16)
      .flat_map(|x| (0..20).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .min()
      .unwrap_or(99);
    assert!(
      min_stone_y <= 5,
      "stone must sink through lake water onto sand (min_stone_y={min_stone_y})"
    );
  }

  #[test]
  fn stone_blob_rolls_downhill_on_slope() {
    let mut w = World::new(11);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..12 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 4 + (11 - x).min(6);
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    stamp_blob(&mut w, 5, 12, 2, 2);
    let cfg = CompetentFallConfig {
      max_passes: 32,
      roll_drop_cells: 1,
      max_roll_events: 8,
      min_impact_fall_cells: 99,
      ..CompetentFallConfig::default()
    };
    for _ in 0..6 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let rock_xs: Vec<i32> = (0..12)
      .filter(|&x| {
        (3..=18).any(|y| {
          w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone)
        })
      })
      .collect();
    let loose = (0..12)
      .flat_map(|x| (3..=18).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::LooseRock))
      .count();
    assert!(
      rock_xs.iter().any(|&x| x > 6),
      "body should roll right down the sand slope (rock cols={rock_xs:?})"
    );
    assert!(
      loose <= 2,
      "slope tumble should stay mostly intact stone (loose={loose})"
    );
  }

  #[test]
  fn overhang_blob_still_detects_downhill() {
    let mut w = World::new(11);
    // Stair slope down to the left.
    for x in 0..10 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 2 + x;
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    // 3×3 with a corner overhang so some columns lack support under the body.
    stamp_blob(&mut w, 5, 8, 3, 3);
    let regions = competent_active_regions(&w, &[], 8);
    let comps = build_components(&w, &regions);
    assert!(!comps.is_empty());
    let cfg = CompetentFallConfig::default();
    let dir = downhill_roll_dir(&w, &comps[0], &cfg);
    assert_eq!(dir, Some(-1), "overhang must not abort downhill detection");
  }

  #[test]
  fn tip_pivot_is_downhill_toe() {
    let cells = vec![
      (5, 5, Cell::solid(MaterialId::Stone)),
      (6, 5, Cell::solid(MaterialId::Stone)),
      (5, 6, Cell::solid(MaterialId::Stone)),
      (6, 6, Cell::solid(MaterialId::Stone)),
    ];
    let set: HashSet<_> = cells.iter().map(|(x, y, _)| (*x, *y)).collect();
    let comp = Component {
      cells,
      set,
      min_y: 5,
      max_y: 6,
    };
    assert_eq!(roll_pivot(&comp, 1), (6, 5), "right roll pivots on right toe");
    assert_eq!(roll_pivot(&comp, -1), (5, 5), "left roll pivots on left toe");
  }
}
