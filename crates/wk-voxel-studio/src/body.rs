//! Activate paint → [`BodyGraph`], and S1 rigid-bone stepping.
//!
//! Connected Bone components become rigid parts. Fixture cells are
//! immovable. A bone that 4-touches a fixture (or another anchored
//! bone through fixture adjacency at activate time) hangs in place;
//! free bones fall under discrete gravity until they rest.

use std::collections::{HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;
use wk_voxel::World;

use crate::tissue::{TissueKind, TissuePaint};

/// What a rigid part is made of (S1: bone + fixture only).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartKind {
    Bone,
    Fixture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivateError {
    /// No bone or fixture pixels to build from.
    Empty,
}

/// One rigid connected component in paint space, plus live translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidPart {
    pub id: u32,
    pub kind: PartKind,
    /// Paint-space cells relative to the part (absolute at activate).
    pub cells: Vec<(i32, i32)>,
    /// Integer translation applied to every cell (cells = rest + offset).
    pub offset_x: i32,
    pub offset_y: i32,
    /// True when this part must not move (fixture, or bone hung on one).
    pub anchored: bool,
}

impl RigidPart {
    pub fn world_cells(&self) -> impl Iterator<Item = (i32, i32)> + '_ {
        self.cells
            .iter()
            .map(|(x, y)| (*x + self.offset_x, *y + self.offset_y))
    }

    pub fn contains_world(&self, gx: i32, gy: i32) -> bool {
        self.world_cells().any(|(x, y)| x == gx && y == gy)
    }
}

/// Activated studio body for simulation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BodyGraph {
    pub parts: Vec<RigidPart>,
}

impl BodyGraph {
    pub fn bone_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| p.kind == PartKind::Bone)
            .count()
    }

    pub fn fixture_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| p.kind == PartKind::Fixture)
            .count()
    }

    pub fn anchored_bone_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| p.kind == PartKind::Bone && p.anchored)
            .count()
    }

    /// Tissue kind under a world cell after offsets (for draw).
    pub fn kind_at(&self, gx: i32, gy: i32) -> Option<TissueKind> {
        for p in &self.parts {
            if p.contains_world(gx, gy) {
                return Some(match p.kind {
                    PartKind::Bone => TissueKind::Bone,
                    PartKind::Fixture => TissueKind::Fixture,
                });
            }
        }
        None
    }
}

/// Flood-fill Bone / Fixture components and mark bones that touch a
/// fixture as anchored.
pub fn activate(paint: &TissuePaint) -> Result<BodyGraph, ActivateError> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut seen = HashSet::new();
    let mut parts: Vec<RigidPart> = Vec::new();
    let mut next_id = 0u32;

    for y in 0..h {
        for x in 0..w {
            let kind = paint.get(x as u32, y as u32);
            let part_kind = match kind {
                TissueKind::Bone => PartKind::Bone,
                TissueKind::Fixture => PartKind::Fixture,
                _ => continue,
            };
            if !seen.insert((x, y)) {
                continue;
            }
            let cells = flood(paint, x, y, kind, &mut seen);
            if cells.is_empty() {
                continue;
            }
            let anchored = part_kind == PartKind::Fixture;
            parts.push(RigidPart {
                id: next_id,
                kind: part_kind,
                cells,
                offset_x: 0,
                offset_y: 0,
                anchored,
            });
            next_id += 1;
        }
    }

    if parts.is_empty() {
        return Err(ActivateError::Empty);
    }

    // Bones that 4-touch any fixture cell hang.
    let mut fixture_cells: HashSet<(i32, i32)> = HashSet::new();
    for p in &parts {
        if p.kind == PartKind::Fixture {
            for c in &p.cells {
                fixture_cells.insert(*c);
            }
        }
    }
    for p in &mut parts {
        if p.kind != PartKind::Bone || p.anchored {
            continue;
        }
        if p.cells.iter().any(|&(x, y)| {
            [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .any(|(dx, dy)| fixture_cells.contains(&(x + dx, y + dy)))
        }) {
            p.anchored = true;
        }
    }

    Ok(BodyGraph { parts })
}

fn flood(
    paint: &TissuePaint,
    sx: i32,
    sy: i32,
    want: TissueKind,
    seen: &mut HashSet<(i32, i32)>,
) -> Vec<(i32, i32)> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut out = Vec::new();
    let mut q = VecDeque::new();
    // Caller already marked `(sx, sy)` in `seen`.
    q.push_back((sx, sy));
    while let Some((x, y)) = q.pop_front() {
        out.push((x, y));
        for (dx, dy) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
            let nx = x + dx;
            let ny = y + dy;
            if nx < 0 || ny < 0 || nx >= w || ny >= h {
                continue;
            }
            if paint.get(nx as u32, ny as u32) != want {
                continue;
            }
            if seen.insert((nx, ny)) {
                q.push_back((nx, ny));
            }
        }
    }
    out
}

fn cell_passable(world: &World, gx: i32, gy: i32) -> bool {
    match world.get_cell(gx, gy) {
        None => false,
        Some(c) => c.material == MaterialId::Air,
    }
}

/// Occupancy of all parts (world cells).
fn occupied(graph: &BodyGraph) -> HashSet<(i32, i32)> {
    let mut set = HashSet::new();
    for p in &graph.parts {
        for c in p.world_cells() {
            set.insert(c);
        }
    }
    set
}

/// S1 body step: free bones fall one cell down when the whole footprint
/// can move; fixtures and hung bones stay put.
///
/// Call **after** the voxel CA tick (docs/organism/STUDIO.md checklist).
pub fn step_body(graph: &mut BodyGraph, world: &World) {
    if graph.parts.is_empty() {
        return;
    }
    // Stable order by id.
    let mut order: Vec<usize> = (0..graph.parts.len()).collect();
    order.sort_by_key(|&i| graph.parts[i].id);

    for &idx in &order {
        if graph.parts[idx].anchored || graph.parts[idx].kind != PartKind::Bone {
            continue;
        }
        let cells: Vec<(i32, i32)> = graph.parts[idx].world_cells().collect();
        let mut occ = occupied(graph);
        for c in &cells {
            occ.remove(c);
        }
        let can_fall = cells.iter().all(|&(x, y)| {
            let ty = y - 1;
            cell_passable(world, x, ty) && !occ.contains(&(x, ty))
        });
        if can_fall {
            graph.parts[idx].offset_y -= 1;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{ArenaConfig, StudioArena};
    use crate::tissue::TissueKind;

    #[test]
    fn activate_hangs_bone_on_fixture() {
        let mut paint = TissuePaint::new(16, 16);
        // Vertical fixture column.
        for y in 4..12 {
            paint.set(2, y, TissueKind::Fixture);
        }
        // Bone sticking out from fixture.
        paint.set(3, 8, TissueKind::Bone);
        paint.set(4, 8, TissueKind::Bone);
        paint.set(5, 8, TissueKind::Bone);

        let g = activate(&paint).unwrap();
        assert_eq!(g.fixture_count(), 1);
        assert_eq!(g.bone_count(), 1);
        assert_eq!(g.anchored_bone_count(), 1);
    }

    #[test]
    fn free_bone_falls_until_floor() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 32,
            height: 32,
            seed: 2,
            water_to_y: None,
        });
        // Floating bone island (not touching fixture).
        arena.body.paint.set(10, 20, TissueKind::Bone);
        arena.body.paint.set(11, 20, TissueKind::Bone);
        arena.activate().unwrap();
        assert_eq!(arena.body.graph.as_ref().unwrap().anchored_bone_count(), 0);

        let y0 = arena.body.graph.as_ref().unwrap().parts[0].offset_y;
        for _ in 0..40 {
            arena.tick();
        }
        let y1 = arena.body.graph.as_ref().unwrap().parts[0].offset_y;
        assert!(y1 < y0, "free bone should fall (offset {y0} → {y1})");
        // Resting on bedrock floor at y=0 → bone cells at y=1.
        let cells: Vec<_> = arena.body.graph.as_ref().unwrap().parts[0]
            .world_cells()
            .collect();
        assert!(cells.iter().all(|&(_, y)| y >= 1));
        assert!(cells.iter().any(|&(_, y)| y == 1));
    }

    #[test]
    fn hung_bone_does_not_fall() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 32,
            height: 32,
            seed: 3,
            water_to_y: None,
        });
        for y in 5..15 {
            arena.body.paint.set(4, y as u32, TissueKind::Fixture);
        }
        arena.body.paint.set(5, 10, TissueKind::Bone);
        arena.body.paint.set(6, 10, TissueKind::Bone);
        arena.activate().unwrap();
        let y0 = arena.body.graph.as_ref().unwrap().parts
            .iter()
            .find(|p| p.kind == PartKind::Bone)
            .unwrap()
            .offset_y;
        for _ in 0..20 {
            arena.tick();
        }
        let y1 = arena.body.graph.as_ref().unwrap().parts
            .iter()
            .find(|p| p.kind == PartKind::Bone)
            .unwrap()
            .offset_y;
        assert_eq!(y0, y1, "fixture-hung bone must not fall");
    }
}
