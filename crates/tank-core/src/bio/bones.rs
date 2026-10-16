//! Phase 5 M1 — bone parts and joints over the living raster.
//!
//! Studio `activate` flooded Bone paint into `RigidPart`s and Joint paint
//! into hinges. Here the morphology is the evolved body: hard-walled live
//! cells (`wall_h > 0.5`) plus BONE/SHELL secretion pixels form rigid
//! parts; a soft or MUSCLE cell that touches two parts is a joint; MUSCLE
//! cells that touch a joint spring actuate it (via `soft.rs`).

use std::collections::{HashMap, HashSet, VecDeque};

use crate::bio::genome::GenomeStore;
use crate::bio::program::ChildKind;
use crate::bio::store::{CellStore, NONE};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartKind {
    Bone,
    Soft,
}

#[derive(Debug, Clone)]
pub struct RigidPart {
    pub id: u32,
    pub kind: PartKind,
    /// Live cell ids in this part (empty for secretion-only parts).
    pub cells: Vec<u32>,
    /// Absolute pixels owned by secretions in this part.
    pub secretion_px: Vec<(u16, u16)>,
}

#[derive(Debug, Clone)]
pub struct Joint {
    /// Soft / muscle cell that bridges the two parts (NONE if secretion-only).
    pub cell: u32,
    pub part_a: u32,
    pub part_b: u32,
    pub muscle: bool,
}

#[derive(Debug, Clone, Default)]
pub struct BoneGraph {
    pub parts: Vec<RigidPart>,
    pub joints: Vec<Joint>,
    /// Live cell → part id.
    pub cell_part: HashMap<u32, u32>,
}

/// Extract the bone/joint graph for one organism.
pub fn activate(
    cells: &CellStore,
    genomes: &GenomeStore,
    org_id: u32,
) -> Option<BoneGraph> {
    let o = cells.org(org_id)?;
    if o.n_alive < 2 {
        return None;
    }

    // Seed hard live cells as bone candidates.
    let mut hard: Vec<u32> = Vec::new();
    let mut soft_cells: Vec<u32> = Vec::new();
    for s in o.slots.iter().filter(|s| s.cell != NONE) {
        let Some(c) = cells.get(s.cell) else {
            continue;
        };
        let mc = genomes.coeffs(c.genotype, c.mode);
        if mc.is_hard {
            hard.push(s.cell);
        } else {
            soft_cells.push(s.cell);
        }
    }

    // Connected components of hard cells via 4-adjacent footprints.
    let mut seen: HashSet<u32> = HashSet::new();
    let mut parts: Vec<RigidPart> = Vec::new();
    let mut cell_part: HashMap<u32, u32> = HashMap::new();
    let mut next = 0u32;
    for &seed in &hard {
        if !seen.insert(seed) {
            continue;
        }
        let mut q = VecDeque::from([seed]);
        let mut comp = Vec::new();
        while let Some(id) = q.pop_front() {
            comp.push(id);
            let Some(a) = cells.get(id) else {
                continue;
            };
            for &other in &hard {
                if seen.contains(&other) {
                    continue;
                }
                let Some(b) = cells.get(other) else {
                    continue;
                };
                if footprints_touch(a.px, a.py, a.w, a.h, b.px, b.py, b.w, b.h) {
                    seen.insert(other);
                    q.push_back(other);
                }
            }
        }
        let id = next;
        next += 1;
        for &c in &comp {
            cell_part.insert(c, id);
        }
        parts.push(RigidPart {
            id,
            kind: PartKind::Bone,
            cells: comp,
            secretion_px: Vec::new(),
        });
    }

    // Attach BONE/SHELL secretions: flood adjacent secretion pixels onto the
    // nearest hard part, or start a new part when floating.
    if let Some(o) = cells.org(org_id) {
        for sec in &o.secretions {
            if !matches!(sec.kind, ChildKind::SecreteBone | ChildKind::SecreteShell) {
                continue;
            }
            let (px, py) = (sec.px, sec.py);
            // Prefer a hard part whose live cell is adjacent.
            let mut attached = None;
            for p in &parts {
                if p.kind != PartKind::Bone {
                    continue;
                }
                for &cid in &p.cells {
                    let Some(c) = cells.get(cid) else {
                        continue;
                    };
                    if pixel_touches_footprint(px, py, c.px, c.py, c.w, c.h) {
                        attached = Some(p.id);
                        break;
                    }
                }
                if attached.is_some() {
                    break;
                }
            }
            if let Some(pid) = attached {
                if let Some(p) = parts.iter_mut().find(|p| p.id == pid) {
                    p.secretion_px.push((px, py));
                }
            } else {
                let id = next;
                next += 1;
                parts.push(RigidPart {
                    id,
                    kind: PartKind::Bone,
                    cells: Vec::new(),
                    secretion_px: vec![(px, py)],
                });
            }
        }
    }

    // Soft cells that touch ≥2 bone parts are joints.
    let mut joints = Vec::new();
    for &cid in &soft_cells {
        let Some(c) = cells.get(cid) else {
            continue;
        };
        let mc = genomes.coeffs(c.genotype, c.mode);
        let mut touched: Vec<u32> = parts
            .iter()
            .filter(|p| {
                p.cells.iter().any(|&hid| {
                    cells.get(hid).is_some_and(|h| {
                        footprints_touch(c.px, c.py, c.w, c.h, h.px, h.py, h.w, h.h)
                    })
                }) || p.secretion_px.iter().any(|&(px, py)| {
                    pixel_touches_footprint(px, py, c.px, c.py, c.w, c.h)
                })
            })
            .map(|p| p.id)
            .collect();
        touched.sort_unstable();
        touched.dedup();
        if touched.len() >= 2 {
            joints.push(Joint {
                cell: cid,
                part_a: touched[0],
                part_b: touched[1],
                muscle: mc.muscle > 0,
            });
        }
        // Soft cells also get their own soft "part" for spring tagging.
        let id = next;
        next += 1;
        cell_part.insert(cid, id);
        parts.push(RigidPart {
            id,
            kind: PartKind::Soft,
            cells: vec![cid],
            secretion_px: Vec::new(),
        });
    }

    if parts.is_empty() {
        return None;
    }
    Some(BoneGraph {
        parts,
        joints,
        cell_part,
    })
}

/// Stiffness multiplier for a spring between two cells (bone–bone ≫ joint).
pub fn spring_stiff_scale(graph: &BoneGraph, cell_a: u32, cell_b: u32) -> f32 {
    let (pa, pb) = match (graph.cell_part.get(&cell_a), graph.cell_part.get(&cell_b)) {
        (Some(&a), Some(&b)) => (a, b),
        _ => return 1.0,
    };
    if pa == pb {
        // Intra-part: nearly rigid.
        return 2.2;
    }
    let ka = graph.parts.iter().find(|p| p.id == pa).map(|p| p.kind);
    let kb = graph.parts.iter().find(|p| p.id == pb).map(|p| p.kind);
    match (ka, kb) {
        (Some(PartKind::Bone), Some(PartKind::Bone)) => {
            // Bone–bone across a joint: medium (hinge).
            if graph.joints.iter().any(|j| {
                (j.part_a == pa && j.part_b == pb) || (j.part_a == pb && j.part_b == pa)
            }) {
                0.85
            } else {
                1.4
            }
        }
        _ => 1.0,
    }
}

fn footprints_touch(ax: u16, ay: u16, aw: u8, ah: u8, bx: u16, by: u16, bw: u8, bh: u8) -> bool {
    let a1x = ax as i32 + aw as i32;
    let a1y = ay as i32 + ah as i32;
    let b1x = bx as i32 + bw as i32;
    let b1y = by as i32 + bh as i32;
    ax as i32 <= b1x && bx as i32 <= a1x && ay as i32 <= b1y && by as i32 <= a1y
}

fn pixel_touches_footprint(px: u16, py: u16, fx: u16, fy: u16, fw: u8, fh: u8) -> bool {
    let px = px as i32;
    let py = py as i32;
    px + 1 >= fx as i32
        && px <= fx as i32 + fw as i32
        && py + 1 >= fy as i32
        && py <= fy as i32 + fh as i32
}
