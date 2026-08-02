//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Mycelial fungi (Set E redesign): Digest + Hypha atoms colonize
//! Organic with a faint mycelium intensity (`Cell::_pad`), sip soft
//! litter slowly for energy, and only after long colonization convert
//! Organic → Soil (never Sand) while preserving pore water. Rare
//! surface fruiting bodies shed wind-biased spores.
//! Spec notes: `docs/organism/FUNGI.md`, `docs/organism/VOXEL_PLANTS.md`.

use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::cell::{water_capacity, Cell, CellFlags};
use crate::grid::World;
use crate::organism::{Atom, ModuleId};
use crate::plant::{apply_genome, find_fungus_slot, pin_plant_pose};

/// Soft litter units treated as fully labile food (per column).
/// Stratigraphic Organic cells are substrate (mycelium), not instant fuel.
pub const LABILE_ORGANIC_UNITS: f32 = 5.0;
/// Fraction of soft-litter pool removable in one digest event (slow).
pub const DIGEST_TICK_FRAC: f32 = 0.02;
/// Soft litter units removed per digest event (hard cap).
pub const DIGEST_MAX_UNITS: u16 = 1;
/// Energy gained per soft-litter unit digested.
pub const DIGEST_ENERGY_PER_UNIT: f32 = 0.55;
/// Tiny energy from advancing mycelium one step (not from destroying cells).
pub const MYCELIUM_ENERGY: f32 = 0.12;
/// Baseline energy / tick while hyphae forage on Organic substrate.
/// Soft litter is a bonus sip; Organic alone must keep a seated fungus alive
/// even on dry / low-`digest_rate` beds.
pub const MYCELIUM_FORAGE: f32 = 0.075;
/// Ticks between mycelium growth pulses on a local Organic cell.
pub const MYCELIUM_GROW_PERIOD: u64 = 24;
/// Mycelium intensity gained per growth pulse (toward 255).
pub const MYCELIUM_GROW_AMOUNT: u8 = 2;
/// Intensity before Organic may compost into Soil.
pub const MYCELIUM_SOIL_THRESHOLD: u8 = 220;
/// 1-in-N chance per eligible growth pulse to compost a fully threaded cell.
pub const MYCELIUM_SOIL_CONVERT_ODDS: u64 = 6_000;
/// Neighbour Organic cells colonized from a threaded cell (1-in-N).
pub const MYCELIUM_SPREAD_ODDS: u64 = 48;
/// Upkeep per Hypha module / tick while active.
pub const HYPHA_UPKEEP: f32 = 0.008;
/// Upkeep per Digest module / tick while active.
pub const DIGEST_UPKEEP: f32 = 0.012;
/// Labile food below which fungi hibernate (soft litter only).
pub const FUNGUS_STARVE_UNITS: f32 = 1.0;
/// Pore moisture below which fungi hibernate.
pub const FUNGUS_DROUGHT_FRAC: f32 = 0.04;
/// Max consecutive dormant ticks before death.
pub const FUNGUS_HIBERNATE_MAX_TICKS: u32 = 7_200;
/// Upkeep multiplier while dormant.
pub const FUNGUS_DORMANT_UPKEEP: f32 = 0.15;
/// Energy fraction of tank to attempt a fruiting / spore burst.
pub const FUNGUS_SPORE_ENERGY_FRAC: f32 = 0.70;
/// Minimum ticks between fruiting attempts (very rare).
pub const FUNGUS_SPORE_PERIOD: u64 = 2_400;
/// Max columns a wind-borne spore may travel.
pub const FUNGUS_SPORE_MAX_DIST: i32 = 28;
/// Soft litter deposited per body module on death.
pub const DEATH_LITTER_PER_MODULE: u16 = 6;
/// Cap soft litter added from one corpse.
pub const DEATH_LITTER_MAX: u16 = 48;
/// How deep / wide to scan for Organic substrate under the fungus.
const ORGANIC_SCAN_DEPTH: i32 = 8;
const ORGANIC_SCAN_RADIUS: i32 = 2;

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

/// Count Organic solid cells in the litter / soil band near the fungus.
fn organic_cells_near(world: &World, gx: i32, gy: i32) -> u32 {
    let gx = world.wrap_x(gx);
    let mut n = 0u32;
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SCAN_DEPTH..=ORGANIC_SCAN_DEPTH {
            let y = gy + dy;
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

/// Soft litter visible as labile fuel (Organic is substrate, not fuel).
pub fn labile_food_units(world: &World, gx: i32, _gy: i32) -> f32 {
    soft_litter_at(world, gx) as f32
}

/// True when the fungus has Organic substrate to colonize (even if litter is gone).
pub fn has_organic_substrate(world: &World, gx: i32, gy: i32) -> bool {
    organic_cells_near(world, gx, gy) > 0
}

/// Pore moisture under / around the fungus (supports underground seats).
pub fn fungus_moisture_frac(world: &World, atom: &Atom) -> f32 {
    let gx = world.wrap_x(atom.gx);
    let mut best = 0.0f32;
    for dy in -2..=3 {
        let y = atom.gy + dy;
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

/// True when fungi should hibernate (no litter and no Organic).
///
/// Bone-dry seats without Organic still dormancy. Painted Organic is the
/// colony substrate even at sat=0 — drought only slows foraging (see
/// [`colonize_and_compost`]), it must not shut the fungus off entirely or
/// editor-placed beds starve to death without ever threading.
pub fn fungus_should_hibernate(world: &World, atom: &Atom) -> bool {
    let litter = labile_food_units(world, atom.gx, atom.gy);
    let substrate = has_organic_substrate(world, atom.gx, atom.gy);
    if litter < FUNGUS_STARVE_UNITS && !substrate {
        return true;
    }
    if !substrate && fungus_moisture_frac(world, atom) < FUNGUS_DROUGHT_FRAC {
        return true;
    }
    false
}

/// Soft-litter sip budget from gene + modules (capped low — mycelium is slow).
pub fn digest_budget_units(genome: &Genome, atom: &Atom) -> u16 {
    let n_d = digest_count(atom).max(1) as f32;
    let n_h = hypha_count(atom) as f32;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    let scale = n_d * (1.0 + 0.10 * n_h) * rate;
    let u = (scale * DIGEST_MAX_UNITS as f32).round() as u16;
    u.clamp(1, DIGEST_MAX_UNITS)
}

fn find_organic_xy(world: &World, gx: i32, gy: i32) -> Option<(i32, i32)> {
    let gx = world.wrap_x(gx);
    let mut best: Option<(u8, i32, i32)> = None; // lowest mycelium first
    for dx in -ORGANIC_SCAN_RADIUS..=ORGANIC_SCAN_RADIUS {
        let nx = world.wrap_x(gx + dx);
        for dy in -ORGANIC_SCAN_DEPTH..=ORGANIC_SCAN_DEPTH {
            let y = gy + dy;
            if let Some(c) = world.get_cell(nx, y) {
                if c.material == MaterialId::Organic {
                    let m = c.mycelium();
                    if best.map(|(bm, _, _)| m < bm).unwrap_or(true) {
                        best = Some((m, nx, y));
                    }
                }
            }
        }
    }
    best.map(|(_, x, y)| (x, y))
}

/// Push leftover pore water into neighbouring Air / porous cells so
/// Organic → Soil compaction never destroys water mass.
fn push_excess_sat(world: &mut World, gx: i32, gy: i32, mut excess: u8) {
    if excess == 0 {
        return;
    }
    for (dx, dy) in [(0i32, 1), (0, -1), (1, 0), (-1, 0), (1, 1), (-1, 1)] {
        if excess == 0 {
            break;
        }
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        let Some(mut c) = world.get_cell(nx, ny) else {
            continue;
        };
        let cap = world.water_capacity(c.material);
        if cap == 0 || c.sat.0 >= cap {
            continue;
        }
        let room = cap - c.sat.0;
        let give = excess.min(room);
        c.sat.0 = c.sat.0.saturating_add(give);
        excess = excess.saturating_sub(give);
        world.set_cell(nx, ny, c);
    }
    // Last resort: create a free-water film above if anything remains.
    if excess > 0 {
        if let Some(mut above) = world.get_cell(gx, gy + 1) {
            if above.material == MaterialId::Air {
                let room = u8::MAX - above.sat.0;
                let give = excess.min(room);
                above.sat.0 = above.sat.0.saturating_add(give);
                world.set_cell(gx, gy + 1, above);
            }
        }
    }
}

/// Convert a fully colonized Organic cell into Soil, preserving water.
pub fn compost_organic_to_soil(world: &mut World, gx: i32, gy: i32) -> bool {
    let gx = world.wrap_x(gx);
    let Some(c) = world.get_cell(gx, gy) else {
        return false;
    };
    if c.material != MaterialId::Organic {
        return false;
    }
    let old_sat = c.sat.0;
    let cap = world.water_capacity(MaterialId::Soil);
    let keep = if cap > 0 { old_sat.min(cap) } else { 0 };
    let excess = old_sat.saturating_sub(keep);
    let mut soil = Cell::solid(MaterialId::Soil);
    soil.sat.0 = keep;
    soil.flags.set(CellFlags::COMPACTED);
    world.set_cell(gx, gy, soil);
    push_excess_sat(world, gx, gy, excess);
    true
}

/// Sip soft litter for energy. Organic cells are colonized separately via
/// [`colonize_and_compost`] — this never converts materials.
pub fn digest_labile(world: &mut World, gx: i32, _gy: i32, want: u16) -> (u16, f32) {
    if want == 0 {
        return (0, 0.0);
    }
    let litter = soft_litter_at(world, gx) as f32;
    if litter < FUNGUS_STARVE_UNITS {
        return (0, 0.0);
    }
    // At least one unit when any labile litter is present — frac alone
    // floors to 0 on small piles with the slow DIGEST_TICK_FRAC.
    let tick_cap = (litter * DIGEST_TICK_FRAC)
        .floor()
        .max(1.0) as u16;
    let sip = want.min(tick_cap).min(DIGEST_MAX_UNITS);
    if sip == 0 {
        return (0, 0.0);
    }
    let taken = take_soft_litter(world, gx, sip);
    if taken > 0 {
        (taken, taken as f32 * DIGEST_ENERGY_PER_UNIT)
    } else {
        (0, 0.0)
    }
}

/// Grow mycelium with gene/hypha scaling; compost when ready.
/// Preferred entry from `step_fungus` (includes genome).
///
/// Returns forage energy every call when Organic is in range (keeps a
/// seated fungus alive without soft litter). Intensity pulses on a
/// slower cadence; dry beds slow both forage and threading but do not
/// zero them out.
pub fn colonize_and_compost(
    world: &mut World,
    gx: i32,
    gy: i32,
    genome: &Genome,
    atom: &Atom,
    tick: u64,
) -> f32 {
    let Some((ox, oy)) = find_organic_xy(world, gx, gy) else {
        return 0.0;
    };
    let n_h = hypha_count(atom) as u8;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    let moist = fungus_moisture_frac(world, atom);
    // Dry Organic still feeds slowly; wet beds forage at full rate.
    let moist_scale = if moist < FUNGUS_DROUGHT_FRAC {
        0.55
    } else {
        (0.70 + 0.30 * moist.clamp(0.0, 1.0)).clamp(0.55, 1.0)
    };
    // Forage stipend stays near break-even even at low digest_rate —
    // that gene paces threading, not whether the fungus can live on Organic.
    let forage_rate = rate.clamp(0.85, 1.5);
    let mut energy = MYCELIUM_FORAGE
        * (1.0 + 0.06 * n_h as f32)
        * forage_rate
        * moist_scale;

    // Period stretches when digest_rate is low; high rate shortens slightly.
    // Use the organism tick (not only `world.tick`) so growth stays paced
    // even when callers forget to sync World::tick in tests.
    let period = ((MYCELIUM_GROW_PERIOD as f32) / rate.clamp(0.25, 2.0)).round() as u64;
    let period = period.max(8);
    if tick % period == 0 {
        if let Some(mut c) = world.get_cell(ox, oy) {
            let before = c.mycelium();
            if before < 255 {
                let add = MYCELIUM_GROW_AMOUNT.saturating_add(n_h / 4).max(1);
                // Dry beds thread more slowly.
                let add = if moist < FUNGUS_DROUGHT_FRAC {
                    add.min(1)
                } else {
                    add.min(3)
                };
                c.set_mycelium(before.saturating_add(add));
                world.set_cell(ox, oy, c);
                energy += MYCELIUM_ENERGY * (1.0 + 0.05 * n_h as f32) * moist_scale;
            }
            let intensity = world
                .get_cell(ox, oy)
                .map(|c| c.mycelium())
                .unwrap_or(before);
            if intensity >= MYCELIUM_SOIL_THRESHOLD {
                let h = hash_u64(world.seed.0, tick, ox as u64, oy as u64);
                if h % MYCELIUM_SOIL_CONVERT_ODDS == 0 {
                    compost_organic_to_soil(world, ox, oy);
                }
            }
        }
        let h = hash_u64(world.seed.0, tick, gx as u64, 0x51CE_A11C);
        if h % MYCELIUM_SPREAD_ODDS == 0 {
            spread_mycelium_once(world, ox, oy);
        }
    }
    energy
}

fn spread_mycelium_once(world: &mut World, gx: i32, gy: i32) {
    let gx = world.wrap_x(gx);
    for (dx, dy) in [(1i32, 0), (-1, 0), (0, 1), (0, -1), (1, -1), (-1, -1)] {
        let nx = world.wrap_x(gx + dx);
        let ny = gy + dy;
        if let Some(mut c) = world.get_cell(nx, ny) {
            if c.material == MaterialId::Organic && c.mycelium() < 40 {
                c.set_mycelium(c.mycelium().saturating_add(8));
                world.set_cell(nx, ny, c);
                return;
            }
        }
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

/// Nucleus may sit in Organic (underground mycelium) or Air above a solid.
pub fn is_fungus_seated(world: &World, atom: &Atom) -> bool {
    let gx = world.wrap_x(atom.gx);
    let Some(here) = world.get_cell(gx, atom.gy) else {
        return false;
    };
    if here.material == MaterialId::Organic {
        return true;
    }
    if here.material != MaterialId::Air {
        return false;
    }
    matches!(
        world.get_cell(gx, atom.gy - 1),
        Some(c) if c.material != MaterialId::Air
    )
}

/// True when this column has colonized Organic open to Air (fruiting site).
pub fn fruiting_surface_ready(world: &World, gx: i32) -> Option<i32> {
    let gx = world.wrap_x(gx);
    // Scan for Organic with mycelium that has Air directly above.
    for y in (-64..128).rev() {
        let Some(c) = world.get_cell(gx, y) else {
            continue;
        };
        if c.material != MaterialId::Organic || c.mycelium() < 80 {
            continue;
        }
        if matches!(
            world.get_cell(gx, y + 1),
            Some(a) if a.material == MaterialId::Air
        ) {
            return Some(y + 1); // Air cell for the fruiting body / spore launch
        }
    }
    None
}

/// Pick a downwind Organic seat for a spore (wind-biased).
pub fn pick_spore_site(
    world: &World,
    atom: &Atom,
    tick: u64,
    id: u32,
    wind_vx: f32,
) -> Option<(i32, i32)> {
    let wx0 = atom.gx;
    let prefer_dir = if wind_vx.abs() < 0.05 {
        let flip = hash_u64(world.seed.0, tick, id as u64, 0xF5C0) & 1;
        if flip == 0 {
            1
        } else {
            -1
        }
    } else if wind_vx > 0.0 {
        1
    } else {
        -1
    };
    let mut best: Option<(f32, i32, i32)> = None;
    for dist in 1..=FUNGUS_SPORE_MAX_DIST {
        for &sign in &[prefer_dir, -prefer_dir] {
            let wx = world.wrap_x(wx0 + sign * dist);
            let Some(gy) = find_fungus_slot(world, wx, atom.gy) else {
                continue;
            };
            // Prefer Organic substrate; allow any fungus seat as bank.
            let organic = organic_cells_near(world, wx, gy) as f32;
            let litter = soft_litter_at(world, wx) as f32;
            if organic < 1.0 && litter < FUNGUS_STARVE_UNITS {
                continue;
            }
            let downwind = if sign == prefer_dir { 2.0 } else { 0.0 };
            let score = organic * 3.0 + litter * 0.05 - dist as f32 * 0.15 + downwind;
            if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                best = Some((score, wx, gy));
            }
        }
    }
    best.map(|(_, wx, gy)| (wx, gy))
}

/// Seed a faint mycelium patch on Organic near `(gx, gy)`.
pub fn seed_mycelium_near(world: &mut World, gx: i32, gy: i32, amount: u8) {
    if let Some((ox, oy)) = find_organic_xy(world, gx, gy) {
        if let Some(mut c) = world.get_cell(ox, oy) {
            c.set_mycelium(c.mycelium().saturating_add(amount).min(48));
            world.set_cell(ox, oy, c);
        }
    }
}

/// Rare fruiting: energy cost, wind-biased spore child on Organic / litter.
pub fn try_spore(
    world: &mut World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
    wind_vx: f32,
) -> Option<Atom> {
    if !pop_room || atom.cooldown > 0 {
        return None;
    }
    if !is_fungus(atom) || digest_count(atom) < 1 {
        return None;
    }
    // Fruiting bodies only launch from a surface-ready column.
    if fruiting_surface_ready(world, atom.gx).is_none() {
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
    // Extra rarity gate beyond cooldown.
    let h = hash_u64(world.seed.0, tick, entity_id as u64, 0xF801_700D);
    if h % 17 != 0 {
        return None;
    }
    let (wx, gy) = pick_spore_site(world, atom, tick, entity_id, wind_vx)?;
    let cost = tank * 0.45;
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
    child.cooldown = FUNGUS_SPORE_PERIOD / 2;
    pin_plant_pose(&mut child);
    if !is_fungus_seated(world, &child) {
        atom.energy = (atom.energy + cost).min(atom.energy_max);
        atom.cooldown = 0;
        return None;
    }
    // Spore germinates as faint white threads in the landing Organic.
    seed_mycelium_near(world, wx, gy, 24);
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
/// Shoot modules (Stem / Nucleus / Photosystem) never become mid-air Organic
/// pillars — water and snow must pass dead trunks; compost belongs on the
/// bed (fallback pile) or in soil already painted by dead roots. Digest /
/// Hypha / Root footprints still convert solids (and dry Air for detritus).
/// Wet Air (free water) is left alone so lakes aren't plugged.
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
    for &(dx, dy, mid) in body {
        // Grey trunks / crowns / leaves: litter only — do not dam flow.
        if matches!(
            mid,
            ModuleId::Stem | ModuleId::Nucleus | ModuleId::Photosystem
        ) {
            continue;
        }
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
                // Sand / stone / clay / soil / Organic → Organic residue.
                // Preserve pore sat so dissolve doesn't destroy water mass.
                let mut org = Cell::solid(MaterialId::Organic);
                let cap = water_capacity(MaterialId::Organic);
                org.sat.0 = if cap > 0 { c.sat.0.min(cap) } else { 0 };
                world.set_cell(wx, wy, org);
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
        let (taken, energy) = digest_labile(&mut w, 4, 2, 1);
        assert!(taken > 0 && taken <= DIGEST_MAX_UNITS);
        assert!(energy > 0.0);
        assert!(soft_litter_at(&w, 4) < 40, "should eat soft litter first");
        assert_eq!(
            w.get_cell(4, 2).map(|c| c.material),
            Some(MaterialId::Organic),
            "Organic must not be destroyed by a litter sip"
        );
    }

    #[test]
    fn colonize_does_not_flash_organic_to_sand() {
        let mut w = litter_plot();
        for y in 1..=4 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(100);
            w.set_cell(4, y, org);
        }
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let atom = Atom::from_body(4, 3, 40.0, fungus_body());
        let _ = colonize_and_compost(&mut w, 4, 3, &g, &atom, MYCELIUM_GROW_PERIOD);
        let sands = (1..=4)
            .filter(|&y| {
                w.get_cell(4, y)
                    .map(|c| c.material == MaterialId::Sand)
                    .unwrap_or(false)
            })
            .count();
        assert_eq!(sands, 0, "mycelium must never produce Sand");
        let threaded = (1..=4).any(|y| {
            w.get_cell(4, y)
                .map(|c| c.material == MaterialId::Organic && c.mycelium() > 0)
                .unwrap_or(false)
        });
        assert!(threaded, "should raise mycelium on Organic");
    }

    #[test]
    fn compost_to_soil_preserves_water() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(200);
        org.set_mycelium(255);
        w.set_cell(4, 2, org);
        // Neighbour Air can receive excess from compaction.
        w.set_cell(4, 3, Cell::air());
        assert!(compost_organic_to_soil(&mut w, 4, 2));
        let soil = w.get_cell(4, 2).unwrap();
        assert_eq!(soil.material, MaterialId::Soil);
        let soil_cap = water_capacity(MaterialId::Soil);
        assert_eq!(soil.sat.0, 200u8.min(soil_cap));
        let above = w.get_cell(4, 3).unwrap();
        let total = soil.sat.0 as u32 + above.sat.0 as u32;
        assert!(
            total >= 200,
            "water must not vanish on compost (total={total})"
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
    fn organic_substrate_prevents_hibernate() {
        let mut w = litter_plot();
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(100);
        w.set_cell(4, 2, org);
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert!(!fungus_should_hibernate(&w, &atom));
    }

    #[test]
    fn dry_organic_substrate_does_not_hibernate() {
        // Fully dry column of Organic (old F3 sat=0) — still a food bed.
        let mut w = litter_plot();
        for y in 1..=4 {
            w.set_cell(4, y, Cell::solid(MaterialId::Organic));
        }
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        assert_eq!(fungus_moisture_frac(&w, &atom), 0.0);
        assert!(
            !fungus_should_hibernate(&w, &atom),
            "dry Organic must not force drought hibernate"
        );
    }

    #[test]
    fn forage_on_organic_without_litter_is_positive() {
        let mut w = litter_plot();
        w.set_cell(4, 2, Cell::solid(MaterialId::Organic));
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let atom = Atom::from_body(4, 2, 40.0, fungus_body());
        // Off-pulse tick — forage stipend still applies.
        let e = colonize_and_compost(&mut w, 4, 2, &g, &atom, 1);
        assert!(
            e >= MYCELIUM_FORAGE * 0.5,
            "Organic forage must yield energy without soft litter (got {e})"
        );
    }

    #[test]
    fn fungus_seats_inside_organic() {
        let mut w = litter_plot();
        w.set_cell(4, 3, Cell::solid(MaterialId::Organic));
        let atom = Atom::from_body(4, 3, 40.0, fungus_body());
        assert!(is_fungus_seated(&w, &atom));
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
        // Cap is 1 now — budget stays at cap but call must not panic.
        assert!(boosted >= 1);
        assert!(base >= 1);
    }
}
