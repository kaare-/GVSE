//! Phase 5 M0 — soft bodies: cells are verlet nodes, division bonds are
//! springs; MUSCLE scales the rest length of the springs it touches.
//!
//! Continuous positions live here; the raster (pixel claims + physics
//! occupancy) is updated by discrete one-pixel moves after the solver.
//! CILIA thrust (`step::swim_bodies`) still translates free bodies; this
//! module deforms them.

use crate::bio::bones;
use crate::bio::genome::GenomeStore;
use crate::bio::store::{CellStore, NONE};
use crate::consts::{PX_PER_CELL, SOFT_ITERS, SOFT_STIFF, TICK_HZ};
use crate::grid::PhysicsGrid;
use crate::pixels::PixelLayer;

/// One cell centre in continuous pixel space.
#[derive(Debug, Clone)]
pub struct SoftNode {
    pub cell: u32,
    pub x: f32,
    pub y: f32,
    pub px: f32,
    pub py: f32,
}

/// A spring between two nodes (division bond).
#[derive(Debug, Clone)]
pub struct SoftSpring {
    pub a: u16,
    pub b: u16,
    pub rest: f32,
}

#[derive(Debug, Clone, Default)]
pub struct SoftBody {
    pub nodes: Vec<SoftNode>,
    pub springs: Vec<SoftSpring>,
    /// Topology fingerprint (`n_alive`, seed cell) so we rebuild on change.
    pub key: u64,
}

#[derive(Debug, Default)]
pub struct SoftStore {
    /// Parallel to `CellStore` org ids (sparse; empty = no soft body yet).
    bodies: Vec<Option<SoftBody>>,
}

impl SoftStore {
    pub fn clear(&mut self) {
        self.bodies.clear();
    }

    fn body_mut(&mut self, org: u32) -> &mut Option<SoftBody> {
        let i = org as usize;
        if i >= self.bodies.len() {
            self.bodies.resize_with(i + 1, || None);
        }
        &mut self.bodies[i]
    }

    pub fn get(&self, org: u32) -> Option<&SoftBody> {
        self.bodies.get(org as usize).and_then(|b| b.as_ref())
    }
}

/// Rebuild or refresh soft bodies, then integrate. Returns how many orgs
/// took at least one discrete pixel step from the solver.
pub fn step(
    soft: &mut SoftStore,
    cells: &mut CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
) -> u32 {
    let orgs: Vec<u32> = cells
        .iter_orgs()
        .filter(|(_, o)| o.alive && o.n_alive >= 2)
        .map(|(id, _)| id)
        .collect();
    let mut moved = 0u32;
    for org_id in orgs {
        // Soft deformation is MUSCLE's job; cilia thrust and chamber
        // geometry stay on the discrete raster until M1 bones.
        if !org_has_muscle(cells, genomes, org_id) {
            continue;
        }
        if ensure(soft, cells, genomes, org_id).is_none() {
            continue;
        }
        if integrate(soft, cells, genomes, grid, pixels, org_id) {
            moved += 1;
        }
    }
    moved
}

fn org_has_muscle(cells: &CellStore, genomes: &GenomeStore, org_id: u32) -> bool {
    let Some(o) = cells.org(org_id) else {
        return false;
    };
    o.slots.iter().filter(|s| s.cell != NONE).any(|s| {
        cells
            .get(s.cell)
            .map(|c| genomes.coeffs(c.genotype, c.mode).muscle > 0)
            .unwrap_or(false)
    })
}

fn topology_key(cells: &CellStore, org_id: u32) -> Option<u64> {
    let o = cells.org(org_id)?;
    let mut k = (o.n_alive as u64) << 32 | o.seed as u64;
    for s in &o.slots {
        if s.cell != NONE {
            k = k.wrapping_mul(0x9E37_79B9).wrapping_add(s.cell as u64);
        }
    }
    Some(k)
}

fn ensure(
    soft: &mut SoftStore,
    cells: &CellStore,
    genomes: &GenomeStore,
    org_id: u32,
) -> Option<()> {
    let key = topology_key(cells, org_id)?;
    let needs = !matches!(soft.get(org_id), Some(b) if b.key == key && !b.nodes.is_empty());
    if needs {
        *soft.body_mut(org_id) = Some(build(cells, genomes, org_id, key)?);
    } else {
        // Keep continuous positions; refresh centres from the raster if a
        // discrete move drifted them more than a pixel (glue/thrust).
        sync_from_raster(soft.body_mut(org_id).as_mut()?, cells);
    }
    Some(())
}

fn build(cells: &CellStore, genomes: &GenomeStore, org_id: u32, key: u64) -> Option<SoftBody> {
    let o = cells.org(org_id)?;
    let mut nodes = Vec::new();
    let mut index_of = std::collections::HashMap::new();
    for s in o.slots.iter().filter(|s| s.cell != NONE) {
        let c = cells.get(s.cell)?;
        let x = c.px as f32 + c.w as f32 * 0.5;
        let y = c.py as f32 + c.h as f32 * 0.5;
        let i = nodes.len() as u16;
        index_of.insert(s.cell, i);
        nodes.push(SoftNode {
            cell: s.cell,
            x,
            y,
            px: x,
            py: y,
        });
    }
    if nodes.len() < 2 {
        return None;
    }
    let mut springs = Vec::new();
    for n in &nodes {
        let Some(c) = cells.get(n.cell) else {
            continue;
        };
        if c.parent_cell == NONE {
            continue;
        }
        let Some(&ai) = index_of.get(&c.parent_cell) else {
            continue;
        };
        let Some(&bi) = index_of.get(&n.cell) else {
            continue;
        };
        let a = &nodes[ai as usize];
        let b = &nodes[bi as usize];
        let rest = ((a.x - b.x).hypot(a.y - b.y)).max(1.0);
        // Prefer a spring along a muscle-bearing bond (rest still geometric).
        let _ = genomes; // muscle flag is read at integrate time
        springs.push(SoftSpring { a: ai, b: bi, rest });
    }
    if springs.is_empty() {
        // Fallback: chain consecutive slots so a body without recorded
        // parents still gets a soft graph.
        for i in 0..nodes.len().saturating_sub(1) {
            let a = &nodes[i];
            let b = &nodes[i + 1];
            let rest = ((a.x - b.x).hypot(a.y - b.y)).max(1.0);
            springs.push(SoftSpring {
                a: i as u16,
                b: (i + 1) as u16,
                rest,
            });
        }
    }
    Some(SoftBody { nodes, springs, key })
}

fn sync_from_raster(body: &mut SoftBody, cells: &CellStore) {
    for n in &mut body.nodes {
        let Some(c) = cells.get(n.cell) else {
            continue;
        };
        let x = c.px as f32 + c.w as f32 * 0.5;
        let y = c.py as f32 + c.h as f32 * 0.5;
        if (x - n.x).abs() > 1.5 || (y - n.y).abs() > 1.5 {
            n.px += x - n.x;
            n.py += y - n.y;
            n.x = x;
            n.y = y;
        }
    }
}

fn integrate(
    soft: &mut SoftStore,
    cells: &mut CellStore,
    genomes: &GenomeStore,
    grid: &PhysicsGrid,
    pixels: &mut PixelLayer,
    org_id: u32,
) -> bool {
    let Some(body) = soft.body_mut(org_id).as_mut() else {
        return false;
    };
    // Forces + verlet predict.
    for n in &mut body.nodes {
        let Some(c) = cells.get(n.cell) else {
            continue;
        };
        let mc = genomes.coeffs(c.genotype, c.mode);
        let (vx, vy) = (n.x - n.px, n.y - n.py);
        let (ax, ay) = env_accel(grid, n.x, n.y, mc.buoy as i8);
        // MUSCLE that is "running" pays and contracts via spring rest below;
        // a lone muscle still gets a tiny self-jitter so it visibly twitches.
        let (jx, jy) = if mc.muscle > 0 && c.running {
            let a = (mc.muscle as f32) * 0.002;
            (a * (c.heading as i8 as f32).sin(), -a * (c.heading as i8 as f32).cos())
        } else {
            (0.0, 0.0)
        };
        let drag = 0.92;
        let nx = n.x + vx * drag + ax + jx;
        let ny = n.y + vy * drag + ay + jy;
        n.px = n.x;
        n.py = n.y;
        n.x = nx;
        n.y = ny;
    }

    // Muscle actuation on incident springs (scale rest length).
    let act: Vec<f32> = body
        .nodes
        .iter()
        .map(|n| {
            let Some(c) = cells.get(n.cell) else {
                return 0.0;
            };
            let mc = genomes.coeffs(c.genotype, c.mode);
            if mc.muscle > 0 && c.running {
                (mc.muscle as f32 / 8.0).clamp(0.0, 1.0)
            } else {
                0.0
            }
        })
        .collect();

    // M1: bone parts stiffen intra-part springs; joints stay compliant.
    let bone = bones::activate(cells, genomes, org_id);
    for _ in 0..SOFT_ITERS {
        for s in &body.springs {
            let (ai, bi) = (s.a as usize, s.b as usize);
            if ai >= body.nodes.len() || bi >= body.nodes.len() {
                continue;
            }
            let act_ab = act[ai].max(act[bi]);
            let rest = s.rest * (1.0 - 0.28 * act_ab);
            let stiff = SOFT_STIFF
                * bone
                    .as_ref()
                    .map(|g| {
                        bones::spring_stiff_scale(g, body.nodes[ai].cell, body.nodes[bi].cell)
                    })
                    .unwrap_or(1.0);
            let (ax, ay) = (body.nodes[ai].x, body.nodes[ai].y);
            let (bx, by) = (body.nodes[bi].x, body.nodes[bi].y);
            let dx = bx - ax;
            let dy = by - ay;
            let dist = (dx * dx + dy * dy).sqrt().max(1e-3);
            let diff = (dist - rest) / dist * stiff;
            let ox = dx * 0.5 * diff;
            let oy = dy * 0.5 * diff;
            body.nodes[ai].x += ox;
            body.nodes[ai].y += oy;
            body.nodes[bi].x -= ox;
            body.nodes[bi].y -= oy;
        }
    }

    // Rasterise: move each cell at most 1 px toward its node centre.
    let mut any = false;
    let targets: Vec<(u32, i32, i32)> = body
        .nodes
        .iter()
        .filter_map(|n| {
            let c = cells.get(n.cell)?;
            let tx = (n.x - c.w as f32 * 0.5).round() as i32;
            let ty = (n.y - c.h as f32 * 0.5).round() as i32;
            let dx = (tx - c.px as i32).clamp(-1, 1);
            let dy = (ty - c.py as i32).clamp(-1, 1);
            if dx == 0 && dy == 0 {
                return None;
            }
            Some((n.cell, c.px as i32 + dx, c.py as i32 + dy))
        })
        .collect();
    for (id, nx, ny) in targets {
        if nx < 0 || ny < 0 {
            continue;
        }
        let (ox, oy) = match cells.get(id) {
            Some(c) => (c.px as i32, c.py as i32),
            None => continue,
        };
        if cells.move_cell(pixels, id, nx as u16, ny as u16) {
            any = true;
            if let Some(c) = cells.get_mut(id) {
                c.moved_dx = c.moved_dx.saturating_add((nx - ox) as i8);
                c.moved_dy = c.moved_dy.saturating_add((ny - oy) as i8);
            }
        }
    }
    // Re-sync continuous positions to whatever the raster accepted.
    if let Some(body) = soft.body_mut(org_id).as_mut() {
        for n in &mut body.nodes {
            if let Some(c) = cells.get(n.cell) {
                n.x = c.px as f32 + c.w as f32 * 0.5;
                n.y = c.py as f32 + c.h as f32 * 0.5;
            }
        }
    }
    let _ = PX_PER_CELL;
    let _ = TICK_HZ;
    any
}

fn env_accel(grid: &PhysicsGrid, x: f32, y: f32, buoy: i8) -> (f32, f32) {
    let vx = (x / PX_PER_CELL as f32).floor() as i32;
    let vis_h = grid.visible_h();
    let py_top = (y / 1.0).floor() as i32;
    // Pixel y is top-origin; physics y is bottom-origin.
    let phy_y_from_top = py_top / PX_PER_CELL as i32;
    let vy = vis_h - 1 - phy_y_from_top;
    let cell = grid.get(vx, vy);
    let mut ay = 0.0f32;
    let ax = 0.0f32;
    if cell.is_standing_water() || (cell.is_air() && cell.sat > 24) {
        // Slight buoyancy from the BUOY block; water drag already in verlet.
        ay -= (buoy as f32) * 0.0008;
    } else if cell.is_air() {
        ay += 0.04; // fall in air
    } else {
        // Sand / rock: strong friction — kill most velocity next predict.
        ay += 0.0;
    }
    (ax, ay)
}
