//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Minimal Set D land plant (docs/organism/PLANTS.md § C + D1–D4):
//! Root + Stem + Photosystem on a fixed crown. Drinks pore `sat`,
//! elongates by `StemVsLeafVsRoot` / `RootDepthBias`. Shade race = D2.
//! Vegetative sprouts = D3. Root starch tank + drought hibernate = D4.

use std::collections::HashSet;

use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::cell::water_capacity;
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};

/// Energy from one sat unit drunk by roots.
pub const ROOT_WATER_ENERGY: f32 = 0.08;
/// Fractional sip progress per Root module per tick. Integer sat only
/// leaves the cell when the accumulator crosses 1 — stops roots from
/// flash-drying hills (column `ROOT_SIP_KG_PER_ROOT` spirit).
pub const ROOT_SIP_FRAC_PER_ROOT: f32 = 0.025;
/// Hard cap on sat units removed in one drink event.
pub const ROOT_SIP_MAX_SAT: u8 = 1;
/// Soft stress drain while drying (Stressed band). Hibernate handles
/// the bone-dry case — keep this tiny so short droughts are survivable.
pub const DROUGHT_STRESS_DRAIN: f32 = 0.003;
/// Pore fill fraction below which photo slows and stress starts.
pub const DROUGHT_STRESS_FRAC: f32 = 0.06;
/// Pore fill fraction that triggers drought dormancy (hibernate).
pub const DROUGHT_DORMANT_FRAC: f32 = 0.015;
/// Max consecutive dormant ticks before the plant dies (~2.5 min @ 60 Hz).
pub const DROUGHT_HIBERNATE_MAX_TICKS: u32 = 9_000;
/// Upkeep multiplier while drought-dormant (respiration only).
pub const DROUGHT_DORMANT_UPKEEP: f32 = 0.12;
/// Land-plant basal upkeep vs plankton (woody roots respire less).
pub const PLANT_UPKEEP_MULT: f32 = 0.35;
/// Extra score weight so roots prefer wetter substrate cells.
pub const ROOT_MOISTURE_AFFINITY: f32 = 2.8;
/// Soft local density: roots packed near the crown pay this extra.
pub const ROOT_CROWN_BLOB_PENALTY: f32 = 1.8;
/// Score bonus for stepping *into* Organic / Sand beds.
pub const ROOT_ORGANIC_AFFINITY: f32 = 0.85;
pub const ROOT_SAND_AFFINITY: f32 = 0.45;
/// Former invent threshold — kept as docs/history. Trunks are never
/// invented from a stemless body anymore; olive only elongates.
pub const STEM_INVENT_MIN_ALLOC: f32 = 0.18;

/// Soft caps so 1× bodies stay readable.
pub const MAX_ROOT_MODULES: usize = 16;
pub const MAX_STEM_MODULES: usize = 10;
pub const MAX_PHOTO_MODULES: usize = 12;
/// Extra Root modules allowed per photosystem beyond the sprout minimum.
pub const LAND_ROOTS_PER_PHOTOSYSTEM: usize = 3;
/// Fraction of spawn tank unlocked as storage per Root module.
pub const ROOT_STORE_FRAC: f32 = 0.04;
/// Cap on capacity multiplier from roots (`base_max × this`).
pub const ROOT_STORE_MAX_MULT: f32 = 2.0;

/// Energy fraction of spawn tank required before tissue growth.
pub const LAND_GROW_ENERGY_FRAC: f32 = 0.30;
/// Ticks between growth attempts.
pub const LAND_GROW_PERIOD: u64 = 48;
/// Base energy to place one Root pixel in soft substrate.
pub const ROOT_ELONGATE_BASE_COST: f32 = 2.4;
/// Energy to place one Stem / Photosystem pixel.
pub const SHOOT_GROW_COST: f32 = 1.6;
/// Energy fraction of spawn tank to fire a vegetative sprout.
pub const LAND_SPROUT_ENERGY_FRAC: f32 = 0.52;
/// Ticks between sprout attempts.
pub const LAND_SPROUT_PERIOD: u64 = 48;
/// Painted Root modules required before a sprout may fire.
pub const LAND_SPROUT_MIN_ROOTS: usize = 3;
/// Max columns a rhizome sprout may emerge from the crown.
pub const ROOT_SPROUT_MAX_DIST: i32 = 6;
/// Fraction of spawn tank spent to sprout (child gets half).
pub const LAND_SPROUT_COST_FRAC: f32 = 0.45;

/// Moisture band driving photo / growth / hibernate gates.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DroughtBand {
    Hydrated,
    Stressed,
    Dormant,
}

pub fn drought_band(moist_frac: f32) -> DroughtBand {
    if moist_frac < DROUGHT_DORMANT_FRAC {
        DroughtBand::Dormant
    } else if moist_frac < DROUGHT_STRESS_FRAC {
        DroughtBand::Stressed
    } else {
        DroughtBand::Hydrated
    }
}

/// Soft useful-root budget: sprout minimum + leaf-driven extras.
pub fn useful_root_budget(atom: &Atom) -> usize {
    LAND_SPROUT_MIN_ROOTS
        .saturating_add(atom.photosystem_count().saturating_mul(LAND_ROOTS_PER_PHOTOSYSTEM))
        .min(MAX_ROOT_MODULES)
}

/// Drought-aware soft budget — stress lifts the cap so plants keep digging.
pub fn useful_root_budget_for(atom: &Atom, drought: DroughtBand) -> usize {
    let base = useful_root_budget(atom);
    match drought {
        DroughtBand::Hydrated | DroughtBand::Dormant => base,
        DroughtBand::Stressed => {
            let lift = (MAX_ROOT_MODULES.saturating_sub(base) + 1) / 2;
            base.saturating_add(lift).min(MAX_ROOT_MODULES)
        }
    }
}

pub fn roots_past_soft_budget_for(atom: &Atom, drought: DroughtBand) -> bool {
    root_count(atom) >= useful_root_budget_for(atom, drought)
}

/// Effective energy tank from painted roots (starch / reserve analogy).
///
/// Photo, basal upkeep, and growth floors stay keyed to `base_max`; only
/// the storage clamp uses this larger capacity.
pub fn energy_capacity(base_max: f32, n_roots: usize) -> f32 {
    let base = base_max.max(1.0);
    let mult = (1.0 + ROOT_STORE_FRAC * n_roots as f32).min(ROOT_STORE_MAX_MULT);
    base * mult
}

/// Spawn-tank reference used for growth / sprout floors (not root-inflated).
pub fn tank_ref(atom: &Atom) -> f32 {
    let base = if atom.energy_base_max >= 1.0 {
        atom.energy_base_max
    } else {
        atom.energy_max
    };
    base.max(1.0)
}

/// Sync `energy_max` from root count; clamp current energy into the tank.
pub fn sync_root_storage(atom: &mut Atom) {
    if atom.energy_base_max < 1.0 {
        atom.energy_base_max = atom.energy_max.max(1.0);
    }
    let cap = energy_capacity(atom.energy_base_max, root_count(atom));
    atom.energy_max = cap;
    if atom.energy > cap {
        atom.energy = cap;
    }
}

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
///
/// Only removes sat from **porous solids** (never free Air water).
/// Removed sat should be returned to atmospheric humidity by the caller
/// so plants don't destroy water mass.
///
/// Returns (energy gained, sat removed, preferred (gx,gy) for humidity deposit).
pub fn drink_roots(world: &mut World, atom: &mut Atom) -> (f32, u32, (i32, i32)) {
    let n_roots = root_count(atom).max(1) as f32;
    atom.sip_acc = (atom.sip_acc + n_roots * ROOT_SIP_FRAC_PER_ROOT).min(2.0);
    let budget = atom.sip_acc.floor() as u8;
    if budget == 0 {
        return (0.0, 0, (atom.gx, atom.gy));
    }
    let want = budget.min(ROOT_SIP_MAX_SAT);
    let mut energy = 0.0f32;
    let mut taken = 0u8;
    let mut deposit_at = (atom.gx, atom.gy);
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Root || taken >= want {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if let Some(n) = sip_porous(world, wx, wy, want - taken) {
            energy += ROOT_WATER_ENERGY * n as f32;
            taken += n;
            deposit_at = (wx, wy);
            continue;
        }
        if let Some(n) = sip_porous(world, wx, wy - 1, want - taken) {
            energy += ROOT_WATER_ENERGY * n as f32;
            taken += n;
            deposit_at = (wx, wy - 1);
        }
    }
    atom.sip_acc = (atom.sip_acc - taken as f32).max(0.0);
    (energy, taken as u32, deposit_at)
}

fn sip_porous(world: &mut World, gx: i32, gy: i32, want: u8) -> Option<u8> {
    if want == 0 {
        return None;
    }
    let cell = world.get_cell(gx, gy)?;
    // Never drink free water / wet Air — only pore sat in solids.
    if cell.material == MaterialId::Air {
        return None;
    }
    let cap = water_capacity(cell.material);
    if cap == 0 || cell.sat.0 == 0 {
        return None;
    }
    let take = want.min(cell.sat.0).min(ROOT_SIP_MAX_SAT);
    let mut next = cell;
    next.sat.0 = cell.sat.0 - take;
    world.set_cell(gx, gy, next);
    Some(take)
}

/// Paint Root modules into `MaterialId::Organic` in place (preserve pore sat).
/// Returns how many cells were converted. Used when a land plant dies so the
/// root stencil stays in the ground as dead organic matter.
pub fn leave_dead_roots_in_place(world: &mut World, atom: &Atom) -> u32 {
    use crate::cell::Cell;
    let mut painted = 0u32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Root {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        let Some(c) = world.get_cell(wx, wy) else {
            continue;
        };
        match c.material {
            MaterialId::Bedrock | MaterialId::Ice | MaterialId::Snow | MaterialId::Water => {}
            MaterialId::Air if !c.sat.is_empty() => {
                // Don't plug free water with Organic.
            }
            MaterialId::Organic => {
                painted += 1; // already organic residue
            }
            _ => {
                let mut org = Cell::solid(MaterialId::Organic);
                let cap = water_capacity(MaterialId::Organic);
                org.sat.0 = if cap > 0 { c.sat.0.min(cap) } else { 0 };
                world.set_cell(wx, wy, org);
                painted += 1;
            }
        }
    }
    painted
}

/// Drop Photosystem modules as falling Organic litter (dry Air only).
/// Leaves peel off the corpse immediately; stems linger grey until dissolve.
pub fn drop_dead_leaves(world: &mut World, atom: &Atom) -> u32 {
    use crate::cell::Cell;
    let mut painted = 0u32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Photosystem {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        let Some(c) = world.get_cell(wx, wy) else {
            continue;
        };
        if c.material == MaterialId::Air && c.sat.is_empty() {
            world.set_cell(wx, wy, Cell::solid(MaterialId::Organic));
            painted += 1;
        }
    }
    painted
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

/// Fungus seat: Air above any solid. Prefers Organic / wet Sand, but
/// will land on bare rock too (may starve later — that's fine).
pub fn find_fungus_slot(world: &World, gx: i32, gy: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    let mut best: Option<(i32, i32)> = None; // score, y
    let consider = |world: &World, y: i32, best: &mut Option<(i32, i32)>| {
        if !fungus_crown(world, gx, y) {
            return;
        }
        let score = fungus_seat_score(world, gx, y);
        if best.map(|(s, _)| score > s).unwrap_or(true) {
            *best = Some((score, y));
        }
    };
    for dy in [0, 1, -1, 2, -2, 3, -3, 4, -4, 6, -6, 8, -8] {
        consider(world, gy + dy, &mut best);
    }
    if best.is_none() {
        for y in (gy - 32..=gy + 32).rev() {
            consider(world, y, &mut best);
        }
    }
    best.map(|(_, y)| y)
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

fn fungus_crown(world: &World, gx: i32, nucleus_y: i32) -> bool {
    let Some(air) = world.get_cell(gx, nucleus_y) else {
        return false;
    };
    if air.material != MaterialId::Air {
        return false;
    }
    matches!(
        world.get_cell(gx, nucleus_y - 1),
        Some(c) if c.material != MaterialId::Air
    )
}

fn fungus_seat_score(world: &World, gx: i32, nucleus_y: i32) -> i32 {
    let Some(below) = world.get_cell(gx, nucleus_y - 1) else {
        return 0;
    };
    match below.material {
        MaterialId::Organic => 100,
        MaterialId::Sand => {
            let cap = water_capacity(MaterialId::Sand).max(1);
            40 + (below.sat.0 as i32 * 40) / cap as i32
        }
        MaterialId::Clay => 30,
        _ if water_capacity(below.material) > 0 => 10,
        _ => 1, // bare rock / ice-adjacent solids — allowed but poor
    }
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
    let tank = tank_ref(atom);
    let grow_floor = tank * LAND_GROW_ENERGY_FRAC;
    if atom.energy < grow_floor {
        return 0.0;
    }

    let host_moist = root_moisture_frac(world, atom);
    let drought = drought_band(host_moist);
    let thirsty = matches!(drought, DroughtBand::Stressed);
    // Urge a lateral runner before sprouting (column rhizome bias).
    let need_runner = !has_lateral_runner(atom)
        && n_roots >= LAND_SPROUT_MIN_ROOTS.saturating_sub(1);
    // Past the soft root:shoot budget, only grow roots when thirsty or
    // forcing a rhizome runner.
    if roots_past_soft_budget_for(atom, drought) && !need_runner && !thirsty {
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
    let banking_for_sprout = atom.energy >= tank * LAND_SPROUT_ENERGY_FRAC * 0.85;
    // Cardinal only — diagonal steps packed crown mats beside live roots.
    const DIRS: [(i16, i16); 3] = [(0, -1), (-1, 0), (1, 0)];
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
            // Moisture tropism: sample the step cell and one deeper.
            let moist = cell_moisture_frac(world, wx, wy)
                .max(cell_moisture_frac(world, wx, wy - 1))
                .max(cell_moisture_frac(world, wx, wy - 2) * 0.85);
            let down = if dy < 0 { 1.0 } else { 0.0 };
            let lateral = if dx != 0 && dy == 0 { 0.35 } else { 0.0 };
            let mut score = moist * ROOT_MOISTURE_AFFINITY
                + depth_bias * down
                + (1.0 - depth_bias) * lateral
                - pen * 0.03;
            // Organic is transformed dead-root compost — fine to sit beside
            // or step into (score bonus below). Live roots stay exclusive.
            match cell.material {
                MaterialId::Organic => score += ROOT_ORGANIC_AFFINITY,
                MaterialId::Sand => score += ROOT_SAND_AFFINITY,
                _ => {}
            }
            // Extra nudge into clearly wetter cells than the tip's host.
            let tip_moist = cell_moisture_frac(
                world,
                world.wrap_x(atom.gx + tx as i32),
                atom.gy + ty as i32,
            );
            if moist > tip_moist + 0.05 {
                score += (moist - tip_moist) * 1.6;
            }
            // Can't place next to another *alive* root (parent tip OK).
            // Vertical dive may skim diagonally past a same-row runner so
            // a rhizome doesn't permanently block the crown column.
            let beside_live = atom.body.iter().any(|&(rx, ry, m)| {
                if m != ModuleId::Root || (rx, ry) == (tx, ty) {
                    return false;
                }
                if (rx - nx).abs() > 1 || (ry - ny).abs() > 1 {
                    return false;
                }
                let skim_runner = dx == 0 && dy < 0 && ry == ty && rx != tx;
                !skim_runner
            });
            if beside_live {
                continue;
            }
            // Same-row roots need a gap column (no 3-wide crown fan).
            // Rhizome may extend one cardinal step farther from x=0.
            let same_row_crowd = atom.body.iter().any(|&(rx, ry, m)| {
                if m != ModuleId::Root || ry != ny || (rx, ry) == (tx, ty) {
                    return false;
                }
                if (rx - nx).abs() > 2 {
                    return false;
                }
                let extending_out =
                    dy == 0 && (nx - tx).abs() == 1 && rx.abs() < nx.abs();
                !extending_out
            });
            if same_row_crowd {
                continue;
            }
            // One live thread per column — no lateral into an occupied lane.
            if dx != 0 {
                let col_taken = atom
                    .body
                    .iter()
                    .any(|&(rx, _, m)| m == ModuleId::Root && rx == nx);
                if col_taken {
                    continue;
                }
            }
            // Under-crown blob tax — prefer diving past the shallow mass.
            if ny >= -3 && nx.abs() <= 2 {
                let shallow = atom
                    .body
                    .iter()
                    .filter(|&&(rx, ry, m)| m == ModuleId::Root && ry >= -3 && rx.abs() <= 2)
                    .count();
                if shallow >= 3 {
                    score -= ROOT_CROWN_BLOB_PENALTY * (shallow as f32 - 2.0);
                }
            }
            // Rhizome urge when banking / missing a runner — but scale by
            // (1 − depth_bias) so deep divers still elongate downward.
            if need_runner || banking_for_sprout {
                let rhizome = (1.0 - depth_bias).max(0.12);
                if dx != 0 && dy == 0 {
                    score += 2.8 * rhizome;
                } else if dx != 0 && dy < 0 {
                    score += 1.1 * rhizome;
                }
                if nx != 0 {
                    score += 1.2 * rhizome;
                }
                if need_runner && dy < 0 && dx == 0 {
                    score -= 1.2 * rhizome;
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

/// World cells occupied by live or grey-corpse Stem (trunk) modules.
pub fn collect_trunk_world_cells<'a, I, J>(atoms: I, corpses: J) -> HashSet<(i32, i32)>
where
    I: IntoIterator<Item = &'a Atom>,
    J: IntoIterator<Item = &'a crate::organism::Corpse>,
{
    let mut out = HashSet::new();
    for a in atoms {
        for &(dx, dy, m) in &a.body {
            if m == ModuleId::Stem {
                out.insert((a.gx + dx as i32, a.gy + dy as i32));
            }
        }
    }
    for c in corpses {
        for &(dx, dy, m) in &c.body {
            if m == ModuleId::Stem {
                out.insert((c.gx + dx as i32, c.gy + dy as i32));
            }
        }
    }
    out
}

/// True when a new Stem at body `(nx,ny)` keeps a gap from foreign trunks.
/// Own same-column olive (vertical stack) is allowed; Moore neighbours that
/// are other live/dead stems are not. Leaves are unrestricted.
pub fn stem_spacing_ok(atom: &Atom, nx: i16, ny: i16, trunks: &HashSet<(i32, i32)>) -> bool {
    let wx = atom.gx + nx as i32;
    let wy = atom.gy + ny as i32;
    if trunks.contains(&(wx, wy)) {
        let own_here = atom
            .body
            .iter()
            .any(|&(bx, by, m)| m == ModuleId::Stem && bx == nx && by == ny);
        if !own_here {
            return false;
        }
    }
    for ox in -1i32..=1 {
        for oy in -1i32..=1 {
            if ox == 0 && oy == 0 {
                continue;
            }
            let tx = wx + ox;
            let ty = wy + oy;
            if !trunks.contains(&(tx, ty)) {
                continue;
            }
            // Vertical stack on this plant's own column — OK.
            let own_same_col = atom.body.iter().any(|&(bx, by, m)| {
                m == ModuleId::Stem
                    && bx == nx
                    && atom.gx + bx as i32 == tx
                    && atom.gy + by as i32 == ty
            });
            if own_same_col {
                continue;
            }
            return false;
        }
    }
    true
}

/// Stem upward or leaf place from surplus allocation.
///
/// Structure rules (stricter than free collage):
/// - Stem stacks only on Stem / Nucleus / Root — never on a leaf.
/// - Leaves attach to the highest Stem (or Nucleus if leafless chassis).
/// - Stemless bodies stay stemless: olive only elongates painted Stem.
/// - New Stem needs a Moore gap from other live/dead trunks (leaves may touch).
pub fn try_grow_shoot(atom: &mut Atom, tick: u64, trunks: &HashSet<(i32, i32)>) -> f32 {
    let (w_stem, w_leaf, _) = atom.genome.alloc_weights();
    let tank = tank_ref(atom);
    if atom.energy < tank * (LAND_GROW_ENERGY_FRAC + 0.08) {
        return 0.0;
    }
    let occupied: HashSet<(i16, i16)> = atom.body.iter().map(|&(x, y, _)| (x, y)).collect();
    let n_stem = stem_count(atom);
    let n_photo = atom.photosystem_count();

    let roll = hash01(tick, atom.gx as u64, atom.age_ticks, 0x5707);
    let shoot_sum = (w_stem + w_leaf).max(1e-6);
    let prefer_leaf = roll < w_leaf / shoot_sum;
    let cost = SHOOT_GROW_COST;
    if atom.energy < cost + 1.0 {
        return 0.0;
    }

    // Hard lock: no painted Stem ⇒ no trunk, regardless of alloc_stem.
    let can_grow_stem = n_stem > 0 && n_stem < MAX_STEM_MODULES && w_stem >= 0.08;

    let place_leaf = |atom: &mut Atom, occupied: &HashSet<(i16, i16)>| -> bool {
        if n_photo >= MAX_PHOTO_MODULES {
            return false;
        }
        // Attach beside the tallest stem (or nucleus if leafless chassis).
        let anchor = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .max_by_key(|(_, y, _)| *y)
            .map(|&(x, y, _)| (x, y))
            .or_else(|| {
                atom.body
                    .iter()
                    .find(|(_, _, m)| *m == ModuleId::Nucleus)
                    .map(|&(x, y, _)| (x, y))
            });
        let Some((tx, ty)) = anchor else {
            return false;
        };
        // Keep the olive tip clear whenever a trunk exists — stacking a
        // leaf on `(0,+1)` produced leaf→stem→leaf towers and blocked
        // further stem growth. Stemless chassis may still put a leaf up.
        // Leaves may sit next to other leaves — no spacing tax.
        let dirs: &[(i16, i16)] = if n_stem > 0 {
            &[(1, 0), (-1, 0), (1, 1), (-1, 1)]
        } else {
            &[(1, 0), (-1, 0), (0, 1), (1, 1), (-1, 1)]
        };
        for &(dx, dy) in dirs {
            let nx = tx + dx;
            let ny = ty + dy;
            if ny > 16 || occupied.contains(&(nx, ny)) {
                continue;
            }
            // Never plant a leaf directly above another leaf.
            if dy == 1
                && atom
                    .body
                    .iter()
                    .any(|&(x, y, m)| m == ModuleId::Photosystem && x == nx && y == ny - 1)
            {
                continue;
            }
            atom.energy -= cost;
            atom.body.push((nx, ny, ModuleId::Photosystem));
            return true;
        }
        false
    };

    let place_stem = |atom: &mut Atom, occupied: &HashSet<(i16, i16)>| -> bool {
        if !can_grow_stem {
            return false;
        }
        // Elongate the tallest painted stem with clear air above.
        let mut anchors: Vec<(i16, i16)> = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .map(|&(x, y, _)| (x, y))
            .collect();
        anchors.sort_by_key(|&(_, y)| std::cmp::Reverse(y));
        for (ax, ay) in anchors {
            let nx = ax;
            let ny = ay + 1;
            if ny > 16 || occupied.contains(&(nx, ny)) {
                continue;
            }
            // Hard rule: stem never sits on a photosystem cell.
            if atom
                .body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Photosystem && x == nx && y == ay)
            {
                continue;
            }
            if !stem_spacing_ok(atom, nx, ny, trunks) {
                continue;
            }
            atom.energy -= cost;
            atom.body.push((nx, ny, ModuleId::Stem));
            return true;
        }
        false
    };

    if prefer_leaf {
        if place_leaf(atom, &occupied) {
            return cost;
        }
        if place_stem(atom, &occupied) {
            return cost;
        }
    } else {
        if place_stem(atom, &occupied) {
            return cost;
        }
        if place_leaf(atom, &occupied) {
            return cost;
        }
    }
    0.0
}

/// Bias genome allocation toward tissues that are already painted.
/// A Root+Nucleus+Photosystem chassis won't invent a trunk from the
/// default `alloc_stem = 0.25` (shoot growth also hard-locks stemless).
pub fn sync_alloc_to_body(genome: &mut Genome, body: &[BodyModule]) {
    let has_stem = body.iter().any(|(_, _, m)| *m == ModuleId::Stem);
    let has_root = body.iter().any(|(_, _, m)| *m == ModuleId::Root);
    let has_leaf = body.iter().any(|(_, _, m)| *m == ModuleId::Photosystem);
    if !has_stem {
        genome.alloc_stem = genome.alloc_stem.min(0.05);
    }
    if !has_root {
        genome.alloc_root = genome.alloc_root.min(0.05);
    }
    if !has_leaf {
        genome.alloc_leaf = genome.alloc_leaf.min(0.05);
    }
}

/// Child body for vegetative sprout — inherits stemless vs stemmed habit.
pub fn sprout_body(parent: &Atom) -> Vec<BodyModule> {
    if stem_count(parent) > 0 {
        crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus()
    } else {
        vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Photosystem),
        ]
    }
}

/// One growth pulse: root and/or shoot from allocation weights.
pub fn try_grow_plant(
    world: &World,
    atom: &mut Atom,
    tick: u64,
    trunks: &HashSet<(i32, i32)>,
) -> f32 {
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
            spent += try_grow_shoot(atom, tick, trunks);
        }
    } else {
        spent += try_grow_shoot(atom, tick, trunks);
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
/// Child chassis follows the parent (stemless stays stemless); genome is
/// mutated then re-synced so alloc can't reintroduce a trunk.
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
    let tank = tank_ref(atom);
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

    let body = sprout_body(atom);
    let mut child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    sync_alloc_to_body(&mut child_genome, &body);
    // Child inherits spawn-tank size, not the parent's root-inflated max.
    let mut child = Atom::from_body(wx, gy, tank, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, child.energy_max);
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
