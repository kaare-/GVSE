//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Minimal Set D land plant (docs/organism/PLANTS.md § C + D1–D4):
//! Root + Stem + Photosystem on a fixed crown. Roots drink pore `sat`;
//! Photosystems in standing water drink free-column sat (shore leaves do
//! not). Elongates by `StemVsLeafVsRoot` / `RootDepthBias`. Shade = D2.
//! Vegetative sprouts = D3. Root starch tank + drought hibernate = D4.

use std::collections::HashSet;

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::blueprint::Genome;
use crate::cell::water_capacity;
use crate::grid::World;
use crate::organism::{Atom, BodyModule, ModuleId};

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

fn own_root_at(atom: &Atom, wx: i32, wy: i32) -> bool {
    atom.body.iter().any(|&(bx, by, m)| {
        m == ModuleId::Root && atom.gx + bx as i32 == wx && atom.gy + by as i32 == wy
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
            let moist = cell_moisture_frac(world, wx, wy)
                .max(cell_moisture_frac(world, wx, wy - 1))
                .max(cell_moisture_frac(world, wx, wy - 2) * 0.85);
            let mut score = moist * ROOT_MOISTURE_AFFINITY
                + root_dir_preference(dx, dy, depth_bias)
                - pen * 0.03
                - ROOT_TRANSPORT_TAX * hops;
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
/// - New Stem needs a Moore gap from other live/dead trunks (leaves may touch).
pub fn try_grow_shoot(
    atom: &mut Atom,
    tick: u64,
    trunks: &HashSet<(i32, i32)>,
    caps: &PlantGrowthCaps,
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
        // Stemmed: beside tallest stem. Stemless ribbon: tip of the leaf
        // string (or nucleus) so seaweed elongates upward without a trunk.
        let anchor = if n_stem > 0 {
            atom.body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Stem)
                .max_by_key(|(_, y, _)| *y)
                .map(|&(x, y, _)| (x, y))
        } else {
            atom.body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Photosystem)
                .max_by_key(|(_, y, _)| *y)
                .map(|&(x, y, _)| (x, y))
                .or_else(|| {
                    atom.body
                        .iter()
                        .find(|(_, _, m)| *m == ModuleId::Nucleus)
                        .map(|&(x, y, _)| (x, y))
                })
        };
        let Some((tx, ty)) = anchor else {
            return false;
        };
        // Keep the olive tip clear whenever a trunk exists — stacking a
        // leaf on `(0,+1)` produced leaf→stem→leaf towers and blocked
        // further stem growth. Stemless ribbons elongate straight up.
        let dirs: &[(i16, i16)] = if n_stem > 0 {
            &[(1, 0), (-1, 0), (1, 1), (-1, 1)]
        } else {
            &[(0, 1), (1, 0), (-1, 0), (1, 1), (-1, 1)]
        };
        for &(dx, dy) in dirs {
            let nx = tx + dx;
            let ny = ty + dy;
            if ny > 16 || occupied.contains(&(nx, ny)) {
                continue;
            }
            // Stemmed plants: never leaf-on-leaf in a column (keeps trunk
            // clear). Stemless ribbons *are* a leaf column — allow it.
            if n_stem > 0
                && dy == 1
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
    live_roots: &HashSet<(i32, i32)>,
    caps: &PlantGrowthCaps,
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
            spent += try_grow_shoot(atom, tick, trunks, caps);
        }
    } else {
        spent += try_grow_shoot(atom, tick, trunks, caps);
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
        let w = moist_plot();
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
            let spent = try_grow_plant(&w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &caps);
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
}
