//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Set E litter fungi (thin slice): Digest + Hypha on soft litter /
//! Organic cells. Spec: `docs/organism/FUNGI.md`.

use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::cell::{water_capacity, Cell};
use crate::grid::World;
use crate::organism::{Atom, ModuleId};
use crate::plant::{apply_genome, find_plant_slot, pin_plant_pose};

/// Soft litter units treated as fully labile food (per column).
/// Stratigraphic Organic cells contribute this many labile units each.
pub const LABILE_ORGANIC_UNITS: f32 = 5.0;
/// Fraction of visible labile pool removable in one digest event.
pub const DIGEST_TICK_FRAC: f32 = 0.12;
/// Soft unit cap removed per digest event.
pub const DIGEST_MAX_UNITS: u16 = 4;
/// Energy gained per litter/Organic unit digested.
pub const DIGEST_ENERGY_PER_UNIT: f32 = 0.55;
/// Upkeep per Hypha module / tick while active.
pub const HYPHA_UPKEEP: f32 = 0.012;
/// Upkeep per Digest module / tick while active.
pub const DIGEST_UPKEEP: f32 = 0.020;
/// Labile food below which fungi hibernate.
pub const FUNGUS_STARVE_UNITS: f32 = 1.0;
/// Pore moisture below which fungi hibernate.
pub const FUNGUS_DROUGHT_FRAC: f32 = 0.04;
/// Max consecutive dormant ticks before death.
pub const FUNGUS_HIBERNATE_MAX_TICKS: u32 = 7_200;
/// Upkeep multiplier while dormant.
pub const FUNGUS_DORMANT_UPKEEP: f32 = 0.15;
/// Energy fraction of tank to attempt a spore burst.
pub const FUNGUS_SPORE_ENERGY_FRAC: f32 = 0.55;
/// Spore attempt period (ticks).
pub const FUNGUS_SPORE_PERIOD: u64 = 64;
/// Max columns a spore may land from the parent.
pub const FUNGUS_SPORE_MAX_DIST: i32 = 5;
/// Soft litter deposited per body module on death.
pub const DEATH_LITTER_PER_MODULE: u16 = 6;
/// Cap soft litter added from one corpse.
pub const DEATH_LITTER_MAX: u16 = 48;
/// How many Organic cells to scan below the crown when counting food.
const ORGANIC_SCAN_DEPTH: i32 = 6;

/// True when the body is a detritus habit (Digest, no Root/Stem).
pub fn is_fungus(atom: &Atom) -> bool {
    let has_digest = atom.body.iter().any(|(_, _, m)| *m == ModuleId::Digest);
    let has_root = atom.body.iter().any(|(_, _, m)| *m == ModuleId::Root);
    let has_stem = atom.body.iter().any(|(_, _, m)| *m == ModuleId::Stem);
    has_digest && !has_root && !has_stem
}

pub fn digest_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Digest)
        .count()
}

pub fn hypha_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Hypha)
        .count()
}

/// Soft litter units at wrapped column `gx`.
pub fn soft_litter_at(world: &World, gx: i32) -> u16 {
    let gx = world.wrap_x(gx);
    world.soft_litter.get(&gx).copied().unwrap_or(0)
}

/// Add soft litter at column `gx` (death / seeding).
pub fn add_soft_litter(world: &mut World, gx: i32, units: u16) {
    if units == 0 {
        return;
    }
    let gx = world.wrap_x(gx);
    let e = world.soft_litter.entry(gx).or_insert(0);
    *e = e.saturating_add(units);
}

fn take_soft_litter(world: &mut World, gx: i32, want: u16) -> u16 {
    let gx = world.wrap_x(gx);
    let Some(e) = world.soft_litter.get_mut(&gx) else {
        return 0;
    };
    let take = (*e).min(want);
    *e = e.saturating_sub(take);
    if *e == 0 {
        world.soft_litter.remove(&gx);
    }
    take
}

/// Count Organic solid cells in the litter band under `gx` near `gy`.
fn organic_cells_near(world: &World, gx: i32, gy: i32) -> u32 {
    let gx = world.wrap_x(gx);
    let mut n = 0u32;
    for dx in [0i32, -1, 1] {
        let nx = world.wrap_x(gx + dx);
        for dy in 0..=ORGANIC_SCAN_DEPTH {
            let y = gy - dy;
            if matches!(
                world.get_cell(nx, y),
                Some(c) if c.material == MaterialId::Organic
            ) {
                n += 1;
            }
        }
    }
    n
}

/// Labile food units visible to fungi at this crown.
pub fn labile_food_units(world: &World, gx: i32, gy: i32) -> f32 {
    let litter = soft_litter_at(world, gx) as f32;
    let organic = organic_cells_near(world, gx, gy) as f32 * LABILE_ORGANIC_UNITS;
    litter + organic
}

/// Pore moisture under the fungus crown.
pub fn fungus_moisture_frac(world: &World, atom: &Atom) -> f32 {
    let gx = world.wrap_x(atom.gx);
    let mut best = 0.0f32;
    for dy in 0..=3 {
        let y = atom.gy - dy;
        if let Some(c) = world.get_cell(gx, y) {
            if c.material == MaterialId::Air {
                continue;
            }
            let cap = water_capacity(c.material);
            if cap > 0 {
                best = best.max(c.sat.0 as f32 / cap as f32);
            }
        }
    }
    best
}

/// True when fungi should hibernate (no food and/or bone-dry).
pub fn fungus_should_hibernate(world: &World, atom: &Atom) -> bool {
    if labile_food_units(world, atom.gx, atom.gy) < FUNGUS_STARVE_UNITS {
        return true;
    }
    fungus_moisture_frac(world, atom) < FUNGUS_DROUGHT_FRAC
}

/// Unit budget for one digest event from gene + modules.
pub fn digest_budget_units(genome: &Genome, atom: &Atom) -> u16 {
    let n_d = digest_count(atom).max(1) as f32;
    let n_h = hypha_count(atom) as f32;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    let scale = n_d * (1.0 + 0.15 * n_h) * rate;
    let u = (scale * DIGEST_MAX_UNITS as f32).round() as u16;
    u.clamp(1, DIGEST_MAX_UNITS.saturating_mul(2))
}

fn find_organic_xy(world: &World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    let gx = world.wrap_x(gx);
    for dx in [0i32, -1, 1] {
        let nx = world.wrap_x(gx + dx);
        for dy in 0..=ORGANIC_SCAN_DEPTH {
            let y = gy - dy;
            if matches!(
                world.get_cell(nx, y),
                Some(c) if c.material == MaterialId::Organic
            ) {
                return Some((nx, y));
            }
        }
    }
    None
}

/// Remove up to `want` labile units; returns (units_taken, energy).
/// Prefers soft litter, then peels Organic cells → Air (lite cavity).
pub fn digest_labile(world: &mut World, gx: i32, gy: i32, want: u16) -> (u16, f32) {
    if want == 0 {
        return (0, 0.0);
    }
    let labile = labile_food_units(world, gx, gy);
    if labile < FUNGUS_STARVE_UNITS {
        return (0, 0.0);
    }
    let tick_cap = (labile * DIGEST_TICK_FRAC).floor().max(0.0) as u16;
    let mut left = want.min(tick_cap).min(DIGEST_MAX_UNITS);
    if left == 0 {
        return (0, 0.0);
    }

    let mut taken = 0u16;
    let from_litter = take_soft_litter(world, gx, left);
    taken += from_litter;
    left = left.saturating_sub(from_litter);

    while left > 0 {
        let Some((ox, oy)) = find_organic_xy(world, gx, gy) else {
            break;
        };
        world.set_cell(ox, oy, Cell::air());
        let spend = (LABILE_ORGANIC_UNITS.ceil() as u16).max(1).min(left);
        taken = taken.saturating_add(spend);
        left = left.saturating_sub(spend);
    }

    if taken > 0 {
        (taken, taken as f32 * DIGEST_ENERGY_PER_UNIT)
    } else {
        (0, 0.0)
    }
}

/// Active upkeep for Digest + Hypha tissue.
pub fn fungus_upkeep(atom: &Atom, dormant: bool) -> f32 {
    let base = DIGEST_UPKEEP * digest_count(atom) as f32
        + HYPHA_UPKEEP * hypha_count(atom) as f32;
    if dormant {
        base * FUNGUS_DORMANT_UPKEEP
    } else {
        base
    }
}

/// Nucleus sits on Air above any solid (litter band / Organic OK).
pub fn is_fungus_seated(world: &World, atom: &Atom) -> bool {
    let gx = world.wrap_x(atom.gx);
    let Some(air) = world.get_cell(gx, atom.gy) else {
        return false;
    };
    if air.material != MaterialId::Air {
        return false;
    }
    matches!(
        world.get_cell(gx, atom.gy - 1),
        Some(c) if c.material != MaterialId::Air
    )
}

/// Pick a nearby column with labile food for a spore landing.
pub fn pick_spore_site(world: &World, atom: &Atom, tick: u64, id: u32) -> Option<i32> {
    let wx0 = atom.gx;
    let mut best: Option<(f32, i32)> = None;
    for dist in 1..=FUNGUS_SPORE_MAX_DIST {
        for sign in [1i32, -1] {
            let wx = world.wrap_x(wx0 + sign * dist);
            let Some(gy) = find_plant_slot(world, wx, atom.gy) else {
                continue;
            };
            let food = labile_food_units(world, wx, gy);
            if food < FUNGUS_STARVE_UNITS {
                continue;
            }
            let score = food - dist as f32 * 0.3;
            if best.map(|(s, _)| score > s).unwrap_or(true) {
                best = Some((score, wx));
            }
        }
    }
    if let Some((_, wx)) = best {
        return Some(wx);
    }
    // Fallback: any plantable neighbour (spore bank).
    let flip = hash_u64(world.seed.0, tick, id as u64, 0xF5C0) & 1;
    let dir = if flip == 0 { 1 } else { -1 };
    for dist in 1..=FUNGUS_SPORE_MAX_DIST {
        let wx = world.wrap_x(wx0 + dir * dist);
        if find_plant_slot(world, wx, atom.gy).is_some() {
            return Some(wx);
        }
    }
    None
}

/// Spore fission: child fungus on a neighbour litter column.
pub fn try_spore(
    world: &World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
) -> Option<Atom> {
    if !pop_room || atom.cooldown > 0 {
        return None;
    }
    if !is_fungus(atom) || digest_count(atom) < 1 {
        return None;
    }
    let tank = if atom.energy_base_max >= 1.0 {
        atom.energy_base_max
    } else {
        atom.energy_max
    }
    .max(1.0);
    if atom.energy < tank * FUNGUS_SPORE_ENERGY_FRAC {
        return None;
    }
    let wx = pick_spore_site(world, atom, tick, entity_id)?;
    let gy = find_plant_slot(world, wx, atom.gy)?;
    let cost = tank * 0.40;
    if atom.energy < cost {
        return None;
    }
    atom.energy -= cost;
    atom.cooldown = FUNGUS_SPORE_PERIOD;

    let body = crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus();
    let child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    let mut child = Atom::from_body(wx, gy, tank, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, child.energy_max);
    child.cooldown = FUNGUS_SPORE_PERIOD;
    pin_plant_pose(&mut child);
    if !is_fungus_seated(world, &child) {
        atom.energy = (atom.energy + cost).min(atom.energy_max);
        atom.cooldown = 0;
        return None;
    }
    Some(child)
}

/// Soft litter + one Organic on the bed (fallback when no body cells paint).
pub fn deposit_death_litter(world: &mut World, gx: i32, gy: i32, n_modules: usize) {
    let units = (DEATH_LITTER_PER_MODULE as usize)
        .saturating_mul(n_modules.max(1))
        .min(DEATH_LITTER_MAX as usize) as u16;
    add_soft_litter(world, gx, units);
    deposit_organic_cell(world, gx, gy);
}

/// Dissolve a lingering corpse into Organic matter + soft litter.
///
/// Body footprint: dry Air and non-bedrock solids become `MaterialId::Organic`
/// in place (ghost-root / standing-dead → soil). Wet Air (free water) is left
/// alone so lakes aren't plugged; if nothing painted, fall back to a bed pile.
pub fn dissolve_corpse_to_organic(
    world: &mut World,
    gx: i32,
    gy: i32,
    body: &[(i16, i16, ModuleId)],
) {
    let n_modules = body.len().max(1);
    let units = (DEATH_LITTER_PER_MODULE as usize)
        .saturating_mul(n_modules)
        .min(DEATH_LITTER_MAX as usize) as u16;
    add_soft_litter(world, gx, units);

    let mut painted = 0u32;
    for &(dx, dy, _) in body {
        let wx = world.wrap_x(gx + dx as i32);
        let wy = gy + dy as i32;
        let Some(c) = world.get_cell(wx, wy) else {
            continue;
        };
        match c.material {
            MaterialId::Air if c.sat.is_empty() => {
                world.set_cell(wx, wy, Cell::solid(MaterialId::Organic));
                painted += 1;
            }
            MaterialId::Air => {
                // Free water — leave the lake; litter already banked.
            }
            MaterialId::Bedrock | MaterialId::Ice | MaterialId::Snow | MaterialId::Water => {}
            _ => {
                // Sand / stone / clay / Organic under roots → Organic residue.
                world.set_cell(wx, wy, Cell::solid(MaterialId::Organic));
                painted += 1;
            }
        }
    }
    if painted == 0 {
        deposit_organic_cell(world, gx, gy);
    }
}

fn deposit_organic_cell(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    let mut y = gy;
    for _ in 0..64 {
        match world.get_cell(gx, y) {
            Some(c) if c.material != MaterialId::Air => {
                if let Some(above) = world.get_cell(gx, y + 1) {
                    if above.material == MaterialId::Air && above.sat.is_empty() {
                        world.set_cell(gx, y + 1, Cell::solid(MaterialId::Organic));
                    }
                }
                return;
            }
            None => return,
            Some(_) => y -= 1,
        }
    }
}

fn hash_u64(a: u64, b: u64, c: u64, salt: u64) -> u64 {
    let mut x = a
        .wrapping_mul(0x9E37_79B9_7F4A_7C15)
        .wrapping_add(b)
        .wrapping_add(c)
        .wrapping_add(salt);
    x ^= x >> 30;
    x = x.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    x ^= x >> 27;
    x
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::Sat;
    use crate::chunk::ChunkCoord;
    use crate::organism::BodyModule;

    fn litter_plot() -> World {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    fn fungus_body() -> Vec<BodyModule> {
        crate::blueprint::Blueprint::minimal_fungus().modules_relative_to_nucleus()
    }

    #[test]
    fn digest_prefers_soft_litter() {
        let mut w = litter_plot();
        add_soft_litter(&mut w, 4, 40);
        w.set_cell(4, 2, Cell::solid(MaterialId::Organic));
        let (taken, energy) = digest_labile(&mut w, 4, 2, 4);
        assert!(taken > 0 && taken <= 4);
        assert!(energy > 0.0);
        assert!(soft_litter_at(&w, 4) < 40, "should eat soft litter first");
        assert_eq!(
            w.get_cell(4, 2).map(|c| c.material),
            Some(MaterialId::Organic)
        );
    }

    #[test]
    fn starve_column_hibernates() {
        let w = litter_plot();
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(fungus_should_hibernate(&w, &atom));
    }

    #[test]
    fn rich_litter_does_not_hibernate() {
        let mut w = litter_plot();
        add_soft_litter(&mut w, 4, 40);
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(!fungus_should_hibernate(&w, &atom));
    }

    #[test]
    fn hypha_raises_digest_budget() {
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let mut atom = Atom::from_body(4, 2, 40.0, fungus_body());
        atom.genome = g;
        let base = digest_budget_units(&g, &atom);
        atom.body.push((4, 0, ModuleId::Hypha));
        atom.body.push((5, 0, ModuleId::Hypha));
        let boosted = digest_budget_units(&g, &atom);
        assert!(boosted >= base);
    }
}
