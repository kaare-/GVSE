//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Competent rock (Stone / Limestone) falls as connected rigid bodies:
//! 1. Vertical drop through Air (including lake water).
//! 2. Impact shatter — bottom face converts to fallable debris.
//! 3. Slope roll + soft-bed embed on sand / soil / clay.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use wk_material::{MaterialId, MaterialRegistry};

use crate::active::ActiveChunk;
use crate::cell::{is_competent_rock, is_grain, Cell, Sat};
use crate::chunk::{ChunkCoord, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::failure::roof_collapse_debris;
use crate::grid::World;

/// Max vertical cells per tick (sky paint → ground in one frame on FPS path).
pub const COMPETENT_FALL_PASSES: u32 = 64;
pub const COMPETENT_FALL_PASSES_FPS: u32 = 32;

/// Live-tunable competent-body knobs (Tab → Geotech).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct CompetentFallConfig {
  pub enable: bool,
  /// Vertical fall attempts per tick.
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
      min_impact_fall_cells: 2,
      heavy_impact_fall_cells: 12,
      max_impact_cells: 48,
      roll_drop_cells: 1,
      max_roll_events: 8,
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

#[inline]
fn passable_for_body(cell: &Cell) -> bool {
  cell.material == MaterialId::Air
}

#[inline]
fn is_soft_embed_bed(material: MaterialId) -> bool {
  matches!(
    material,
    MaterialId::Sand | MaterialId::Soil | MaterialId::Clay | MaterialId::Gravel
  )
}

struct Component {
  cells: Vec<(i32, i32, Cell)>,
  set: HashSet<(i32, i32)>,
  min_y: i32,
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
    for (dx, dy) in [(0_i32, 1), (1, 0), (0, -1), (-1, 0)] {
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
      Component { cells, set, min_y }
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
    if !passable_for_body(&dst) {
      return false;
    }
  }
  true
}

fn translate_component(world: &mut World, comp: &Component, dx: i32, dy: i32) -> bool {
  if !can_translate(world, comp, dx, dy) {
    return false;
  }
  let mut moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(x, y, c)| (world.wrap_x(x + dx), y + dy, *c))
    .collect();
  moves.sort_by_key(|(_, y, _)| *y);
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
      Some(c) => passable_for_body(&c),
    })
}

fn support_below(world: &World, gx: i32, gy: i32) -> Option<(MaterialId, i32)> {
  let Some(below) = world.get_cell(gx, gy - 1) else {
    return None;
  };
  if below.material == MaterialId::Air {
    return None;
  }
  Some((below.material, gy - 1))
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
    if passable_for_body(&Cell {
      material: bed_mat,
      ..Cell::default()
    }) {
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
    if applied >= cfg.max_impact_cells {
      break;
    }
  }
  applied
}

fn downhill_roll_dir(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<i32> {
  let mut cols: HashSet<i32> = HashSet::new();
  for (gx, _, _) in &comp.cells {
    cols.insert(*gx);
  }
  let mut best_dx = 0_i32;
  let mut best_drop = 0_i32;
  for &gx in &cols {
    let center = column_top_support_y(world, gx, &comp.set)?;
    for dx in [-1_i32, 1] {
      let nx = world.wrap_x(gx + dx);
      if cols.contains(&nx) {
        continue;
      }
      let side = column_top_support_y(world, nx, &HashSet::new())?;
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

fn try_roll_downhill(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<(i32, i32)> {
  if is_floating(world, comp) {
    return None;
  }
  let dx = downhill_roll_dir(world, comp, cfg)?;
  if can_translate(world, comp, dx, -1) {
    return Some((dx, -1));
  }
  if can_translate(world, comp, dx, 0) {
    return Some((dx, 0));
  }
  None
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
          Some(b) if passable_for_body(&b) => true,
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

fn expand_competent_regions(active: &[ActiveChunk]) -> Vec<ActiveChunk> {
  use std::collections::HashMap;
  if active.is_empty() {
    return Vec::new();
  }
  let full = crate::chunk::Rect::full();
  let mut map: HashMap<ChunkCoord, ActiveChunk> = HashMap::new();
  for ac in active {
    map.insert(ac.coord, ActiveChunk {
      coord: ac.coord,
      rect: full,
    });
    // Bodies can fall across the chunk seam into the slab below.
    let below = ChunkCoord::new(ac.coord.cx, ac.coord.cy - 1);
    map.entry(below).or_insert(ActiveChunk {
      coord: below,
      rect: full,
    });
  }
  let mut out: Vec<_> = map.into_values().collect();
  out.sort_by(|a, b| {
    a.coord
      .cy
      .cmp(&b.coord.cy)
      .then(a.coord.cx.cmp(&b.coord.cx))
  });
  out
}

fn competent_active_regions(world: &World, active: &[ActiveChunk]) -> Vec<ActiveChunk> {
  if active.is_empty() {
    return world
      .chunks
      .keys()
      .copied()
      .map(|coord| ActiveChunk {
        coord,
        rect: crate::chunk::Rect::full(),
      })
      .collect();
  }
  expand_competent_regions(active)
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
      Some((bed, _)) if !passable_for_body(&Cell {
        material: bed,
        ..Cell::default()
      }) && !is_soft_embed_bed(bed)
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
  let gained = if dy < 0 { 1 } else { 0 };
  fall_streak.insert((from.0 + dx, from.1 + dy), streak + gained);
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
  let max_passes = if fps_path {
    cfg.max_passes.min(COMPETENT_FALL_PASSES_FPS)
  } else {
    cfg.max_passes.max(COMPETENT_FALL_PASSES_FPS)
  };
  let regions = competent_active_regions(world, active);
  let mut stats = CompetentFallStats::default();
  let mut fall_streak: HashMap<(i32, i32), u32> = HashMap::new();
  for _ in 0..max_passes {
    let components = build_components(world, &regions);
    if components.is_empty() {
      break;
    }
    let mut moved = false;
    for comp in components {
      let anchor = comp_anchor(&comp);
      if can_translate(world, &comp, 0, -1) {
        if translate_component(world, &comp, 0, -1) {
          stats.fall_moves += 1;
          advance_streak(&mut fall_streak, anchor, 0, -1);
          moved = true;
        }
        continue;
      }
      if is_floating(world, &comp) {
        continue;
      }
      let streak = *fall_streak.get(&anchor).unwrap_or(&0);
      if rests_on_soft_bed(world, &comp) {
        if stats.roll_moves < cfg.max_roll_events {
          if let Some((dx, dy)) = try_roll_downhill(world, &comp, cfg) {
            if translate_component(world, &comp, dx, dy) {
              stats.roll_moves += 1;
              advance_streak(&mut fall_streak, anchor, dx, dy);
              moved = true;
              continue;
            }
          }
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
      if stats.roll_moves < cfg.max_roll_events {
        if let Some((dx, dy)) = try_roll_downhill(world, &comp, cfg) {
          if translate_component(world, &comp, dx, dy) {
            stats.roll_moves += 1;
            advance_streak(&mut fall_streak, anchor, dx, dy);
            moved = true;
          }
        }
      }
    }
    if !moved {
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
  enable && is_competent_rock(material) && passable_for_body(below)
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
        (6..=18).any(|y| {
          matches!(
            w.get_cell(x, y).map(|c| c.material),
            Some(MaterialId::Stone) | Some(MaterialId::LooseRock)
          )
        })
      })
      .collect();
    assert!(
      rock_xs.iter().any(|&x| x > 6),
      "body should roll right down the sand slope (rock cols={rock_xs:?})"
    );
  }
}
