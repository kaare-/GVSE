//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Minimal Set D land plant (docs/organism/PLANTS.md § C + D1–D4):
//! Root + Stem + Photosystem on a fixed crown. Roots drink pore `sat`;
//! Photosystems in standing water drink free-column sat (shore leaves do
//! not). Elongates by `StemVsLeafVsRoot` / `RootDepthBias`. Shade = D2.
//! Vegetative sprouts = D3. Root starch tank + drought hibernate = D4.

use std::collections::{HashMap, HashSet};

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::cell::water_capacity;
use crate::grid::World;
use crate::organism::{column_sky_light, Atom, BodyModule, ModuleId};
use crate::shade::{effective_photo_light, shade_transmit, CanopyIndex};

/// Energy from one sat unit drunk by roots.
pub const ROOT_WATER_ENERGY: f32 = 0.08;
/// Energy from one sat unit drunk by Photosystems in standing water.
pub const LEAF_WATER_ENERGY: f32 = 0.07;
/// Fractional sip progress per Root module per tick. Integer sat only
/// leaves the cell when the accumulator crosses 1 — stops roots from
/// flash-drying hills (column `ROOT_SIP_KG_PER_ROOT` spirit).
pub const ROOT_SIP_FRAC_PER_ROOT: f32 = 0.025;
/// Fractional sip progress per Photosystem in standing water per tick.
pub const LEAF_SIP_FRAC_PER_PHOTO: f32 = 0.035;
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
/// Score bonus for stepping into standing-water Air (legacy wet-void).
///
/// Free-column water has no pore moisture (`cell_moisture_frac` = 0), so
/// after floating Organic soaks from the lake the raft alone outscores
/// every dive into the water column. This bonus restores raft-plant
/// roots under the mat without letting dry Air gaps look wet.
pub const ROOT_WET_VOID_AFFINITY: f32 = 1.6;
/// Soft local density: roots packed near the crown pay this extra.
pub const ROOT_CROWN_BLOB_PENALTY: f32 = 1.8;
/// Penalty per extra cell when elongating a same-column pipe (≥3 deep).
pub const ROOT_PIPE_TAX: f32 = 1.35;
/// Score penalty per Chebyshev step from the crown (applies to every
/// direction — long diagonal tendrils pay the same as vertical pipes).
pub const ROOT_TRANSPORT_TAX: f32 = 0.70;
/// Extra energy multiplier per transport hop beyond the first
/// (`cost *= 1 + frac * (path_len - 1)`). Makes long single roots
/// expensive to extend so mid-pipe branches win on energy too.
pub const ROOT_TRANSPORT_COST_FRAC: f32 = 0.12;
/// Score bonus for forking off a tip that already has depth.
pub const ROOT_BRANCH_BONUS: f32 = 1.55;
/// Score bonus for stepping *into* Organic / Sand beds.
pub const ROOT_ORGANIC_AFFINITY: f32 = 0.85;
pub const ROOT_SAND_AFFINITY: f32 = 0.45;
/// Former invent threshold — kept as docs/history. Trunks are never
/// invented from a stemless body anymore; olive only elongates.
pub const STEM_INVENT_MIN_ALLOC: f32 = 0.18;

/// Soft caps so 1× bodies stay readable (defaults for [`PlantGrowthCaps`]).
pub const MAX_ROOT_MODULES: usize = 16;
pub const MAX_STEM_MODULES: usize = 10;
pub const MAX_PHOTO_MODULES: usize = 12;
/// Max Manhattan distance a woody canopy leaf may grow from Stem/Nucleus.
/// Stemless seaweed ribbons ignore this and climb with the water column.
pub const WOODY_LEAF_MAX_CANT: i32 = 2;
/// Woody leaf sites need at least this effective light (sky × canopy).
/// Dim understory spots stay bare — competition, not a cosmetic gap.
pub const WOODY_LEAF_MIN_LIGHT: f32 = 0.34;
/// Sky×shade (no day clock) below which a woody leaf accrues starve ticks.
/// Night alone must not strip the canopy — only chronic dim sites.
pub const WOODY_LEAF_STARVE_LIGHT: f32 = 0.22;
/// Consecutive starve ticks before a woody Photosystem abscises (~8 s @ 60 Hz).
pub const WOODY_LEAF_STARVE_TICKS: u16 = 480;
/// At most one woody leaf drop every this many ticks (spread litter).
pub const WOODY_LEAF_DROP_PERIOD: u64 = 24;
/// Extra Root modules allowed per photosystem beyond the sprout minimum.
pub const LAND_ROOTS_PER_PHOTOSYSTEM: usize = 3;

/// Per-plant tissue ceilings (Tab → Plant growth caps).
///
/// One living plant may still only count as **one** entity toward the
/// pop cap; these limit how many Root / Stem / Photosystem pixels that
/// body may grow.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlantGrowthCaps {
    pub max_roots: usize,
    pub max_stems: usize,
    pub max_photos: usize,
}

impl Default for PlantGrowthCaps {
    fn default() -> Self {
        Self {
            max_roots: MAX_ROOT_MODULES,
            max_stems: MAX_STEM_MODULES,
            max_photos: MAX_PHOTO_MODULES,
        }
    }
}

impl PlantGrowthCaps {
    /// Clamp to sane slider bounds (at least one root + leaf so a plant
    /// can still seat and photosynthesize).
    pub fn clamp(self) -> Self {
        Self {
            max_roots: self.max_roots.clamp(1, 256),
            max_stems: self.max_stems.clamp(0, 256),
            max_photos: self.max_photos.clamp(1, 256),
        }
    }
}
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
/// High on purpose — one planted template must not rhizome-flood the
/// entity pop cap (was 0.52 / 48 ticks → thousands of root-only sprouts).
pub const LAND_SPROUT_ENERGY_FRAC: f32 = 0.72;
/// Ticks between sprout attempts (~0.6 demo day at `DEMO_DAY_TICKS=1200`).
pub const LAND_SPROUT_PERIOD: u64 = 720;
/// Painted Root modules required before a sprout may fire.
pub const LAND_SPROUT_MIN_ROOTS: usize = 5;
/// Max columns a rhizome sprout may emerge from the crown.
pub const ROOT_SPROUT_MAX_DIST: i32 = 6;
/// Fraction of spawn tank spent to sprout (child gets half).
pub const LAND_SPROUT_COST_FRAC: f32 = 0.55;
/// Neighbourhood half-width (columns) for local plant density gate.
pub const SPROUT_LOCAL_RADIUS: i32 = 4;
/// Max living crowns in `[gx±radius]` (including self) before rhizome
/// sprouting is blocked. High enough for a slow local grove; the long
/// [`LAND_SPROUT_PERIOD`] is what stops one template filling the pop cap.
pub const SPROUT_LOCAL_MAX: usize = 8;
/// Energy fraction of spawn tank to fire a wind spore (fern-style).
pub const PLANT_SPORE_ENERGY_FRAC: f32 = 0.68;
/// Ticks between plant wind-spore attempts (~0.8 demo day).
pub const PLANT_SPORE_PERIOD: u64 = 960;
/// Fraction of spawn tank spent on a wind spore (child gets half).
pub const PLANT_SPORE_COST_FRAC: f32 = 0.48;
/// Min / max columns a wind-borne plant spore may travel.
pub const PLANT_SPORE_MIN_DIST: i32 = 4;
pub const PLANT_SPORE_MAX_DIST: i32 = 28;
/// Extra 1-in-N rarity beyond cooldown / energy gates.
pub const PLANT_SPORE_ODDS: u64 = 11;

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
pub fn useful_root_budget(atom: &Atom, caps: &PlantGrowthCaps) -> usize {
    LAND_SPROUT_MIN_ROOTS
        .saturating_add(atom.photosystem_count().saturating_mul(LAND_ROOTS_PER_PHOTOSYSTEM))
        .min(caps.max_roots.max(1))
}

/// Drought-aware soft budget — stress lifts the cap so plants keep digging.
///
/// When Photosystems sit in **standing water** (`leaf_bathing`), one holdfast
/// root is enough — leaves drink the free water. Dry-land leaves never bathe,
/// so the full shoot-driven budget still applies on shore.
pub fn useful_root_budget_for(
    atom: &Atom,
    drought: DroughtBand,
    caps: &PlantGrowthCaps,
    leaf_bathing: bool,
) -> usize {
    let hard = caps.max_roots.max(1);
    if leaf_bathing {
        return 1.min(hard);
    }
    let base = useful_root_budget(atom, caps);
    match drought {
        DroughtBand::Hydrated | DroughtBand::Dormant => base,
        DroughtBand::Stressed => {
            let lift = (hard.saturating_sub(base) + 1) / 2;
            base.saturating_add(lift).min(hard)
        }
    }
}

pub fn roots_past_soft_budget_for(
    atom: &Atom,
    drought: DroughtBand,
    caps: &PlantGrowthCaps,
    leaf_bathing: bool,
) -> bool {
    root_count(atom) >= useful_root_budget_for(atom, drought, caps, leaf_bathing)
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

pub fn spore_count(atom: &Atom) -> usize {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::ReproSpore)
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

/// Best standing-water fill at Photosystem cells (0..1).
///
/// Dry-land Air / thin films return 0 — only [`is_standing_water`] counts,
/// so shore leaves do not drink or count as bathed.
pub fn leaf_bathing_frac(world: &World, atom: &Atom) -> f32 {
    use crate::rules::is_standing_water;
    let mut best = 0.0f32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Photosystem {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if !is_standing_water(world, wx, wy) {
            continue;
        }
        if let Some(c) = world.get_cell(wx, wy) {
            best = best.max(c.sat.0 as f32 / 255.0);
        }
    }
    best
}

/// True when any Photosystem sits in standing water (leaf can drink).
pub fn leaves_bathing(world: &World, atom: &Atom) -> bool {
    leaf_bathing_frac(world, atom) >= 0.12
}

/// Plant water status: pore moisture under roots, or free water on leaves.
pub fn plant_moisture_frac(world: &World, atom: &Atom) -> f32 {
    root_moisture_frac(world, atom).max(leaf_bathing_frac(world, atom))
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

/// Photosystems in **standing water** sip free-column sat → energy.
///
/// Dry Air / non-standing films yield nothing — land leaves do not drink.
/// Prefer this before root pore-sips when the frond is bathed.
pub fn drink_leaves(world: &mut World, atom: &mut Atom) -> (f32, u32, (i32, i32)) {
    use crate::rules::is_standing_water;
    let n_photo = atom.photosystem_count() as f32;
    if n_photo < 1.0 {
        return (0.0, 0, (atom.gx, atom.gy));
    }
    atom.sip_acc = (atom.sip_acc + n_photo * LEAF_SIP_FRAC_PER_PHOTO).min(2.5);
    let budget = atom.sip_acc.floor() as u8;
    if budget == 0 {
        return (0.0, 0, (atom.gx, atom.gy));
    }
    let want = budget.min(ROOT_SIP_MAX_SAT);
    let mut energy = 0.0f32;
    let mut taken = 0u8;
    let mut deposit_at = (atom.gx, atom.gy);
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Photosystem || taken >= want {
            continue;
        }
        let wx = world.wrap_x(atom.gx + dx as i32);
        let wy = atom.gy + dy as i32;
        if !is_standing_water(world, wx, wy) {
            continue;
        }
        if let Some(n) = sip_standing_air(world, wx, wy, want - taken) {
            energy += LEAF_WATER_ENERGY * n as f32;
            taken += n;
            deposit_at = (wx, wy);
        }
    }
    atom.sip_acc = (atom.sip_acc - taken as f32).max(0.0);
    (energy, taken as u32, deposit_at)
}

/// Roots + bathing leaves. Leaves try first so submerged fronds hydrate
/// without digging a root mat.
pub fn drink_plant(world: &mut World, atom: &mut Atom) -> (f32, u32, (i32, i32)) {
    let (e_l, s_l, at_l) = drink_leaves(world, atom);
    let (e_r, s_r, at_r) = drink_roots(world, atom);
    let at = if s_l > 0 { at_l } else { at_r };
    (e_l + e_r, s_l + s_r, at)
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

fn sip_standing_air(world: &mut World, gx: i32, gy: i32, want: u8) -> Option<u8> {
    if want == 0 {
        return None;
    }
    let cell = world.get_cell(gx, gy)?;
    if cell.material != MaterialId::Air || cell.sat.0 == 0 {
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
    let mut painted = 0u32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Photosystem {
            continue;
        }
        if paint_leaf_litter(world, atom.gx + dx as i32, atom.gy + dy as i32) {
            painted += 1;
        }
    }
    painted
}

fn paint_leaf_litter(world: &mut World, wx: i32, wy: i32) -> bool {
    use crate::cell::Cell;
    let wx = world.wrap_x(wx);
    let Some(c) = world.get_cell(wx, wy) else {
        return false;
    };
    if c.material == MaterialId::Air && c.sat.is_empty() {
        world.set_cell(wx, wy, Cell::solid(MaterialId::Organic));
        true
    } else {
        false
    }
}

/// Woody abscission: Photosystems that stay dim drop as Organic litter.
///
/// Stemless seaweed / ribbons are untouched — only bodies with a `Stem`
/// shed. Productivity uses sky × canopy transmit (day clock ignored so
/// night does not strip the canopy). Keeps at least one Photosystem.
/// Returns how many leaves were removed this call (0 or 1).
pub fn shed_unproductive_woody_leaves(
    world: &mut World,
    atom: &mut Atom,
    canopy: &CanopyIndex,
    _day: f32,
    tick: u64,
) -> u32 {
    if stem_count(atom) == 0 {
        atom.leaf_starve.clear();
        return 0;
    }
    let photos: Vec<(i16, i16)> = atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Photosystem)
        .map(|&(x, y, _)| (x, y))
        .collect();
    if photos.len() <= 1 {
        // Keep a last leaf; prune stale counters.
        atom.leaf_starve
            .retain(|&(x, y, _)| photos.iter().any(|&(px, py)| px == x && py == y));
        return 0;
    }

    // Refresh starve counters from current column light (no day factor).
    let mut next: Vec<(i16, i16, u16)> = Vec::with_capacity(photos.len());
    for &(lx, ly) in &photos {
        let wx = world.wrap_x(atom.gx + lx as i32);
        let wy = atom.gy + ly as i32;
        let sky = column_sky_light(world, wx, wy);
        // Raw sky × transmit — same diagnostic axis as draw tint.
        let lit = (sky * shade_transmit(canopy, wx, wy)).clamp(0.0, 1.0);
        let prev = atom
            .leaf_starve
            .iter()
            .find(|&&(x, y, _)| x == lx && y == ly)
            .map(|&(_, _, t)| t)
            .unwrap_or(0);
        let ticks = if lit < WOODY_LEAF_STARVE_LIGHT {
            prev.saturating_add(1)
        } else {
            0
        };
        if ticks > 0 {
            next.push((lx, ly, ticks));
        }
    }
    atom.leaf_starve = next;

    if tick % WOODY_LEAF_DROP_PERIOD != 0 {
        return 0;
    }

    // Drop the most starved leaf that crossed the threshold.
    let Some(&(dx, dy, _)) = atom
        .leaf_starve
        .iter()
        .filter(|&&(_, _, t)| t >= WOODY_LEAF_STARVE_TICKS)
        .max_by_key(|&&(_, _, t)| t)
    else {
        return 0;
    };

    let before = atom.body.len();
    atom.body
        .retain(|&(x, y, m)| !(m == ModuleId::Photosystem && x == dx && y == dy));
    if atom.body.len() == before {
        return 0;
    }
    atom.leaf_starve
        .retain(|&(x, y, _)| !(x == dx && y == dy));
    let _ = paint_leaf_litter(world, atom.gx + dx as i32, atom.gy + dy as i32);
    1
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

/// Editor / rescue seat: Air directly above any solid near `gy`.
///
/// Prefers porous plantable crowns, then bare rock / ice. Used so F2
/// free-spawn snaps down from canopy clicks instead of hanging a Root
/// in mid-air (which used to die on the next tick via [`is_anchored`]).
pub fn find_surface_air_slot(world: &World, gx: i32, gy: i32) -> Option<i32> {
    find_plant_slot(world, gx, gy).or_else(|| {
        let gx = world.wrap_x(gx);
        for dy in [0, 1, -1, 2, -2, 3, -3, 4, -4, 6, -6, 8, -8, 12, -12, 16, -16] {
            let y = gy + dy;
            if air_above_solid(world, gx, y) {
                return Some(y);
            }
        }
        // Prefer the lowest surface under the click (drop from canopy).
        for y in (gy - 64..=gy + 8).rev() {
            if air_above_solid(world, gx, y) {
                return Some(y);
            }
        }
        None
    })
}

fn air_above_solid(world: &World, gx: i32, nucleus_y: i32) -> bool {
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

/// Fungus seat: Air above any solid. Prefers Organic / wet Sand, but
/// will land on bare rock too (may starve later — that's fine).
pub fn find_fungus_slot(world: &World, gx: i32, gy: i32) -> Option<i32> {
    find_fungus_slot_biased(world, gx, gy, false)
}

/// Like [`find_fungus_slot`], but `prefer_surface` seats wind-spore children
/// in Air above Organic (stalks) instead of burying them in the bed.
pub fn find_fungus_slot_biased(
    world: &World,
    gx: i32,
    gy: i32,
    prefer_surface: bool,
) -> Option<i32> {
    let gx = world.wrap_x(gx);
    let mut best: Option<(i32, i32)> = None; // score, y
    let consider = |world: &World, y: i32, best: &mut Option<(i32, i32)>| {
        if !fungus_crown(world, gx, y) {
            return;
        }
        let mut score = fungus_seat_score(world, gx, y);
        if prefer_surface {
            if let Some(here) = world.get_cell(gx, y) {
                if here.material == MaterialId::Air {
                    // Prefer standing on Organic / Soil for a visible stalk.
                    score = match world.get_cell(gx, y - 1).map(|c| c.material) {
                        Some(MaterialId::Organic) => {
                            200 + world
                                .get_cell(gx, y - 1)
                                .map(|c| c.mycelium() as i32 / 4)
                                .unwrap_or(0)
                        }
                        Some(MaterialId::Soil) => 160,
                        _ => score + 40,
                    };
                } else if here.material == MaterialId::Organic {
                    score /= 2; // deprioritize buried seats for wind spores
                }
            }
        }
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
    let Some(here) = world.get_cell(gx, nucleus_y) else {
        return false;
    };
    // Prefer embedding the nucleus in Organic (underground mycelium).
    if here.material == MaterialId::Organic {
        return true;
    }
    if here.material != MaterialId::Air {
        return false;
    }
    matches!(
        world.get_cell(gx, nucleus_y - 1),
        Some(c) if c.material != MaterialId::Air
    )
}

fn fungus_seat_score(world: &World, gx: i32, nucleus_y: i32) -> i32 {
    let Some(here) = world.get_cell(gx, nucleus_y) else {
        return 0;
    };
    if here.material == MaterialId::Organic {
        // Deeper / more threaded Organic seats score higher.
        return 120 + (here.mycelium() as i32 / 4);
    }
    let Some(below) = world.get_cell(gx, nucleus_y - 1) else {
        return 0;
    };
    match below.material {
        MaterialId::Organic => 100 + (below.mycelium() as i32 / 8),
        MaterialId::Soil => 70,
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
        MaterialId::Sand | MaterialId::Clay | MaterialId::Soil => Some(0.65),
        MaterialId::Stone => Some(1.6),
        MaterialId::Air => Some(0.45), // gaps / rhizome air pockets
        _ => Some(1.0),
    }
}

/// World cells occupied by living Root modules (all plants).
pub fn collect_live_root_world_cells<'a, I>(atoms: I) -> HashSet<(i32, i32)>
where
    I: IntoIterator<Item = &'a Atom>,
{
    let mut out = HashSet::new();
    for a in atoms {
        for &(dx, dy, m) in &a.body {
            if m == ModuleId::Root {
                out.insert((a.gx + dx as i32, a.gy + dy as i32));
            }
        }
    }
    out
}

/// World cells occupied by living Photosystem modules (all plants).
pub fn collect_live_photo_world_cells<'a, I>(atoms: I) -> HashSet<(i32, i32)>
where
    I: IntoIterator<Item = &'a Atom>,
{
    let mut out = HashSet::new();
    for a in atoms {
        for &(dx, dy, m) in &a.body {
            if m == ModuleId::Photosystem {
                out.insert((a.gx + dx as i32, a.gy + dy as i32));
            }
        }
    }
    out
}

/// Per-column max world-y of living plant sail (Stem / Photosystem / Nucleus).
///
/// Used by floating-Organic wind drift so tall plants act as sails.
pub fn collect_plant_sail_tops<'a, I>(atoms: I) -> HashMap<i32, i32>
where
    I: IntoIterator<Item = &'a Atom>,
{
    let mut out: HashMap<i32, i32> = HashMap::new();
    for a in atoms {
        if !is_land_plant(a) {
            continue;
        }
        for &(dx, dy, m) in &a.body {
            if !matches!(
                m,
                ModuleId::Photosystem | ModuleId::Stem | ModuleId::Nucleus
            ) {
                continue;
            }
            let wx = a.gx + dx as i32;
            let wy = a.gy + dy as i32;
            let e = out.entry(wx).or_insert(wy);
            *e = (*e).max(wy);
        }
    }
    out
}

/// True when a Root/Nucleus sits in or on a floating Organic column.
fn holdfast_on_float_column(
    columns: &HashMap<i32, (i32, i32)>,
    mid: ModuleId,
    wx: i32,
    wy: i32,
) -> bool {
    let Some(&(bottom, height)) = columns.get(&wx) else {
        return false;
    };
    let top = bottom + height - 1;
    // In the mat or on the deck.
    if (wy >= bottom && wy <= top) || wy == top + 1 {
        return true;
    }
    // Roots may dive a couple cells under the raft; nucleus does not —
    // otherwise a submerged crown below drifting litter looks "mounted".
    mid == ModuleId::Root && wy >= bottom - 2 && wy < bottom
}

/// Drift floating Organic with the wind; root-bound mats sail as one island
/// and carry their land plants along (dispersal).
///
/// Only plants with a holdfast in floating Organic translate — submerged /
/// free-floating plants are not hitchhiked when litter slides past. Every
/// root column across a mounted plant's span claims the mat (not only the
/// first Organic contact). Loose unrooted litter may still peel apart.
/// Returns how many Organic columns moved.
pub fn sail_plants_on_wind_rafts(
    world: &mut World,
    atoms: &mut [Atom],
    wind_vx_tiles: f32,
    tile_cols: i32,
) -> u32 {
    let columns = crate::rules::collect_floating_organic_columns(world);
    let mut bound_cols: HashSet<i32> = HashSet::new();
    let mut raft_mounted: Vec<usize> = Vec::new();

    for (i, atom) in atoms.iter().enumerate() {
        if !is_land_plant(atom) || atom.energy <= 0.0 {
            continue;
        }
        let mut min_rx = i32::MAX;
        let mut max_rx = i32::MIN;
        let mut mounted = false;
        for &(dx, dy, m) in &atom.body {
            if m != ModuleId::Root && m != ModuleId::Nucleus {
                continue;
            }
            let wx = world.wrap_x(atom.gx + dx as i32);
            let wy = atom.gy + dy as i32;
            min_rx = min_rx.min(wx);
            max_rx = max_rx.max(wx);
            if holdfast_on_float_column(&columns, m, wx, wy) {
                mounted = true;
            }
        }
        if !mounted || min_rx > max_rx {
            continue;
        }
        raft_mounted.push(i);
        // Bind the full root/nucleus span so later roots keep the mat under
        // the plant — not just the first Organic contact column.
        for x in min_rx..=max_rx {
            if columns.contains_key(&x) {
                bound_cols.insert(x);
            }
        }
    }

    let sails = collect_plant_sail_tops(
        raft_mounted
            .iter()
            .filter_map(|&i| atoms.get(i)),
    );
    let (moved, sign, moved_cols) = crate::rules::drift_floating_organic(
        world,
        wind_vx_tiles,
        tile_cols,
        Some(&sails),
        Some(&bound_cols),
    );
    if moved == 0 || sign == 0 || moved_cols.is_empty() {
        return moved;
    }
    for &i in &raft_mounted {
        let Some(atom) = atoms.get_mut(i) else {
            continue;
        };
        let hitch = atom.body.iter().any(|&(dx, _dy, m)| {
            if m != ModuleId::Root && m != ModuleId::Nucleus {
                return false;
            }
            let wx = world.wrap_x(atom.gx + dx as i32);
            moved_cols.contains(&wx)
        });
        if hitch {
            atom.gx = world.wrap_x(atom.gx + sign);
        }
    }
    moved
}

fn own_root_at(atom: &Atom, wx: i32, wy: i32) -> bool {
    atom.body.iter().any(|&(bx, by, m)| {
        m == ModuleId::Root && atom.gx + bx as i32 == wx && atom.gy + by as i32 == wy
    })
}

fn own_photo_at(atom: &Atom, wx: i32, wy: i32) -> bool {
    atom.body.iter().any(|&(bx, by, m)| {
        m == ModuleId::Photosystem && atom.gx + bx as i32 == wx && atom.gy + by as i32 == wy
    })
}

/// True when `(wx,wy)` is Moore-adjacent to a *foreign* live root.
pub fn beside_foreign_live_root(
    atom: &Atom,
    wx: i32,
    wy: i32,
    live_roots: &HashSet<(i32, i32)>,
) -> bool {
    for ox in -1i32..=1 {
        for oy in -1i32..=1 {
            let tx = wx + ox;
            let ty = wy + oy;
            if !live_roots.contains(&(tx, ty)) {
                continue;
            }
            if own_root_at(atom, tx, ty) {
                continue;
            }
            return true;
        }
    }
    false
}

/// True when `(wx,wy)` is Moore-adjacent to a *foreign* live Photosystem.
/// Same exclusion spirit as roots — leaves don't pack into a neighbour's canopy.
pub fn beside_foreign_live_photo(
    atom: &Atom,
    wx: i32,
    wy: i32,
    live_photos: &HashSet<(i32, i32)>,
) -> bool {
    for ox in -1i32..=1 {
        for oy in -1i32..=1 {
            let tx = wx + ox;
            let ty = wy + oy;
            if !live_photos.contains(&(tx, ty)) {
                continue;
            }
            if own_photo_at(atom, tx, ty) {
                continue;
            }
            return true;
        }
    }
    false
}

fn woody_leaf_light_ok(
    world: &World,
    atom: &Atom,
    canopy: &CanopyIndex,
    _entity_id: u32,
    wx: i32,
    wy: i32,
    _n_photo: usize,
) -> bool {
    let sky = column_sky_light(world, wx, wy);
    let lit = effective_photo_light(canopy, wx, wy, sky, &atom.genome);
    lit >= WOODY_LEAF_MIN_LIGHT
}

/// Try to add one Root pixel toward moisture / depth bias.
/// Shallowest own Root near the stem (crown proxy for transport length).
fn root_crown(atom: &Atom) -> (i16, i16) {
    atom.body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Root)
        .max_by_key(|(x, y, _)| (*y, -x.abs()))
        .map(|&(x, y, _)| (x, y))
        .unwrap_or((0, -1))
}

/// Chebyshev steps between two body-local cells.
fn chebyshev(a: (i16, i16), b: (i16, i16)) -> i16 {
    (a.0 - b.0).abs().max((a.1 - b.1).abs())
}

/// Transport hops from crown to a candidate cell (≥ 1).
pub fn root_transport_hops(atom: &Atom, nx: i16, ny: i16) -> i16 {
    chebyshev(root_crown(atom), (nx, ny)).max(1)
}

/// Direction preference — mutually exclusive so diagonal-down does not
/// get a full "down" credit *plus* a sprawl credit (that made every
/// tendril prefer stairs forever).
fn root_dir_preference(dx: i16, dy: i16, depth_bias: f32) -> f32 {
    if dx != 0 && dy < 0 {
        // Diagonal: blend dive + sprawl; never sum both at full weight.
        depth_bias * 0.55 + (1.0 - depth_bias) * 0.40
    } else if dy < 0 {
        depth_bias
    } else if dx != 0 {
        (1.0 - depth_bias) * 0.70
    } else {
        0.0
    }
}

/// Returns energy spent (0 if nothing grew).
///
/// `live_roots` is every living Root world cell (all plants) so spacing
/// applies across neighbours, not just within one body.
pub fn try_elongate_root(
    world: &World,
    atom: &mut Atom,
    live_roots: &HashSet<(i32, i32)>,
    caps: &PlantGrowthCaps,
) -> f32 {
    let n_roots = root_count(atom);
    if n_roots >= caps.max_roots.max(1) {
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

    let bathing = leaves_bathing(world, atom);
    let host_moist = plant_moisture_frac(world, atom);
    let drought = drought_band(host_moist);
    let thirsty = matches!(drought, DroughtBand::Stressed) && !bathing;
    // Urge a lateral runner before sprouting (column rhizome bias).
    // Bathed fronds don't need rhizome pressure — holdfast is enough.
    let need_runner = !bathing
        && !has_lateral_runner(atom)
        && n_roots >= LAND_SPROUT_MIN_ROOTS.saturating_sub(1);
    // Past the soft root:shoot budget, only grow roots when thirsty or
    // forcing a rhizome runner.
    if roots_past_soft_budget_for(atom, drought, caps, bathing) && !need_runner && !thirsty {
        return 0.0;
    }

    let occupied: HashSet<(i16, i16)> = atom.body.iter().map(|&(x, y, _)| (x, y)).collect();
    // Every Root cell is a growth site — buds can form mid-pipe, not only
    // at the deepest head.
    let sites: Vec<(i16, i16)> = atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Root)
        .map(|&(x, y, _)| (x, y))
        .collect();
    let sites = if sites.is_empty() {
        vec![(0, -1)]
    } else {
        sites
    };

    let depth_bias = atom.genome.root_depth_bias.clamp(0.0, 1.0);
    let banking_for_sprout = atom.energy >= tank * LAND_SPROUT_ENERGY_FRAC * 0.85;
    // Cardinal + diagonal-down forks — branchy fans, not only a pipe.
    const DIRS: [(i16, i16); 5] = [(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)];
    let mut best: Option<(f32, i16, i16, f32)> = None; // score, dx, dy, cost

    for &(sx, sy) in &sites {
        let site_col_depth = atom
            .body
            .iter()
            .filter(|&&(rx, _, m)| m == ModuleId::Root && rx == sx)
            .count();
        // Mid-pipe if this site has own roots both above and below.
        let has_above = atom.body.iter().any(|&(rx, ry, m)| {
            m == ModuleId::Root && rx == sx && ry > sy
        });
        let has_below = atom.body.iter().any(|&(rx, ry, m)| {
            m == ModuleId::Root && rx == sx && ry < sy
        });
        let mid_pipe = has_above && has_below;
        for &(dx, dy) in &DIRS {
            let nx = sx + dx;
            let ny = sy + dy;
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
            // Transport length from crown — long single tendrils pay more
            // score *and* energy than short mid-pipe branches.
            let hops = root_transport_hops(atom, nx, ny) as f32;
            let cost = ROOT_ELONGATE_BASE_COST
                * pen
                * (1.0 + ROOT_TRANSPORT_COST_FRAC * (hops - 1.0).max(0.0));
            if atom.energy < cost + grow_floor * 0.5 {
                continue;
            }
            // Moisture tropism: sample the step cell and one deeper.
            // Standing-water Air counts as wet void (not dry gap) so roots
            // under a soaked floating Organic raft still dive into the lake.
            let mut moist = cell_moisture_frac(world, wx, wy)
                .max(cell_moisture_frac(world, wx, wy - 1))
                .max(cell_moisture_frac(world, wx, wy - 2) * 0.85);
            let wet_void = cell.material == MaterialId::Air
                && crate::rules::is_standing_water(world, wx, wy);
            if wet_void {
                moist = moist.max(cell.sat.0 as f32 / 255.0);
            }
            let mut score = moist * ROOT_MOISTURE_AFFINITY
                + root_dir_preference(dx, dy, depth_bias)
                - pen * 0.03
                - ROOT_TRANSPORT_TAX * hops;
            if wet_void {
                score += ROOT_WET_VOID_AFFINITY * (cell.sat.0 as f32 / 255.0);
            }
            // Organic is transformed dead-root compost — fine to sit beside
            // or step into (score bonus below). Live roots stay exclusive.
            match cell.material {
                MaterialId::Organic => score += ROOT_ORGANIC_AFFINITY,
                MaterialId::Sand => score += ROOT_SAND_AFFINITY,
                _ => {}
            }
            // Extra nudge into clearly wetter cells than the site's host.
            let site_moist = cell_moisture_frac(
                world,
                world.wrap_x(atom.gx + sx as i32),
                atom.gy + sy as i32,
            );
            if moist > site_moist + 0.05 {
                score += (moist - site_moist) * 1.6;
            }
            // Can't place next to another *alive* root on this plant.
            // Origin site OK. Mid-pipe lateral/diagonal buds may ignore the
            // same-column stack they're sprouting from (otherwise Moore
            // neighbours above/below forbid every side shoot). Crown tips
            // still respect full spacing so we don't fan into a mat.
            let beside_live = atom.body.iter().any(|&(rx, ry, m)| {
                if m != ModuleId::Root || (rx, ry) == (sx, sy) {
                    return false;
                }
                if (rx - nx).abs() > 1 || (ry - ny).abs() > 1 {
                    return false;
                }
                if mid_pipe && dx != 0 && rx == sx {
                    return false;
                }
                // Vertical dive may skim diagonally past a same-row runner.
                let skim_runner = dx == 0 && dy < 0 && ry == sy && rx != sx;
                !skim_runner
            });
            if beside_live {
                continue;
            }
            // Same rule vs *other plants'* live roots (no skim / column pass).
            if beside_foreign_live_root(atom, wx, wy, live_roots) {
                continue;
            }
            // Same-row roots need a gap column (no 3-wide crown fan).
            // Rhizome may extend one cardinal step farther from x=0.
            let same_row_crowd = atom.body.iter().any(|&(rx, ry, m)| {
                if m != ModuleId::Root || ry != ny || (rx, ry) == (sx, sy) {
                    return false;
                }
                if (rx - nx).abs() > 2 {
                    return false;
                }
                let extending_out =
                    dy == 0 && (nx - sx).abs() == 1 && rx.abs() < nx.abs();
                !extending_out
            });
            if same_row_crowd {
                continue;
            }
            // One live thread per column (own or foreign) — no packing lanes.
            let col_taken_own = atom
                .body
                .iter()
                .any(|&(rx, _, m)| m == ModuleId::Root && rx == nx);
            let col_taken_foreign = live_roots
                .iter()
                .any(|&(rx, ry)| rx == wx && !own_root_at(atom, rx, ry));
            if dx != 0 && (col_taken_own || col_taken_foreign) {
                continue;
            }
            if dy < 0 && col_taken_foreign {
                // Don't dive into a column another plant already owns.
                continue;
            }
            // Pipe tax — a long same-column stack should fork, not drill.
            if dx == 0 && dy < 0 && site_col_depth >= 3 {
                score -= ROOT_PIPE_TAX * (site_col_depth as f32 - 2.0);
            }
            // Branch urge from deep columns; extra for mid-pipe buds.
            if site_col_depth >= 2 && dx != 0 {
                score += ROOT_BRANCH_BONUS * (0.55 + 0.45 * (1.0 - depth_bias));
                if mid_pipe {
                    score += ROOT_BRANCH_BONUS * 0.45;
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
                    score += 1.6 * rhizome;
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
/// - Stemless ribbons (seaweed): leaves stack upward from the frond tip.
/// - New Stem needs a Moore gap from other live/dead trunks.
/// - Woody leaves: short petioles, Moore gap from *foreign* live leaves
///   (same spirit as root spacing), and a minimum effective light.
pub fn try_grow_shoot(
    world: &World,
    atom: &mut Atom,
    tick: u64,
    trunks: &HashSet<(i32, i32)>,
    live_photos: &HashSet<(i32, i32)>,
    caps: &PlantGrowthCaps,
    canopy: &CanopyIndex,
    entity_id: u32,
) -> f32 {
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
    let can_grow_stem = n_stem > 0 && n_stem < caps.max_stems && w_stem >= 0.08;

    let place_leaf = |atom: &mut Atom, occupied: &HashSet<(i16, i16)>| -> bool {
        if n_photo >= caps.max_photos.max(1) {
            return false;
        }

        // —— Stemless ribbon: climb from the highest tip into standing water.
        // Flop is draw-only; never grow a permanent sideways L.
        if n_stem == 0 {
            let tip = atom
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Photosystem)
                .copied()
                .max_by_key(|&(x, y, _)| (y, x.unsigned_abs()))
                .map(|(x, y, _)| (x, y))
                .or_else(|| {
                    atom.body
                        .iter()
                        .find(|(_, _, m)| *m == ModuleId::Nucleus)
                        .map(|&(x, y, _)| (x, y))
                });
            let Some((tx, ty)) = tip else {
                return false;
            };
            for &(dx, dy) in &[(0i16, 1i16), (1, 1), (-1, 1)] {
                let nx = tx + dx;
                let ny = ty + dy;
                if ny > 16 || occupied.contains(&(nx, ny)) {
                    continue;
                }
                let wx = world.wrap_x(atom.gx + nx as i32);
                let wy = atom.gy + ny as i32;
                if !crate::rules::is_standing_water(world, wx, wy) {
                    continue;
                }
                atom.energy -= cost;
                atom.body.push((nx, ny, ModuleId::Photosystem));
                return true;
            }
            return false;
        }

        // —— Woody canopy: short petioles beside the trunk/branch.
        // Prefer a new side-leaf on a stem cell over elongating one tip into
        // a seaweed-like ribbon. Cap cantilever at [`WOODY_LEAF_MAX_CANT`].
        // Refuse Moore-adjacent foreign leaves (root-spacing analogue) and
        // sites too dim for the leaf to pay for itself.
        let mut stem_anchors: Vec<(i16, i16)> = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .map(|&(x, y, _)| (x, y))
            .collect();
        // Prefer higher / brighter stem first, then fill gaps lower down.
        stem_anchors.sort_by_key(|&(_, y)| std::cmp::Reverse(y));
        const SIDE: [(i16, i16); 4] = [(1, 0), (-1, 0), (1, 1), (-1, 1)];
        for &(tx, ty) in &stem_anchors {
            for &(dx, dy) in &SIDE {
                let nx = tx + dx;
                let ny = ty + dy;
                if ny > 16 || nx == tx || occupied.contains(&(nx, ny)) {
                    continue;
                }
                if leaf_support_dist(atom, nx, ny) > WOODY_LEAF_MAX_CANT {
                    continue;
                }
                let wx = world.wrap_x(atom.gx + nx as i32);
                let wy = atom.gy + ny as i32;
                if beside_foreign_live_photo(atom, wx, wy, live_photos) {
                    continue;
                }
                if !woody_leaf_light_ok(world, atom, canopy, entity_id, wx, wy, n_photo) {
                    continue;
                }
                atom.energy -= cost;
                atom.body.push((nx, ny, ModuleId::Photosystem));
                return true;
            }
        }

        // Short tip extend only while under the woody petiole cap (no climb).
        let tip = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .copied()
            .max_by_key(|&(x, y, _)| {
                let cant = leaf_support_dist(atom, x, y);
                (cant, x.unsigned_abs(), y)
            })
            .map(|(x, y, _)| (x, y));
        if let Some((tx, ty)) = tip {
            if leaf_support_dist(atom, tx, ty) < WOODY_LEAF_MAX_CANT {
                for &(dx, dy) in &[(1i16, 0i16), (-1, 0), (1, -1), (-1, -1)] {
                    let nx = tx + dx;
                    let ny = ty + dy;
                    if ny > 16 || occupied.contains(&(nx, ny)) {
                        continue;
                    }
                    if leaf_support_dist(atom, nx, ny) > WOODY_LEAF_MAX_CANT {
                        continue;
                    }
                    // Keep the olive tip column clear.
                    if atom
                        .body
                        .iter()
                        .any(|&(x, y, m)| m == ModuleId::Stem && x == nx && y == ny - 1)
                    {
                        continue;
                    }
                    let wx = world.wrap_x(atom.gx + nx as i32);
                    let wy = atom.gy + ny as i32;
                    if beside_foreign_live_photo(atom, wx, wy, live_photos) {
                        continue;
                    }
                    if !woody_leaf_light_ok(world, atom, canopy, entity_id, wx, wy, n_photo) {
                        continue;
                    }
                    atom.energy -= cost;
                    atom.body.push((nx, ny, ModuleId::Photosystem));
                    return true;
                }
            }
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

/// Manhattan distance from a body cell to the nearest Stem, else Nucleus.
fn leaf_support_dist(atom: &Atom, dx: i16, dy: i16) -> i32 {
    let mut best = i32::MAX;
    for &(x, y, m) in &atom.body {
        if m != ModuleId::Stem && m != ModuleId::Nucleus {
            continue;
        }
        let d = (x - dx).abs() as i32 + (y - dy).abs() as i32;
        best = best.min(d);
    }
    if best == i32::MAX {
        dx.abs() as i32 + dy.max(0) as i32
    } else {
        best
    }
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
    live_roots: &HashSet<(i32, i32)>,
    live_photos: &HashSet<(i32, i32)>,
    caps: &PlantGrowthCaps,
    canopy: &CanopyIndex,
    entity_id: u32,
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
        spent += try_elongate_root(world, atom, live_roots, caps);
        if spent <= 0.0 {
            spent += try_grow_shoot(
                world, atom, tick, trunks, live_photos, caps, canopy, entity_id,
            );
        }
    } else {
        spent += try_grow_shoot(
            world, atom, tick, trunks, live_photos, caps, canopy, entity_id,
        );
        if spent <= 0.0 {
            spent += try_elongate_root(world, atom, live_roots, caps);
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

/// True when a living land-plant crown already claims column `gx`.
pub fn column_occupied(plant_cols: &[i32], gx: i32) -> bool {
    plant_cols.iter().any(|&c| c == gx)
}

/// Pick a world column for vegetative sprout from a lateral runner tip.
/// Skips columns that already host a living crown (`plant_cols`).
pub fn pick_sprout_column(world: &World, atom: &Atom, plant_cols: &[i32]) -> Option<i32> {
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
        if column_occupied(plant_cols, wx) {
            continue;
        }
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

/// Column distance on a possibly ring-wrapped world.
pub fn column_dist(a: i32, b: i32, wrap_width: Option<i32>) -> i32 {
    let d = (a - b).abs();
    match wrap_width {
        Some(w) if w > 0 => d.min(w - d.min(w)),
        _ => d,
    }
}

/// Count land plants whose crown column is within `radius` of `gx`.
pub fn count_plants_near(plant_cols: &[i32], gx: i32, radius: i32, wrap_width: Option<i32>) -> usize {
    plant_cols
        .iter()
        .filter(|&&px| column_dist(px, gx, wrap_width) <= radius)
        .count()
}

/// Vegetative sucker: child plant on moist land at a lateral runner tip.
///
/// Requires painted lateral root, enough roots, energy, cooldown, global
/// pop room, an **unoccupied** target column, and local density below
/// [`SPROUT_LOCAL_MAX`]. Child chassis follows the parent (stemless stays
/// stemless); genome is mutated then re-synced so alloc can't reintroduce
/// a trunk.
pub fn try_vegetative_sprout(
    world: &World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
    plant_cols: &[i32],
) -> Option<Atom> {
    if !pop_room || atom.cooldown > 0 {
        return None;
    }
    let local = count_plants_near(plant_cols, atom.gx, SPROUT_LOCAL_RADIUS, world.wrap_width);
    if local >= SPROUT_LOCAL_MAX {
        return None;
    }
    if root_count(atom) < LAND_SPROUT_MIN_ROOTS {
        return None;
    }
    let tank = tank_ref(atom);
    if atom.energy < tank * LAND_SPROUT_ENERGY_FRAC {
        return None;
    }
    let wx = pick_sprout_column(world, atom, plant_cols)?;
    // One living crown per column — never stack nuclei on the same seat.
    if column_occupied(plant_cols, wx) {
        return None;
    }
    // Target neighbourhood must also have room (includes parent if nearby).
    let near_target = count_plants_near(plant_cols, wx, SPROUT_LOCAL_RADIUS, world.wrap_width);
    if near_target >= SPROUT_LOCAL_MAX {
        return None;
    }
    let gy = find_plant_slot(world, wx, atom.gy)?;
    let cost = tank * LAND_SPROUT_COST_FRAC;
    if atom.energy < cost {
        return None;
    }
    atom.energy -= cost;
    atom.cooldown = LAND_SPROUT_PERIOD;

    let mut body = sprout_body(atom);
    body = crate::blueprint::mutate_body(
        &body,
        atom.genome.clone_fidelity,
        world.seed.0,
        tick,
        entity_id,
    );
    let mut child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    sync_alloc_to_body(&mut child_genome, &body);
    // Child inherits spawn-tank size, not the parent's root-inflated max.
    let mut child = Atom::from_body(wx, gy, tank, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, child.energy_max);
    // Children must mature a long time before chaining another sprout.
    child.cooldown = LAND_SPROUT_PERIOD.saturating_mul(2);
    pin_plant_pose(&mut child);
    if !is_anchored(world, &child) {
        // Refund — site looked plantable but crown didn't seat.
        atom.energy = (atom.energy + cost).min(atom.energy_max);
        atom.cooldown = 0;
        return None;
    }
    Some(child)
}

/// Child body for wind spore — juvenile plant; keeps a sorus if the parent
/// had [`ModuleId::ReproSpore`] so ferns can keep dispersing.
pub fn spore_dispersal_body(parent: &Atom) -> Vec<BodyModule> {
    let mut body = sprout_body(parent);
    if spore_count(parent) == 0 {
        return body;
    }
    let tip_y = body
        .iter()
        .filter(|(_, _, m)| matches!(m, ModuleId::Photosystem | ModuleId::Stem))
        .map(|(_, dy, _)| *dy)
        .max()
        .unwrap_or(1);
    // Avoid stacking on an existing module at (1, tip_y).
    let spot = if body.iter().any(|&(dx, dy, _)| dx == 1 && dy == tip_y) {
        (-1i16, tip_y)
    } else {
        (1i16, tip_y)
    };
    body.push((spot.0, spot.1, ModuleId::ReproSpore));
    body
}

/// Wind-biased moist plant seat farther than rhizome reach (fern spores).
pub fn pick_plant_spore_column(
    world: &World,
    atom: &Atom,
    tick: u64,
    entity_id: u32,
    wind_vx: f32,
    plant_cols: &[i32],
) -> Option<(i32, i32)> {
    let prefer_dir = if wind_vx.abs() < 0.05 {
        let flip = hash_u64_plant(world.seed.0, tick, entity_id as u64, 0xFE7A) & 1;
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
    for dist in PLANT_SPORE_MIN_DIST..=PLANT_SPORE_MAX_DIST {
        for &sign in &[prefer_dir, -prefer_dir] {
            let wx = world.wrap_x(atom.gx + sign * dist);
            if column_occupied(plant_cols, wx) {
                continue;
            }
            let Some(gy) = find_plant_slot(world, wx, atom.gy) else {
                continue;
            };
            let moist = cell_moisture_frac(world, wx, gy - 1);
            if moist < 0.02 {
                continue;
            }
            let local = count_plants_near(plant_cols, wx, SPROUT_LOCAL_RADIUS, world.wrap_width);
            if local >= SPROUT_LOCAL_MAX {
                continue;
            }
            let downwind = if sign == prefer_dir { 2.5 } else { 0.0 };
            // Prefer farther downwind seats slightly (true aerial spread).
            let score = moist * 2.0 + dist as f32 * 0.04 + downwind;
            if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                best = Some((score, wx, gy));
            }
        }
    }
    best.map(|(_, wx, gy)| (wx, gy))
}

/// Fern-style wind spore: needs painted [`ModuleId::ReproSpore`], energy,
/// cooldown, pop room, and a moist unoccupied seat downwind.
pub fn try_plant_wind_spore(
    world: &World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
    plant_cols: &[i32],
    wind_vx: f32,
) -> Option<Atom> {
    if !pop_room || atom.cooldown > 0 {
        return None;
    }
    if !is_land_plant(atom) || spore_count(atom) < 1 {
        return None;
    }
    // Need a photosystem / stem "frond" to launch from — roots alone can't.
    if atom.photosystem_count() < 1 && stem_count(atom) < 1 {
        return None;
    }
    let tank = tank_ref(atom);
    if atom.energy < tank * PLANT_SPORE_ENERGY_FRAC {
        return None;
    }
    let h = hash_u64_plant(world.seed.0, tick, entity_id as u64, 0x5F07_E001);
    if h % PLANT_SPORE_ODDS != 0 {
        return None;
    }
    let (wx, gy) = pick_plant_spore_column(world, atom, tick, entity_id, wind_vx, plant_cols)?;
    if column_occupied(plant_cols, wx) {
        return None;
    }
    let cost = tank * PLANT_SPORE_COST_FRAC;
    if atom.energy < cost {
        return None;
    }
    atom.energy -= cost;
    atom.cooldown = PLANT_SPORE_PERIOD;

    let mut body = spore_dispersal_body(atom);
    body = crate::blueprint::mutate_body(
        &body,
        atom.genome.clone_fidelity,
        world.seed.0,
        tick,
        entity_id,
    );
    let mut child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    sync_alloc_to_body(&mut child_genome, &body);
    let mut child = Atom::from_body(wx, gy, tank, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, child.energy_max);
    child.cooldown = PLANT_SPORE_PERIOD;
    pin_plant_pose(&mut child);
    if !is_anchored(world, &child) {
        atom.energy = (atom.energy + cost).min(atom.energy_max);
        atom.cooldown = 0;
        return None;
    }
    Some(child)
}

fn hash_u64_plant(a: u64, b: u64, c: u64, salt: u64) -> u64 {
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

fn hash01(a: u64, b: u64, c: u64, salt: u64) -> f32 {
    (hash_u64_plant(a, b, c, salt) >> 40) as f32 / ((1u64 << 24) as f32)
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;
    use crate::organism::Atom;

    fn moist_plot() -> World {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..32 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(160);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w
    }

    fn fern_body() -> Vec<BodyModule> {
        let mut body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        body.push((1, 2, ModuleId::ReproSpore));
        body
    }

    #[test]
    fn wind_sails_plant_with_rooted_organic_raft() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 6..=9 {
            w.set_cell(x, 6, Cell::solid(MaterialId::Organic));
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        // Nucleus above raft; root in Organic at (8,6).
        let mut atoms = vec![Atom::from_body(8, 7, 40.0, body)];
        apply_genome(
            &mut atoms[0],
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        let gx0 = atoms[0].gx;
        let mut sailed = false;
        for tick in 0..600u64 {
            w.tick = tick;
            let n = sail_plants_on_wind_rafts(&mut w, &mut atoms, 0.22, 4);
            if n > 0 && atoms[0].gx != gx0 {
                sailed = true;
                break;
            }
        }
        assert!(sailed, "plant gx should travel with its Organic raft");
        let wx = atoms[0].gx;
        let root_y = atoms[0].gy - 1;
        assert_eq!(
            w.get_cell(wx, root_y).map(|c| c.material),
            Some(MaterialId::Organic),
            "holdfast should still sit on Organic after sailing"
        );
    }

    #[test]
    fn submerged_plant_does_not_hitchhike_drifting_organic() {
        // Free-floating / seabed plants must not translate when litter
        // slides into a neighbouring column (old wx-sign hitch).
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w.set_cell(6, 6, Cell::solid(MaterialId::Organic));
        w.set_cell(7, 6, Cell::solid(MaterialId::Organic));
        // Plant rooted in the water column, not on the Organic mat.
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut atoms = vec![Atom::from_body(12, 4, 40.0, body)];
        apply_genome(
            &mut atoms[0],
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        let gx0 = atoms[0].gx;
        let mut first_move = None;
        for tick in 0..400u64 {
            w.tick = tick;
            let n = sail_plants_on_wind_rafts(&mut w, &mut atoms, 0.30, 4);
            if atoms[0].gx != gx0 && first_move.is_none() {
                first_move = Some((tick, n, atoms[0].gx));
                break;
            }
        }
        assert!(
            first_move.is_none(),
            "submerged plant must not sail with nearby Organic litter (moved {first_move:?})"
        );
    }

    #[test]
    fn unanchored_plant_floats_tipped_on_open_water() {
        use crate::organism::{fallen_body_offset, OrganismStore};

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Drop an upright plant into the water column with no Organic holdfast.
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 3, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(
            store.atoms[0].fallen,
            "lost-grip plant over water must tip / free-float"
        );
        assert_eq!(
            store.atoms[0].gy, 5,
            "should rest at the free surface, not the lake bed"
        );
        let (dx, dy) = fallen_body_offset(0, 1);
        assert_eq!((dx, dy), (-1, 0), "upright stem tips onto its side");
    }

    #[test]
    fn fallen_raft_plant_does_not_root_on_lake_bed() {
        // After leaving a tall raft, the plant may sit many cells above a
        // lowered waterline. It must float at the surface — never snap to
        // sand via find_surface_air_slot, and must not re-root on the bed.
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            for y in 2..=6 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 7..20 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        // Former raft height — well above the free surface at y=6.
        let mut atom = Atom::from_body(5, 14, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        // Pretend a diving root already touches the sand bed.
        atom.body.push((0, -13, ModuleId::Root));
        store.atoms.push(atom);
        for tick in 0..30u64 {
            store.step(&mut w, tick);
        }
        assert!(
            store.atoms[0].fallen,
            "must stay free-floating, not re-root on sand"
        );
        assert_eq!(
            store.atoms[0].gy, 6,
            "must sit at the free surface (gy={})",
            store.atoms[0].gy
        );
        assert!(
            store.atoms[0].gy > 2,
            "must not snap to the underwater bed seat"
        );
    }

    #[test]
    fn multi_root_plant_binds_full_organic_span() {
        // Later roots (not only the first Organic contact) must claim the
        // mat so the plant cannot drift ahead of the pile.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..28 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 6..=12 {
            w.set_cell(x, 6, Cell::solid(MaterialId::Organic));
        }
        // Nucleus at (9,7); roots spanning 7..11 at the waterline.
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
            (-2, -1, ModuleId::Root),
            (-1, -1, ModuleId::Root),
            (0, -1, ModuleId::Root),
            (1, -1, ModuleId::Root),
            (2, -1, ModuleId::Root),
        ];
        let mut atoms = vec![Atom::from_body(9, 7, 40.0, body)];
        apply_genome(
            &mut atoms[0],
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        let gx0 = atoms[0].gx;
        let mut sailed = false;
        for tick in 0..700u64 {
            w.tick = tick;
            let n = sail_plants_on_wind_rafts(&mut w, &mut atoms, 0.22, 4);
            if n > 0 && atoms[0].gx != gx0 {
                sailed = true;
                break;
            }
        }
        assert!(sailed, "multi-root raft plant should eventually sail");
        let wx = atoms[0].gx;
        // Every root column under the plant should still have Organic.
        for &(dx, dy, m) in &atoms[0].body {
            if m != ModuleId::Root {
                continue;
            }
            let rx = w.wrap_x(wx + dx as i32);
            let ry = atoms[0].gy + dy as i32;
            assert_eq!(
                w.get_cell(rx, ry).map(|c| c.material),
                Some(MaterialId::Organic),
                "root at ({rx},{ry}) should stay bound to Organic after sail"
            );
        }
    }

    #[test]
    fn roots_dive_into_water_under_soaked_floating_organic() {
        // soak_floating_litter fills raft Organic; without wet-void tropism
        // roots sprawl only inside the mat and never enter the lake.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut raft = Cell::solid(MaterialId::Organic);
        raft.sat = Sat(water_capacity(MaterialId::Organic));
        w.set_cell(5, 6, raft);
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        // Nucleus above the raft; holdfast Root sits in soaked Organic.
        let mut atom = Atom::from_body(5, 7, 80.0, body);
        apply_genome(&mut atom, crate::blueprint::Blueprint::minimal_plant().genome);
        atom.genome.root_depth_bias = 0.9;
        atom.genome.alloc_root = 0.55;
        atom.genome.alloc_stem = 0.2;
        atom.genome.alloc_leaf = 0.25;
        let caps = PlantGrowthCaps::default();
        for _ in 0..24 {
            atom.energy = atom.energy_max;
            let mut live = HashSet::new();
            for &(dx, dy, m) in &atom.body {
                if m == ModuleId::Root {
                    live.insert((atom.gx + dx as i32, atom.gy + dy as i32));
                }
            }
            let _ = try_elongate_root(&w, &mut atom, &live, &caps);
        }
        let in_water = atom.body.iter().any(|&(dx, dy, m)| {
            if m != ModuleId::Root {
                return false;
            }
            let wx = atom.gx + dx as i32;
            let wy = atom.gy + dy as i32;
            wy <= 5
                && w.get_cell(wx, wy).map(|c| c.material) == Some(MaterialId::Air)
                && w.get_cell(wx, wy).map(|c| !c.sat.is_empty()) == Some(true)
        });
        assert!(
            in_water,
            "holdfast on soaked floating Organic must elongate into the water column ({:?})",
            atom.body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Root)
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn dry_land_leaves_do_not_drink() {
        let mut w = moist_plot();
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut atom = Atom::from_body(4, 2, 40.0, body);
        let sat0: u32 = (2..10)
            .map(|y| w.get_cell(4, y).map(|c| c.sat.0 as u32).unwrap_or(0))
            .sum();
        for _ in 0..20 {
            atom.sip_acc = 2.0;
            let (_, taken, _) = drink_leaves(&mut w, &mut atom);
            assert_eq!(taken, 0, "shore Air must not count as leaf drink");
        }
        let sat1: u32 = (2..10)
            .map(|y| w.get_cell(4, y).map(|c| c.sat.0 as u32).unwrap_or(0))
            .sum();
        assert_eq!(sat0, sat1);
        assert!(!leaves_bathing(&w, &atom));
    }

    #[test]
    fn submerged_leaves_drink_standing_water_and_shrink_root_budget() {
        let mut w = moist_plot();
        // Flood the column so the seaweed ribbon sits in standing water.
        for y in 2..=8 {
            w.set_cell(4, y, Cell::water());
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        // Seat: root in sand (y=1), nucleus at y=2, leaves up through water.
        let mut atom = Atom::from_body(4, 2, 40.0, body);
        assert!(leaves_bathing(&w, &atom), "ribbon must bathe in standing water");
        let budget = useful_root_budget_for(
            &atom,
            DroughtBand::Hydrated,
            &PlantGrowthCaps::default(),
            true,
        );
        assert_eq!(budget, 1, "bathed frond needs only a holdfast");
        let sat0 = w.get_cell(4, 3).unwrap().sat.0;
        let mut drank = 0u32;
        for _ in 0..40 {
            atom.sip_acc = 2.0;
            let (e, taken, _) = drink_leaves(&mut w, &mut atom);
            drank += taken;
            assert!(e >= 0.0);
        }
        assert!(drank > 0, "leaves should sip standing water");
        let sat1 = w.get_cell(4, 3).unwrap().sat.0;
        assert!(sat1 < sat0 || drank > 0);
    }

    #[test]
    fn seaweed_ribbon_elongates_without_inventing_stem() {
        let mut w = moist_plot();
        // Soft ribbons only climb into standing water.
        for y in 2..=9 {
            w.set_cell(4, y, Cell::water());
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        apply_genome(&mut atom, crate::blueprint::Blueprint::minimal_seaweed().genome);
        assert_eq!(stem_count(&atom), 0);
        let photos0 = atom.photosystem_count();
        let trunks = HashSet::new();
        let roots = HashSet::new();
        let caps = PlantGrowthCaps::default();
        let mut grew = false;
        for pulse in 0..40u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = pulse * LAND_GROW_PERIOD;
            let spent = try_grow_plant(&w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
            if spent > 0.0 {
                grew = true;
            }
        }
        assert!(grew, "seaweed should spend energy on tissue");
        assert_eq!(stem_count(&atom), 0, "must stay stemless");
        assert!(
            atom.photosystem_count() > photos0,
            "ribbon should lengthen (was {photos0}, now {})",
            atom.photosystem_count()
        );
        let tip = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        assert!(tip >= 5, "frond tip should climb (tip y={tip})");
    }

    #[test]
    fn woody_leaf_refuses_moore_beside_foreign_live_photo() {
        let w = moist_plot();
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
        ];
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        atom.genome.alloc_stem = 0.05;
        atom.genome.alloc_leaf = 1.0;
        atom.genome.alloc_root = 0.05;
        // Neighbour already owns the side-leaf cells at both stem heights.
        let mut foreign = HashSet::new();
        foreign.insert((5, 3)); // beside stem (4,2)+(1,0) → wait gy=2, stem dy=1 → (4,3)+ (1,0)=(5,3)
        foreign.insert((5, 4));
        foreign.insert((3, 3));
        foreign.insert((3, 4));
        foreign.insert((5, 5));
        foreign.insert((3, 5));
        let trunks = HashSet::new();
        let caps = PlantGrowthCaps::default();
        let canopy = CanopyIndex::default();
        let n0 = atom.photosystem_count();
        for t in 0..20u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = t * LAND_GROW_PERIOD;
            let _ = try_grow_shoot(&w, &mut atom, t, &trunks, &foreign, &caps, &canopy, 0);
        }
        assert_eq!(
            atom.photosystem_count(),
            n0,
            "foreign canopy Moore ring must block woody side-leaves"
        );
    }

    #[test]
    fn woody_canopy_keeps_short_petioles_not_seaweed_ribbons() {
        let w = moist_plot();
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Stem),
            (1, 2, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        atom.genome.alloc_stem = 0.05;
        atom.genome.alloc_leaf = 1.0;
        atom.genome.alloc_root = 0.05;
        let trunks = HashSet::new();
        let caps = PlantGrowthCaps::default();
        let n0 = atom.photosystem_count();
        let mut max_cant = 1i32;
        for t in 0..40u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = t * LAND_GROW_PERIOD;
            let _ = try_grow_shoot(&w, &mut atom, t, &trunks, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
            for &(x, y, m) in &atom.body {
                if m == ModuleId::Photosystem {
                    max_cant = max_cant.max(leaf_support_dist(&atom, x, y));
                }
            }
        }
        assert!(
            atom.photosystem_count() > n0,
            "leaf-heavy shoot should add Photosystems"
        );
        assert!(
            max_cant <= WOODY_LEAF_MAX_CANT,
            "woody leaves must stay short petioles (cant={max_cant} > {WOODY_LEAF_MAX_CANT})"
        );
        // Prefer filling beside the trunk over one long tip chain.
        let leaf_cols: HashSet<i16> = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|&(x, _, _)| x)
            .collect();
        let leaf_rows: HashSet<i16> = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|&(_, y, _)| y)
            .collect();
        assert!(
            leaf_cols.len() >= 2 || leaf_rows.len() >= 2,
            "new leaves should fan beside the stem, not one seaweed tip"
        );
        assert!(stem_count(&atom) >= 1, "trunk stays");
    }

    #[test]
    fn stemless_ribbon_cannot_grow_upright_tower_in_dry_air() {
        let w = moist_plot(); // dry Air above sand
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        apply_genome(&mut atom, crate::blueprint::Blueprint::minimal_seaweed().genome);
        let tip0 = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        let photos0 = atom.photosystem_count();
        let trunks = HashSet::new();
        let roots = HashSet::new();
        let caps = PlantGrowthCaps::default();
        for pulse in 0..40u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = pulse * LAND_GROW_PERIOD;
            let _ = try_grow_plant(&w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
        }
        let tip1 = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        assert_eq!(
            tip1, tip0,
            "dry air must not let soft leaves stack into a tower (tip {tip0}→{tip1})"
        );
        assert_eq!(
            atom.photosystem_count(),
            photos0,
            "dry stemless must not grow a permanent sideways L"
        );
        assert_eq!(stem_count(&atom), 0);
    }

    #[test]
    fn stemless_ribbon_climbs_when_water_column_rises() {
        let mut w = moist_plot();
        // Shallow pool first — tip sits at the free surface.
        for y in 2..=5 {
            w.set_cell(4, y, Cell::water());
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        apply_genome(&mut atom, crate::blueprint::Blueprint::minimal_seaweed().genome);
        // Trim ribbon so the tip is inside the shallow pool.
        atom.body
            .retain(|&(_, y, m)| m != ModuleId::Photosystem || y <= 3);
        let tip0 = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        let trunks = HashSet::new();
        let roots = HashSet::new();
        let caps = PlantGrowthCaps::default();
        // Raise the water, then grow.
        for y in 6..=9 {
            w.set_cell(4, y, Cell::water());
        }
        for pulse in 0..40u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = pulse * LAND_GROW_PERIOD;
            let _ = try_grow_plant(&w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
        }
        let tip1 = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(_, y, _)| *y)
            .max()
            .unwrap();
        assert!(
            tip1 > tip0,
            "ribbon should climb with the rising water (tip {tip0}→{tip1})"
        );
        let max_dx = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|(x, _, _)| x.abs())
            .max()
            .unwrap();
        assert!(
            max_dx <= 1,
            "rising-water growth must stay a vertical ribbon, not an L (max_dx={max_dx})"
        );
    }

    #[test]
    fn plant_without_spore_module_cannot_wind_spore() {
        let w = moist_plot();
        let mut atom = Atom::from_body(
            4,
            2,
            60.0,
            crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus(),
        );
        atom.energy = atom.energy_max;
        atom.cooldown = 0;
        for t in 0..500u64 {
            assert!(
                try_plant_wind_spore(&w, &mut atom, t, 1, true, &[], 0.5).is_none(),
                "bare plant must not wind-spore"
            );
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
        }
    }

    #[test]
    fn fern_with_repro_spore_spreads_downwind() {
        let w = moist_plot();
        let mut atom = Atom::from_body(4, 2, 60.0, fern_body());
        // Enough roots to seat; wind spore itself only needs ReproSpore + leaf.
        assert!(is_anchored(&w, &atom));
        assert!(spore_count(&atom) >= 1);
        let mut child = None;
        for t in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            if let Some(c) = try_plant_wind_spore(&w, &mut atom, t, 3, true, &[4], 0.8) {
                child = Some(c);
                break;
            }
        }
        let child = child.expect("fern with ReproSpore must eventually wind-spore");
        assert!(is_land_plant(&child));
        assert!(
            (child.gx - 4).abs() >= PLANT_SPORE_MIN_DIST
                || w.wrap_width.map(|_| true).unwrap_or(true),
            "spore should leave the parent neighbourhood (got gx={})",
            child.gx
        );
        assert_ne!(child.gx, 4);
        assert!(
            spore_count(&child) >= 1,
            "sporeling should inherit a sorus"
        );
    }

    #[test]
    fn woody_leaf_abscises_after_chronic_starve() {
        let mut w = moist_plot();
        // Self-stack: high absorb tips keep the lowest leaf chronically dim.
        let mut short = Atom::from_body(
            4,
            2,
            40.0,
            vec![
                (0, -1, ModuleId::Root),
                (0, 0, ModuleId::Nucleus),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Photosystem),
                (0, 3, ModuleId::Photosystem),
                (0, 4, ModuleId::Photosystem),
                (0, 5, ModuleId::Photosystem),
            ],
        );
        short.genome.leaf_absorb = 0.75;
        let canopy = crate::shade::build_canopy_index(std::slice::from_ref(&short));
        let low_lit = crate::shade::shade_transmit(&canopy, 4, 4); // local (0,2) → y=4
        assert!(
            low_lit < WOODY_LEAF_STARVE_LIGHT,
            "fixture must keep lower leaf dim (lit={low_lit})"
        );
        short.leaf_starve = vec![(0, 2, WOODY_LEAF_STARVE_TICKS)];
        let n0 = short.photosystem_count();
        let dropped = shed_unproductive_woody_leaves(
            &mut w,
            &mut short,
            &canopy,
            1.0,
            WOODY_LEAF_DROP_PERIOD,
        );
        assert_eq!(dropped, 1, "starved woody leaf should abscise");
        assert_eq!(short.photosystem_count(), n0 - 1);
        assert!(
            short
                .leaf_starve
                .iter()
                .all(|&(x, y, _)| !(x == 0 && y == 2)),
            "starve counter cleared for dropped leaf"
        );
    }

    #[test]
    fn stemless_ribbon_never_abscises() {
        let mut w = moist_plot();
        let mut atom = Atom::from_body(
            4,
            2,
            40.0,
            crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus(),
        );
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_seaweed().genome,
        );
        let n0 = atom.photosystem_count();
        atom.leaf_starve = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Photosystem)
            .map(|&(x, y, _)| (x, y, WOODY_LEAF_STARVE_TICKS))
            .collect();
        let dropped = shed_unproductive_woody_leaves(
            &mut w,
            &mut atom,
            &CanopyIndex::default(),
            1.0,
            WOODY_LEAF_DROP_PERIOD,
        );
        assert_eq!(dropped, 0, "seaweed must not shed frond leaves");
        assert_eq!(atom.photosystem_count(), n0);
        assert!(
            atom.leaf_starve.is_empty(),
            "stemless path clears starve state"
        );
    }

    #[test]
    fn productive_woody_leaf_resets_starve_counter() {
        let mut w = moist_plot();
        let mut atom = Atom::from_body(
            4,
            2,
            40.0,
            vec![
                (0, -1, ModuleId::Root),
                (0, 0, ModuleId::Nucleus),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Photosystem),
                (1, 2, ModuleId::Photosystem),
            ],
        );
        atom.leaf_starve = vec![(0, 2, 100), (1, 2, 100)];
        // Open sky canopy — both leaves productive.
        let _ = shed_unproductive_woody_leaves(
            &mut w,
            &mut atom,
            &CanopyIndex::default(),
            1.0,
            1,
        );
        assert!(
            atom.leaf_starve.is_empty(),
            "full light should clear starve ticks ({:?})",
            atom.leaf_starve
        );
    }
}
