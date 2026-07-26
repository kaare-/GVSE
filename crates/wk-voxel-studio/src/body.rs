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

/// One rigid connected component in paint space, plus live pose.
///
/// Free bones translate via `offset_*`. Hinged bones keep `offset_*=0` and
/// store absolute cells rewritten from [`Joint::local_cells`] × `hinge_angle`
/// around the live pivot (true articulation — not sideways slide).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RigidPart {
    pub id: u32,
    pub kind: PartKind,
    pub cells: Vec<(i32, i32)>,
    pub offset_x: i32,
    pub offset_y: i32,
    /// Fixture or bone welded to a fixture — never moves.
    pub anchored: bool,
    /// Joint-linked to an anchored part — no gravity, may articulate.
    #[serde(default)]
    pub hinged: bool,
    /// Radians from rest pose for hinged bones (0 = as painted).
    #[serde(default)]
    pub hinge_angle: f32,
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
    /// Paint-space joint cell (rest pose).
    pub pivot: (i32, i32),
    pub limit: JointLimit,
    /// Parent link (closer to fixture root).
    #[serde(default)]
    pub parent_part: u32,
    /// Child bone that rotates about this hinge.
    #[serde(default)]
    pub swing_part: u32,
    /// Swing-part cells relative to [`Self::pivot`] at rest (activate pose).
    #[serde(default)]
    pub local_cells: Vec<(i32, i32)>,
    /// Unused historically; angle delta lives on [`RigidPart::hinge_angle`].
    #[serde(default)]
    pub rest_angle: f32,
    /// Centroid distance from pivot to the swinging part at activate.
    #[serde(default = "default_rest_radius")]
    pub rest_radius: f32,
}

fn default_rest_radius() -> f32 {
    1.0
}

/// Discrete hinge step (~12°).
const HINGE_STEP: f32 = 0.22;

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

/// Painted nerve thread (1-px signal path).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NerveStrand {
    pub id: u32,
    pub cells: Vec<(i32, i32)>,
    /// Muscle ids this strand touches (4-neighbour).
    pub muscle_ids: Vec<u32>,
    /// Neuron cluster ids this strand touches.
    pub neuron_ids: Vec<u32>,
}

/// Connected NeuronBlob mass — controller site when area ≥ 2×2.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeuronCluster {
    pub id: u32,
    pub cells: Vec<(i32, i32)>,
    /// True when the blob spans at least 2×2 (processing mass).
    pub is_controller: bool,
}

/// Activated studio body for simulation.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BodyGraph {
    pub parts: Vec<RigidPart>,
    pub joints: Vec<Joint>,
    pub muscles: Vec<Muscle>,
    pub nerves: Vec<NerveStrand>,
    pub neurons: Vec<NeuronCluster>,
    /// At least one controller-sized neuron blob is present.
    pub has_controller: bool,
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

    pub fn hinged_bone_count(&self) -> usize {
        self.parts
            .iter()
            .filter(|p| p.kind == PartKind::Bone && p.hinged)
            .count()
    }

    /// World-space joint pivot cells for drawing after activate.
    pub fn joint_world_pivots(&self) -> Vec<(i32, i32)> {
        self.joints.iter().map(|j| pivot_world(self, j)).collect()
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
            if muscle_world_cells(self, m)
                .into_iter()
                .any(|(x, y)| x == gx && y == gy)
            {
                return Some(TissueKind::Muscle);
            }
        }
        for n in &self.neurons {
            if n.cells.iter().any(|&(x, y)| x == gx && y == gy) {
                return Some(TissueKind::NeuronBlob);
            }
        }
        for n in &self.nerves {
            if n.cells.iter().any(|&(x, y)| x == gx && y == gy) {
                return Some(TissueKind::Nerve);
            }
        }
        None
    }

    /// Signed hinge angle of the swinging bone (radians from rest).
    pub fn joint_angle_delta(&self, joint_idx: usize) -> Option<f32> {
        let j = self.joints.get(joint_idx)?;
        let swing = self.part_index(j.swing_part)?;
        Some(self.parts[swing].hinge_angle)
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
                hinged: false,
                hinge_angle: 0.0,
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

    let mut joints = collect_joints(paint, &cell_owner);
    // Bones linked by a joint chain to an anchored part articulate (no fall-off).
    mark_hinged_from_joints(&mut parts, &joints);
    fill_joint_rest_poses(&mut joints, &parts);
    let muscles = collect_muscles(paint, &cell_owner, &parts, &joints);
    let neurons = collect_neurons(paint);
    let nerves = collect_nerves(paint, &muscles, &neurons);
    let has_controller = neurons.iter().any(|n| n.is_controller);

    Ok(BodyGraph {
        parts,
        joints,
        muscles,
        nerves,
        neurons,
        has_controller,
        script_phase: 0.0,
    })
}

/// Count ForceSensor cells that sit between two rigid parts — common Joint mix-up.
pub fn force_sensors_bridging_parts(paint: &TissuePaint) -> usize {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut owner: HashMap<(i32, i32), u32> = HashMap::new();
    let mut next = 1u32;
    let mut seen = HashSet::new();
    for y in 0..h {
        for x in 0..w {
            let kind = paint.get(x as u32, y as u32);
            if !matches!(kind, TissueKind::Bone | TissueKind::Fixture) {
                continue;
            }
            if !seen.insert((x, y)) {
                continue;
            }
            let cells = flood(paint, x, y, kind, &mut seen);
            for c in cells {
                owner.insert(c, next);
            }
            next += 1;
        }
    }
    let mut n = 0usize;
    for y in 0..h {
        for x in 0..w {
            if paint.get(x as u32, y as u32) != TissueKind::ForceSensor {
                continue;
            }
            let mut ids: Vec<u32> = neighbors4(x, y)
                .into_iter()
                .filter_map(|c| owner.get(&c).copied())
                .collect();
            ids.sort_unstable();
            ids.dedup();
            if ids.len() >= 2 {
                n += 1;
            }
        }
    }
    n
}

/// BFS through joints from anchored parts → mark distal bones as hinged.
fn mark_hinged_from_joints(parts: &mut [RigidPart], joints: &[Joint]) {
    let mut supported: HashSet<u32> = parts
        .iter()
        .filter(|p| p.anchored)
        .map(|p| p.id)
        .collect();
    let mut q: VecDeque<u32> = supported.iter().copied().collect();
    while let Some(id) = q.pop_front() {
        for j in joints {
            let other = if j.part_a == id {
                j.part_b
            } else if j.part_b == id {
                j.part_a
            } else {
                continue;
            };
            if supported.insert(other) {
                q.push_back(other);
            }
        }
    }
    for p in parts.iter_mut() {
        if p.kind == PartKind::Bone && !p.anchored && supported.contains(&p.id) {
            p.hinged = true;
        }
    }
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
                // u32::MAX = unset (0 is a valid part id).
                parent_part: u32::MAX,
                swing_part: u32::MAX,
                local_cells: Vec::new(),
                rest_angle: 0.0,
                rest_radius: 1.0,
            });
        }
    }
    joints
}

/// Graph distance from fixture-anchored roots along joints.
fn part_root_dist(parts: &[RigidPart], joints: &[Joint]) -> HashMap<u32, u32> {
    let mut dist: HashMap<u32, u32> = HashMap::new();
    let mut q = VecDeque::new();
    for p in parts {
        if p.anchored {
            dist.insert(p.id, 0);
            q.push_back(p.id);
        }
    }
    while let Some(id) = q.pop_front() {
        let d = dist[&id];
        for j in joints {
            let other = if j.part_a == id {
                j.part_b
            } else if j.part_b == id {
                j.part_a
            } else {
                continue;
            };
            if let std::collections::hash_map::Entry::Vacant(e) = dist.entry(other) {
                e.insert(d + 1);
                q.push_back(other);
            }
        }
    }
    dist
}

fn fill_joint_rest_poses(joints: &mut [Joint], parts: &[RigidPart]) {
    let dist = part_root_dist(parts, joints);
    for j in joints.iter_mut() {
        let da = dist.get(&j.part_a).copied();
        let db = dist.get(&j.part_b).copied();
        let (parent_id, swing_id) = match (da, db) {
            (Some(a), Some(b)) if a < b => (j.part_a, j.part_b),
            (Some(a), Some(b)) if b < a => (j.part_b, j.part_a),
            (Some(_), None) => (j.part_a, j.part_b),
            (None, Some(_)) => (j.part_b, j.part_a),
            _ => continue,
        };
        let Some(swing) = parts.iter().find(|p| p.id == swing_id && p.hinged) else {
            continue;
        };
        j.parent_part = parent_id;
        j.swing_part = swing_id;
        j.local_cells = swing
            .cells
            .iter()
            .map(|(x, y)| (*x - j.pivot.0, *y - j.pivot.1))
            .collect();
        let (cx, cy) = swing.centroid();
        let (px, py) = (j.pivot.0 as f32, j.pivot.1 as f32);
        j.rest_angle = 0.0;
        j.rest_radius = (cx - px).hypot(cy - py).max(1.0);
    }
}

fn rotate_local_cells(
    local: &[(i32, i32)],
    pivot: (i32, i32),
    angle: f32,
) -> Vec<(i32, i32)> {
    let (s, c) = angle.sin_cos();
    let mut out: Vec<(i32, i32)> = local
        .iter()
        .map(|&(lx, ly)| {
            let fx = lx as f32 * c - ly as f32 * s;
            let fy = lx as f32 * s + ly as f32 * c;
            (pivot.0 + fx.round() as i32, pivot.1 + fy.round() as i32)
        })
        .collect();
    out.sort_unstable();
    out.dedup();
    out
}

fn collect_muscles(
    paint: &TissuePaint,
    owner: &HashMap<(i32, i32), u32>,
    parts: &[RigidPart],
    joints: &[Joint],
) -> Vec<Muscle> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let joint_parts: HashMap<(i32, i32), (u32, u32)> = joints
        .iter()
        .map(|j| (j.pivot, (j.part_a, j.part_b)))
        .collect();
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
                    // Muscle beside a joint counts as spanning that hinge.
                    if let Some(&(a, b)) = joint_parts.get(&n) {
                        touch.push(a);
                        touch.push(b);
                    }
                    if paint.get(n.0 as u32, n.1 as u32).joint_limit().is_some() {
                        // Joint cell may not be in joint_parts if wiring failed;
                        // still try owner neighbors of the joint for a second part.
                        for jn in neighbors4(n.0, n.1) {
                            if let Some(&id) = owner.get(&jn) {
                                touch.push(id);
                            }
                        }
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

fn collect_neurons(paint: &TissuePaint) -> Vec<NeuronCluster> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut next = 0u32;
    for y in 0..h {
        for x in 0..w {
            if paint.get(x as u32, y as u32) != TissueKind::NeuronBlob {
                continue;
            }
            if !seen.insert((x, y)) {
                continue;
            }
            let cells = flood(paint, x, y, TissueKind::NeuronBlob, &mut seen);
            if cells.is_empty() {
                continue;
            }
            let min_x = cells.iter().map(|c| c.0).min().unwrap_or(0);
            let max_x = cells.iter().map(|c| c.0).max().unwrap_or(0);
            let min_y = cells.iter().map(|c| c.1).min().unwrap_or(0);
            let max_y = cells.iter().map(|c| c.1).max().unwrap_or(0);
            let span_w = (max_x - min_x + 1) as usize;
            let span_h = (max_y - min_y + 1) as usize;
            // Spec: ≥2×2 nerve mass = processing / controller site.
            let is_controller = span_w >= 2 && span_h >= 2 && cells.len() >= 4;
            out.push(NeuronCluster {
                id: next,
                cells,
                is_controller,
            });
            next += 1;
        }
    }
    out
}

fn collect_nerves(
    paint: &TissuePaint,
    muscles: &[Muscle],
    neurons: &[NeuronCluster],
) -> Vec<NerveStrand> {
    let w = paint.width as i32;
    let h = paint.height as i32;
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    let mut next = 0u32;
    let muscle_at = |x: i32, y: i32| -> Option<u32> {
        muscles
            .iter()
            .find(|m| m.cells.iter().any(|&c| c == (x, y)))
            .map(|m| m.id)
    };
    let neuron_at = |x: i32, y: i32| -> Option<u32> {
        neurons
            .iter()
            .find(|n| n.cells.iter().any(|&c| c == (x, y)))
            .map(|n| n.id)
    };
    for y in 0..h {
        for x in 0..w {
            if paint.get(x as u32, y as u32) != TissueKind::Nerve {
                continue;
            }
            if !seen.insert((x, y)) {
                continue;
            }
            let cells = flood(paint, x, y, TissueKind::Nerve, &mut seen);
            let mut muscle_ids = Vec::new();
            let mut neuron_ids = Vec::new();
            for &(cx, cy) in &cells {
                for (nx, ny) in neighbors4(cx, cy) {
                    if let Some(id) = muscle_at(nx, ny) {
                        muscle_ids.push(id);
                    }
                    if let Some(id) = neuron_at(nx, ny) {
                        neuron_ids.push(id);
                    }
                }
            }
            muscle_ids.sort_unstable();
            muscle_ids.dedup();
            neuron_ids.sort_unstable();
            neuron_ids.dedup();
            out.push(NerveStrand {
                id: next,
                cells,
                muscle_ids,
                neuron_ids,
            });
            next += 1;
        }
    }
    out
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
    if graph.parts[idx].anchored || graph.parts[idx].hinged {
        // Hinged bones articulate via [`try_set_hinge_angle`], not translation.
        return false;
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

/// Move several free parts together (free-floating assemblies under gravity).
fn try_move_parts(
    graph: &mut BodyGraph,
    world: &mut World,
    indices: &[usize],
    dx: i32,
    dy: i32,
) -> bool {
    if dx == 0 && dy == 0 || indices.is_empty() {
        return true;
    }
    if indices.iter().any(|&i| graph.parts[i].anchored) {
        return false;
    }
    let mut moving: HashSet<(i32, i32)> = HashSet::new();
    let mut cells_by_part: Vec<(usize, Vec<(i32, i32)>)> = Vec::new();
    for &idx in indices {
        let cells: Vec<(i32, i32)> = graph.parts[idx].world_cells().collect();
        for c in &cells {
            moving.insert(*c);
        }
        cells_by_part.push((idx, cells));
    }
    let mut occ = occupied(graph);
    for c in &moving {
        occ.remove(c);
    }
    for (_idx, cells) in &cells_by_part {
        for &(x, y) in cells {
            let tx = x + dx;
            let ty = y + dy;
            if !cell_passable(world, tx, ty) {
                return false;
            }
            // Allow landing in a cell another moving part is vacating.
            if occ.contains(&(tx, ty)) && !moving.contains(&(tx, ty)) {
                return false;
            }
        }
    }
    for (_idx, cells) in &cells_by_part {
        for &(x, y) in cells {
            push_sat(world, x + dx, y + dy, x, y);
        }
    }
    for &idx in indices {
        graph.parts[idx].offset_x += dx;
        graph.parts[idx].offset_y += dy;
    }
    true
}

fn chebyshev(a: (i32, i32), b: (i32, i32)) -> i32 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

fn joint_for_swing<'a>(graph: &'a BodyGraph, swing_id: u32) -> Option<&'a Joint> {
    graph
        .joints
        .iter()
        .find(|j| j.swing_part == swing_id && !j.local_cells.is_empty())
}

/// Sum of relative hinge angles from the fixture root through `part_id`.
fn cumulative_angle(graph: &BodyGraph, part_id: u32) -> f32 {
    let mut total = 0.0;
    let mut cur = part_id;
    for _ in 0..32 {
        let Some(j) = joint_for_swing(graph, cur) else {
            break;
        };
        let Some(idx) = graph.part_index(cur) else {
            break;
        };
        total += graph.parts[idx].hinge_angle;
        cur = j.parent_part;
        if graph
            .part_index(cur)
            .is_some_and(|i| graph.parts[i].anchored)
        {
            break;
        }
    }
    total
}

/// World-space hinge pivot — follows parent link pose in a serial chain.
fn pivot_world(graph: &BodyGraph, joint: &Joint) -> (i32, i32) {
    if joint.local_cells.is_empty() || joint.swing_part == u32::MAX {
        return joint.pivot;
    }
    let Some(pi) = graph.part_index(joint.parent_part) else {
        return joint.pivot;
    };
    let parent = &graph.parts[pi];
    if parent.anchored || !parent.hinged {
        return (
            joint.pivot.0 + parent.offset_x,
            joint.pivot.1 + parent.offset_y,
        );
    }
    let Some(pj) = joint_for_swing(graph, joint.parent_part) else {
        return joint.pivot;
    };
    let pp = pivot_world(graph, pj);
    let parent_cum = cumulative_angle(graph, joint.parent_part);
    let lx = (joint.pivot.0 - pj.pivot.0) as f32;
    let ly = (joint.pivot.1 - pj.pivot.1) as f32;
    let (s, c) = parent_cum.sin_cos();
    (
        pp.0 + (lx * c - ly * s).round() as i32,
        pp.1 + (lx * s + ly * c).round() as i32,
    )
}

/// Swing parts that must be re-posed after `root_swing` moves (self + children).
fn chain_from(graph: &BodyGraph, root_swing: u32) -> Vec<u32> {
    let dist = part_root_dist(&graph.parts, &graph.joints);
    let root_d = dist.get(&root_swing).copied().unwrap_or(0);
    let mut ids: Vec<u32> = graph
        .joints
        .iter()
        .filter(|j| j.swing_part != u32::MAX && !j.local_cells.is_empty())
        .filter(|j| {
            let d = dist.get(&j.swing_part).copied().unwrap_or(0);
            d >= root_d
                && (j.swing_part == root_swing
                    || is_descendant_of(graph, j.swing_part, root_swing))
        })
        .map(|j| j.swing_part)
        .collect();
    ids.sort_by_key(|id| dist.get(id).copied().unwrap_or(0));
    ids.dedup();
    if !ids.contains(&root_swing) {
        ids.insert(0, root_swing);
    }
    ids
}

fn is_descendant_of(graph: &BodyGraph, part: u32, ancestor: u32) -> bool {
    let mut cur = part;
    for _ in 0..32 {
        let Some(j) = joint_for_swing(graph, cur) else {
            return false;
        };
        if j.parent_part == ancestor {
            return true;
        }
        cur = j.parent_part;
        if graph
            .part_index(cur)
            .is_some_and(|i| graph.parts[i].anchored)
        {
            return false;
        }
    }
    false
}

fn stamp_swing_cells(graph: &BodyGraph, joint: &Joint) -> Option<Vec<(i32, i32)>> {
    let pivot = pivot_world(graph, joint);
    let cum = cumulative_angle(graph, joint.swing_part);
    let cells = rotate_local_cells(&joint.local_cells, pivot, cum);
    if cells.is_empty() || !cells.iter().any(|&c| chebyshev(c, pivot) <= 1) {
        return None;
    }
    Some(cells)
}

#[cfg(test)]
fn part_touches_pivot(part: &RigidPart, pivot: (i32, i32)) -> bool {
    part.world_cells().any(|c| chebyshev(c, pivot) <= 1)
}

#[cfg(test)]
fn joints_satisfied(graph: &BodyGraph) -> bool {
    graph.joints.iter().all(|j| {
        if j.local_cells.is_empty() {
            return true;
        }
        let Some(si) = graph.part_index(j.swing_part) else {
            return true;
        };
        let p = pivot_world(graph, j);
        part_touches_pivot(&graph.parts[si], p)
    })
}

/// Apply a relative hinge angle; re-poses this link and all distal children.
fn try_set_hinge_angle(
    graph: &mut BodyGraph,
    world: &World,
    joint: &Joint,
    angle: f32,
) -> bool {
    if joint.local_cells.is_empty() {
        return false;
    }
    let Some(swing_idx) = graph.part_index(joint.swing_part) else {
        return false;
    };
    if !graph.parts[swing_idx].hinged || !hinge_angle_within_limit(joint, angle) {
        return false;
    }
    let affected = chain_from(graph, joint.swing_part);
    let saved: Vec<(u32, Vec<(i32, i32)>, f32)> = affected
        .iter()
        .filter_map(|&id| {
            let idx = graph.part_index(id)?;
            Some((
                id,
                graph.parts[idx].cells.clone(),
                graph.parts[idx].hinge_angle,
            ))
        })
        .collect();
    graph.parts[swing_idx].hinge_angle = angle;

    let mut occ = occupied(graph);
    for &(id, _, _) in &saved {
        if let Some(idx) = graph.part_index(id) {
            for c in graph.parts[idx].world_cells() {
                occ.remove(&c);
            }
        }
    }

    let snaps: Vec<Joint> = affected
        .iter()
        .filter_map(|&id| joint_for_swing(graph, id).cloned())
        .collect();
    for j in &snaps {
        let Some(new_cells) = stamp_swing_cells(graph, j) else {
            // revert
            for (id, cells, ang) in saved {
                if let Some(idx) = graph.part_index(id) {
                    graph.parts[idx].cells = cells;
                    graph.parts[idx].hinge_angle = ang;
                }
            }
            return false;
        };
        for &(x, y) in &new_cells {
            if !cell_passable(world, x, y) || occ.contains(&(x, y)) {
                for (id, cells, ang) in saved {
                    if let Some(idx) = graph.part_index(id) {
                        graph.parts[idx].cells = cells;
                        graph.parts[idx].hinge_angle = ang;
                    }
                }
                return false;
            }
        }
        let Some(idx) = graph.part_index(j.swing_part) else {
            continue;
        };
        for c in &new_cells {
            occ.insert(*c);
        }
        graph.parts[idx].cells = new_cells;
        graph.parts[idx].offset_x = 0;
        graph.parts[idx].offset_y = 0;
    }
    true
}

fn hinge_angle_within_limit(joint: &Joint, angle: f32) -> bool {
    if matches!(joint.limit, JointLimit::Full) {
        return true;
    }
    let max = joint.limit.max_turns() * std::f32::consts::TAU;
    angle.abs() <= max + 0.02
}

/// Re-stamp all hinged links root→leaf so child pivots follow parents.
fn enforce_joints(graph: &mut BodyGraph, _world: &mut World) {
    let dist = part_root_dist(&graph.parts, &graph.joints);
    let mut order: Vec<Joint> = graph
        .joints
        .iter()
        .filter(|j| !j.local_cells.is_empty())
        .cloned()
        .collect();
    order.sort_by_key(|j| dist.get(&j.swing_part).copied().unwrap_or(0));
    for j in order {
        if let Some(cells) = stamp_swing_cells(graph, &j) {
            if let Some(idx) = graph.part_index(j.swing_part) {
                graph.parts[idx].cells = cells;
                graph.parts[idx].offset_x = 0;
                graph.parts[idx].offset_y = 0;
            }
        }
    }
}

/// Joint-connected component ids (part ids).
fn joint_components(graph: &BodyGraph) -> Vec<Vec<usize>> {
    let n = graph.parts.len();
    let mut parent: Vec<usize> = (0..n).collect();
    fn find(parent: &mut [usize], i: usize) -> usize {
        let mut i = i;
        while parent[i] != i {
            parent[i] = parent[parent[i]];
            i = parent[i];
        }
        i
    }
    fn unite(parent: &mut [usize], a: usize, b: usize) {
        let pa = find(parent, a);
        let pb = find(parent, b);
        if pa != pb {
            parent[pa] = pb;
        }
    }
    for j in &graph.joints {
        if let (Some(ia), Some(ib)) = (graph.part_index(j.part_a), graph.part_index(j.part_b)) {
            unite(&mut parent, ia, ib);
        }
    }
    let mut groups: HashMap<usize, Vec<usize>> = HashMap::new();
    for i in 0..n {
        let root = find(&mut parent, i);
        groups.entry(root).or_default().push(i);
    }
    groups.into_values().collect()
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

/// Drive muscles with open-loop sinusoids (S2).
///
/// Antagonist phasing: muscle `id` gets a π·id shift so left/right pairs
/// alternate instead of co-contracting into the joint.
pub fn script_muscles(graph: &mut BodyGraph, tick: u64) {
    graph.script_phase = tick as f32 * 0.12;
    for m in &mut graph.muscles {
        let phase = graph.script_phase + m.id as f32 * std::f32::consts::PI;
        m.actuation = (phase.sin() * 0.5 + 0.5).clamp(0.0, 1.0);
    }
}

/// World cells for a muscle blob (rotates with its hinged bone around the pivot).
fn muscle_world_cells(graph: &BodyGraph, m: &Muscle) -> Vec<(i32, i32)> {
    let dist = part_root_dist(&graph.parts, &graph.joints);
    // Prefer the more distal hinged attachment so child-link muscles follow the chain.
    let mut hinged_ids: Vec<u32> = [m.part_a, m.part_b]
        .into_iter()
        .filter(|&id| {
            graph
                .parts
                .iter()
                .any(|p| p.id == id && p.hinged && joint_for_swing(graph, id).is_some())
        })
        .collect();
    hinged_ids.sort_by_key(|id| std::cmp::Reverse(dist.get(id).copied().unwrap_or(0)));
    if let Some(&id) = hinged_ids.first() {
        if let Some(j) = joint_for_swing(graph, id) {
            let pivot = pivot_world(graph, j);
            let cum = cumulative_angle(graph, id);
            let local: Vec<(i32, i32)> = m
                .cells
                .iter()
                .map(|(x, y)| (*x - j.pivot.0, *y - j.pivot.1))
                .collect();
            return rotate_local_cells(&local, pivot, cum);
        }
    }
    for &id in &[m.part_a, m.part_b] {
        if let Some(p) = graph
            .parts
            .iter()
            .find(|p| p.id == id && !p.anchored && p.kind == PartKind::Bone)
        {
            return m
                .cells
                .iter()
                .map(|(x, y)| (*x + p.offset_x, *y + p.offset_y))
                .collect();
        }
    }
    m.cells.clone()
}

/// Contractile length: rotated muscle tissue → non-swinging attachment.
fn muscle_span_length(graph: &BodyGraph, m: &Muscle) -> f32 {
    let Some(ia) = graph.part_index(m.part_a) else {
        return 0.0;
    };
    let Some(ib) = graph.part_index(m.part_b) else {
        return 0.0;
    };
    let other = if graph.parts[ia].hinged && !graph.parts[ib].hinged {
        ib
    } else if graph.parts[ib].hinged && !graph.parts[ia].hinged {
        ia
    } else if !graph.parts[ia].anchored && graph.parts[ib].anchored {
        ib
    } else if !graph.parts[ib].anchored && graph.parts[ia].anchored {
        ia
    } else {
        ib
    };
    let cells = muscle_world_cells(graph, m);
    if cells.is_empty() {
        return 0.0;
    }
    let n = cells.len() as f32;
    let mx = cells.iter().map(|c| c.0 as f32).sum::<f32>() / n;
    let my = cells.iter().map(|c| c.1 as f32).sum::<f32>() / n;
    let mut best = f32::MAX;
    for (x, y) in graph.parts[other].world_cells() {
        best = best.min((mx - x as f32).hypot(my - y as f32));
    }
    if best.is_finite() {
        best
    } else {
        0.0
    }
}

fn shared_joint<'a>(graph: &'a BodyGraph, a: u32, b: u32) -> Option<&'a Joint> {
    graph.joints.iter().find(|j| {
        (j.part_a == a && j.part_b == b) || (j.part_b == a && j.part_a == b)
    })
}

/// Preferred hinge rotation sign from muscle placement (cross product).
/// Negative ⇒ clockwise in screen space with +y up.
fn muscle_torque_sign(graph: &BodyGraph, joint: &Joint, muscle: &Muscle) -> f32 {
    let Some(si) = graph.part_index(joint.swing_part) else {
        return 0.0;
    };
    let pivot = pivot_world(graph, joint);
    let (cx, cy) = graph.parts[si].centroid();
    let cells = muscle_world_cells(graph, muscle);
    if cells.is_empty() {
        return 0.0;
    }
    let n = cells.len() as f32;
    let mx = cells.iter().map(|c| c.0 as f32).sum::<f32>() / n;
    let my = cells.iter().map(|c| c.1 as f32).sum::<f32>() / n;
    let rx = cx - pivot.0 as f32;
    let ry = cy - pivot.1 as f32;
    let mux = mx - pivot.0 as f32;
    let muy = my - pivot.1 as f32;
    let cross = rx * muy - ry * mux;
    if cross.abs() < 1e-3 {
        // Vertical hang fallback: muscle on +x pulls tip toward −x (CW).
        if mux.abs() < 1e-3 {
            return 0.0;
        }
        return -mux.signum();
    }
    // Contract toward the muscle side (shorten that path).
    -cross.signum()
}

/// Rotate the hinge; prefer span shortening, else torque direction.
fn try_hinge_muscle_step(
    graph: &mut BodyGraph,
    world: &World,
    joint: &Joint,
    muscle: &Muscle,
) -> bool {
    let Some(swing_idx) = graph.part_index(joint.swing_part) else {
        return false;
    };
    let current = graph.parts[swing_idx].hinge_angle;
    let before = muscle_span_length(graph, muscle);
    let torque = muscle_torque_sign(graph, joint, muscle);
    let mut dirs = [HINGE_STEP, -HINGE_STEP, HINGE_STEP * 0.5, -HINGE_STEP * 0.5];
    if torque < 0.0 {
        dirs = [-HINGE_STEP, -HINGE_STEP * 0.5, HINGE_STEP, HINGE_STEP * 0.5];
    } else if torque > 0.0 {
        dirs = [HINGE_STEP, HINGE_STEP * 0.5, -HINGE_STEP, -HINGE_STEP * 0.5];
    }
    let mut best: Option<(f32, f32)> = None; // angle, score
    for dir in dirs {
        let cand = current + dir;
        let saved_cells = graph.parts[swing_idx].cells.clone();
        let saved_ang = graph.parts[swing_idx].hinge_angle;
        if !try_set_hinge_angle(graph, world, joint, cand) {
            continue;
        }
        let len = muscle_span_length(graph, muscle);
        let gain = before - len;
        let toward_torque = torque != 0.0 && dir.signum() == torque;
        // Restore probe pose.
        graph.parts[swing_idx].cells = saved_cells;
        graph.parts[swing_idx].hinge_angle = saved_ang;
        // Span shorten wins; otherwise accept a valid torque step even if the
        // discrete length metric is flat (common for short side muscles).
        if gain > 0.02 || (toward_torque && gain >= -0.25) {
            let score = gain + if toward_torque { 0.5 } else { 0.0 };
            let replace = best.map(|(_, s)| score > s).unwrap_or(true);
            if replace {
                best = Some((cand, score));
            }
        }
    }
    if let Some((ang, _)) = best {
        return try_set_hinge_angle(graph, world, joint, ang);
    }
    false
}

/// Apply muscle contraction as hinge rotations (or linear pulls without a joint).
fn apply_muscle_forces(graph: &mut BodyGraph, world: &mut World) {
    let pulls: Vec<usize> = graph
        .muscles
        .iter()
        .enumerate()
        .filter(|(_, m)| m.actuation >= 0.55)
        .map(|(i, _)| i)
        .collect();
    for mi in pulls {
        let (a, b) = {
            let m = &graph.muscles[mi];
            (m.part_a, m.part_b)
        };
        if let Some(j) = shared_joint(graph, a, b).cloned() {
            let muscle = graph.muscles[mi].clone();
            let _ = try_hinge_muscle_step(graph, world, &j, &muscle);
            continue;
        }

        let (ia, ib) = match (graph.part_index(a), graph.part_index(b)) {
            (Some(ia), Some(ib)) => (ia, ib),
            _ => continue,
        };
        let move_idx = if !graph.parts[ia].anchored && !graph.parts[ia].hinged {
            ia
        } else if !graph.parts[ib].anchored && !graph.parts[ib].hinged {
            ib
        } else {
            continue;
        };
        let toward_idx = if move_idx == ia { ib } else { ia };
        let (mx, my) = graph.parts[move_idx].centroid();
        let mut best = graph.parts[toward_idx].centroid();
        let mut best_d = f32::MAX;
        for (x, y) in graph.parts[toward_idx].world_cells() {
            let d = (x as f32 - mx).hypot(y as f32 - my);
            if d < best_d {
                best_d = d;
                best = (x as f32, y as f32);
            }
        }
        let dx = (best.0 - mx).signum() as i32;
        let dy = (best.1 - my).signum() as i32;
        if dx != 0 {
            let _ = try_move_part(graph, world, move_idx, dx, 0);
        } else if dy != 0 {
            let _ = try_move_part(graph, world, move_idx, 0, dy);
        }
    }
}

fn apply_gravity(graph: &mut BodyGraph, world: &mut World) {
    let mut comps = joint_components(graph);
    comps.sort_by_key(|c| c.iter().map(|&i| graph.parts[i].id).min().unwrap_or(0));
    for mut indices in comps {
        indices.sort_by_key(|&i| graph.parts[i].id);
        let has_anchor = indices.iter().any(|&i| graph.parts[i].anchored);
        if has_anchor {
            // Fixture-rooted chain: hinged bones stay up (muscle articulates).
            continue;
        }
        // Free-floating assembly — fall together so joints don't separate.
        let movable: Vec<usize> = indices
            .into_iter()
            .filter(|&i| graph.parts[i].kind == PartKind::Bone)
            .collect();
        if movable.is_empty() {
            continue;
        }
        let busy = movable.iter().any(|&idx| {
            let id = graph.parts[idx].id;
            graph.muscles.iter().any(|m| {
                m.actuation >= 0.55 && (m.part_a == id || m.part_b == id)
            })
        });
        if busy {
            continue;
        }
        let _ = try_move_parts(graph, world, &movable, 0, -1);
    }
}

/// S1/S2 body step after voxel CA.
///
/// When `scripted_muscle` is true, open-loop sinusoid overwrites actuation.
/// When false, actuation is left as set by the neural controller (or caller).
/// Muscle forces always run when muscles exist so net-driven episodes move.
/// Joints keep hinged bones attached to their fixture-rooted chain.
pub fn step_body(graph: &mut BodyGraph, world: &mut World, scripted_muscle: bool) {
    if graph.parts.is_empty() {
        return;
    }
    if !graph.muscles.is_empty() {
        if scripted_muscle {
            script_muscles(graph, world.tick);
        }
        apply_muscle_forces(graph, world);
    }

    apply_gravity(graph, world);
    enforce_joints(graph, world);
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
    fn hinged_distal_bone_does_not_fall_off() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 48,
            height: 32,
            seed: 11,
            water_to_y: None,
        });
        arena.physics = StudioPhysicsConfig::body_only();
        // Scripted off so net/noise isn't required — pure gravity + joint.
        arena.physics.scripted_muscle = false;
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

        let g = arena.activate().unwrap();
        assert!(g.joints.len() >= 1, "expected joint between bones");
        assert!(
            g.hinged_bone_count() >= 1,
            "distal bone should be hinged to fixture chain"
        );
        let distal_id = g
            .parts
            .iter()
            .find(|p| p.kind == PartKind::Bone && p.hinged)
            .map(|p| p.id)
            .expect("hinged bone");
        for _ in 0..50 {
            arena.tick();
        }
        let distal = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == distal_id)
            .unwrap();
        assert_eq!(
            distal.offset_y, 0,
            "hinged distal bone must not fall off (offset_y={})",
            distal.offset_y
        );
        assert!(
            joints_satisfied(arena.body.graph.as_ref().unwrap()),
            "joint hinge must remain satisfied"
        );
    }

    #[test]
    fn vertical_arm_swings_not_compresses() {
        use crate::scenarios::vertical_arm_arena;
        let mut arena = vertical_arm_arena(JointLimit::Half);
        arena.physics.scripted_muscle = true;
        let g = arena.activate().unwrap();
        assert!(g.joints.len() >= 1);
        assert!(g.muscles.len() >= 2);
        assert!(!g.joints[0].local_cells.is_empty());
        let distal_id = g.joints[0].swing_part;
        let mut max_abs_angle = 0.0f32;
        for _ in 0..160 {
            arena.tick();
            let g = arena.body.graph.as_ref().unwrap();
            let p = g.parts.iter().find(|p| p.id == distal_id).unwrap();
            max_abs_angle = max_abs_angle.max(p.hinge_angle.abs());
            assert_eq!(p.offset_x, 0, "no sideways slide");
            assert_eq!(p.offset_y, 0, "no compress slide");
        }
        assert!(
            max_abs_angle > 0.1,
            "antagonists should rotate the hinge (max |θ|={max_abs_angle})"
        );
        let max = JointLimit::Half.max_turns() * std::f32::consts::TAU + 0.02;
        assert!(
            max_abs_angle <= max + 0.01,
            "Half gate: |Δθ|={max_abs_angle} max={max}"
        );
    }

    #[test]
    fn joint_quarter_gate_blocks_wide_swing() {
        use crate::scenarios::vertical_arm_arena;
        let mut arena = vertical_arm_arena(JointLimit::Quarter);
        arena.physics.scripted_muscle = true;
        arena.activate().unwrap();
        for _ in 0..200 {
            arena.tick();
        }
        let delta = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .joint_angle_delta(0)
            .unwrap()
            .abs();
        let max = JointLimit::Quarter.max_turns() * std::f32::consts::TAU + 0.02;
        assert!(
            delta <= max + 0.01,
            "Quarter gate should clamp swing (|Δθ|={delta} > {max})"
        );
    }

    #[test]
    fn free_jointed_pair_falls_together() {
        let mut arena = StudioArena::new(ArenaConfig {
            width: 32,
            height: 32,
            seed: 12,
            water_to_y: None,
        });
        arena.physics = StudioPhysicsConfig::body_only();
        arena.physics.scripted_muscle = false;
        arena.body.paint.set(8, 20, TissueKind::Bone);
        arena.body.paint.set(9, 20, TissueKind::Bone);
        arena.body.paint.set(10, 20, TissueKind::JointHalf);
        arena.body.paint.set(11, 20, TissueKind::Bone);
        arena.body.paint.set(12, 20, TissueKind::Bone);
        arena.activate().unwrap();
        let y0: Vec<i32> = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .filter(|p| p.kind == PartKind::Bone)
            .map(|p| p.offset_y)
            .collect();
        for _ in 0..25 {
            arena.tick();
        }
        let g = arena.body.graph.as_ref().unwrap();
        let y1: Vec<i32> = g
            .parts
            .iter()
            .filter(|p| p.kind == PartKind::Bone)
            .map(|p| p.offset_y)
            .collect();
        assert!(y1.iter().all(|&y| y < y0[0]), "pair should fall");
        assert_eq!(y1[0], y1[1], "jointed bones must share gravity offset");
        assert!(joints_satisfied(g), "hinge must hold while falling");
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
        let c0 = arena
            .body
            .graph
            .as_ref()
            .unwrap()
            .parts
            .iter()
            .find(|p| p.id == free_id)
            .unwrap()
            .centroid();
        for _ in 0..80 {
            arena.tick();
        }
        let g = arena.body.graph.as_ref().unwrap();
        let free = g.parts.iter().find(|p| p.id == free_id).unwrap();
        let c1 = free.centroid();
        let moved = (c0.0 - c1.0).hypot(c0.1 - c1.1) > 0.5 || free.hinge_angle.abs() > 0.1;
        let tension = g.mean_tension();
        assert!(
            moved || tension > 0.01,
            "scripted muscle should articulate or report tension (centroid {c0:?}→{c1:?}, θ={}, T={tension})",
            free.hinge_angle
        );
        assert_eq!(free.offset_x, 0, "hinged fin bone must not slide on x");
        assert_eq!(free.offset_y, 0, "hinged fin bone must not slide on y");
    }

    #[test]
    fn force_sensor_between_parts_is_not_a_joint() {
        let mut paint = TissuePaint::new(24, 16);
        for y in 4..12 {
            paint.set(2, y, TissueKind::Fixture);
        }
        paint.set(3, 8, TissueKind::Bone);
        paint.set(4, 8, TissueKind::Bone);
        paint.set(5, 8, TissueKind::ForceSensor); // blue — not a hinge
        paint.set(6, 8, TissueKind::Bone);
        paint.set(7, 8, TissueKind::Bone);
        let g = activate(&paint).unwrap();
        assert!(g.joints.is_empty(), "ForceSensor must not create joints");
        assert_eq!(force_sensors_bridging_parts(&paint), 1);
        assert_eq!(g.hinged_bone_count(), 0);
    }

    #[test]
    fn muscle_beside_joint_links_both_parts() {
        let mut paint = TissuePaint::new(24, 16);
        for y in 4..12 {
            paint.set(2, y, TissueKind::Fixture);
        }
        paint.set(3, 8, TissueKind::Bone);
        paint.set(4, 8, TissueKind::Bone);
        paint.set(5, 8, TissueKind::JointHalf);
        paint.set(6, 8, TissueKind::Bone);
        paint.set(7, 8, TissueKind::Bone);
        paint.set(8, 8, TissueKind::Bone);
        // Muscle only under distal bone, but touches the joint cell above.
        paint.set(5, 9, TissueKind::Muscle);
        paint.set(6, 9, TissueKind::Muscle);
        paint.set(7, 9, TissueKind::Muscle);
        let g = activate(&paint).unwrap();
        assert!(g.joints.len() >= 1);
        assert!(
            g.muscles.len() >= 1,
            "muscle adjacent to joint should span the hinge"
        );
        assert!(g.hinged_bone_count() >= 1);
    }

    #[test]
    fn activate_wires_nerve_and_controller_blob() {
        let mut paint = TissuePaint::new(24, 24);
        for y in 4..14 {
            paint.set(2, y, TissueKind::Fixture);
        }
        paint.set(3, 8, TissueKind::Bone);
        paint.set(4, 8, TissueKind::Bone);
        paint.set(5, 8, TissueKind::JointHalf);
        paint.set(6, 8, TissueKind::Bone);
        paint.set(7, 8, TissueKind::Bone);
        paint.set(4, 9, TissueKind::Muscle);
        paint.set(5, 9, TissueKind::Muscle);
        paint.set(6, 9, TissueKind::Muscle);
        // Neuron blob ≥2×2
        paint.set(8, 10, TissueKind::NeuronBlob);
        paint.set(9, 10, TissueKind::NeuronBlob);
        paint.set(8, 11, TissueKind::NeuronBlob);
        paint.set(9, 11, TissueKind::NeuronBlob);
        // Nerve path from blob toward muscle
        paint.set(7, 10, TissueKind::Nerve);
        paint.set(6, 10, TissueKind::Nerve);

        let g = activate(&paint).unwrap();
        assert!(g.has_controller, "2×2 neuron blob should be a controller");
        assert!(!g.neurons.is_empty());
        assert!(!g.nerves.is_empty());
        assert!(
            g.nerves.iter().any(|n| !n.neuron_ids.is_empty()),
            "nerve should touch neuron cluster"
        );
    }
}
