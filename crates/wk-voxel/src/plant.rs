//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Minimal Set D land plant (docs/organism/PLANTS.md § C + D1 growth):
//! Root + Stem + Photosystem on a fixed crown. Drinks pore `sat`,
//! elongates by `StemVsLeafVsRoot` / `RootDepthBias`. Shade race = D2.

use std::collections::HashSet;

use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::cell::water_capacity;
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};

/// Energy from one sat unit drunk by roots.
pub const ROOT_WATER_ENERGY: f32 = 0.04;
/// Max sat removed per Root module per tick.
pub const ROOT_SIP_SAT: u8 = 1;
/// Soft stress drain when roots find no pore water.
pub const DROUGHT_STRESS_DRAIN: f32 = 0.06;
/// Pore fill fraction below which photo/growth feel dry.
pub const DROUGHT_STRESS_FRAC: f32 = 0.08;

/// Soft caps so 1× bodies stay readable.
pub const MAX_ROOT_MODULES: usize = 16;
pub const MAX_STEM_MODULES: usize = 10;
pub const MAX_PHOTO_MODULES: usize = 12;

/// Energy fraction of tank required before tissue growth.
pub const LAND_GROW_ENERGY_FRAC: f32 = 0.30;
/// Ticks between growth attempts.
pub const LAND_GROW_PERIOD: u64 = 48;
/// Base energy to place one Root pixel in soft substrate.
pub const ROOT_ELONGATE_BASE_COST: f32 = 2.4;
/// Energy to place one Stem / Photosystem pixel.
pub const SHOOT_GROW_COST: f32 = 1.6;
/// Energy fraction of tank to fire a vegetative sprout.
pub const LAND_SPROUT_ENERGY_FRAC: f32 = 0.52;
/// Ticks between sprout attempts.
pub const LAND_SPROUT_PERIOD: u64 = 48;
/// Painted Root modules required before a sprout may fire.
pub const LAND_SPROUT_MIN_ROOTS: usize = 3;
/// Max columns a rhizome sprout may emerge from the crown.
pub const ROOT_SPROUT_MAX_DIST: i32 = 6;
/// Fraction of tank spent to sprout (child gets half).
pub const LAND_SPROUT_COST_FRAC: f32 = 0.45;

/// True when the body includes a Root (land habit).
pub fn is_land_plant(atom: &Atom) -> bool {
    atom.body.iter().any(|(_, _, m)| *m == ModuleId::Root)
}

pub fn root_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Root)
        .count()
}

pub fn stem_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Stem)
        .count()
}

/// Best pore-fill fraction under/at Root modules (0..1).
pub fn root_moisture_frac(world: &World, atom: &Atom) -> f32 {
    let mut best = 0.0f32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Root {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        best = best.max(cell_moisture_frac(world, wx, wy));
        best = best.max(cell_moisture_frac(world, wx, wy - 1));
    }
    best
}

fn cell_moisture_frac(world: &World, gx: i32, gy: i32) -> f32 {
    let Some(cell) = world.get_cell(gx, gy) else {
        return 0.0;
    };
    if cell.material == MaterialId::Air {
        return 0.0;
    }
    let cap = water_capacity(cell.material);
    if cap == 0 {
        return 0.0;
    }
    cell.sat.0 as f32 / cap as f32
}

/// At least one Root sits in/on porous (or any) solid purchase.
pub fn is_anchored(world: &World, atom: &Atom) -> bool {
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Root {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if solid_purchase(world, wx, wy) {
            return true;
        }
        if solid_purchase(world, wx, wy - 1) {
            return true;
        }
    }
    false
}

fn solid_purchase(world: &World, gx: i32, gy: i32) -> bool {
    matches!(
        world.get_cell(gx, gy),
        Some(c) if c.material != MaterialId::Air
    )
}

/// Pull a little pore water from solids under roots → energy.
/// Returns (energy gained, sat removed).
pub fn drink_roots(world: &mut World, atom: &Atom) -> (f32, u32) {
    let mut energy = 0.0f32;
    let mut taken = 0u32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Root {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if let Some(n) = sip_porous(world, wx, wy) {
            energy += ROOT_WATER_ENERGY * n as f32;
            taken += n as u32;
            continue;
        }
        if let Some(n) = sip_porous(world, wx, wy - 1) {
            energy += ROOT_WATER_ENERGY * n as f32;
            taken += n as u32;
        }
    }
    (energy, taken)
}

fn sip_porous(world: &mut World, gx: i32, gy: i32) -> Option<u8> {
    let cell = world.get_cell(gx, gy)?;
    if cell.material == MaterialId::Air {
        return None;
    }
    let cap = water_capacity(cell.material);
    if cap == 0 || cell.sat.0 == 0 {
        return None;
    }
    let take = ROOT_SIP_SAT.min(cell.sat.0);
    let mut next = cell;
    next.sat.0 = cell.sat.0 - take;
    world.set_cell(gx, gy, next);
    Some(take)
}

/// Nucleus y for a plant: Air cell directly above a porous solid near `gy`.
pub fn find_plant_slot(world: &World, gx: i32, gy: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    for dy in [0, 1, -1, 2, -2, 3, -3, 4, -4, 6, -6, 8, -8] {
        let y = gy + dy;
        if plantable_crown(world, gx, y) {
            return Some(y);
        }
    }
    for y in (gy - 32..=gy + 32).rev() {
        if plantable_crown(world, gx, y) {
            return Some(y);
        }
    }
    None
}

fn plantable_crown(world: &World, gx: i32, nucleus_y: i32) -> bool {
    let Some(air) = world.get_cell(gx, nucleus_y) else {
        return false;
    };
    if air.material != MaterialId::Air {
        return false;
    }
    let Some(below) = world.get_cell(gx, nucleus_y - 1) else {
        return false;
    };
    if below.material == MaterialId::Air {
        return false;
    }
    water_capacity(below.material) > 0
}

/// Pin continuous pose to the integer crown (no buoyancy).
pub fn pin_plant_pose(atom: &mut Atom) {
    atom.fy = atom.gy as f32;
    atom.vel_y = 0.0;
    atom.last_water_top = None;
}

/// Penetrate multiplier — higher = harder / costlier. `None` = refuse.
fn penetrate_cost(mat: MaterialId) -> Option<f32> {
    match mat {
        MaterialId::Bedrock | MaterialId::Ice | MaterialId::Snow | MaterialId::Water => None,
        MaterialId::Organic => Some(0.35),
        MaterialId::Sand | MaterialId::Clay => Some(0.65),
        MaterialId::Stone => Some(1.6),
        MaterialId::Air => Some(0.45), // gaps / rhizome air pockets
        _ => Some(1.0),
    }
}

/// Try to add one Root pixel toward moisture / depth bias.
/// Returns energy spent (0 if nothing grew).
pub fn try_elongate_root(world: &World, atom: &mut Atom) -> f32 {
    let n_roots = root_count(atom);
    if n_roots >= MAX_ROOT_MODULES {
        return 0.0;
    }
    let (_, _, w_root) = atom.genome.alloc_weights();
    if w_root < 0.08 {
        return 0.0;
    }
    let tank = atom.energy_max.max(1.0);
    let grow_floor = tank * LAND_GROW_ENERGY_FRAC;
    if atom.energy < grow_floor {
        return 0.0;
    }

    let occupied: HashSet<(i16, i16)> = atom.body.iter().map(|&(x, y, _)| (x, y)).collect();
    let tips: Vec<(i16, i16)> = atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Root)
        .map(|&(x, y, _)| (x, y))
        .collect();
    let tips = if tips.is_empty() {
        vec![(0, -1)]
    } else {
        tips
    };

    let depth_bias = atom.genome.root_depth_bias.clamp(0.0, 1.0);
    // Urge a lateral runner before sprouting (column rhizome bias).
    let need_runner = !has_lateral_runner(atom)
        && n_roots >= LAND_SPROUT_MIN_ROOTS.saturating_sub(1);
    let banking_for_sprout = atom.energy >= tank * LAND_SPROUT_ENERGY_FRAC * 0.85;
    const DIRS: [(i16, i16); 5] = [(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)];
    let mut best: Option<(f32, i16, i16, f32)> = None; // score, dx, dy, cost

    for &(tx, ty) in &tips {
        for &(dx, dy) in &DIRS {
            let nx = tx + dx;
            let ny = ty + dy;
            if ny > 1 || ny < -18 || nx.abs() > 10 {
                continue;
            }
            if occupied.contains(&(nx, ny)) {
                continue;
            }
            let wx = world.wrap_x(atom.gx + nx as i32);
            let wy = atom.gy + ny as i32;
            let Some(cell) = world.get_cell(wx, wy) else {
                continue;
            };
            let Some(pen) = penetrate_cost(cell.material) else {
                continue;
            };
            // Prefer solid substrate; allow Air only as a rare gap step.
            if cell.material == MaterialId::Air && dy == 0 {
                // lateral air — skip (roots stay in ground)
                continue;
            }
            let cost = ROOT_ELONGATE_BASE_COST * pen;
            if atom.energy < cost + grow_floor * 0.5 {
                continue;
            }
            let moist = cell_moisture_frac(world, wx, wy)
                .max(cell_moisture_frac(world, wx, wy - 1));
            let down = if dy < 0 { 1.0 } else { 0.0 };
            let lateral = if dx != 0 && dy == 0 { 0.35 } else { 0.0 };
            let mut score = moist + depth_bias * down + (1.0 - depth_bias) * lateral - pen * 0.03;
            if need_runner || banking_for_sprout {
                if dx != 0 && dy == 0 {
                    score += 2.8;
                } else if dx != 0 && dy < 0 {
                    score += 1.1;
                }
                if nx != 0 {
                    score += 1.2;
                }
                if need_runner && dy < 0 && dx == 0 {
                    score -= 1.2;
                }
            }
            if best.map(|(s, ..)| score > s).unwrap_or(true) {
                best = Some((score, nx, ny, cost));
            }
        }
    }

    let Some((_score, nx, ny, cost)) = best else {
        return 0.0;
    };
    atom.energy -= cost;
    atom.body.push((nx, ny, ModuleId::Root));
    cost
}

/// Stem upward or leaf place from surplus allocation.
pub fn try_grow_shoot(atom: &mut Atom, tick: u64) -> f32 {
    let (w_stem, w_leaf, _) = atom.genome.alloc_weights();
    let tank = atom.energy_max.max(1.0);
    if atom.energy < tank * (LAND_GROW_ENERGY_FRAC + 0.08) {
        return 0.0;
    }
    let occupied: HashSet<(i16, i16)> = atom.body.iter().map(|&(x, y, _)| (x, y)).collect();
    let n_stem = stem_count(atom);
    let n_photo = atom.photosystem_count();

    let roll = hash01(tick, atom.gx as u64, atom.age_ticks, 0x5707);
    let prefer_leaf = roll < w_leaf / (w_stem + w_leaf).max(1e-6);
    let cost = SHOOT_GROW_COST;
    if atom.energy < cost + 1.0 {
        return 0.0;
    }

    if prefer_leaf && n_photo < MAX_PHOTO_MODULES {
        let top = atom
            .body
            .iter()
            .max_by_key(|(_, y, _)| *y)
            .map(|&(x, y, _)| (x, y));
        if let Some((tx, ty)) = top {
            for &(dx, dy) in &[(0i16, 1), (1, 1), (-1, 1), (1, 0), (-1, 0)] {
                let nx = tx + dx;
                let ny = ty + dy;
                if ny > 16 || occupied.contains(&(nx, ny)) {
                    continue;
                }
                atom.energy -= cost;
                atom.body.push((nx, ny, ModuleId::Photosystem));
                return cost;
            }
        }
    } else if n_stem < MAX_STEM_MODULES && w_stem >= 0.08 {
        let anchor = atom
            .body
            .iter()
            .filter(|(_, _, m)| {
                matches!(
                    *m,
                    ModuleId::Stem | ModuleId::Nucleus | ModuleId::Root | ModuleId::Photosystem
                )
            })
            .max_by_key(|(_, y, _)| *y)
            .map(|&(x, y, _)| (x, y));
        if let Some((ax, ay)) = anchor {
            let nx = ax;
            let ny = ay + 1;
            if ny <= 16 && !occupied.contains(&(nx, ny)) {
                atom.energy -= cost;
                atom.body.push((nx, ny, ModuleId::Stem));
                return cost;
            }
        }
    }
    0.0
}

/// One growth pulse: root and/or shoot from allocation weights.
pub fn try_grow_plant(world: &World, atom: &mut Atom, tick: u64) -> f32 {
    if atom.age_ticks % LAND_GROW_PERIOD != 0 {
        return 0.0;
    }
    let (w_stem, w_leaf, w_root) = atom.genome.alloc_weights();
    let roll = hash01(tick, atom.gx as u64, atom.gy as u64, 0x6110);
    let mut spent = 0.0;
    // Weighted pick: try preferred tissue first, then the other.
    let try_root_first = roll < w_root || (w_root >= w_stem && w_root >= w_leaf);
    if try_root_first {
        spent += try_elongate_root(world, atom);
        if spent <= 0.0 {
            spent += try_grow_shoot(atom, tick);
        }
    } else {
        spent += try_grow_shoot(atom, tick);
        if spent <= 0.0 {
            spent += try_elongate_root(world, atom);
        }
    }
    spent
}

/// True when at least one Root sits in a column other than the crown.
pub fn has_lateral_runner(atom: &Atom) -> bool {
    atom.body
        .iter()
        .any(|&(dx, _, m)| m == ModuleId::Root && dx != 0)
}

/// Pick a world column for vegetative sprout from a lateral runner tip.
pub fn pick_sprout_column(world: &World, atom: &Atom) -> Option<i32> {
    if !has_lateral_runner(atom) || root_count(atom) < LAND_SPROUT_MIN_ROOTS {
        return None;
    }
    let mut best: Option<(i32, f32)> = None; // |dx|, score
    let mut best_wx = atom.gx;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Root || dx == 0 {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let dist = dx.abs() as i32;
        if dist < 1 || dist > ROOT_SPROUT_MAX_DIST {
            continue;
        }
        // Need a plantable crown near the tip column.
        let tip_y = atom.gy + dy as i32;
        let Some(slot) = find_plant_slot(world, wx, tip_y.max(atom.gy)) else {
            continue;
        };
        let moist = cell_moisture_frac(world, wx, slot - 1);
        if moist < 0.02 {
            continue;
        }
        let score = moist + dist as f32 * 0.05;
        if best.map(|(d, s)| dist > d || (dist == d && score > s)).unwrap_or(true) {
            best = Some((dist, score));
            best_wx = wx;
        }
    }
    best.map(|_| best_wx)
}

/// Vegetative sucker: child plant on moist land at a lateral runner tip.
///
/// Requires painted lateral root, enough roots, energy, and cooldown.
/// Child body is a fresh minimal plant; genome is mutated from parent.
pub fn try_vegetative_sprout(
    world: &World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
) -> Option<Atom> {
    if !pop_room || atom.cooldown > 0 {
        return None;
    }
    if root_count(atom) < LAND_SPROUT_MIN_ROOTS {
        return None;
    }
    let tank = atom.energy_max.max(1.0);
    if atom.energy < tank * LAND_SPROUT_ENERGY_FRAC {
        return None;
    }
    let wx = pick_sprout_column(world, atom)?;
    let gy = find_plant_slot(world, wx, atom.gy)?;
    let cost = tank * LAND_SPROUT_COST_FRAC;
    if atom.energy < cost {
        return None;
    }
    atom.energy -= cost;
    atom.cooldown = LAND_SPROUT_PERIOD;

    let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
    let child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    let mut child = Atom::from_body(wx, gy, atom.energy_max, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, atom.energy_max);
    child.cooldown = LAND_SPROUT_PERIOD;
    pin_plant_pose(&mut child);
    if !is_anchored(world, &child) {
        // Refund — site looked plantable but crown didn't seat.
        atom.energy = (atom.energy + cost).min(atom.energy_max);
        atom.cooldown = 0;
        return None;
    }
    Some(child)
}

fn hash01(a: u64, b: u64, c: u64, salt: u64) -> f32 {
    let mut x = a
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(b)
        .wrapping_add(c)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    (x >> 40) as f32 / ((1u64 << 24) as f32)
}

/// Apply genome fields that also live as Atom pose knobs.
pub fn sync_atom_from_genome(atom: &mut Atom) {
    atom.buoyancy_bias = atom.genome.buoyancy_bias.clamp(0.0, 1.0);
    atom.clone_fidelity = atom.genome.clone_fidelity.clamp(0.05, 1.0);
}

pub fn apply_genome(atom: &mut Atom, genome: Genome) {
    atom.genome = genome;
    sync_atom_from_genome(atom);
}

/// Body helper for tests / templates.
pub fn body_has_module(body: &[BodyModule], mid: ModuleId) -> bool {
    body.iter().any(|(_, _, m)| *m == mid)
}
