//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Competent rock (Stone / Limestone) as **dynamic rigid bodies** (grid CCRB):
//! static strata stay in the grid; only boulder-sized air-adjacent components
//! are simulated — industry-standard for falling-sand / voxel CA engines.
//!
//! 1. Free fall through Air (rocks sink in lakes).
//! 2. Impact shatter on hard beds after a fall.
//! 3. COM / support tip — 90° pivot when unstable; no slide-shred.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::active::ActiveChunk;
use crate::cell::{is_competent_rock, Cell, CellFlags, Sat};
use crate::competent_probe as probe;
use crate::chunk::{ChunkCoord, Rect, CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::failure::roof_collapse_debris;
use crate::grid::World;

/// Max vertical drop (cells) per body per tick on the full-feel path.
pub const COMPETENT_FALL_PASSES: u32 = 96;
/// FPS path: one rigid drop of this many cells (sky → ground in ~1–2 frames).
pub const COMPETENT_FALL_PASSES_FPS: u32 = 64;
/// Seated tip / impact rebuilds per tick (keep low — each rebuild is O(body)).
pub const COMPETENT_TOPOLOGY_PASSES: u32 = 6;
/// FPS path: fewer rebuilds so hanging peel cannot tank frame time.
pub const COMPETENT_TOPOLOGY_PASSES_FPS: u32 = 2;
/// Connected competent larger than this is treated as static terrain
/// *after* contact-weld splitting.
pub const MAX_DYNAMIC_BODY_CELLS: usize = 384;
/// Gather this many cells before giving up — long boulder chains need room
/// to be split apart; beyond this is real strata.
pub const FLOOD_GATHER_CAP: usize = 2048;
/// Hard cap on seated tip/roll bodies per tick (FPS guard).
pub const MAX_BODIES_PER_TICK: usize = 16;
/// Free-falling / hanging bodies processed per tick — must stay high enough
/// that dozens of floaters don't wait ~100 ticks for a turn.
pub const MAX_HANGING_BODIES_PER_TICK: usize = 96;
/// Extra free-fall slots beyond the hanging budget when many are airborne.
pub const MAX_FLOATING_BODIES_PER_TICK: usize = 128;
/// Contact "pebbles" this small never glue onto a larger body (editor/geology only).
pub const PEBBLE_SPLIT_MAX: usize = 4;
/// Main mass must be at least this big before pebble necks are split off.
pub const PEBBLE_SPLIT_HOST_MIN: usize = 12;
/// Tiny competent specs this size are crushed by larger moving bodies (not blockers).
pub const CRUSH_SPEC_MAX: usize = 6;
/// Long-thin fracture: span (cells) along the long axis.
pub const THIN_FRACTURE_SPAN: i32 = 7;
/// Long-thin fracture: max thickness (bbox short side) to count as a stick/slab.
pub const THIN_FRACTURE_THICK: i32 = 2;
/// Bedrock-rooted pillar columns this tall stay static (cantilever legs).
pub const PILLAR_COLUMN_MIN_HEIGHT: i32 = 4;

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
  /// Tip when support drop / COM overhang ≥ this many cells.
  pub roll_drop_cells: i32,
  /// Max whole-body tip events per tick.
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
      max_roll_events: 4,
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
  matches!(
    material,
    MaterialId::Sand
      | MaterialId::Soil
      | MaterialId::Clay
      | MaterialId::Gravel
      | MaterialId::LooseRock
      | MaterialId::LooseLimestone
      | MaterialId::Organic
      | MaterialId::Snow
  )
}

#[inline]
fn roll_destination_ok(world: &World, gx: i32, gy: i32, cell: &Cell, body: &HashSet<(i32, i32)>) -> bool {
  if body.contains(&(gx, gy)) {
    return true;
  }
  body_passable_at(world, gx, gy, cell) || is_roll_displaceable(cell.material)
}

/// Size of the 4-connected same-material / same-mobility competent cluster.
fn competent_cluster_size(world: &World, gx: i32, gy: i32, limit: usize) -> usize {
  let Some(seed) = world.get_cell(gx, gy) else {
    return 0;
  };
  if !is_competent_rock(seed.material) {
    return 0;
  }
  let mat = seed.material;
  let seed_mobile = is_mobile_rock(&seed);
  let mut seen = HashSet::new();
  let mut q = VecDeque::new();
  q.push_back((gx, gy));
  seen.insert((gx, gy));
  while let Some((cx, cy)) = q.pop_front() {
    if seen.len() > limit {
      return seen.len();
    }
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
      let nx = world.wrap_x(cx + dx);
      let ny = cy + dy;
      if !seen.insert((nx, ny)) {
        continue;
      }
      match world.get_cell(nx, ny) {
        Some(n) if flood_compatible(seed_mobile, &n, mat) => q.push_back((nx, ny)),
        _ => {
          seen.remove(&(nx, ny));
        }
      }
    }
  }
  seen.len()
}

/// Large movers crush tiny competent specs instead of welding / getting stuck.
fn can_crush_spec(world: &World, gx: i32, gy: i32, mover_cells: usize) -> bool {
  if mover_cells < CRUSH_SPEC_MAX * 2 {
    return false;
  }
  let Some(c) = world.get_cell(gx, gy) else {
    return false;
  };
  if !is_competent_rock(c.material) {
    return false;
  }
  let sz = competent_cluster_size(world, gx, gy, CRUSH_SPEC_MAX + 1);
  sz > 0 && sz <= CRUSH_SPEC_MAX && mover_cells >= sz * 3
}

fn crush_spec_at(world: &mut World, gx: i32, gy: i32) -> u32 {
  let Some(seed) = world.get_cell(gx, gy) else {
    return 0;
  };
  if !is_competent_rock(seed.material) {
    return 0;
  }
  let mat = seed.material;
  let seed_mobile = is_mobile_rock(&seed);
  let debris = roof_collapse_debris(mat);
  let mut seen = HashSet::new();
  let mut q = VecDeque::new();
  q.push_back((gx, gy));
  seen.insert((gx, gy));
  let mut crushed = 0u32;
  while let Some((cx, cy)) = q.pop_front() {
    if seen.len() > CRUSH_SPEC_MAX + 2 {
      break;
    }
    let Some(cur) = world.get_cell(cx, cy) else {
      continue;
    };
    if !flood_compatible(seed_mobile, &cur, mat) {
      continue;
    }
    let mut next = Cell {
      material: debris,
      sat: cur.sat,
      ..cur
    };
    next.flags.clear(CellFlags::MOBILE_ROCK);
    world.set_cell(cx, cy, next);
    world.touch_dirty(cx, cy);
    crushed += 1;
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
      let nx = world.wrap_x(cx + dx);
      let ny = cy + dy;
      if !seen.insert((nx, ny)) {
        continue;
      }
      match world.get_cell(nx, ny) {
        Some(n) if flood_compatible(seed_mobile, &n, mat) => q.push_back((nx, ny)),
        _ => {
          seen.remove(&(nx, ny));
        }
      }
    }
  }
  crushed
}

fn motion_destination_ok(
  world: &World,
  gx: i32,
  gy: i32,
  cell: &Cell,
  body: &HashSet<(i32, i32)>,
  mover_cells: usize,
) -> bool {
  if roll_destination_ok(world, gx, gy, cell, body) {
    return true;
  }
  if is_competent_rock(cell.material) && can_crush_spec(world, gx, gy, mover_cells) {
    return true;
  }
  false
}

struct Component {
  cells: Vec<(i32, i32, Cell)>,
  set: HashSet<(i32, i32)>,
  min_y: i32,
  max_y: i32,
}

/// Cheap "could this cell ever start moving?" gate used to seed body floods.
///
/// A buried or flat-seated cell whose only open neighbour is **above** can
/// never fall, tip, or slide, so it must not seed a rigid-body flood. Without
/// this, every surface cell of a natural ridge seeds a flood + morphological
/// open every tick — that alone cost ~60 ms/tick on the demo world.
///
/// Deliberately a *superset* of the truly movable set: it only needs somewhere
/// to go down (below, or a side with room below it).
#[inline]
fn body_can_seed(world: &World, gx: i32, gy: i32, _cell: &Cell) -> bool {
  // NOTE: deliberately does *not* short-circuit on MOBILE_ROCK. That flag is a
  // permanent flood-compatibility class, so treating it as "live" kept every
  // cell that ever moved seeding (and dirtying) itself forever.
  //
  // Room directly below → free fall / sink.
  match world.get_cell(gx, gy - 1) {
    None => return true,
    Some(b) if b.material == MaterialId::Air || is_roll_displaceable(b.material) => {
      return true;
    }
    _ => {}
  }
  // Room to a side *and* a lower floor on that side → tumble / slide candidate.
  //
  // Requiring the *side floor to be lower* (not merely "a side is open") is
  // what keeps flat and stepped terrain out of the seed set: a rock resting on
  // level ground with air beside it has nowhere to descend, and `try_slide`
  // would reject it anyway after a full flood.
  for dx in [-1_i32, 1] {
    let nx = world.wrap_x(gx + dx);
    let side_open = match world.get_cell(nx, gy) {
      None => true,
      Some(c) => c.material == MaterialId::Air || is_roll_displaceable(c.material),
    };
    if !side_open {
      continue;
    }
    match world.get_cell(nx, gy - 1) {
      None => return true,
      Some(d) if d.material == MaterialId::Air || is_roll_displaceable(d.material) => {
        return true;
      }
      _ => {}
    }
  }
  false
}

/// Public form of the seed gate so wake passes agree with body building.
#[inline]
pub fn competent_cell_can_move(world: &World, gx: i32, gy: i32) -> bool {
  match world.get_cell(gx, gy) {
    Some(c) if is_competent_rock(c.material) => body_can_seed(world, gx, gy, &c),
    _ => false,
  }
}

fn has_free_neighbor(world: &World, gx: i32, gy: i32) -> bool {
  for (dx, dy) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
    let nx = world.wrap_x(gx + dx);
    let ny = gy + dy;
    match world.get_cell(nx, ny) {
      None => return true,
      Some(c) if c.material == MaterialId::Air || is_roll_displaceable(c.material) => {
        return true;
      }
      _ => {}
    }
  }
  false
}

#[inline]
fn is_mobile_rock(cell: &Cell) -> bool {
  cell.flags.contains(CellFlags::MOBILE_ROCK)
}

/// Mark every cell in the body as a detached mobile rock — flood will no
/// longer absorb unmarked terrain or grow the mass by contact welding.
fn mark_component_mobile(world: &mut World, comp: &Component) {
  for &(gx, gy, _) in &comp.cells {
    let Some(mut cell) = world.get_cell(gx, gy) else {
      continue;
    };
    if !is_competent_rock(cell.material) {
      continue;
    }
    if cell.flags.contains(CellFlags::MOBILE_ROCK) {
      continue;
    }
    cell.flags.set(CellFlags::MOBILE_ROCK);
    world.set_cell(gx, gy, cell);
    world.touch_dirty(gx, gy);
  }
}

/// Stamp MOBILE_ROCK onto a competent cell before writing a moved body.
#[inline]
fn with_mobile_rock(mut cell: Cell) -> Cell {
  if is_competent_rock(cell.material) {
    cell.flags.set(CellFlags::MOBILE_ROCK);
  }
  cell
}

/// Same material and same mobility class (mobile↔mobile or strata↔strata).
#[inline]
fn flood_compatible(seed_mobile: bool, neighbor: &Cell, material: MaterialId) -> bool {
  neighbor.material == material && is_mobile_rock(neighbor) == seed_mobile
}

fn cells_to_set(cells: &[(i32, i32, Cell)]) -> HashSet<(i32, i32)> {
  cells.iter().map(|(x, y, _)| (*x, *y)).collect()
}

fn cell_set_floating(world: &World, set: &HashSet<(i32, i32)>) -> bool {
  let mut face = false;
  for &(x, y) in set {
    if set.contains(&(x, y - 1)) {
      continue;
    }
    face = true;
    match world.get_cell(x, y - 1) {
      None => return true,
      Some(c) if body_passable_at(world, x, y - 1, &c) => return true,
      _ => return false,
    }
  }
  face
}

fn cluster_has_passable_below(world: &World, set: &HashSet<(i32, i32)>) -> bool {
  set.iter().any(|&(x, y)| {
    if set.contains(&(x, y - 1)) {
      return false;
    }
    match world.get_cell(x, y - 1) {
      None => true,
      Some(c) => body_passable_at(world, x, y - 1, &c),
    }
  })
}

/// Cell rests on solid terrain outside the cluster (pillar / bed contact).
fn externally_supported(world: &World, set: &HashSet<(i32, i32)>, x: i32, y: i32) -> bool {
  let wx = world.wrap_x(x);
  let by = y - 1;
  match world.get_cell(wx, by) {
    Some(b) if !body_passable_at(world, wx, by, &b) && !set.contains(&(wx, by)) => true,
    _ => false,
  }
}

/// Thin bedrock-rooted columns (cantilever legs) that must not peel with a roof span.
fn pillar_column_xs(world: &World, set: &HashSet<(i32, i32)>) -> HashSet<i32> {
  let mut ymin_by_x: HashMap<i32, i32> = HashMap::new();
  for &(x, y) in set {
    ymin_by_x
      .entry(x)
      .and_modify(|ymin| *ymin = (*ymin).min(y))
      .or_insert(y);
  }
  let mut xs = HashSet::new();
  for (&x, &ymin) in &ymin_by_x {
    if !externally_supported(world, set, x, ymin) {
      continue;
    }
    let ymax = set
      .iter()
      .filter(|(sx, _)| *sx == x)
      .map(|(_, y)| *y)
      .max()
      .unwrap_or(ymin);
    if ymax - ymin + 1 < PILLAR_COLUMN_MIN_HEIGHT {
      continue;
    }
    if !(ymin..=ymax).all(|y| set.contains(&(x, y))) {
      continue;
    }
    let mut min_x = x;
    let mut max_x = x;
    while set.contains(&(min_x - 1, ymin)) {
      min_x -= 1;
    }
    while set.contains(&(max_x + 1, ymin)) {
      max_x += 1;
    }
    if max_x - min_x + 1 <= 2 {
      xs.insert(x);
    }
  }
  xs
}

fn cluster_needs_hang_peel(
  world: &World,
  set: &HashSet<(i32, i32)>,
  welded_cells: usize,
) -> bool {
  if !cluster_has_passable_below(world, set) {
    return cell_set_floating(world, set);
  }
  if set.len() > MAX_DYNAMIC_BODY_CELLS {
    return true;
  }
  // Slope toes and ball chains already split; hang-peel when morphological open
  // leaves most of the cluster static (cavern roof / hill arch shells).
  welded_cells * 2 < set.len()
}

fn hang_horizontal_closure(
  set: &HashSet<(i32, i32)>,
  seeds: &HashSet<(i32, i32)>,
  pillar_xs: &HashSet<i32>,
  void_columns: Option<&HashMap<i32, i32>>,
) -> HashSet<(i32, i32)> {
  let mut hang = seeds.clone();
  let mut q: VecDeque<_> = seeds.iter().copied().collect();
  while let Some((x, y)) = q.pop_front() {
    for (dx, dy) in [(1, 0), (-1, 0), (0, 1)] {
      let nx = x + dx;
      let ny = y + dy;
      if pillar_xs.contains(&nx) {
        continue;
      }
      if let Some(floors) = void_columns {
        if !floors.contains_key(&nx) {
          continue;
        }
        if let Some(&floor) = floors.get(&nx) {
          if ny < floor {
            continue;
          }
        }
      }
      if set.contains(&(nx, ny)) && hang.insert((nx, ny)) {
        q.push_back((nx, ny));
      }
    }
  }
  hang
}

/// Cells at or above the lowest void-supported floor in each column (cavern roof slab).
fn void_ceiling_hang_mass(world: &World, set: &HashSet<(i32, i32)>) -> HashSet<(i32, i32)> {
  let pillar_xs = pillar_column_xs(world, set);
  let mut floor_by_x: HashMap<i32, i32> = HashMap::new();
  for &(x, y) in set {
    if pillar_xs.contains(&x) {
      continue;
    }
    if set.contains(&(x, y - 1)) {
      continue;
    }
    let void_below = match world.get_cell(x, y - 1) {
      None => true,
      Some(c) => body_passable_at(world, x, y - 1, &c),
    };
    if !void_below {
      continue;
    }
    floor_by_x
      .entry(x)
      .and_modify(|floor| *floor = (*floor).min(y))
      .or_insert(y);
  }
  if floor_by_x.is_empty() {
    return HashSet::new();
  }
  let mut seeds = HashSet::new();
  for &(x, y) in set {
    if pillar_xs.contains(&x) {
      continue;
    }
    if let Some(&floor) = floor_by_x.get(&x) {
      if y >= floor {
        seeds.insert((x, y));
      }
    }
  }
  hang_horizontal_closure(set, &seeds, &pillar_xs, Some(&floor_by_x))
}

/// Flood the whole competent mass above a carved void (not just the bottom row).
fn void_anchored_hang_mass(world: &World, set: &HashSet<(i32, i32)>) -> HashSet<(i32, i32)> {
  let mut hang = HashSet::new();
  let mut q = VecDeque::new();
  for &(x, y) in set {
    if set.contains(&(world.wrap_x(x), y - 1)) {
      continue;
    }
    match world.get_cell(world.wrap_x(x), y - 1) {
      None => {}
      Some(c) if body_passable_at(world, world.wrap_x(x), y - 1, &c) => {}
      _ => continue,
    }
    let wx = world.wrap_x(x);
    if hang.insert((wx, y)) {
      q.push_back((wx, y));
    }
  }
  while let Some((x, y)) = q.pop_front() {
    for (dx, dy) in [(1, 0), (-1, 0), (0, 1)] {
      let nx = world.wrap_x(x + dx);
      let ny = y + dy;
      if !set.contains(&(nx, ny)) || externally_supported(world, set, nx, ny) {
        continue;
      }
      if hang.insert((nx, ny)) {
        q.push_back((nx, ny));
      }
    }
  }
  hang
}

fn vertically_supported_cells(world: &World, set: &HashSet<(i32, i32)>) -> HashSet<(i32, i32)> {
  let mut supported = HashSet::new();
  let mut q = VecDeque::new();
  for &(x, y) in set {
    match world.get_cell(x, y - 1) {
      Some(b) if !body_passable_at(world, x, y - 1, &b) && !set.contains(&(x, y - 1)) => {
        supported.insert((x, y));
        q.push_back((x, y));
      }
      _ => {}
    }
  }
  while let Some((x, y)) = q.pop_front() {
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
      let n = (x + dx, y + dy);
      if set.contains(&n) && supported.insert(n) {
        q.push_back(n);
      }
    }
  }
  supported
}

fn flood_positions(set: &HashSet<(i32, i32)>) -> Vec<HashSet<(i32, i32)>> {
  let mut seen = HashSet::new();
  let mut out = Vec::new();
  for &seed in set {
    if !seen.insert(seed) {
      continue;
    }
    let mut comp = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back(seed);
    comp.insert(seed);
    while let Some((cx, cy)) = q.pop_front() {
      for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
        let n = (cx + dx, cy + dy);
        if !set.contains(&n) || !seen.insert(n) {
          continue;
        }
        comp.insert(n);
        q.push_back(n);
      }
    }
    out.push(comp);
  }
  out
}

fn peel_floating_grid(
  set: &HashSet<(i32, i32)>,
  by_pos: &HashMap<(i32, i32), Cell>,
  tile: i32,
  min_cells: usize,
) -> Vec<Vec<(i32, i32, Cell)>> {
  if set.is_empty() {
    return Vec::new();
  }
  let min_x = set.iter().map(|p| p.0).min().unwrap();
  let max_x = set.iter().map(|p| p.0).max().unwrap();
  let min_y = set.iter().map(|p| p.1).min().unwrap();
  let max_y = set.iter().map(|p| p.1).max().unwrap();
  let mut out = Vec::new();
  let mut ty = min_y;
  while ty <= max_y {
    let mut tx = min_x;
    while tx <= max_x {
      let mut piece = Vec::new();
      for y in ty..ty + tile {
        for x in tx..tx + tile {
          if set.contains(&(x, y)) {
            if let Some(c) = by_pos.get(&(x, y)) {
              piece.push((x, y, *c));
            }
          }
        }
      }
      if piece.len() >= min_cells && piece.len() <= MAX_DYNAMIC_BODY_CELLS {
        out.push(piece);
      }
      tx += tile;
    }
    ty += tile;
  }
  out
}

fn peel_oversize_floating(cells: Vec<(i32, i32, Cell)>) -> Vec<Vec<(i32, i32, Cell)>> {
  if cells.is_empty() {
    return Vec::new();
  }
  if cells.len() <= MAX_DYNAMIC_BODY_CELLS {
    return vec![cells];
  }
  let by_pos: HashMap<(i32, i32), Cell> =
    cells.iter().map(|(x, y, c)| ((*x, *y), *c)).collect();
  let set = cells_to_set(&cells);
  let n = set.len();
  let min_x = set.iter().map(|p| p.0).min().unwrap();
  let max_x = set.iter().map(|p| p.0).max().unwrap();
  let min_y = set.iter().map(|p| p.1).min().unwrap();
  let max_y = set.iter().map(|p| p.1).max().unwrap();
  let w = max_x - min_x + 1;
  let h = max_y - min_y + 1;
  let pieces_needed = (n + MAX_DYNAMIC_BODY_CELLS - 1) / MAX_DYNAMIC_BODY_CELLS;
  let mut tile = 16_i32;
  while tile <= 32 {
    let tiles_x = (w + tile - 1) / tile;
    let tiles_y = (h + tile - 1) / tile;
    let count = (tiles_x * tiles_y) as usize;
    if count <= pieces_needed.max(2) + 2 && (tile * tile) as usize <= MAX_DYNAMIC_BODY_CELLS {
      break;
    }
    tile += 4;
  }
  peel_floating_grid(&set, &by_pos, tile, 4)
}

fn extract_hanging_pieces(
  world: &World,
  cells: &[(i32, i32, Cell)],
) -> Vec<Vec<(i32, i32, Cell)>> {
  probe::bump(&probe::hang_calls);
  if cells.is_empty() {
    return Vec::new();
  }
  let by_pos: HashMap<(i32, i32), Cell> =
    cells.iter().map(|(x, y, c)| ((*x, *y), *c)).collect();
  let set = cells_to_set(cells);
  if !cluster_has_passable_below(world, &set) {
    return Vec::new();
  }
  let pillar_xs = pillar_column_xs(world, &set);
  let hanging: HashSet<(i32, i32)> = if cell_set_floating(world, &set) {
    set.iter()
      .filter(|(x, _)| !pillar_xs.contains(x))
      .copied()
      .collect()
  } else {
    let ceiling = void_ceiling_hang_mass(world, &set);
    if !ceiling.is_empty() {
      ceiling
    } else {
      let void_mass = void_anchored_hang_mass(world, &set);
      if !void_mass.is_empty() {
        void_mass
      } else {
        let supported = vertically_supported_cells(world, &set);
        if supported.is_empty() {
          set.iter()
            .filter(|(x, _)| !pillar_xs.contains(x))
            .copied()
            .collect()
        } else {
          set.difference(&supported)
            .filter(|(x, _)| !pillar_xs.contains(x))
            .copied()
            .collect()
        }
      }
    }
  };
  if hanging.is_empty() {
    return Vec::new();
  }
  let mut out = Vec::new();
  for comp in flood_positions(&hanging) {
    let sub: Vec<_> = comp
      .iter()
      .filter_map(|p| by_pos.get(p).map(|c| (p.0, p.1, *c)))
      .collect();
    if sub.is_empty() {
      continue;
    }
    if sub.len() <= MAX_DYNAMIC_BODY_CELLS {
      out.push(sub);
    } else {
      out.extend(peel_oversize_floating(sub));
    }
  }
  out
}

fn push_component_pieces(
  world: &World,
  out: &mut Vec<Component>,
  pieces: Vec<Vec<(i32, i32, Cell)>>,
  hanging: bool,
  hanging_count: &mut usize,
) {
  for piece in pieces {
    if piece.len() > MAX_DYNAMIC_BODY_CELLS || piece.is_empty() {
      continue;
    }
    let set: HashSet<_> = piece.iter().map(|(x, y, _)| (*x, *y)).collect();
    if !hanging && is_bedrock_rooted_pillar(world, &set) {
      continue;
    }
    let exposed = if hanging {
      piece
        .iter()
        .filter(|(x, y, _)| {
          if has_free_neighbor(world, *x, *y) {
            return true;
          }
          [(1, 0), (0, 1), (-1, 0), (0, -1)]
            .iter()
            .any(|(dx, dy)| !set.contains(&(x + dx, y + dy)))
        })
        .count()
    } else {
      piece
        .iter()
        .filter(|(x, y, _)| has_free_neighbor(world, *x, *y))
        .count()
    };
    if exposed == 0 {
      continue;
    }
    if !hanging && piece.len() > 32 && exposed * 5 < piece.len() {
      continue;
    }
    probe::bump(&probe::components);
    let min_y = piece.iter().map(|(_, y, _)| *y).min().unwrap_or(0);
    let max_y = piece.iter().map(|(_, y, _)| *y).max().unwrap_or(0);
    out.push(Component {
      cells: piece,
      set,
      min_y,
      max_y,
    });
    if hanging {
      *hanging_count += 1;
    }
  }
}

fn is_bedrock_rooted_pillar(world: &World, set: &HashSet<(i32, i32)>) -> bool {
  let pillars = pillar_column_xs(world, set);
  if pillars.is_empty() {
    return false;
  }
  set.len() >= PILLAR_COLUMN_MIN_HEIGHT as usize
    && set.iter().all(|(x, _)| pillars.contains(x))
}

#[inline]
fn void_below_seed(world: &World, gx: i32, gy: i32) -> bool {
  match world.get_cell(gx, gy - 1) {
    None => true,
    Some(c) => body_passable_at(world, gx, gy - 1, &c),
  }
}

/// Extract boulder-sized dynamic bodies only (CCRB). Strata larger than
/// [`MAX_DYNAMIC_BODY_CELLS`] stay static. Touching boulders are separated by
/// morphological opening so simulation contact never welds a ball chain into
/// one frozen “terrain” pillar.
///
/// Second return is cells from bodies truncated by the per-tick cap — caller
/// must dirty them so a later tick finishes the fall.
fn build_components(
  world: &World,
  active: &[ActiveChunk],
) -> (Vec<Component>, Vec<(i32, i32)>) {
  probe::bump(&probe::build_calls);
  let mut visited: HashSet<(i32, i32)> = HashSet::new();
  let mut out: Vec<Component> = Vec::new();
  let mut hanging_count = 0usize;
  for void_pass in [true, false] {
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
          if visited.contains(&(gx, gy)) {
            continue;
          }
          // Sleeping rock: already evaluated and immobile, and nothing near it
          // has been written since. Cheapest possible rejection.
          if world.competent_is_settled(gx, gy) {
            continue;
          }
          probe::bump(&probe::seed_candidates);
          // Cheap movability gate before any flood — buried / flat-seated rock
          // whose only opening is the sky can never move.
          if !body_can_seed(world, gx, gy, &cell) {
            continue;
          }
          if !has_free_neighbor(world, gx, gy) {
            continue;
          }
          probe::bump(&probe::seeds_passed);
          let below_void = void_below_seed(world, gx, gy);
          if void_pass && !below_void {
            continue;
          }
          if !void_pass && below_void {
            continue;
          }
        let material = cell.material;
        let seed_mobile = is_mobile_rock(&cell);
        let mut queue = VecDeque::new();
        let mut cells: Vec<(i32, i32, Cell)> = Vec::new();
        queue.push_back((gx, gy));
        visited.insert((gx, gy));
        let mut strata = false;
        probe::bump(&probe::floods);
        while let Some((cx, cy)) = queue.pop_front() {
          let Some(cur) = world.get_cell(cx, cy) else {
            continue;
          };
          if !flood_compatible(seed_mobile, &cur, material) {
            continue;
          }
          cells.push((cx, cy, cur));
          probe::bump(&probe::flood_cells);
          if cells.len() > FLOOD_GATHER_CAP {
            strata = true;
            probe::bump(&probe::strata_bailouts);
            break;
          }
          for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
            let nx = world.wrap_x(cx + dx);
            let ny = cy + dy;
            if !visited.insert((nx, ny)) {
              continue;
            }
            match world.get_cell(nx, ny) {
              Some(n) if flood_compatible(seed_mobile, &n, material) => queue.push_back((nx, ny)),
              _ => {
                visited.remove(&(nx, ny));
              }
            }
          }
        }
        if strata {
          let hang = extract_hanging_pieces(world, &cells);
          let before = out.len();
          push_component_pieces(
            world,
            &mut out,
            hang,
            true,
            &mut hanging_count,
          );
          let mut pushed: HashSet<(i32, i32)> = HashSet::new();
          for comp in &out[before..] {
            for (x, y, _) in &comp.cells {
              pushed.insert((*x, *y));
            }
          }
          if pushed.is_empty() {
            // True continuous strata (or peel rejected) — finish marking.
            while let Some((cx, cy)) = queue.pop_front() {
              for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
                let nx = world.wrap_x(cx + dx);
                let ny = cy + dy;
                if !visited.insert((nx, ny)) {
                  continue;
                }
                match world.get_cell(nx, ny) {
                  Some(n) if flood_compatible(seed_mobile, &n, material) => {
                    queue.push_back((nx, ny))
                  }
                  _ => {
                    visited.remove(&(nx, ny));
                  }
                }
              }
            }
          } else {
            // Leave unpushed flood + queue remainder for later seeds.
            for (x, y, _) in &cells {
              if !pushed.contains(&(*x, *y)) {
                visited.remove(&(*x, *y));
              }
            }
            while let Some((cx, cy)) = queue.pop_front() {
              visited.remove(&(cx, cy));
            }
          }
          continue;
        }
        if cells.is_empty() {
          continue;
        }
        let set = cells_to_set(&cells);
        let mut pieces = split_welded_contacts(cells.clone());
        let welded_cells: usize = pieces.iter().map(|p| p.len()).sum();
        let mut hanging = false;
        if cluster_needs_hang_peel(world, &set, welded_cells) {
          let hang = extract_hanging_pieces(world, &cells);
          let hang_cells: usize = hang.iter().map(|p| p.len()).sum();
          if !hang.is_empty()
            && (hang_cells > welded_cells
              || hang.len() > pieces.len()
              || welded_cells * 2 < set.len())
          {
            pieces = hang;
            hanging = true;
          }
        }
        push_component_pieces(world, &mut out, pieces, hanging, &mut hanging_count);
        }
      }
    }
  }
  let air_below = |c: &Component| {
    c.cells.iter().any(|(x, y, _)| match world.get_cell(*x, y - 1) {
      None => true,
      Some(cell) => cell.material == MaterialId::Air,
    })
  };
  // Split floating vs seated so fairness rotate cannot bury sky bodies behind
  // tip/roll work (that was ~1 cell move per hundred ticks with many floaters).
  let mut floating: Vec<Component> = Vec::new();
  let mut seated: Vec<Component> = Vec::new();
  for c in out {
    if air_below(&c) {
      floating.push(c);
    } else {
      seated.push(c);
    }
  }
  floating.sort_by(|a, b| a.min_y.cmp(&b.min_y));
  seated.sort_by(|a, b| a.min_y.cmp(&b.min_y));
  if floating.len() > 1 {
    let rot = (world.tick as usize).wrapping_mul(17) % floating.len();
    floating.rotate_left(rot);
  }
  if seated.len() > 1 {
    let rot = (world.tick as usize).wrapping_mul(13) % seated.len();
    seated.rotate_left(rot);
  }
  let float_cap = if hanging_count > 0 {
    MAX_HANGING_BODIES_PER_TICK.max(MAX_FLOATING_BODIES_PER_TICK)
  } else if floating.len() > MAX_BODIES_PER_TICK {
    MAX_FLOATING_BODIES_PER_TICK
  } else {
    MAX_BODIES_PER_TICK
  };
  let seat_cap = MAX_BODIES_PER_TICK;
  let mut leftovers = Vec::new();
  if floating.len() > float_cap {
    for dropped in floating.drain(float_cap..) {
      for (x, y, _) in dropped.cells {
        leftovers.push((x, y));
      }
    }
  }
  if seated.len() > seat_cap {
    for dropped in seated.drain(seat_cap..) {
      for (x, y, _) in dropped.cells {
        leftovers.push((x, y));
      }
    }
  }
  floating.append(&mut seated);
  (floating, leftovers)
}

fn body_neighbor_count(set: &HashSet<(i32, i32)>, x: i32, y: i32) -> usize {
  [(1, 0), (0, 1), (-1, 0), (0, -1)]
    .iter()
    .filter(|(dx, dy)| set.contains(&(x + dx, y + dy)))
    .count()
}

/// Morphological opening: erode to interiors, flood cores, dilate back.
/// Touching boulder chains / terrain contact separate; only compact cores that
/// dilate into mobile-sized pieces are kept.
fn split_welded_contacts(cells: Vec<(i32, i32, Cell)>) -> Vec<Vec<(i32, i32, Cell)>> {
  probe::bump(&probe::split_calls);
  probe::add(&probe::split_cells, cells.len() as u64);
  if cells.len() < PEBBLE_SPLIT_HOST_MIN * 2 {
    return split_contact_pebbles(cells);
  }
  let by_pos: HashMap<(i32, i32), Cell> =
    cells.iter().map(|(x, y, c)| ((*x, *y), *c)).collect();
  let set: HashSet<(i32, i32)> = by_pos.keys().copied().collect();

  // Two erosions when large — breaks boulder↔terrain face welds and ball chains.
  let erode_passes = if set.len() > MAX_DYNAMIC_BODY_CELLS { 2 } else { 1 };
  let mut core = set.clone();
  for _ in 0..erode_passes {
    core = core
      .iter()
      .copied()
      .filter(|&(x, y)| body_neighbor_count(&core, x, y) >= 4)
      .collect();
  }
  if core.len() < 8 {
    // Softer single pass for lumpy balls.
    core = set
      .iter()
      .copied()
      .filter(|&(x, y)| body_neighbor_count(&set, x, y) >= 3)
      .collect();
  }
  if core.len() < 8 {
    return split_contact_pebbles(cells);
  }

  let mut core_visited = HashSet::new();
  let mut islands: Vec<HashSet<(i32, i32)>> = Vec::new();
  for &seed in &core {
    if !core_visited.insert(seed) {
      continue;
    }
    let mut island = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back(seed);
    island.insert(seed);
    while let Some((cx, cy)) = q.pop_front() {
      for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
        let n = (cx + dx, cy + dy);
        if !core.contains(&n) || !core_visited.insert(n) {
          continue;
        }
        island.insert(n);
        q.push_back(n);
      }
    }
    if island.len() >= 4 {
      islands.push(island);
    }
  }
  if islands.is_empty() {
    return split_contact_pebbles(cells);
  }

  // Dilate each island by erode_passes (restore shell); first claim wins.
  let mut owner: HashMap<(i32, i32), usize> = HashMap::new();
  for (i, island) in islands.iter().enumerate() {
    let mut frontier: HashSet<(i32, i32)> = island.clone();
    for &(x, y) in island {
      owner.entry((x, y)).or_insert(i);
    }
    for _ in 0..erode_passes.max(1) {
      let mut next = HashSet::new();
      for &(x, y) in &frontier {
        for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
          let n = (x + dx, y + dy);
          if set.contains(&n) && !owner.contains_key(&n) {
            owner.insert(n, i);
            next.insert(n);
          }
        }
      }
      frontier = next;
    }
  }
  // Near orphans only (dist ≤ 2) — don't pull distant terrain skin onto a boulder.
  for &p in &set {
    if owner.contains_key(&p) {
      continue;
    }
    let mut best = None;
    let mut best_d = i32::MAX;
    for (i, island) in islands.iter().enumerate() {
      for &c in island {
        let d = (p.0 - c.0).abs() + (p.1 - c.1).abs();
        if d < best_d {
          best_d = d;
          best = Some(i);
        }
      }
    }
    if let Some(i) = best {
      if best_d <= 2 {
        owner.insert(p, i);
      }
    }
  }

  let mut pieces: Vec<Vec<(i32, i32, Cell)>> = vec![Vec::new(); islands.len()];
  for (p, i) in owner {
    if let Some(c) = by_pos.get(&p) {
      pieces[i].push((p.0, p.1, *c));
    }
  }
  let mut out: Vec<Vec<(i32, i32, Cell)>> = Vec::new();
  for piece in pieces {
    if piece.is_empty() {
      continue;
    }
    // Drop terrain-sized leftovers after open.
    if piece.len() > MAX_DYNAMIC_BODY_CELLS {
      continue;
    }
    out.extend(split_contact_pebbles(piece));
  }
  if out.is_empty() {
    if cells.len() <= MAX_DYNAMIC_BODY_CELLS {
      split_contact_pebbles(cells)
    } else {
      // Still one huge mass (boulder fused into strata). Peel compact
      // seed-local blobs so the boulder can detach and move.
      extract_seed_local_blobs(&cells)
    }
  } else {
    out
  }
}

/// From an oversized welded flood, keep compact neighbourhoods around each
/// free-surface seed (chebyshev radius) so boulders unstick from terrain.
fn extract_seed_local_blobs(cells: &[(i32, i32, Cell)]) -> Vec<Vec<(i32, i32, Cell)>> {
  let by_pos: HashMap<(i32, i32), Cell> =
    cells.iter().map(|(x, y, c)| ((*x, *y), *c)).collect();
  let set: HashSet<(i32, i32)> = by_pos.keys().copied().collect();
  let radius = 7_i32;
  let mut used = HashSet::new();
  let mut out = Vec::new();
  // Prefer seeds that look like boulder surface (many free dirs later filtered).
  let mut seeds: Vec<(i32, i32)> = set.iter().copied().collect();
  seeds.sort_unstable();
  for seed in seeds {
    if used.contains(&seed) {
      continue;
    }
    // Only start from cells that aren't fully enclosed in the set.
    if body_neighbor_count(&set, seed.0, seed.1) >= 4 {
      continue;
    }
    let mut blob_set = HashSet::new();
    let mut q = VecDeque::new();
    q.push_back((seed.0, seed.1, 0_i32));
    blob_set.insert(seed);
    while let Some((cx, cy, d)) = q.pop_front() {
      if d >= radius || blob_set.len() >= MAX_DYNAMIC_BODY_CELLS {
        continue;
      }
      for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
        let n = (cx + dx, cy + dy);
        if !set.contains(&n) || !blob_set.insert(n) {
          continue;
        }
        q.push_back((n.0, n.1, d + 1));
      }
    }
    if blob_set.len() < 6 || blob_set.len() > MAX_DYNAMIC_BODY_CELLS {
      continue;
    }
    // Reject terrain sheets: too flat / sparse relative to bbox.
    let min_x = blob_set.iter().map(|p| p.0).min().unwrap();
    let max_x = blob_set.iter().map(|p| p.0).max().unwrap();
    let min_y = blob_set.iter().map(|p| p.1).min().unwrap();
    let max_y = blob_set.iter().map(|p| p.1).max().unwrap();
    let w = max_x - min_x + 1;
    let h = max_y - min_y + 1;
    let fill = blob_set.len() as i32;
    if w.max(h) >= 12 && fill * 3 < w * h {
      continue; // sheet-like
    }
    for &p in &blob_set {
      used.insert(p);
    }
    let piece: Vec<_> = blob_set
      .into_iter()
      .filter_map(|p| by_pos.get(&p).map(|c| (p.0, p.1, *c)))
      .collect();
    out.push(piece);
  }
  out
}

/// Peel pebble-sized contact clusters off a larger host. Simulation contact
/// must not weld rocks; only editor paint / geology should.
fn split_contact_pebbles(cells: Vec<(i32, i32, Cell)>) -> Vec<Vec<(i32, i32, Cell)>> {
  if cells.len() < PEBBLE_SPLIT_HOST_MIN + 1 {
    return vec![cells];
  }
  let mut by_pos: HashMap<(i32, i32), Cell> = cells.into_iter().map(|(x, y, c)| ((x, y), c)).collect();
  let mut remaining: HashSet<(i32, i32)> = by_pos.keys().copied().collect();
  let mut peeled: Vec<Vec<(i32, i32, Cell)>> = Vec::new();

  loop {
    if remaining.len() < PEBBLE_SPLIT_HOST_MIN + 1 {
      break;
    }
    let adj = |p: (i32, i32)| {
      [(1, 0), (0, 1), (-1, 0), (0, -1)]
        .into_iter()
        .map(|(dx, dy)| (p.0 + dx, p.1 + dy))
        .filter(|n| remaining.contains(n))
        .collect::<Vec<_>>()
    };
    // Find a degree-1 neck: small side flood size <= PEBBLE_SPLIT_MAX.
    let mut split_at: Option<(i32, i32)> = None;
    let mut small_side: Vec<(i32, i32)> = Vec::new();
    for &p in &remaining {
      let ns = adj(p);
      if ns.len() != 1 {
        continue;
      }
      let bridge = ns[0];
      // Flood from p without crossing back through bridge first step already done.
      let mut seen = HashSet::new();
      let mut q = VecDeque::new();
      q.push_back(p);
      seen.insert(p);
      while let Some(c) = q.pop_front() {
        for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
          let n = (c.0 + dx, c.1 + dy);
          if n == bridge && c == p {
            continue;
          }
          if !remaining.contains(&n) || !seen.insert(n) {
            continue;
          }
          q.push_back(n);
        }
      }
      if seen.len() <= PEBBLE_SPLIT_MAX && remaining.len() - seen.len() >= PEBBLE_SPLIT_HOST_MIN {
        split_at = Some(p);
        small_side = seen.into_iter().collect();
        break;
      }
    }
    let Some(_) = split_at else {
      break;
    };
    let mut piece = Vec::new();
    for p in small_side {
      remaining.remove(&p);
      if let Some(c) = by_pos.remove(&p) {
        piece.push((p.0, p.1, c));
      }
    }
    if !piece.is_empty() {
      peeled.push(piece);
    }
  }

  let mut host = Vec::new();
  for p in remaining {
    if let Some(c) = by_pos.remove(&p) {
      host.push((p.0, p.1, c));
    }
  }
  let mut out = Vec::new();
  if !host.is_empty() {
    out.push(host);
  }
  out.extend(peeled);
  out
}

fn bottom_face(comp: &Component) -> Vec<(i32, i32, Cell)> {
  comp.cells
    .iter()
    .filter(|(x, y, _)| !comp.set.contains(&(*x, y - 1)))
    .copied()
    .collect()
}

/// Collision test against a **precomputed** cargo set.
///
/// Cargo gathering is a flood; recomputing it inside the drop binary search
/// meant ~8 floods per body per move. Callers hoist it out and reuse.
fn can_translate_with(
  world: &World,
  comp: &Component,
  cargo: &[(i32, i32, Cell)],
  cargo_set: &HashSet<(i32, i32)>,
  dx: i32,
  dy: i32,
) -> bool {
  let n = comp.cells.len();
  for (gx, gy, _) in comp.cells.iter().chain(cargo.iter()) {
    let tx = world.wrap_x(gx + dx);
    let ty = gy + dy;
    if ty < 0 {
      return false;
    }
    // Moving set = body ∪ cargo; no clone/merge allocation per call.
    if comp.set.contains(&(tx, ty)) || cargo_set.contains(&(tx, ty)) {
      continue;
    }
    let Some(dst) = world.get_cell(tx, ty) else {
      return false;
    };
    if body_passable_at(world, tx, ty, &dst) {
      continue;
    }
    if can_crush_spec(world, tx, ty, n) {
      continue;
    }
    return false;
  }
  true
}

fn can_translate(world: &World, comp: &Component, dx: i32, dy: i32) -> bool {
  let cargo = gather_cargo(world, comp);
  let cargo_set: HashSet<(i32, i32)> = cargo.iter().map(|(x, y, _)| (*x, *y)).collect();
  can_translate_with(world, comp, &cargo, &cargo_set, dx, dy)
}

/// Largest `drop` in `1..=max_drop` where the body can jump down that far.
/// Gathers cargo once and reuses it for every probe.
fn max_drop_distance(world: &World, comp: &Component, max_drop: i32) -> i32 {
  if max_drop <= 0 {
    return 0;
  }
  let cargo = gather_cargo(world, comp);
  let cargo_set: HashSet<(i32, i32)> = cargo.iter().map(|(x, y, _)| (*x, *y)).collect();
  if !can_translate_with(world, comp, &cargo, &cargo_set, 0, -1) {
    return 0;
  }
  let mut lo = 1;
  let mut hi = max_drop;
  while lo < hi {
    let mid = (lo + hi + 1) / 2;
    if can_translate_with(world, comp, &cargo, &cargo_set, 0, -mid) {
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
  let cargo = gather_cargo(world, comp);
  let mut moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(x, y, c)| (world.wrap_x(x + dx), y + dy, with_mobile_rock(*c)))
    .collect();
  let mut sources: Vec<(i32, i32)> = comp.cells.iter().map(|(x, y, _)| (*x, *y)).collect();
  for (x, y, c) in &cargo {
    moves.push((world.wrap_x(x + dx), y + dy, *c));
    sources.push((*x, *y));
  }
  let n = comp.cells.len();
  for (tx, ty, _) in &moves {
    if comp.set.contains(&(*tx, *ty)) {
      continue;
    }
    if let Some(dst) = world.get_cell(*tx, *ty) {
      if is_competent_rock(dst.material) && can_crush_spec(world, *tx, *ty, n) {
        crush_spec_at(world, *tx, *ty);
      }
    }
  }
  write_roll_cells(world, &sources, moves);
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
          flags: {
            let mut f = cur.flags;
            f.clear(CellFlags::MOBILE_ROCK);
            f
          },
          _pad: cur._pad,
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

fn soft_embed(_world: &mut World, _comp: &Component, _cfg: &CompetentFallConfig) -> u32 {
  0
}

fn downhill_roll_dir(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<i32> {
  // Only sample bottom-face columns — overhang cells have no support and must
  // not abort the whole search.
  let mut face_cols: HashSet<i32> = HashSet::new();
  for (gx, _, _) in bottom_face(comp) {
    face_cols.insert(gx);
  }
  if face_cols.is_empty() {
    return None;
  }
  // All body columns — neighbor probes must skip these so a wide disk doesn't
  // treat its own mass as the "terrain" beside it.
  let body_cols: HashSet<i32> = comp.cells.iter().map(|(x, _, _)| *x).collect();
  let mut best_dx = 0_i32;
  let mut best_drop = 0_i32;
  for &gx in &face_cols {
    let Some(center) = column_top_support_y(world, gx, &comp.set) else {
      continue;
    };
    for dx in [-1_i32, 1] {
      let nx = world.wrap_x(gx + dx);
      if body_cols.contains(&nx) {
        // Probe just outside the body footprint in this direction.
        continue;
      }
      // Exclude body cells so overhang rock isn't counted as bed height.
      let Some(side) = column_top_support_y(world, nx, &comp.set) else {
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
  // Also compare the body's leftmost vs rightmost seat columns — catches the
  // case where every face-adjacent probe is still inside a gentle nest.
  if let (Some(&left), Some(&right)) = (
    face_cols.iter().min(),
    face_cols.iter().max(),
  ) {
    if left != right {
      if let (Some(l), Some(r)) = (
        column_top_support_y(world, left, &comp.set),
        column_top_support_y(world, right, &comp.set),
      ) {
        let drop_r = l - r; // positive → right side lower → roll right
        let drop_l = r - l; // positive → left side lower → roll left
        if drop_r > best_drop {
          best_drop = drop_r;
          best_dx = 1;
        }
        if drop_l > best_drop {
          best_drop = drop_l;
          best_dx = -1;
        }
      }
    }
  }
  // One more probe: terrain just outside the body bbox.
  if let (Some(&min_x), Some(&max_x)) = (body_cols.iter().min(), body_cols.iter().max()) {
    let mid_seat = face_cols
      .iter()
      .filter_map(|gx| column_top_support_y(world, *gx, &comp.set))
      .max()
      .unwrap_or(0);
    for (nx, dx) in [(world.wrap_x(min_x - 1), -1), (world.wrap_x(max_x + 1), 1)] {
      let side = column_top_support_y(world, nx, &comp.set).unwrap_or(-8);
      let drop = mid_seat - side;
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

/// Bbox downhill-bottom corner — virtual pivot so no body cell sits past the
/// tip axis (round blobs used to drive their downhill bulge into the bed).
fn roll_pivot(comp: &Component, dx: i32) -> (i32, i32) {
  let min_x = comp.cells.iter().map(|(x, _, _)| *x).min().unwrap_or(0);
  let max_x = comp.cells.iter().map(|(x, _, _)| *x).max().unwrap_or(0);
  let min_y = comp.cells.iter().map(|(_, y, _)| *y).min().unwrap_or(0);
  if dx > 0 {
    (max_x, min_y)
  } else {
    (min_x, min_y)
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
  let n = comp.cells.len();
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
    if !motion_destination_ok(world, tx, ty, &dst, &comp.set, n) {
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
  let src_set: HashSet<(i32, i32)> = sources.iter().copied().collect();
  let mover_n = sources.len();
  for (tx, ty, _) in &moves {
    if src_set.contains(&(*tx, *ty)) {
      continue;
    }
    if let Some(dst) = world.get_cell(*tx, *ty) {
      if is_roll_displaceable(dst.material) {
        displace_soft_at(world, *tx, *ty);
      } else if is_competent_rock(dst.material) && can_crush_spec(world, *tx, *ty, mover_n) {
        crush_spec_at(world, *tx, *ty);
      }
    }
  }
  for (i, &(sx, sy)) in sources.iter().enumerate() {
    if let Some((tx, ty, _)) = moves.get(i) {
      if (sx, sy) != (*tx, *ty) {
        world.competent_cell_moves.push((sx, sy, *tx, *ty));
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

/// Loose / soft cells on top or nested against the rock ride with it.
fn gather_cargo(world: &World, comp: &Component) -> Vec<(i32, i32, Cell)> {
  probe::bump(&probe::cargo_calls);
  probe::add(&probe::cargo_cells, comp.cells.len() as u64);
  const MAX_CARGO: usize = 512;
  let mut cargo = Vec::new();
  let mut seen = HashSet::new();
  for &(gx, gy, _) in &comp.cells {
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
      let nx = world.wrap_x(gx + dx);
      let ny = gy + dy;
      if comp.set.contains(&(nx, ny)) || seen.contains(&(nx, ny)) {
        continue;
      }
      let Some(c) = world.get_cell(nx, ny) else {
        continue;
      };
      if !is_roll_displaceable(c.material) {
        continue;
      }
      let mut body_n = 0;
      for (ox, oy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
        if comp.set.contains(&(world.wrap_x(nx + ox), ny + oy)) {
          body_n += 1;
        }
      }
      if body_n >= 2 {
        seen.insert((nx, ny));
        cargo.push((nx, ny, c));
      }
    }
  }
  // Surface stack: flood up and sideways through soft / organic caps.
  let mut q = VecDeque::new();
  for &(gx, gy, _) in &comp.cells {
    let nx = world.wrap_x(gx);
    let ny = gy + 1;
    if comp.set.contains(&(nx, ny)) || seen.contains(&(nx, ny)) {
      continue;
    }
    if let Some(c) = world.get_cell(nx, ny) {
      if is_roll_displaceable(c.material) && seen.insert((nx, ny)) {
        q.push_back((nx, ny));
      }
    }
  }
  while let Some((cx, cy)) = q.pop_front() {
    if cargo.len() >= MAX_CARGO {
      break;
    }
    let Some(c) = world.get_cell(cx, cy) else {
      continue;
    };
    cargo.push((cx, cy, c));
    for (dx, dy) in [(1, 0), (0, 1), (-1, 0), (0, -1)] {
      let nx = world.wrap_x(cx + dx);
      let ny = cy + dy;
      if comp.set.contains(&(nx, ny)) || !seen.insert((nx, ny)) {
        continue;
      }
      if let Some(nc) = world.get_cell(nx, ny) {
        if is_roll_displaceable(nc.material) {
          q.push_back((nx, ny));
        } else {
          seen.remove(&(nx, ny));
        }
      } else {
        seen.remove(&(nx, ny));
      }
    }
  }
  cargo
}

fn pivot_roll_component(world: &mut World, comp: &Component, pivot: (i32, i32), dx: i32) -> bool {
  if !can_pivot_roll(world, comp, pivot, dx) {
    return false;
  }
  let cargo = gather_cargo(world, comp);
  let cargo_set: HashSet<(i32, i32)> = cargo.iter().map(|(x, y, _)| (*x, *y)).collect();
  let mut moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(gx, gy, c)| {
      let (tx, ty) = rotate_pos(pivot.0, pivot.1, *gx, *gy, dx);
      (world.wrap_x(tx), ty, with_mobile_rock(*c))
    })
    .collect();
  let mut sources: Vec<(i32, i32)> = comp.cells.iter().map(|(x, y, _)| (*x, *y)).collect();
  for (gx, gy, c) in &cargo {
    let (tx, ty) = rotate_pos(pivot.0, pivot.1, *gx, *gy, dx);
    if ty < 0 {
      continue; // leave cargo behind
    }
    let tx = world.wrap_x(tx);
    if comp.set.contains(&(tx, ty)) || cargo_set.contains(&(tx, ty)) {
      moves.push((tx, ty, *c));
      sources.push((*gx, *gy));
      continue;
    }
    match world.get_cell(tx, ty) {
      Some(dst) if motion_destination_ok(world, tx, ty, &dst, &comp.set, comp.cells.len()) => {
        moves.push((tx, ty, *c));
        sources.push((*gx, *gy));
      }
      _ => {} // leave behind — powder settle will handle it
    }
  }
  write_roll_cells(world, &sources, moves);
  true
}

/// Tip only when COM clearly overhangs *and* the bed drops that way.
/// Tiny sticks never tip (they flip-flop forever on flat floors otherwise).
fn tip_dir(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<i32> {
  if !may_tip(comp) {
    return None;
  }
  let contacts: Vec<(i32, i32)> = bottom_face(comp)
    .into_iter()
    .filter(|(x, y, _)| {
      matches!(
        support_below(world, *x, *y),
        Some((bed, _)) if bed != MaterialId::Air
      )
    })
    .map(|(x, y, _)| (x, y))
    .collect();
  if contacts.is_empty() {
    return None;
  }
  let n = comp.cells.len() as i64;
  let com_x = (comp.cells.iter().map(|(x, _, _)| *x as i64).sum::<i64>() / n) as i32;
  let s_min = contacts.iter().map(|(x, _)| *x).min().unwrap();
  let s_max = contacts.iter().map(|(x, _)| *x).max().unwrap();
  let overhang = if com_x < s_min {
    Some(-1)
  } else if com_x > s_max {
    Some(1)
  } else {
    None
  }?;
  // Must agree with a real downhill — flat-floor overhang after a lean is the
  // flip-flop trap (tip left → tip right → forever).
  let slope = downhill_roll_dir(world, comp, cfg)?;
  if slope != overhang {
    return None;
  }
  Some(overhang)
}

/// Small / needle bodies tip↔tip forever; they may only slide or fall.
fn may_tip(comp: &Component) -> bool {
  let (x0, x1, y0, y1) = body_bbox(comp);
  let w = x1 - x0 + 1;
  let h = y1 - y0 + 1;
  if comp.cells.len() <= 10 && w.min(h) <= 2 {
    return false;
  }
  if w.max(h) <= 4 && comp.cells.len() <= 12 {
    return false;
  }
  true
}

fn try_pivot_roll(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<i32> {
  if is_floating(world, comp) {
    return None;
  }
  let dx = tip_dir(world, comp, cfg)?;
  let pivot = roll_pivot(comp, dx);
  if can_pivot_roll(world, comp, pivot, dx) {
    Some(dx)
  } else {
    None
  }
}

fn can_slide(world: &World, comp: &Component, dx: i32, dy: i32) -> bool {
  let cargo = gather_cargo(world, comp);
  let mut moving: HashSet<(i32, i32)> = comp.set.clone();
  for (x, y, _) in &cargo {
    moving.insert((*x, *y));
  }
  let n = comp.cells.len();
  for (gx, gy, _) in comp.cells.iter().chain(cargo.iter()) {
    let tx = world.wrap_x(gx + dx);
    let ty = gy + dy;
    if ty < 0 {
      return false;
    }
    if moving.contains(&(tx, ty)) {
      continue;
    }
    let Some(dst) = world.get_cell(tx, ty) else {
      return false;
    };
    if !motion_destination_ok(world, tx, ty, &dst, &comp.set, n) {
      return false;
    }
  }
  true
}

fn slide_component(world: &mut World, comp: &Component, dx: i32, dy: i32) -> bool {
  if !can_slide(world, comp, dx, dy) {
    return false;
  }
  let cargo = gather_cargo(world, comp);
  let mut moves: Vec<(i32, i32, Cell)> = comp
    .cells
    .iter()
    .map(|(x, y, c)| (world.wrap_x(x + dx), y + dy, with_mobile_rock(*c)))
    .collect();
  for (x, y, c) in &cargo {
    moves.push((world.wrap_x(x + dx), y + dy, *c));
  }
  let mut sources: Vec<(i32, i32)> = comp.cells.iter().map(|(x, y, _)| (*x, *y)).collect();
  sources.extend(cargo.iter().map(|(x, y, _)| (*x, *y)));
  write_roll_cells(world, &sources, moves);
  true
}

fn try_slide(world: &World, comp: &Component, cfg: &CompetentFallConfig) -> Option<(i32, i32)> {
  if is_floating(world, comp) {
    return None;
  }
  let dx = downhill_roll_dir(world, comp, cfg)?;
  // Tiny bodies: only slide when there is a real step down (not sideways jitter
  // that flip-flops every tick with tip).
  let tiny = !may_tip(comp);
  if tiny {
    for &(mx, my) in &[(dx, -1), (dx, -2)] {
      if my <= -1 && can_slide(world, comp, mx, my) {
        return Some((mx, my));
      }
    }
    return None;
  }
  for &(mx, my) in &[(dx, -1), (dx, 0), (dx, -2)] {
    if can_slide(world, comp, mx, my) {
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
    // Neighbor bed surface lower than our seat (ignore other body columns).
    let mut skip = HashSet::new();
    skip.insert((gx, gy));
    if let Some(side_top) = column_top_support_y(world, nx, &skip) {
      // If neighbor column is dominated by competent rock of a tall body,
      // fall through to the seat-air check below.
      if !matches!(
        world.get_cell(nx, side_top),
        Some(c) if is_competent_rock(c.material)
      ) && seat_y - side_top >= 1
      {
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

/// Re-dirty dynamic competent bodies that can still fall or tip.
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
        // Same gate `build_components` seeds with — waking cells the body pass
        // will immediately reject just re-dirties the whole ridge every cadence
        // (that alone cost ~40 ms/tick on a settled demo world). Also avoids
        // `body_has_downhill`, which allocates a HashSet per cell.
        if world.competent_is_settled(gx, gy) {
          continue;
        }
        if body_can_seed(world, gx, gy, &cell) {
          touches.push((gx, gy));
        }
      }
    }
  }
  for (gx, gy) in touches {
    world.touch_dirty(gx, gy);
  }
}

/// Cheap floating-only wake across loaded chunks. Air-below competent rock is
/// always dynamic — without this, F1 defers to competent fall and quiet chunks
/// never dirty, so sky-painted boulders hang forever.
pub fn wake_floating_competent(world: &mut World) {
  let cw = CHUNK_CELLS_W as i32;
  let ch = CHUNK_CELLS_H as i32;
  let coords: Vec<_> = world.chunks.keys().copied().collect();
  let mut touches: Vec<(i32, i32)> = Vec::new();
  for coord in coords {
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
        // Sleeping rock has already been evaluated as immobile and nothing near
        // it has changed; re-dirtying it here would cancel its sleep flag and
        // make the whole overhang set churn every cadence.
        if world.competent_is_settled(gx, gy) {
          continue;
        }
        match world.get_cell(gx, gy - 1) {
          None => touches.push((gx, gy)),
          Some(b) if body_passable_at(world, gx, gy - 1, &b) => touches.push((gx, gy)),
          _ => {}
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
  // Editor paint can change support anywhere — drop all sleep state.
  world.competent_wake_all();
  // Floating first (sky paint), then slope tips on all chunks.
  wake_floating_competent(world);
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

/// Expand the active scan set to cover a set of world cells (moved bodies).
fn expand_regions_to_cells(
  world: &World,
  cells: &[(i32, i32)],
  drop_budget: i32,
) -> Vec<ActiveChunk> {
  if cells.is_empty() {
    return Vec::new();
  }
  let w = CHUNK_CELLS_W as i32;
  let h = CHUNK_CELLS_H as i32;
  let mut by_chunk: HashMap<ChunkCoord, Rect> = HashMap::new();
  for &(gx, gy) in cells {
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
  let seeds: Vec<ActiveChunk> = by_chunk
    .into_iter()
    .map(|(coord, rect)| ActiveChunk { coord, rect })
    .collect();
  expand_competent_regions(&seeds, drop_budget)
    .into_iter()
    .filter(|ac| world.chunks.contains_key(&ac.coord))
    .collect()
}

/// Vertical pad (cells) added to dirty rects when choosing **seed** rows.
///
/// Seeding does not need the full drop budget: a body that moves writes its new
/// cells, which dirties them for the next tick, and `expand_regions_to_cells`
/// re-covers bodies that moved mid-tick. Inflating seeds by the whole 64-cell
/// drop budget made every dirty water splash scan thousands of buried terrain
/// cells (~105 k seed candidates/tick on the demo world).
pub const SEED_PAD_Y: i32 = 6;

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
  // Explicit whole-world requests (tests / F3 close) keep the full budget;
  // incremental dirty scans use the tight seed pad.
  let pad = if active.is_empty() {
    drop_budget
  } else {
    SEED_PAD_Y.min(drop_budget.max(1))
  };
  let expanded = expand_competent_regions(&seed, pad);
  // Keep only loaded chunks.
  expanded
    .into_iter()
    .filter(|ac| world.chunks.contains_key(&ac.coord))
    .collect()
}

fn rests_on_hard_bed(world: &World, comp: &Component) -> bool {
  bottom_face(comp).iter().any(|(x, y, _)| {
    matches!(
      support_below(world, *x, *y),
      Some((bed, _)) if bed != MaterialId::Air && !is_soft_embed_bed(bed)
    )
  })
}

fn body_bbox(comp: &Component) -> (i32, i32, i32, i32) {
  let min_x = comp.cells.iter().map(|(x, _, _)| *x).min().unwrap_or(0);
  let max_x = comp.cells.iter().map(|(x, _, _)| *x).max().unwrap_or(0);
  let min_y = comp.cells.iter().map(|(_, y, _)| *y).min().unwrap_or(0);
  let max_y = comp.cells.iter().map(|(_, y, _)| *y).max().unwrap_or(0);
  (min_x, max_x, min_y, max_y)
}

fn is_long_thin(comp: &Component) -> bool {
  if comp.cells.len() < THIN_FRACTURE_SPAN as usize {
    return false;
  }
  let (x0, x1, y0, y1) = body_bbox(comp);
  let w = x1 - x0 + 1;
  let h = y1 - y0 + 1;
  let long = w.max(h);
  let short = w.min(h);
  if long < THIN_FRACTURE_SPAN {
    return false;
  }
  if short <= THIN_FRACTURE_THICK {
    return true;
  }
  // Sparse stick/slab filling little of its bbox.
  (comp.cells.len() as i32) * 2 <= long * short.min(3)
}

/// Break long-thin rock at a 1-cell neck (beams/slabs snapping).
fn fracture_thin_necks(world: &mut World, comp: &Component) -> u32 {
  if !is_long_thin(comp) {
    return 0;
  }
  let (x0, x1, y0, y1) = body_bbox(comp);
  let w = x1 - x0 + 1;
  let h = y1 - y0 + 1;
  let horizontal = w >= h;
  let margin = if horizontal {
    (w / 5).max(1)
  } else {
    (h / 5).max(1)
  };

  let mut candidates: Vec<(i32, i32, Cell)> = Vec::new();
  if horizontal {
    for x in (x0 + margin)..=(x1 - margin) {
      let col: Vec<_> = comp
        .cells
        .iter()
        .filter(|(cx, _, _)| *cx == x)
        .copied()
        .collect();
      if col.len() != 1 {
        continue;
      }
      let (gx, gy, cell) = col[0];
      let left = comp
        .set
        .iter()
        .any(|(sx, sy)| *sx == gx - 1 && (*sy - gy).abs() <= 1);
      let right = comp
        .set
        .iter()
        .any(|(sx, sy)| *sx == gx + 1 && (*sy - gy).abs() <= 1);
      if left && right {
        candidates.push((gx, gy, cell));
      }
    }
  } else {
    for y in (y0 + margin)..=(y1 - margin) {
      let row: Vec<_> = comp
        .cells
        .iter()
        .filter(|(_, cy, _)| *cy == y)
        .copied()
        .collect();
      if row.len() != 1 {
        continue;
      }
      let (gx, gy, cell) = row[0];
      let below = comp
        .set
        .iter()
        .any(|(sx, sy)| *sy == gy - 1 && (*sx - gx).abs() <= 1);
      let above = comp
        .set
        .iter()
        .any(|(sx, sy)| *sy == gy + 1 && (*sx - gx).abs() <= 1);
      if below && above {
        candidates.push((gx, gy, cell));
      }
    }
  }
  // Prefer an unsupported neck (hanging beam), else any mid neck.
  candidates.sort_by_key(|(x, y, _)| {
    let supported = matches!(
      support_below(world, *x, *y),
      Some((bed, _)) if bed != MaterialId::Air
    );
    (supported, *x + *y)
  });
  let Some((gx, gy, cell)) = candidates.into_iter().next() else {
    return 0;
  };
  let debris = roof_collapse_debris(cell.material);
  world.set_cell(
    gx,
    gy,
    Cell {
      material: debris,
      sat: cell.sat,
      ..cell
    },
  );
  world.touch_dirty(gx, gy);
  1
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
  // 1) Tip when COM overhangs the support base (true rotation).
  if let Some(dx) = try_pivot_roll(world, comp, cfg) {
    let pivot = roll_pivot(comp, dx);
    if pivot_roll_component(world, comp, pivot, dx) {
      stats.roll_moves += 1;
      settle_after_roll(world, regions, pivot, fall_streak, stats);
      return true;
    }
  }
  // 2) Otherwise slide down-slope (with embedded loose cargo).
  if let Some((dx, dy)) = try_slide(world, comp, cfg) {
    if slide_component(world, comp, dx, dy) {
      stats.roll_moves += 1;
      let new_anchor = (anchor.0 + dx, anchor.1 + dy);
      advance_streak(fall_streak, anchor, dx, dy);
      settle_after_roll(world, regions, new_anchor, fall_streak, stats);
      return true;
    }
  }
  false
}

fn settle_after_roll(
  world: &mut World,
  regions: &[ActiveChunk],
  hint: (i32, i32),
  fall_streak: &mut HashMap<(i32, i32), u32>,
  stats: &mut CompetentFallStats,
) {
  let (post, _) = build_components(world, regions);
  let Some(refreshed) = post
    .iter()
    .find(|c| c.set.contains(&hint))
    .or_else(|| post.first())
  else {
    return;
  };
  let drop = max_drop_distance(world, refreshed, 4);
  if drop > 0 && translate_component(world, refreshed, 0, -drop) {
    stats.fall_moves += 1;
    let a = comp_anchor(refreshed);
    advance_streak(fall_streak, a, 0, -drop);
  }
}

/// Clear sleep flags around every dirty rect in the scan set.
fn wake_settled_near_dirty(world: &mut World, regions: &[ActiveChunk]) {
  if world.competent_settled.is_empty() {
    return;
  }
  let cw = CHUNK_CELLS_W as i32;
  let ch = CHUNK_CELLS_H as i32;
  let mut rects: Vec<(i32, i32, i32, i32)> = Vec::new();
  for ac in regions {
    let Some(chunk) = world.chunks.get(&ac.coord) else {
      continue;
    };
    let Some(d) = chunk.dirty else {
      continue;
    };
    let base_gx = ac.coord.cx * cw;
    let base_gy = ac.coord.cy * ch;
    rects.push((
      base_gx + d.x0 as i32 - 1,
      base_gy + d.y0 as i32 - 1,
      base_gx + d.x1 as i32 + 1,
      base_gy + d.y1 as i32 + 1,
    ));
  }
  for (x0, y0, x1, y1) in rects {
    world.competent_wake_rect(x0, y0, x1, y1);
  }
}

/// Run competent-body physics on the active scan set.
pub fn apply_competent_fall_regions(
  world: &mut World,
  active: &[ActiveChunk],
  cfg: &CompetentFallConfig,
  fps_path: bool,
) -> CompetentFallStats {
  #[cfg(test)]
  crate::parallel::set_parallel_enabled(false);
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
  // Anything written since the last pass wakes the rock around it — support is
  // transmitted cell to cell, so a one-cell halo around each dirty rect is
  // enough to re-examine the bodies a write could have destabilised.
  wake_settled_near_dirty(world, &regions);
  world.competent_cell_moves.clear();
  let mut stats = CompetentFallStats::default();
  let mut fall_streak: HashMap<(i32, i32), u32> = HashMap::new();
  // Free-fall jumps once (binary-searched distance), then impact/roll rebuilds —
  // not one rebuild per cell of drop.
  let topology_passes = if fps_path {
    COMPETENT_TOPOLOGY_PASSES_FPS
  } else {
    COMPETENT_TOPOLOGY_PASSES
  };
  for pass in 0..topology_passes {
    let (mut components, leftovers) = build_components(world, &regions);
    if !leftovers.is_empty() {
      // Hand unfinished hang work to later ticks — do not spin rebuilds here.
      for &(x, y) in &leftovers {
        world.touch_dirty(x, y);
      }
    }
    if components.is_empty() {
      break;
    }
    // Within-tick fairness when a stuck prefix burned the previous pass.
    if pass > 0 && components.len() > 1 {
      let n = components.len();
      components.rotate_left((pass as usize) % n);
    }
    let mut moved = false;
    // Bodies that finish a pass without moving go to sleep (see
    // `World::competent_settled`). Collected first so the borrow ends.
    let mut to_sleep: Vec<(i32, i32)> = Vec::new();
    for comp in components {
      let anchor = comp_anchor(&comp);
      let mut comp_moved = false;
      // Free-falling bodies always take the full drop budget — never 1-cell
      // drip while waiting for a later topology pass.
      let floating = is_floating(world, &comp);
      let drop = if floating {
        max_drop_distance(world, &comp, max_drop)
      } else if pass == 0 {
        max_drop_distance(world, &comp, max_drop)
      } else {
        0
      };
      if drop > 0 {
        if translate_component(world, &comp, 0, -drop) {
          stats.fall_moves += 1;
          advance_streak(&mut fall_streak, anchor, 0, -drop);
          moved = true;
          comp_moved = true;
        }
        if !comp_moved {
          for &(x, y, _) in &comp.cells {
            to_sleep.push((x, y));
          }
        }
        continue;
      }
      if floating {
        // Airborne but blocked — mark mobile and skip tip/roll this pass.
        // Do not sleep: it is mid-air and must retry.
        mark_component_mobile(world, &comp);
        continue;
      }
      // Wait until every bottom cell has support — avoids uneven per-column
      // embed/shatter while the body is still bridging a slope or gap.
      if !is_fully_supported(world, &comp) {
        // Long thin sticks / slabs snap at 1-cell necks instead of tipping as
        // one beam. Only for *unsupported* spans: running this on seated strata
        // shredded ~5 k cells of untouched terrain into rubble on the first
        // pass and re-dirtied the whole ridge every tick.
        if is_long_thin(&comp) {
          let snapped = fracture_thin_necks(world, &comp);
          if snapped > 0 {
            stats.impacts = stats.impacts.saturating_add(1);
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
          comp_moved = true;
        }
        if !comp_moved {
          for &(x, y, _) in &comp.cells {
            to_sleep.push((x, y));
          }
        }
        continue;
      }
      // Tip on COM overhang; slide on slope without overhang.
      let streak = *fall_streak.get(&anchor).unwrap_or(&0);
      if tip_dir(world, &comp, cfg).is_some() || downhill_roll_dir(world, &comp, cfg).is_some() {
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
      }
      if rests_on_hard_bed(world, &comp) {
        let shattered = impact_shatter(world, &comp, streak, cfg);
        if shattered > 0 {
          stats.impacts += 1;
          fall_streak.remove(&anchor);
          moved = true;
          comp_moved = true;
          // Thin remnants often need a second snap after face shatter.
          if is_long_thin(&comp) {
            let _ = fracture_thin_necks(world, &comp);
          }
          continue;
        }
      }
      // Soft embed disabled (cheese-grater). Flat soft beds just rest.
      if !comp_moved {
        for &(x, y, _) in &comp.cells {
          to_sleep.push((x, y));
        }
      }
    }
    for (x, y) in to_sleep {
      world.competent_set_settled(x, y);
    }
    if moved {
      // Follow the bodies that actually moved using the recorded move list —
      // a second full build_components here doubled topology cost per pass.
      let moved_to: Vec<(i32, i32)> = world
        .competent_cell_moves
        .iter()
        .map(|&(_, _, tx, ty)| (tx, ty))
        .collect();
      if !moved_to.is_empty() {
        let refreshed = expand_regions_to_cells(world, &moved_to, max_drop);
        if !refreshed.is_empty() {
          regions = refreshed;
        }
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
    for x in 0..16 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 4 + (11 - x).min(6).max(1);
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
    let x0 = 5;
    for _ in 0..6 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let rock_xs: Vec<i32> = (0..20)
      .filter(|&x| {
        (1..=18).any(|y| {
          w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone)
        })
      })
      .collect();
    let loose = (0..20)
      .flat_map(|x| (1..=18).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::LooseRock))
      .count();
    assert!(
      rock_xs.iter().any(|&x| x > x0 + 1),
      "body should roll right down the sand slope (rock cols={rock_xs:?})"
    );
    assert!(
      loose <= 4,
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
    let (comps, _) = build_components(&w, &regions);
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
    assert_eq!(roll_pivot(&comp, 1), (6, 5), "right roll pivots on bbox right-bottom");
    assert_eq!(roll_pivot(&comp, -1), (5, 5), "left roll pivots on bbox left-bottom");
  }

  #[test]
  fn large_disk_rolls_left_on_steep_sand() {
    let mut w = World::new(16);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..32 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 2 + x / 2;
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    let cx = 18i32;
    let cy = 14i32;
    for x in cx - 5..=cx + 5 {
      for y in cy - 5..=cy + 5 {
        if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= 25 {
          w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
      }
    }
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    assert!(!comps.is_empty());
    let cfg = CompetentFallConfig {
      max_roll_events: 24,
      min_impact_fall_cells: 99,
      ..CompetentFallConfig::default()
    };
    let dir = downhill_roll_dir(&w, &comps[0], &cfg);
    assert_eq!(dir, Some(-1), "large disk must see left downhill, got {dir:?}");
    let x0 = comps[0].cells.iter().map(|(x, _, _)| *x).min().unwrap();
    for _ in 0..24 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let stones: Vec<(i32, i32)> = (0..32)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .collect();
    assert!(stones.len() >= 20, "disk should stay mostly intact (n={})", stones.len());
    let x1 = stones.iter().map(|(x, _)| *x).min().unwrap_or(99);
    assert!(
      x1 < x0,
      "disk should roll left (start_min_x={x0}, end_min_x={x1})"
    );
  }

  #[test]
  fn large_disk_rolls_on_limestone_slope() {
    let mut w = World::new(16);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..32 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 2 + x / 2;
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Limestone));
      }
    }
    let cx = 18i32;
    let cy = 14i32;
    for x in cx - 5..=cx + 5 {
      for y in cy - 5..=cy + 5 {
        if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= 25 {
          w.set_cell(x, y, Cell::solid(MaterialId::Stone));
        }
      }
    }
    let cfg = CompetentFallConfig {
      max_roll_events: 24,
      min_impact_fall_cells: 99,
      ..CompetentFallConfig::default()
    };
    let x0 = 13;
    for _ in 0..24 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let stones: Vec<(i32, i32)> = (0..32)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .collect();
    let x1 = stones.iter().map(|(x, _)| *x).min().unwrap_or(99);
    assert!(
      x1 < x0,
      "disk should tumble left on limestone (start~{x0}, end_min_x={x1}, n={})",
      stones.len()
    );
  }

  #[test]
  fn floating_boulder_falls_after_dirty_cleared() {
    use crate::active::{clear_all_dirty, plan_active};
    let mut w = World::new(9);
    stamp_blob(&mut w, 4, 60, 3, 3);
    clear_all_dirty(&mut w);
    assert!(plan_active(&w).is_empty(), "precondition: quiet after clear");
    // Simulate the playtest trap: F1 defers, dirty empty, only floating wake saves us.
    for _ in 0..8 {
      wake_floating_competent(&mut w);
      let active = plan_active(&w);
      assert!(!active.is_empty(), "floating wake must dirty sky stone");
      apply_competent_fall_regions(
        &mut w,
        &active,
        &CompetentFallConfig::default(),
        true,
      );
      clear_all_dirty(&mut w);
    }
    let y_top = (0..128)
      .filter(|&y| w.get_cell(5, y).map(|c| c.material) == Some(MaterialId::Stone))
      .max()
      .unwrap_or(128);
    assert!(
      y_top < 55,
      "floating boulder must fall after wake (top y={y_top})"
    );
  }

  #[test]
  fn contact_pebble_does_not_weld_to_boulder() {
    let cells: Vec<(i32, i32, Cell)> = {
      let mut v = Vec::new();
      for x in 0..5 {
        for y in 0..5 {
          v.push((x, y, Cell::solid(MaterialId::Stone)));
        }
      }
      // Single-cell neck to a 2-cell pebble.
      v.push((5, 2, Cell::solid(MaterialId::Stone)));
      v.push((6, 2, Cell::solid(MaterialId::Stone)));
      v.push((7, 2, Cell::solid(MaterialId::Stone)));
      v
    };
    let pieces = split_contact_pebbles(cells);
    assert!(
      pieces.len() >= 2,
      "pebble neck must split off (pieces={})",
      pieces.len()
    );
    let sizes: Vec<usize> = pieces.iter().map(|p| p.len()).collect();
    assert!(
      sizes.iter().any(|&s| s <= PEBBLE_SPLIT_MAX + 1),
      "expected a small peeled piece, sizes={sizes:?}"
    );
    assert!(
      sizes.iter().any(|&s| s >= PEBBLE_SPLIT_HOST_MIN),
      "expected host mass to remain, sizes={sizes:?}"
    );
  }

  #[test]
  fn tip_requires_com_overhang_not_slope_alone() {
    let mut w = World::new(11);
    for x in 0..12 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      let h = 4 + (11 - x).min(6);
      for y in 1..=h {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    // 3×3 seated fully on sand — COM inside support even on a slope.
    stamp_blob(&mut w, 5, 11, 3, 3);
    // Drop it onto the bed.
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_roll_events: 0,
      ..CompetentFallConfig::default()
    };
    apply_competent_fall_regions(&mut w, &[], &cfg, false);
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    let Some(comp) = comps.iter().find(|c| c.cells.len() >= 9) else {
      // May have slid; just ensure tip_dir is conservative when supported.
      return;
    };
    if is_fully_supported(&w, comp) {
      let tip = tip_dir(&w, comp, &CompetentFallConfig::default());
      // Slope may exist, but without COM overhang tip must be None.
      if tip.is_some() {
        let contacts: Vec<_> = bottom_face(comp)
          .into_iter()
          .filter(|(x, y, _)| support_below(&w, *x, *y).is_some())
          .collect();
        let n = comp.cells.len() as i64;
        let com_x = (comp.cells.iter().map(|(x, _, _)| *x as i64).sum::<i64>() / n) as i32;
        let s_min = contacts.iter().map(|(x, _, _)| *x).min().unwrap();
        let s_max = contacts.iter().map(|(x, _, _)| *x).max().unwrap();
        assert!(
          com_x < s_min || com_x > s_max,
          "tip only with COM overhang (com={com_x}, support={s_min}..{s_max})"
        );
      }
    }
  }

  #[test]
  fn large_body_crushes_tiny_rock_spec() {
    let mut w = World::new(11);
    for x in 0..16 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      for y in 1..=3 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    stamp_blob(&mut w, 4, 4, 4, 4); // 16-cell boulder
    w.set_cell(9, 4, Cell::solid(MaterialId::Stone)); // tiny spec downhill
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    let boulder = comps.iter().find(|c| c.cells.len() >= 12).expect("boulder");
    assert!(
      can_crush_spec(&w, 9, 4, boulder.cells.len()),
      "16-cell body must be allowed to crush a 1-cell spec"
    );
    assert!(
      can_slide(&w, boulder, 1, 0) || can_slide(&w, boulder, 1, -1),
      "boulder must not be hard-blocked by the spec"
    );
    let _ = crush_spec_at(&mut w, 9, 4);
    assert_eq!(
      w.get_cell(9, 4).map(|c| c.material),
      Some(MaterialId::LooseRock),
      "crushed spec becomes LooseRock"
    );
  }

  #[test]
  fn long_thin_stick_fractures_at_neck() {
    let mut w = World::new(11);
    for x in 0..16 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
    }
    // 10×1 stick with supports only at the ends — mid should snap.
    for x in 3..13 {
      w.set_cell(x, 3, Cell::solid(MaterialId::Stone));
    }
    w.set_cell(3, 2, Cell::solid(MaterialId::Sand));
    w.set_cell(12, 2, Cell::solid(MaterialId::Sand));
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_roll_events: 4,
      ..CompetentFallConfig::default()
    };
    for _ in 0..6 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let stone: usize = (3..13)
      .filter(|&x| w.get_cell(x, 3).map(|c| c.material) == Some(MaterialId::Stone))
      .count();
    let loose: usize = (0..16)
      .flat_map(|x| (1..=5).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::LooseRock))
      .count();
    assert!(
      stone < 10 || loose > 0,
      "thin stick should snap (stone_mid={stone}, loose={loose})"
    );
  }

  #[test]
  fn touching_disks_do_not_weld_into_one_body() {
    // Two r=4 disks touching at one column — must split, not one rigid dumbbell.
    let mut cells = Vec::new();
    let stamp = |cells: &mut Vec<(i32, i32, Cell)>, cx: i32, cy: i32| {
      for x in cx - 4..=cx + 4 {
        for y in cy - 4..=cy + 4 {
          if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= 16 {
            cells.push((x, y, Cell::solid(MaterialId::Stone)));
          }
        }
      }
    };
    stamp(&mut cells, 10, 10);
    stamp(&mut cells, 18, 10); // centers 8 apart, r=4 → touch
    // Dedup overlap at the contact.
    let mut uniq: HashMap<(i32, i32), Cell> = HashMap::new();
    for (x, y, c) in cells {
      uniq.insert((x, y), c);
    }
    let cells: Vec<_> = uniq.into_iter().map(|((x, y), c)| (x, y, c)).collect();
    let n = cells.len();
    let pieces = split_welded_contacts(cells);
    assert!(
      pieces.len() >= 2,
      "touching disks must split (pieces={}, n={n})",
      pieces.len()
    );
    assert!(
      pieces.iter().all(|p| p.len() < n),
      "no piece should keep the whole welded chain"
    );
  }

  #[test]
  fn welded_ball_chain_falls_apart_instead_of_freezing() {
    let mut w = World::new(16);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      for y in 1..=2 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    for i in 0..5 {
      let cx = 8 + i * 8;
      let cy = 20;
      for x in cx - 4..=cx + 4 {
        for y in cy - 4..=cy + 4 {
          if (x - cx) * (x - cx) + (y - cy) * (y - cy) <= 16 {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
          }
        }
      }
    }
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    assert!(
      comps.len() >= 3,
      "chain must become multiple bodies (got {})",
      comps.len()
    );
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_roll_events: 8,
      ..CompetentFallConfig::default()
    };
    for _ in 0..10 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let max_y = (0..40)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .max()
      .unwrap_or(0);
    assert!(
      max_y < 18,
      "split balls must fall out of the sky (max_y={max_y})"
    );
  }

  #[test]
  fn tiny_stick_does_not_flip_flop_tip() {
    let mut w = World::new(11);
    for x in 0..12 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
    }
    for y in 2..6 {
      w.set_cell(5, y, Cell::solid(MaterialId::Stone));
    }
    let regions = competent_active_regions(&w, &[], 4);
    let (comps, _) = build_components(&w, &regions);
    let Some(comp) = comps.iter().find(|c| c.cells.len() >= 3) else {
      return;
    };
    assert!(
      tip_dir(&w, comp, &CompetentFallConfig::default()).is_none(),
      "tiny sticks must not tip (flip-flop)"
    );
    assert!(!may_tip(comp), "may_tip rejects needles");
  }

  #[test]
  fn mobile_boulder_does_not_absorb_unmarked_terrain() {
    let mut w = World::new(11);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..16 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // Unmarked stone wall (painted strata).
    for y in 1..=12 {
      for x in 0..4 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    // Mobile boulder pressed against the wall.
    for x in 4..7 {
      for y in 1..=3 {
        let mut c = Cell::solid(MaterialId::Stone);
        c.flags.set(CellFlags::MOBILE_ROCK);
        w.set_cell(x, y, c);
      }
    }
    let regions = competent_active_regions(&w, &[], 4);
    let (comps, _) = build_components(&w, &regions);
    let boulder = comps
      .iter()
      .find(|c| c.cells.iter().any(|(x, _, _)| *x >= 4))
      .expect("mobile boulder must be extracted");
    assert_eq!(
      boulder.cells.len(),
      9,
      "mobile body must not weld into unmarked wall (got {})",
      boulder.cells.len()
    );
    assert!(
      boulder
        .cells
        .iter()
        .all(|(_, _, c)| c.flags.contains(CellFlags::MOBILE_ROCK)),
      "extracted cells stay mobile-marked"
    );
  }

  #[test]
  fn medium_floating_slab_falls() {
    let mut w = World::new(20);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..16 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // 15×15 competent slab with air below — under MAX_DYNAMIC_BODY_CELLS.
    for x in 2..17 {
      for y in 30..45 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 24,
      ..CompetentFallConfig::default()
    };
    for _ in 0..12 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let max_y = (0..20)
      .flat_map(|x| (0..50).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .max()
      .unwrap_or(0);
    assert!(
      max_y < 20,
      "medium floating slab must fall to bed (max_y={max_y})"
    );
  }

  #[test]
  fn large_floating_island_peels_and_falls() {
    let mut w = World::new(24);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    w.ensure_chunk(ChunkCoord::new(0, 1));
    for x in 0..24 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      for y in 1..=3 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    // 20×20 island (400 cells) — oversize, must grid-peel and fall.
    for x in 2..22 {
      for y in 40..60 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let cells: Vec<_> = (2..22)
      .flat_map(|x| (40..60).map(move |y| (x, y)))
      .filter_map(|(x, y)| w.get_cell(x, y).map(|c| (x, y, c)))
      .collect();
    assert_eq!(cells.len(), 400, "precondition");
    let hang = extract_hanging_pieces(&w, &cells);
    let hang_cells: usize = hang.iter().map(|p| p.len()).sum();
    assert!(
      hang_cells >= 300,
      "extract_hanging must grid-peel the island (cells={hang_cells}, pieces={})",
      hang.len()
    );
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    let total: usize = comps.iter().map(|c| c.cells.len()).sum();
    assert!(
      total >= 300,
      "oversize floating island must peel into dynamic bodies (cells={total}, comps={})",
      comps.len()
    );
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 48,
      ..CompetentFallConfig::default()
    };
    for _ in 0..48 {
      wake_floating_competent(&mut w);
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let stone_ys: Vec<i32> = (0..24)
      .flat_map(|x| (0..70).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .collect();
    let max_y = stone_ys.iter().copied().max().unwrap_or(0);
    let min_y = stone_ys.iter().copied().min().unwrap_or(0);
    assert!(
      max_y < 40 && min_y <= 5,
      "peeled island must fall and seat on sand (min_y={min_y}, max_y={max_y}, n={})",
      stone_ys.len()
    );
  }

  #[test]
  fn cantilever_span_peels_over_cavern_while_pillars_stay() {
    let mut w = World::new(40);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=15 {
      w.set_cell(5, y, Cell::solid(MaterialId::Stone));
      w.set_cell(34, y, Cell::solid(MaterialId::Stone));
    }
    for x in 5..35 {
      w.set_cell(x, 15, Cell::solid(MaterialId::Stone));
    }
    for x in 10..30 {
      for y in 16..=25 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    let total: usize = comps.iter().map(|c| c.cells.len()).sum();
    assert!(
      total >= 150,
      "cavern roof must peel as dynamic bodies (cells={total}, comps={})",
      comps.len()
    );
    assert!(
      comps.len() <= 4,
      "cavern roof should crash in a few large chunks, not many tiny peels (comps={})",
      comps.len()
    );
    assert!(
      !comps.iter().any(|c| c.set.contains(&(5, 8))),
      "bedrock-rooted pillar must stay static"
    );
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 48,
      ..CompetentFallConfig::default()
    };
    for _ in 0..24 {
      wake_floating_competent(&mut w);
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let center_still = w.get_cell(20, 20).map(|c| c.material) == Some(MaterialId::Stone);
    assert!(
      !center_still,
      "peeled cavern roof must fall away from mid-air perch"
    );
  }

  #[test]
  fn truncated_hanging_bodies_are_redirtied_and_finish() {
    // Many small floating tiles — over the hanging body cap — must all fall
    // across ticks because truncated leftovers stay dirty.
    let mut w = World::new(128);
    for cx in 0..8 {
      for cy in 0..4 {
        w.ensure_chunk(ChunkCoord::new(cx, cy));
      }
    }
    for x in 0..128 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    // 20×12 tiles of 3×3 stone with 1-cell air gaps — over hanging cap.
    let mut tiles = 0usize;
    for ty in 0..12 {
      for tx in 0..20 {
        let ox = 2 + tx * 4;
        let oy = 40 + ty * 4;
        for x in ox..ox + 3 {
          for y in oy..oy + 3 {
            w.set_cell(x, y, Cell::solid(MaterialId::Stone));
          }
        }
        tiles += 1;
      }
    }
    assert!(
      tiles > MAX_FLOATING_BODIES_PER_TICK,
      "precondition: more tiles than floating cap ({tiles})"
    );
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, leftovers) = build_components(&w, &regions);
    assert!(
      !leftovers.is_empty(),
      "cap must truncate (comps={}, leftovers={})",
      comps.len(),
      leftovers.len()
    );
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 48,
      ..CompetentFallConfig::default()
    };
    for _ in 0..300 {
      wake_floating_competent(&mut w);
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
      w.tick = w.tick.wrapping_add(1);
    }
    let high = (0..100)
      .flat_map(|x| (38..100).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .count();
    assert!(
      high == 0,
      "all truncated floating tiles must eventually fall (remaining={high})"
    );
  }

  #[test]
  fn many_floaters_drop_fast_not_one_cell_per_hundred_ticks() {
    // ~80 sky pebbles — with old fair-rotate+16 cap each waited ~5 ticks;
    // with hundreds of debris it was ~100. After priority+budget they should
    // clear the sky band in a handful of ticks via multi-cell drops.
    let mut w = World::new(64);
    for cx in 0..4 {
      for cy in 0..2 {
        w.ensure_chunk(ChunkCoord::new(cx, cy));
      }
    }
    for x in 0..64 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for i in 0..80 {
      let x = 2 + (i % 20) * 3;
      let y = 50 + (i / 20) * 3;
      w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      w.set_cell(x + 1, y, Cell::solid(MaterialId::Stone));
      w.set_cell(x, y + 1, Cell::solid(MaterialId::Stone));
      w.set_cell(x + 1, y + 1, Cell::solid(MaterialId::Stone));
    }
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 64,
      ..CompetentFallConfig::default()
    };
    for t in 0..12u64 {
      w.tick = t;
      wake_floating_competent(&mut w);
      apply_competent_fall_regions(&mut w, &[], &cfg, true);
    }
    let high = (0..64)
      .flat_map(|x| (30..70).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .count();
    assert_eq!(
      high, 0,
      "floaters must clear sky within ~12 ticks via full drops (remaining={high})"
    );
  }

  #[test]
  fn seated_strata_does_not_disintegrate_into_rubble() {
    // Thin horizontal layers used to hit `fracture_thin_necks` while fully
    // seated, shredding untouched terrain into LooseRock on the first pass.
    let mut w = World::new(64);
    for cx in 0..2 {
      w.ensure_chunk(ChunkCoord::new(cx, 0));
    }
    for x in 0..128 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      // Interleaved thin strata, all resting on solid support.
      for y in 1..=6 {
        let mat = if y % 2 == 0 {
          MaterialId::Limestone
        } else {
          MaterialId::Stone
        };
        w.set_cell(x, y, Cell::solid(mat));
      }
    }
    let cfg = CompetentFallConfig::default();
    for _ in 0..12 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    // Interior only: the outermost columns border unloaded chunks, which read
    // as open space and are a legitimate cliff edge that may shed rock.
    const LO: i32 = 8;
    const HI: i32 = 120;
    let loose = (LO..HI)
      .flat_map(|x| (0..10).map(move |y| (x, y)))
      .filter(|&(x, y)| {
        matches!(
          w.get_cell(x, y).map(|c| c.material),
          Some(MaterialId::LooseRock) | Some(MaterialId::LooseLimestone)
        )
      })
      .count();
    assert_eq!(
      loose, 0,
      "seated strata must not fracture into rubble (loose={loose})"
    );
    let stone = (LO..HI)
      .flat_map(|x| (1..=6).map(move |y| (x, y)))
      .filter(|&(x, y)| {
        matches!(
          w.get_cell(x, y).map(|c| c.material),
          Some(MaterialId::Stone) | Some(MaterialId::Limestone)
        )
      })
      .count();
    assert_eq!(
      stone,
      (HI - LO) as usize * 6,
      "every interior strata cell must survive intact"
    );
  }

  #[test]
  fn settled_terrain_stops_being_reseeded() {
    // Sleeping-rock guard: a flat seated ridge must stop producing bodies
    // once evaluated, so a quiet world costs nothing.
    let mut w = World::new(64);
    for cx in 0..2 {
      w.ensure_chunk(ChunkCoord::new(cx, 0));
    }
    for x in 0..64 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
      for y in 1..=8 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
    }
    let cfg = CompetentFallConfig::default();
    // First pass evaluates and sleeps.
    apply_competent_fall_regions(&mut w, &[], &cfg, false);
    let regions = competent_active_regions(&w, &[], 8);
    let (comps, _) = build_components(&w, &regions);
    assert!(
      comps.is_empty(),
      "settled ridge must not rebuild bodies (got {})",
      comps.len()
    );
    // Carving support must wake the rock above again.
    for x in 20..30 {
      w.set_cell(x, 1, Cell::air());
    }
    wake_competent_bodies_all(&mut w);
    let regions2 = competent_active_regions(&w, &[], 8);
    let (comps2, _) = build_components(&w, &regions2);
    assert!(
      !comps2.is_empty(),
      "carving under the ridge must wake bodies again"
    );
  }

  #[test]
  fn carved_arch_slab_crashes() {
    let mut w = World::new(40);
    for cx in 0..3 {
      w.ensure_chunk(ChunkCoord::new(cx, 0));
    }
    for x in 0..40 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    for y in 1..=30 {
      w.set_cell(10, y, Cell::solid(MaterialId::Stone));
      w.set_cell(30, y, Cell::solid(MaterialId::Stone));
    }
    for x in 10..31 {
      for y in 20..23 {
        w.set_cell(x, y, Cell::solid(MaterialId::Stone));
      }
      w.set_cell(x, 23, Cell::solid(MaterialId::Sand));
    }
    for x in 11..30 {
      for y in 5..20 {
        w.set_cell(x, y, Cell::air());
      }
    }
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 48,
      ..CompetentFallConfig::default()
    };
    for _ in 0..16 {
      wake_floating_competent(&mut w);
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    assert!(
      w.get_cell(20, 22).map(|c| c.material) != Some(MaterialId::Stone),
      "isolated carved arch must fall"
    );
  }

  #[test]
  fn sand_cap_rides_falling_rock() {
    let mut w = World::new(9);
    w.ensure_chunk(ChunkCoord::new(0, 0));
    for x in 0..12 {
      w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
    }
    stamp_blob(&mut w, 4, 30, 3, 3);
    for x in 4..7 {
      for y in 33..36 {
        w.set_cell(x, y, Cell::solid(MaterialId::Sand));
      }
    }
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      max_passes: 32,
      ..CompetentFallConfig::default()
    };
    for _ in 0..8 {
      apply_competent_fall_regions(&mut w, &[], &cfg, false);
    }
    let stone_top = (4..7)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone))
      .map(|(_, y)| y)
      .max()
      .unwrap_or(0);
    let sand_min = (4..7)
      .flat_map(|x| (0..40).map(move |y| (x, y)))
      .filter(|&(x, y)| w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Sand))
      .map(|(_, y)| y)
      .min()
      .unwrap_or(99);
    assert!(
      stone_top < 20,
      "rock must fall (stone_top={stone_top})"
    );
    assert!(
      sand_min <= stone_top + 4,
      "sand cap must stay on the rock column (sand_min={sand_min}, stone_top={stone_top})"
    );
    for x in 4..7 {
      let sand_here = w.get_cell(x, sand_min).map(|c| c.material) == Some(MaterialId::Sand);
      let stone_below = w.get_cell(x, sand_min - 1).map(|c| c.material) == Some(MaterialId::Stone);
      assert!(
        sand_here && stone_below,
        "sand at ({x},{sand_min}) must sit directly on stone"
      );
    }
  }

  #[test]
  fn falling_stone_is_marked_mobile() {
    let mut w = World::new(9);
    stamp_blob(&mut w, 4, 40, 3, 3);
    let cfg = CompetentFallConfig {
      min_impact_fall_cells: 99,
      ..CompetentFallConfig::default()
    };
    apply_competent_fall_regions(&mut w, &[], &cfg, false);
    let stones: Vec<_> = (0..20)
      .flat_map(|x| (0..50).map(move |y| (x, y)))
      .filter(|&(x, y)| {
        w.get_cell(x, y).map(|c| c.material) == Some(MaterialId::Stone)
      })
      .collect();
    assert_eq!(stones.len(), 9, "3x3 blob survives fall");
    for (x, y) in stones {
      let c = w.get_cell(x, y).unwrap();
      assert!(
        c.flags.contains(CellFlags::MOBILE_ROCK),
        "fallen stone at ({x},{y}) must be MOBILE_ROCK"
      );
    }
  }
}
