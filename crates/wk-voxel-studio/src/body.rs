//! Activate paint → [`BodyGraph`], joints, muscles, feedback, body step.
//!
//! S1: rigid bones + fixtures, discrete gravity.
//! S2: joints link bones; muscles actuate; muscle feedback; hydro push.

use std::collections::{HashMap, HashSet, VecDeque};

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;
use wk_voxel::{Cell, Sat, World};

use crate::tissue::{JointLimit, TissueKind, TissuePaint};

/// What a rigid part is made of.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartKind {
    Bone,
    Fixture,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActivateError {
    Empty,
}

/// One rigid connected component in paint space, plus live translation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidPart {
    pub id: u32,
    pub kind: PartKind,
    pub cells: Vec<(i32, i32)>,
    pub offset_x: i32,
    pub offset_y: i32,
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

    pub fn centroid(&self) -> (f32, f32) {
        if self.cells.is_empty() {
            return (0.0, 0.0);
        }
        let n = self.cells.len() as f32;
        let sx: i32 = self.cells.iter().map(|c| c.0 + self.offset_x).sum();
        let sy: i32 = self.cells.iter().map(|c| c.1 + self.offset_y).sum();
        (sx as f32 / n, sy as f32 / n)
    }
}

/// Hinge between two bone parts (or bone↔fixture).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Joint {
    pub part_a: u32,
    pub part_b: u32,
    pub pivot: (i32, i32),
    pub limit: JointLimit,
}

/// Contractile link — command + proprioceptive feedback.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Muscle {
    pub id: u32,
    pub part_a: u32,
    pub part_b: u32,
    /// Paint cells that formed this muscle (for draw).
    pub cells: Vec<(i32, i32)>,
    pub rest_length: f32,
    /// Commanded contraction 0..1 (1 = fully contracted).
    pub actuation: f32,
    /// Current separation of part centroids.
    pub length: f32,
    /// Feedback: how hard the muscle is working (length error + command).
    pub tension: f32,
}

impl Muscle {
    /// Proprioception sample for neural input / fitness.
    pub fn feedback(&self) -> MuscleFeedback {
        MuscleFeedback {
            muscle_id: self.id,
            actuation: self.actuation,
            length: self.length,
            rest_length: self.rest_length,
            tension: self.tension,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct MuscleFeedback {
    pub muscle_id: u32,
    pub actuation: f32,
    pub length: f32,
    pub rest_length: f32,
    pub tension: f32,
}

/// Activated studio body for simulation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BodyGraph {
    pub parts: Vec<RigidPart>,
    pub joints: Vec<Joint>,
    pub muscles: Vec<Muscle>,
    /// Radians-ish phase for scripted sinusoid (accumulates with tick).
    pub script_phase: f32,
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

    pub fn muscle_feedback(&self) -> Vec<MuscleFeedback> {
        self.muscles.iter().map(Muscle::feedback).collect()
    }

    pub fn mean_tension(&self) -> f32 {
        if self.muscles.is_empty() {
            return 0.0;
        }
        self.muscles.iter().map(|m| m.tension).sum::<f32>() / self.muscles.len() as f32
    }

    pub fn kind_at(&self, gx: i32, gy: i32) -> Option<TissueKind> {
        for p in &self.parts {
            if p.contains_world(gx, gy) {
                return Some(match p.kind {
                    PartKind::Bone => TissueKind::Bone,
                    PartKind::Fixture => TissueKind::Fixture,
                });
            }
        }
        for m in &self.muscles {
            if m.cells.iter().any(|&(x, y)| x == gx && y == gy) {
                return Some(TissueKind::Muscle);
            }
        }
        None
    }

    fn part_index(&self, id: u32) -> Option<usize> {
        self.parts.iter().position(|p| p.id == id)
    }
}

/// Flood-fill Bone / Fixture, wire joints + muscles from paint.
pub fn activate(paint: &TissuePaint) -> Result<BodyGraph, ActivateError> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut seen = HashSet::new();
    let mut parts: Vec<RigidPart> = Vec::new();
    let mut next_id = 0u32;
    let mut cell_owner: HashMap<(i32, i32), u32> = HashMap::new();

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
            let id = next_id;
            next_id += 1;
            for c in &cells {
                cell_owner.insert(*c, id);
            }
            parts.push(RigidPart {
                id,
                kind: part_kind,
                cells,
                offset_x: 0,
                offset_y: 0,
                anchored: part_kind == PartKind::Fixture,
            });
        }
    }

    if parts.is_empty() {
        return Err(ActivateError::Empty);
    }

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
            neighbors4(x, y)
                .into_iter()
                .any(|n| fixture_cells.contains(&n))
        }) {
            p.anchored = true;
        }
    }

    let joints = collect_joints(paint, &cell_owner);
    // Bones linked by a joint to an anchored part become hinged (still
    // movable under muscle — not gravity-anchored unless they touch fixture).
    let muscles = collect_muscles(paint, &cell_owner, &parts);

    Ok(BodyGraph {
        parts,
        joints,
        muscles,
        script_phase: 0.0,
    })
}

fn neighbors4(x: i32, y: i32) -> [(i32, i32); 4] {
    [(x + 1, y), (x - 1, y), (x, y + 1), (x, y - 1)]
}

fn collect_joints(paint: &TissuePaint, owner: &HashMap<(i32, i32), u32>) -> Vec<Joint> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut joints = Vec::new();
    let mut seen_pairs: HashSet<(u32, u32, i32, i32)> = HashSet::new();
    for y in 0..h {
        for x in 0..w {
            let kind = paint.get(x as u32, y as u32);
            let Some(limit) = kind.joint_limit() else {
                continue;
            };
            let mut touching: Vec<u32> = neighbors4(x, y)
                .into_iter()
                .filter_map(|n| owner.get(&n).copied())
                .collect();
            touching.sort_unstable();
            touching.dedup();
            if touching.len() < 2 {
                // Joint against fixture+bone often touches one bone + we
                // need fixture ownership — fixtures are in owner too.
                continue;
            }
            let a = touching[0];
            let b = touching[1];
            let key = (a.min(b), a.max(b), x, y);
            if !seen_pairs.insert(key) {
                continue;
            }
            joints.push(Joint {
                part_a: a,
                part_b: b,
                pivot: (x, y),
                limit,
            });
        }
    }
    joints
}

fn collect_muscles(
    paint: &TissuePaint,
    owner: &HashMap<(i32, i32), u32>,
    parts: &[RigidPart],
) -> Vec<Muscle> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut seen = HashSet::new();
    let mut muscles = Vec::new();
    let mut next = 0u32;
    for y in 0..h {
        for x in 0..w {
            if paint.get(x as u32, y as u32) != TissueKind::Muscle {
                continue;
            }
            if !seen.insert((x, y)) {
                continue;
            }
            let cells = flood(paint, x, y, TissueKind::Muscle, &mut seen);
            let mut touch: Vec<u32> = Vec::new();
            for &(cx, cy) in &cells {
                for n in neighbors4(cx, cy) {
                    if let Some(&id) = owner.get(&n) {
                        touch.push(id);
                    }
                }
            }
            touch.sort_unstable();
            touch.dedup();
            if touch.len() < 2 {
                continue;
            }
            let part_a = touch[0];
            let part_b = touch[1];
            let ca = parts.iter().find(|p| p.id == part_a).map(|p| p.centroid());
            let cb = parts.iter().find(|p| p.id == part_b).map(|p| p.centroid());
            let (Some((ax, ay)), Some((bx, by))) = (ca, cb) else {
                continue;
            };
            let rest = ((ax - bx).hypot(ay - by)).max(1.0);
            muscles.push(Muscle {
                id: next,
                part_a,
                part_b,
                cells,
                rest_length: rest,
                actuation: 0.0,
                length: rest,
                tension: 0.0,
            });
            next += 1;
        }
    }
    muscles
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
    q.push_back((sx, sy));
    while let Some((x, y)) = q.pop_front() {
        out.push((x, y));
        for (nx, ny) in neighbors4(x, y) {
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

fn occupied(graph: &BodyGraph) -> HashSet<(i32, i32)> {
    let mut set = HashSet::new();
    for p in &graph.parts {
        for c in p.world_cells() {
            set.insert(c);
        }
    }
    set
}

fn try_move_part(
    graph: &mut BodyGraph,
    world: &mut World,
    idx: usize,
    dx: i32,
    dy: i32,
) -> bool {
    if dx == 0 && dy == 0 {
        return true;
    }
    let cells: Vec<(i32, i32)> = graph.parts[idx].world_cells().collect();
    let mut occ = occupied(graph);
    for c in &cells {
        occ.remove(c);
    }
    let ok = cells.iter().all(|&(x, y)| {
        let tx = x + dx;
        let ty = y + dy;
        cell_passable(world, tx, ty) && !occ.contains(&(tx, ty))
    });
    if !ok {
        return false;
    }
    // Hydro push: shove standing water from destination toward origin.
    for &(x, y) in &cells {
        let tx = x + dx;
        let ty = y + dy;
        push_sat(world, tx, ty, x, y);
    }
    graph.parts[idx].offset_x += dx;
    graph.parts[idx].offset_y += dy;
    true
}

fn push_sat(world: &mut World, from_x: i32, from_y: i32, to_x: i32, to_y: i32) {
    let Some(src) = world.get_cell(from_x, from_y) else {
        return;
    };
    if src.material != MaterialId::Air || src.sat.is_empty() {
        return;
    }
    let Some(dst) = world.get_cell(to_x, to_y) else {
        return;
    };
    if dst.material != MaterialId::Air {
        return;
    }
    let move_amt = (src.sat.0 / 2).max(1).min(src.sat.0);
    let dst_room = 255u8.saturating_sub(dst.sat.0);
    let amt = move_amt.min(dst_room);
    if amt == 0 {
        return;
    }
    world.set_cell(
        from_x,
        from_y,
        Cell {
            sat: Sat(src.sat.0 - amt),
            ..src
        },
    );
    let dst = world.get_cell(to_x, to_y).unwrap();
    world.set_cell(
        to_x,
        to_y,
        Cell {
            sat: Sat(dst.sat.0 + amt),
            ..dst
        },
    );
}

fn refresh_muscle_state(graph: &mut BodyGraph) {
    for i in 0..graph.muscles.len() {
        let (a, b) = (graph.muscles[i].part_a, graph.muscles[i].part_b);
        let ca = graph
            .parts
            .iter()
            .find(|p| p.id == a)
            .map(|p| p.centroid());
        let cb = graph
            .parts
            .iter()
            .find(|p| p.id == b)
            .map(|p| p.centroid());
        let (Some((ax, ay)), Some((bx, by))) = (ca, cb) else {
            continue;
        };
        let len = (ax - bx).hypot(ay - by).max(0.01);
        let m = &mut graph.muscles[i];
        m.length = len;
        let target = m.rest_length * (1.0 - 0.45 * m.actuation.clamp(0.0, 1.0));
        m.tension = (len - target).abs() + m.actuation * 0.25;
    }
}

/// Drive muscles with a shared open-loop sinusoid (S2).
pub fn script_muscles(graph: &mut BodyGraph, tick: u64) {
    graph.script_phase = tick as f32 * 0.12;
    let wave = (graph.script_phase.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    for m in &mut graph.muscles {
        m.actuation = wave;
    }
}

/// Apply muscle contraction as discrete pulls on free bones.
fn apply_muscle_forces(graph: &mut BodyGraph, world: &mut World) {
    let pulls: Vec<(u32, u32, f32)> = graph
        .muscles
        .iter()
        .map(|m| (m.part_a, m.part_b, m.actuation))
        .collect();
    for (a, b, act) in pulls {
        if act < 0.55 {
            continue;
        }
        let (ia, ib) = match (graph.part_index(a), graph.part_index(b)) {
            (Some(ia), Some(ib)) => (ia, ib),
            _ => continue,
        };
        // Prefer moving the non-anchored bone toward the other.
        let (move_idx, toward_idx) = if !graph.parts[ia].anchored && graph.parts[ib].anchored {
            (ia, ib)
        } else if graph.parts[ia].anchored && !graph.parts[ib].anchored {
            (ib, ia)
        } else if !graph.parts[ia].anchored && !graph.parts[ib].anchored {
            // Move the higher-id bone for determinism.
            if graph.parts[ia].id > graph.parts[ib].id {
                (ia, ib)
            } else {
                (ib, ia)
            }
        } else {
            continue;
        };
        let (mx, my) = graph.parts[move_idx].centroid();
        let (tx, ty) = graph.parts[toward_idx].centroid();
        let dx = (tx - mx).signum() as i32;
        let dy = (ty - my).signum() as i32;
        // Prefer horizontal flap for fins.
        if dx != 0 {
            let _ = try_move_part(graph, world, move_idx, dx, 0);
        } else if dy != 0 {
            let _ = try_move_part(graph, world, move_idx, 0, dy);
        }
    }
}

/// S1/S2 body step after voxel CA.
pub fn step_body(graph: &mut BodyGraph, world: &mut World, scripted_muscle: bool) {
    if graph.parts.is_empty() {
        return;
    }
    if scripted_muscle && !graph.muscles.is_empty() {
        script_muscles(graph, world.tick);
        apply_muscle_forces(graph, world);
    }

    // Gravity on free bones (not hinged-only — still apply if unanchored).
    let mut order: Vec<usize> = (0..graph.parts.len()).collect();
    order.sort_by_key(|&i| graph.parts[i].id);
    for &idx in &order {
        if graph.parts[idx].anchored || graph.parts[idx].kind != PartKind::Bone {
            continue;
        }
        // Skip gravity pull if a muscle is actively yanking this tick.
        let busy = graph.muscles.iter().any(|m| {
            m.actuation >= 0.55 && (m.part_a == graph.parts[idx].id || m.part_b == graph.parts[idx].id)
        });
        if busy {
            continue;
        }
        let _ = try_move_part(graph, world, idx, 0, -1);
    }

    refresh_muscle_state(graph);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::arena::{ArenaConfig, StudioArena};
    use crate::physics::StudioPhysicsConfig;
    use crate::tissue::TissueKind;

    #[test]
    fn activate_hangs_bone_on_fixture() {
        let mut paint = TissuePaint::new(16, 16);
        for y in 4..12 {
            paint.set(2, y, TissueKind::Fixture);
        }
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
        arena.physics = StudioPhysicsConfig::body_only();
        arena.body.paint.set(10, 20, TissueKind::Bone);
        arena.body.paint.set(11, 20, TissueKind::Bone);
        arena.activate().unwrap();
        let y0 = arena.body.graph.as_ref().unwrap().parts[0].offset_y;
        for _ in 0..40 {
            arena.tick();
        }
        let y1 = arena.body.graph.as_ref().unwrap().parts[0].offset_y;
        assert!(y1 < y0, "free bone should fall (offset {y0} → {y1})");
    }

    #[test]
    fn hung_bone_does_not_fall() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 32,
            height: 32,
            seed: 3,
            water_to_y: None,
        });
        arena.physics = StudioPhysicsConfig::body_only();
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

    #[test]
    fn muscle_links_two_bones_and_reports_feedback() {
        let mut paint = TissuePaint::new(24, 24);
        for y in 4..14 {
            paint.set(2, y, TissueKind::Fixture);
        }
        // Bone A hung on fixture.
        paint.set(3, 8, TissueKind::Bone);
        paint.set(4, 8, TissueKind::Bone);
        // Joint + free bone B.
        paint.set(5, 8, TissueKind::JointHalf);
        paint.set(6, 8, TissueKind::Bone);
        paint.set(7, 8, TissueKind::Bone);
        paint.set(8, 8, TissueKind::Bone);
        // Muscle between A and B (touches both).
        paint.set(4, 9, TissueKind::Muscle);
        paint.set(5, 9, TissueKind::Muscle);
        paint.set(6, 9, TissueKind::Muscle);

        let g = activate(&paint).unwrap();
        assert!(g.muscles.len() >= 1, "expected muscle link");
        assert!(!g.muscle_feedback().is_empty());
    }

    #[test]
    fn scripted_muscle_moves_free_bone() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 48,
            height: 32,
            seed: 9,
            water_to_y: None,
        });
        arena.physics = StudioPhysicsConfig::body_only();
        for y in 4..20 {
            arena.body.paint.set(2, y as u32, TissueKind::Fixture);
        }
        arena.body.paint.set(3, 10, TissueKind::Bone);
        arena.body.paint.set(4, 10, TissueKind::Bone);
        arena.body.paint.set(5, 10, TissueKind::JointHalf);
        arena.body.paint.set(6, 10, TissueKind::Bone);
        arena.body.paint.set(7, 10, TissueKind::Bone);
        arena.body.paint.set(8, 10, TissueKind::Bone);
        arena.body.paint.set(4, 11, TissueKind::Muscle);
        arena.body.paint.set(5, 11, TissueKind::Muscle);
        arena.body.paint.set(6, 11, TissueKind::Muscle);
        arena.activate().unwrap();

        let free_id = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.kind == PartKind::Bone && !p.anchored)
            .map(|p| p.id)
            .expect("free bone");
        let x0 = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == free_id)
            .unwrap()
            .offset_x;
        for _ in 0..80 {
            arena.tick();
        }
        let x1 = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == free_id)
            .unwrap()
            .offset_x;
        let tension = arena.body.graph.as_ref().unwrap().mean_tension();
        assert!(
            x1 != x0 || tension > 0.01,
            "scripted muscle should move bone or report tension (x {x0}→{x1}, T={tension})"
        );
    }
}
