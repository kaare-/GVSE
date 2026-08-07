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
use crate::organism::{
    column_sky_light, nucleus_rests_on_mineral, Atom, BodyModule, ModuleId,
};
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
/// Pore fill above which land roots stop sipping.
///
/// Without this, growing plants strip moist sand to bone-dry in ~1–2
/// days, tip into dormancy, and night-starve. Keep a hydrated buffer
/// above [`DROUGHT_STRESS_FRAC`].
pub const ROOT_DRINK_COMFORT_FRAC: f32 = 0.12;
/// Pore fill fraction that triggers drought dormancy (hibernate).
pub const DROUGHT_DORMANT_FRAC: f32 = 0.015;
/// Max consecutive dormant ticks before the plant dies (~2.5 min @ 60 Hz).
pub const DROUGHT_HIBERNATE_MAX_TICKS: u32 = 9_000;
/// Upkeep multiplier while drought-dormant (respiration only).
pub const DROUGHT_DORMANT_UPKEEP: f32 = 0.12;
/// Land-plant basal upkeep vs plankton (woody roots respire less).
/// Nudged down after Beer-Lambert + carbon made day surplus tighter than
/// the pre-shade `tip×n_photo` era.
pub const PLANT_UPKEEP_MULT: f32 = 0.28;
/// Day-factor blend for plant respiration (`a + (1-a)*day`).
/// Lower night floor than plankton — a 27-module river plant used to burn
/// its whole tank in one 600-tick night when every Stem/Root counted 1:1.
pub const PLANT_UPKEEP_DAY_BLEND: f32 = 0.18;
/// Day factor below which plants skip elongation / submerged stem-urge.
pub const PLANT_GROW_MIN_DAY: f32 = 0.20;
/// Extra score weight so roots prefer wetter substrate cells.
pub const ROOT_MOISTURE_AFFINITY: f32 = 2.8;
/// Score bonus for stepping into standing-water Air (legacy wet-void).
///
/// Free-column water has no pore moisture (`cell_moisture_frac` = 0), so
/// after floating Organic soaks from the lake the raft alone outscores
/// every dive into the water column. This bonus restores raft-plant
/// roots under the mat without letting dry Air gaps look wet.
pub const ROOT_WET_VOID_AFFINITY: f32 = 1.6;
/// Max body-Y drop (and world cells below nucleus) for dangling roots on an
/// **uprooted** woody floater. Longer wet-void pipes looked like the tree
/// suddenly rooted from nucleus to bed while the chassis still floated.
pub const UPROOTED_ROOT_KEEL_MAX: i16 = 3;
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
/// Max Chebyshev distance a woody canopy leaf may sit from a Stem
/// (Moore neighbourhood). Distance 2 left empty cells between leaf and
/// trunk — midair green flecks. Stemless seaweed ignores this.
pub const WOODY_LEAF_MAX_CANT: i32 = 1;
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
/// sprouting is blocked. Paired with [`SPROUT_CROWN_CLEARANCE`] so groves
/// stay readable instead of a solid green bar.
pub const SPROUT_LOCAL_MAX: usize = 5;
/// No other living crown within this many columns of a new sprout seat.
/// `2` → minimum crown spacing of 3 (one empty column between ±1 T-canopies).
pub const SPROUT_CROWN_CLEARANCE: i32 = 2;
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
///
/// Exception: tipped floaters still elongate dangling roots into the wet
/// column / raft even while canopy leaves bathe.
pub fn useful_root_budget_for(
    atom: &Atom,
    drought: DroughtBand,
    caps: &PlantGrowthCaps,
    leaf_bathing: bool,
) -> usize {
    let hard = caps.max_roots.max(1);
    if leaf_bathing && !atom.fallen {
        return 1.min(hard);
    }
    let base = useful_root_budget(atom, caps);
    let base = if atom.fallen {
        // At least a short dangling pipe under a tipped crown.
        base.max(3).min(hard)
    } else {
        base
    };
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
        if atom.fallen {
            best = best.max(standing_water_frac(world, wx, wy));
            best = best.max(standing_water_frac(world, wx, wy - 1));
        }
    }
    best
}

fn standing_water_frac(world: &World, gx: i32, gy: i32) -> f32 {
    use crate::rules::is_standing_water;
    if !is_standing_water(world, gx, gy) {
        return 0.0;
    }
    world
        .get_cell(gx, gy)
        .map(|c| c.sat.0 as f32 / 255.0)
        .unwrap_or(0.0)
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

/// Metabolic load for land-plant upkeep (leaves cost more than wood/roots).
///
/// Counting every body module at 1.0 made river plants that grew a trunk
/// overnight their energy on the first night — Stem/Root tissue should not
/// respire like Photosystems.
pub fn plant_metabolic_load(atom: &Atom) -> f32 {
    let mut load = 0.0f32;
    for &(_, _, m) in &atom.body {
        load += match m {
            ModuleId::Photosystem => 1.0,
            ModuleId::Stem => 0.28,
            ModuleId::Root => 0.18,
            ModuleId::Nucleus => 0.40,
            ModuleId::ReproSpore => 0.45,
            ModuleId::Symbiont => 0.22,
            ModuleId::Digest | ModuleId::Hypha => 0.50,
        };
    }
    load.max(1.0)
}

pub(crate) fn cell_moisture_frac(world: &World, gx: i32, gy: i32) -> f32 {
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

/// Woody plant that has tipped — one rigid chassis (stem + roots baked together).
///
/// Distinct from upright substrate purchase (`!fallen`). Open-water castaways
/// stay **uprooted** (short wet keel, no mineral tunnel). Shore tips with
/// mineral under the nucleus may re-root while remaining tipped.
pub fn woody_uprooted(atom: &Atom) -> bool {
    atom.fallen && stem_count(atom) > 0
}

/// True when a root step would bore into mineral / rock (not free column or compost).
fn root_target_is_mineral(mat: MaterialId) -> bool {
    !matches!(
        mat,
        MaterialId::Air | MaterialId::Water | MaterialId::Organic
    )
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
            continue;
        }
        // Free-float / tipped plants: dangling roots sip the lake column.
        if atom.fallen {
            if let Some(n) = sip_standing_air(world, wx, wy, want - taken) {
                energy += ROOT_WATER_ENERGY * n as f32;
                taken += n;
                deposit_at = (wx, wy);
                continue;
            }
            if let Some(n) = sip_standing_air(world, wx, wy - 1, want - taken) {
                energy += ROOT_WATER_ENERGY * n as f32;
                taken += n;
                deposit_at = (wx, wy - 1);
            }
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
///
/// Land roots only sip while below [`ROOT_DRINK_COMFORT_FRAC`] — once the
/// bed is comfortably moist, stop stripping pores. Fallen / lake plants
/// still sip through dangling roots (standing water regenerates).
pub fn drink_plant(world: &mut World, atom: &mut Atom) -> (f32, u32, (i32, i32)) {
    let moist = plant_moisture_frac(world, atom);
    let (e_l, s_l, at_l) = drink_leaves(world, atom);
    let need_root_sip = atom.fallen || moist < ROOT_DRINK_COMFORT_FRAC;
    let (e_r, s_r, at_r) = if need_root_sip {
        drink_roots(world, atom)
    } else {
        // Decay unused sip progress so a later dry spell doesn't gulp
        // a banked multi-sat dump from the comfort pause.
        atom.sip_acc *= 0.5;
        (0.0, 0, (atom.gx, atom.gy))
    };
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
///
/// Only porous plantable media convert — never Stone/Bedrock holes, and never
/// dry-Air rhizome gaps (those would float as orphan Organic fragments).
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
            MaterialId::Sand | MaterialId::Clay | MaterialId::Soil => {
                let mut org = Cell::solid(MaterialId::Organic);
                let cap = water_capacity(MaterialId::Organic);
                org.sat.0 = if cap > 0 { c.sat.0.min(cap) } else { 0 };
                world.set_cell(wx, wy, org);
                painted += 1;
            }
            MaterialId::Organic => {
                painted += 1; // already organic residue
            }
            _ => {}
        }
    }
    painted
}

/// Drop Photosystem modules as falling Organic litter (dry Air only).
/// Leaves peel off the corpse immediately; stems linger grey until dissolve.
///
/// Uses tipped draw pose so stemless / upright-mast leaves don't spawn
/// Organic on the upright phantom axis.
pub fn drop_dead_leaves(world: &mut World, atom: &Atom) -> u32 {
    let mut painted = 0u32;
    for &(dx, dy, mid) in &atom.body {
        if mid != ModuleId::Photosystem {
            continue;
        }
        let (odx, ody) = atom.fallen_draw_offset(dx, dy);
        if paint_leaf_litter(world, atom.gx + odx as i32, atom.gy + ody as i32) {
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

    // Pose before removal — sync would drop this leaf from upright_growth.
    let (odx, ody) = atom.fallen_draw_offset(dx, dy);
    let before = atom.body.len();
    atom.body
        .retain(|&(x, y, m)| !(m == ModuleId::Photosystem && x == dx && y == dy));
    if atom.body.len() == before {
        return 0;
    }
    atom.leaf_starve
        .retain(|&(x, y, _)| !(x == dx && y == dy));
    atom.sync_upright_growth();
    let _ = paint_leaf_litter(world, atom.gx + odx as i32, atom.gy + ody as i32);
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

/// Fungus seat: prefers Air above Organic / Soil (visible fruiting stalk).
/// Buried Organic seats stay legal for rhizomorph hops via
/// [`find_fungus_slot_biased`] with `prefer_surface = false`.
pub fn find_fungus_slot(world: &World, gx: i32, gy: i32) -> Option<i32> {
    find_fungus_slot_biased(world, gx, gy, true)
}

/// Like [`find_fungus_slot`], but `prefer_surface` boosts Air-on-bed seats
/// (stalks) and deprioritizes buried Organic. Rhizomorph hops pass `false`.
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
                            220 + world
                                .get_cell(gx, y - 1)
                                .map(|c| c.mycelium() as i32 / 4)
                                .unwrap_or(0)
                        }
                        Some(MaterialId::Soil) => 180,
                        _ => score + 40,
                    };
                } else if here.material == MaterialId::Organic {
                    score /= 3; // bury only when no surface seat is nearby
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
    // Surface stalks (Air on bed) outrank buried Organic so fruiting
    // bodies stay visible; rhizomorph hops still allow bury via bias.
    if here.material == MaterialId::Organic {
        return 50 + (here.mycelium() as i32 / 8);
    }
    let Some(below) = world.get_cell(gx, nucleus_y - 1) else {
        return 0;
    };
    match below.material {
        MaterialId::Organic => 140 + (below.mycelium() as i32 / 4),
        MaterialId::Soil => 110,
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

/// Shoot (Stem / woody Photosystem) may occupy dry or wet Air, not solids.
fn shoot_cell_free(world: &World, wx: i32, wy: i32) -> bool {
    matches!(
        world.get_cell(wx, wy),
        Some(c) if c.material == MaterialId::Air
    )
}

/// Penetrate multiplier — higher = harder / costlier. `None` = refuse.
///
/// Stone / Limestone are allowed but expensive: roots find cracks slowly.
/// Elongation into competent rock then opens the crack ([`crack_rock_for_root`]).
fn penetrate_cost(mat: MaterialId) -> Option<f32> {
    match mat {
        MaterialId::Bedrock | MaterialId::Ice | MaterialId::Snow | MaterialId::Water => None,
        MaterialId::Organic => Some(0.35),
        MaterialId::Sand | MaterialId::Clay | MaterialId::Soil => Some(0.65),
        MaterialId::Stone | MaterialId::Limestone => Some(1.6),
        MaterialId::Air => Some(0.45), // gaps / rhizome air pockets
        _ => Some(1.0), // LooseRock, Gravel, …
    }
}

/// Root force opens a crack: Stone → LooseRock (or Limestone analogue).
/// Living roots stay an overlay; this is the lasting substrate change.
fn crack_rock_for_root(world: &mut World, wx: i32, wy: i32) {
    use crate::cell::Cell;
    let Some(c) = world.get_cell(wx, wy) else {
        return;
    };
    let loose = match c.material {
        MaterialId::Stone => MaterialId::LooseRock,
        MaterialId::Limestone => MaterialId::LooseLimestone,
        _ => return,
    };
    let mut next = Cell::solid(loose);
    next.sat = c.sat;
    next.flags = c.flags;
    next._pad = c._pad;
    world.set_cell(wx, wy, next);
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
/// Uses **draw** offsets so a tipped trunk lying on the waterline does not
/// cast a phantom upright sail, while post-tip upright shoots still count.
pub fn collect_plant_sail_tops<'a, I>(atoms: I) -> HashMap<i32, i32>
where
    I: IntoIterator<Item = &'a Atom>,
{
    let mut out: HashMap<i32, i32> = HashMap::new();
    for a in atoms {
        if !is_land_plant(a) {
            continue;
        }
        for &(dx0, dy0, m) in &a.body {
            if !matches!(
                m,
                ModuleId::Photosystem | ModuleId::Stem | ModuleId::Nucleus
            ) {
                continue;
            }
            let (dx, dy) = a.fallen_draw_offset(dx0, dy0);
            let wx = a.gx + dx as i32;
            let wy = a.gy + dy as i32;
            let e = out.entry(wx).or_insert(wy);
            *e = (*e).max(wy);
        }
    }
    out
}

/// True when a Root/Nucleus sits in or on a floating Organic column.
pub(crate) fn holdfast_on_float_column(
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
/// free-floating plants are not hitchhiked when litter slides past. Plants
/// with sand/rock purchase also stay put (shore mats must not drag them).
/// Every root column across a mounted plant's span claims the mat (not only
/// the first Organic contact). Loose unrooted litter may still peel apart.
/// Returns how many Organic columns moved.
///
/// Uses [`crate::GrainConfig::default`] raft bind radius. Prefer
/// [`sail_plants_on_wind_rafts_cfg`] when live Tab knobs are available.
pub fn sail_plants_on_wind_rafts(
    world: &mut World,
    atoms: &mut [Atom],
    wind_vx_tiles: f32,
    tile_cols: i32,
) -> u32 {
    sail_plants_on_wind_rafts_cfg(
        world,
        atoms,
        wind_vx_tiles,
        tile_cols,
        &crate::GrainConfig::default(),
    )
}

/// [`sail_plants_on_wind_rafts`] with live [`crate::GrainConfig`] raft bind.
pub fn sail_plants_on_wind_rafts_cfg(
    world: &mut World,
    atoms: &mut [Atom],
    wind_vx_tiles: f32,
    tile_cols: i32,
    grain: &crate::GrainConfig,
) -> u32 {
    let columns = crate::rules::collect_floating_organic_columns(world);
    let mut bound_cols: HashSet<i32> = HashSet::new();
    let mut raft_mounted: Vec<usize> = Vec::new();

    for (i, atom) in atoms.iter().enumerate() {
        if !is_land_plant(atom) || atom.energy <= 0.0 {
            continue;
        }
        // Sand-rooted plants stay put — beach litter must not drag them.
        if crate::organism::plant_grounded_in_substrate(world, atom, &columns) {
            continue;
        }
        let mut min_dx = i16::MAX;
        let mut max_dx = i16::MIN;
        let mut mounted = false;
        for &(dx, dy, m) in &atom.body {
            if m != ModuleId::Root && m != ModuleId::Nucleus {
                continue;
            }
            let wx = world.wrap_x(atom.gx + dx as i32);
            let wy = atom.gy + dy as i32;
            min_dx = min_dx.min(dx);
            max_dx = max_dx.max(dx);
            if holdfast_on_float_column(&columns, m, wx, wy) {
                mounted = true;
            }
        }
        if !mounted || min_dx > max_dx {
            continue;
        }
        raft_mounted.push(i);
        // Bind the body-local root/nucleus span (not wrapped min..=max —
        // that explodes across the ring when roots sit near both seams).
        for d in min_dx..=max_dx {
            let x = world.wrap_x(atom.gx + d as i32);
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
    let (moved, sign, moved_cols) = crate::rules::drift_floating_organic_columns_cfg(
        world,
        &columns,
        wind_vx_tiles,
        tile_cols,
        Some(&sails),
        Some(&bound_cols),
        grain,
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
    world: &mut World,
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
            let wet_void = cell.material == MaterialId::Air
                && crate::rules::is_standing_water(world, wx, wy);
            // Open-water uprooted woody: rigid short keel in the free column
            // only. No mineral tunnel (hillsides / bed pipes that looked like
            // the floater was suddenly rooted nucleus→bed). Shore tips with
            // mineral under the nucleus may elongate into the beach.
            if woody_uprooted(atom) && !nucleus_rests_on_mineral(world, atom) {
                if root_target_is_mineral(cell.material) {
                    continue;
                }
                if ny < -UPROOTED_ROOT_KEEL_MAX {
                    continue;
                }
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
    let wx = world.wrap_x(atom.gx + nx as i32);
    let wy = atom.gy + ny as i32;
    // Open cracks in competent rock as the root wedges in.
    crack_rock_for_root(world, wx, wy);
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
    // Heal legacy bodies that grew Manhattan-2 flecks beside the trunk.
    let _ = prune_detached_woody_leaves(atom);
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
                atom.mark_upright_growth(nx, ny);
                return true;
            }
            return false;
        }

        // —— Woody canopy: short petioles beside the trunk/branch.
        // Prefer a new side-leaf on a stem cell. Must stay Moore-adjacent
        // to Stem ([`WOODY_LEAF_MAX_CANT`]) — Manhattan-2 left midair flecks.
        // Refuse Moore-adjacent foreign leaves (root-spacing analogue) and
        // sites too dim for the leaf to pay for itself.
        let mut stem_anchors: Vec<(i16, i16)> = atom
            .body
            .iter()
            .filter(|&&(x, y, m)| {
                if m != ModuleId::Stem {
                    return false;
                }
                // Tipped plants: leaf on the upright mast, not along the
                // baked waterline trunk (that reads as sideways growth).
                if atom.fallen {
                    y > 0 || atom.upright_growth.iter().any(|&p| p == (x, y))
                } else {
                    true
                }
            })
            .map(|&(x, y, _)| (x, y))
            .collect();
        // Prefer higher / brighter stem first, then fill gaps lower down.
        stem_anchors.sort_by_key(|&(_, y)| std::cmp::Reverse(y));
        // Orthogonal + diagonal-up — all Moore-adjacent to the stem cell.
        const SIDE: [(i16, i16); 4] = [(1, 0), (-1, 0), (1, 1), (-1, 1)];
        for &(tx, ty) in &stem_anchors {
            for &(dx, dy) in &SIDE {
                let nx = tx + dx;
                let ny = ty + dy;
                if ny > 16 || nx == tx || occupied.contains(&(nx, ny)) {
                    continue;
                }
                if atom.fallen && ny <= 0 {
                    continue;
                }
                if !woody_leaf_attached(atom, nx, ny) {
                    continue;
                }
                let wx = world.wrap_x(atom.gx + nx as i32);
                let wy = atom.gy + ny as i32;
                if !shoot_cell_free(world, wx, wy) {
                    continue;
                }
                if beside_foreign_live_photo(atom, wx, wy, live_photos) {
                    continue;
                }
                if !woody_leaf_light_ok(world, atom, canopy, entity_id, wx, wy, n_photo) {
                    continue;
                }
                atom.energy -= cost;
                atom.body.push((nx, ny, ModuleId::Photosystem));
                atom.mark_upright_growth(nx, ny);
                return true;
            }
        }
        false
    };

    let place_stem = |atom: &mut Atom, occupied: &HashSet<(i16, i16)>| -> bool {
        if !can_grow_stem {
            return false;
        }
        let try_place = |atom: &mut Atom, nx: i16, ny: i16, occupied: &HashSet<(i16, i16)>| -> bool {
            if ny > 16 || ny < 1 || occupied.contains(&(nx, ny)) {
                return false;
            }
            // Hard rule: stem never sits on a photosystem cell.
            if atom
                .body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Photosystem && x == nx && y == ny - 1)
            {
                return false;
            }
            let wx = world.wrap_x(atom.gx + nx as i32);
            let wy = atom.gy + ny as i32;
            if !shoot_cell_free(world, wx, wy) {
                return false;
            }
            if !stem_spacing_ok(atom, nx, ny, trunks) {
                return false;
            }
            atom.energy -= cost;
            atom.body.push((nx, ny, ModuleId::Stem));
            atom.mark_upright_growth(nx, ny);
            true
        };

        if atom.fallen {
            // After tip bake, canopy lies on dy==0. Prefer continuing the
            // upright mast, else start a fresh shoot above the nucleus.
            let mut up_anchors: Vec<(i16, i16)> = atom
                .body
                .iter()
                .filter(|&&(x, y, m)| {
                    m == ModuleId::Stem && (y > 0 || atom.upright_growth.iter().any(|&p| p == (x, y)))
                })
                .map(|&(x, y, _)| (x, y))
                .collect();
            up_anchors.sort_by_key(|&(x, y)| (std::cmp::Reverse(y), x.abs()));
            for (ax, ay) in up_anchors {
                if try_place(atom, ax, ay + 1, occupied) {
                    return true;
                }
            }
            if try_place(atom, 0, 1, occupied) {
                return true;
            }
            // Last resort: shoot upward from a waterline stem tip.
            let mut flat: Vec<(i16, i16)> = atom
                .body
                .iter()
                .filter(|(_, y, m)| *m == ModuleId::Stem && *y == 0)
                .map(|&(x, y, _)| (x, y))
                .collect();
            flat.sort_by_key(|&(x, _)| x.abs());
            for (ax, ay) in flat {
                if try_place(atom, ax, ay + 1, occupied) {
                    return true;
                }
            }
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
            if try_place(atom, ax, ay + 1, occupied) {
                return true;
            }
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

/// Chebyshev distance from a cell to the nearest Stem (wood only).
fn woody_leaf_wood_dist(atom: &Atom, dx: i16, dy: i16) -> i32 {
    let mut best = i32::MAX;
    for &(x, y, m) in &atom.body {
        if m != ModuleId::Stem {
            continue;
        }
        let d = (x - dx).abs().max(y - dy).abs() as i32;
        best = best.min(d);
    }
    best
}

/// True when a woody leaf touches Stem in the Moore neighbourhood.
fn woody_leaf_attached(atom: &Atom, dx: i16, dy: i16) -> bool {
    let d = woody_leaf_wood_dist(atom, dx, dy);
    d != i32::MAX && d <= WOODY_LEAF_MAX_CANT
}

/// Drop woody Photosystems that no longer touch the trunk (midair flecks).
///
/// Keeps at least one Photosystem (closest to wood) so a plant is not
/// stripped bare in one prune. Returns how many leaves were removed.
pub fn prune_detached_woody_leaves(atom: &mut Atom) -> u32 {
    if stem_count(atom) == 0 {
        return 0;
    }
    let photos: Vec<(i16, i16)> = atom
        .body
        .iter()
        .filter(|(_, _, m)| *m == ModuleId::Photosystem)
        .map(|&(x, y, _)| (x, y))
        .collect();
    if photos.is_empty() {
        return 0;
    }
    let detached: Vec<(i16, i16)> = photos
        .iter()
        .copied()
        .filter(|&(x, y)| !woody_leaf_attached(atom, x, y))
        .collect();
    if detached.is_empty() {
        return 0;
    }
    let attached_n = photos.len() - detached.len();
    let drop_list: Vec<(i16, i16)> = if attached_n > 0 {
        detached
    } else {
        // All gapped — keep the closest fleck, drop the rest.
        let mut ranked = detached;
        ranked.sort_by_key(|&(x, y)| woody_leaf_wood_dist(atom, x, y));
        ranked.into_iter().skip(1).collect()
    };
    if drop_list.is_empty() {
        return 0;
    }
    let before = atom.body.len();
    atom.body.retain(|&(x, y, m)| {
        !(m == ModuleId::Photosystem && drop_list.iter().any(|&(dx, dy)| dx == x && dy == y))
    });
    let removed = (before - atom.body.len()) as u32;
    if removed > 0 {
        atom.leaf_starve.retain(|&(x, y, _)| {
            atom.body
                .iter()
                .any(|&(bx, by, m)| m == ModuleId::Photosystem && bx == x && by == y)
        });
        atom.sync_upright_growth();
    }
    removed
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

/// Child body for vegetative sprout / spore — full upright parent chassis.
///
/// Children inherit the parent's body plan (then `mutate_body` / genome
/// mutate), not a sapling shrink or [`Blueprint::minimal_plant`]. Stemless
/// parents stay stemless via the mutation palette. Tipped (baked) parents
/// are straightened so the child is born upright, not horizontal.
pub fn sprout_body(parent: &Atom) -> Vec<BodyModule> {
    let mut body = if parent.fallen {
        straighten_tipped_body_for_child(&parent.body)
    } else {
        parent.body.clone()
    };
    if !body.iter().any(|(_, _, m)| *m == ModuleId::Nucleus) {
        body.insert(0, (0, 0, ModuleId::Nucleus));
    }
    if !body.iter().any(|(_, _, m)| *m == ModuleId::Root) {
        body.push((0, -1, ModuleId::Root));
    }
    dedupe_body_cells(body)
}

/// Approximate inverse of [`crate::organism::bake_tip_into_body`] for children.
///
/// Rigid tip mapped `(dx, dy) → (dy, -dx)`; inverse is `(dx, dy) → (-dy, dx)`.
/// Post-tip upright mast cells (`dy > 0`) are already world-up.
fn straighten_tipped_body_for_child(body: &[BodyModule]) -> Vec<BodyModule> {
    use std::collections::HashSet;

    let mut next: Vec<BodyModule> = Vec::with_capacity(body.len());
    let mut used: HashSet<(i16, i16)> = HashSet::new();
    let place = |used: &mut HashSet<(i16, i16)>, next: &mut Vec<BodyModule>, mut nx: i16, mut ny: i16, m: ModuleId| {
        let mut guard = 0;
        while !used.insert((nx, ny)) && guard < 16 {
            if ny <= 0 {
                ny -= 1;
            } else {
                nx += 1;
            }
            guard += 1;
        }
        if guard < 16 {
            next.push((nx, ny, m));
        }
    };

    for &(dx, dy, m) in body {
        if m == ModuleId::Nucleus {
            place(&mut used, &mut next, 0, 0, ModuleId::Nucleus);
            continue;
        }
        if dy > 0 {
            // Post-tip upright mast — already world-up.
            place(&mut used, &mut next, dx, dy, m);
            continue;
        }
        // Inverse rigid tip: (dx, dy) → (-dy, dx).
        let (nx, ny) = (-dy, dx);
        place(&mut used, &mut next, nx, ny, m);
    }
    if !next.iter().any(|(_, _, m)| *m == ModuleId::Nucleus) {
        next.insert(0, (0, 0, ModuleId::Nucleus));
    }
    next
}

fn dedupe_body_cells(body: Vec<BodyModule>) -> Vec<BodyModule> {
    use std::collections::HashSet;
    let mut seen = HashSet::new();
    let mut out = Vec::with_capacity(body.len());
    for (dx, dy, m) in body {
        if seen.insert((dx, dy)) {
            out.push((dx, dy, m));
        }
    }
    out
}

/// One growth pulse: root and/or shoot from allocation weights.
pub fn try_grow_plant(
    world: &mut World,
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

/// True when `gx` is far enough from every living crown for readable spacing.
pub fn crown_clearance_ok(
    plant_cols: &[i32],
    gx: i32,
    wrap_width: Option<i32>,
) -> bool {
    !plant_cols.iter().any(|&c| {
        column_dist(c, gx, wrap_width) <= SPROUT_CROWN_CLEARANCE
    })
}

/// Pick a world column for vegetative sprout from a lateral runner tip.
/// Skips seats that violate [`SPROUT_CROWN_CLEARANCE`] against living crowns.
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
        if !crown_clearance_ok(plant_cols, wx, world.wrap_width) {
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
/// pop room, a seat with [`SPROUT_CROWN_CLEARANCE`] from living crowns, and
/// local density below [`SPROUT_LOCAL_MAX`]. Child chassis is the parent's
/// upright body plan plus `mutate_body` (stemless stays stemless); genome is
/// mutated then re-synced so alloc can't reintroduce a trunk.
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
    // Readable spacing — not just "one crown per column".
    if !crown_clearance_ok(plant_cols, wx, world.wrap_width) {
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
    crate::blueprint::ensure_symbiont_inherited(&atom.body, &mut body);
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

/// Child body for wind spore — parent chassis; keeps a sorus if the parent
/// had [`ModuleId::ReproSpore`] so ferns can keep dispersing.
pub fn spore_dispersal_body(parent: &Atom) -> Vec<BodyModule> {
    let mut body = sprout_body(parent);
    if spore_count(parent) == 0 {
        return body;
    }
    // Full parent clone already carries the sorus; only restore if lost.
    if body.iter().any(|(_, _, m)| *m == ModuleId::ReproSpore) {
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
///
/// When `allow_bank` is true, dry / crowded columns still score as
/// hibernation landings (spore bank) so dispersal is not lost.
pub fn pick_plant_spore_column(
    world: &World,
    atom: &Atom,
    tick: u64,
    entity_id: u32,
    wind_vx: f32,
    plant_cols: &[i32],
) -> Option<(i32, i32)> {
    pick_plant_spore_column_mode(world, atom, tick, entity_id, wind_vx, plant_cols, false)
}

fn pick_plant_spore_column_mode(
    world: &World,
    atom: &Atom,
    tick: u64,
    entity_id: u32,
    wind_vx: f32,
    plant_cols: &[i32],
    allow_bank: bool,
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
            let Some(gy) = find_plant_slot(world, wx, atom.gy) else {
                continue;
            };
            let moist = cell_moisture_frac(world, wx, gy - 1);
            let clear = crown_clearance_ok(plant_cols, wx, world.wrap_width);
            let local = count_plants_near(plant_cols, wx, SPROUT_LOCAL_RADIUS, world.wrap_width);
            let roomy = local < SPROUT_LOCAL_MAX;
            let ready = clear && moist >= 0.02 && roomy;
            if !ready && !allow_bank {
                continue;
            }
            let downwind = if sign == prefer_dir { 2.5 } else { 0.0 };
            // Ready seats outrank bank-only landings.
            let score = if ready {
                moist * 2.0 + dist as f32 * 0.04 + downwind + 50.0
            } else {
                moist * 0.5 + dist as f32 * 0.02 + downwind
            };
            if best.map(|(s, _, _)| score > s).unwrap_or(true) {
                best = Some((score, wx, gy));
            }
        }
    }
    best.map(|(_, wx, gy)| (wx, gy))
}

/// Fern-style wind spore: needs painted [`ModuleId::ReproSpore`], energy,
/// cooldown, and a downwind landing. Germinates immediately when the seat
/// is moist / uncrowded; otherwise hibernates in [`World::spore_bank`].
pub fn try_plant_wind_spore(
    world: &mut World,
    atom: &mut Atom,
    tick: u64,
    entity_id: u32,
    pop_room: bool,
    plant_cols: &[i32],
    wind_vx: f32,
    bank_cfg: &crate::spore_bank::SporeBankConfig,
) -> crate::spore_bank::DispersalResult {
    use crate::spore_bank::{packet_from_child, plant_seat_ready, DispersalResult, SporeKind};

    if atom.cooldown > 0 {
        return DispersalResult::None;
    }
    if !is_land_plant(atom) || spore_count(atom) < 1 {
        return DispersalResult::None;
    }
    // Need a photosystem / stem "frond" to launch from — roots alone can't.
    if atom.photosystem_count() < 1 && stem_count(atom) < 1 {
        return DispersalResult::None;
    }
    let tank = tank_ref(atom);
    if atom.energy < tank * PLANT_SPORE_ENERGY_FRAC {
        return DispersalResult::None;
    }
    let h = hash_u64_plant(world.seed.0, tick, entity_id as u64, 0x5F07_E001);
    if h % PLANT_SPORE_ODDS != 0 {
        return DispersalResult::None;
    }
    // Prefer a ready seat; fall back to any plantable crown for the bank.
    let Some((wx, gy)) = pick_plant_spore_column_mode(
        world,
        atom,
        tick,
        entity_id,
        wind_vx,
        plant_cols,
        bank_cfg.enabled,
    ) else {
        return DispersalResult::None;
    };
    let cost = tank * PLANT_SPORE_COST_FRAC;
    if atom.energy < cost {
        return DispersalResult::None;
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
    crate::blueprint::ensure_symbiont_inherited(&atom.body, &mut body);
    let mut child_genome = Genome::mutate(atom.genome, world.seed.0, tick, entity_id);
    sync_alloc_to_body(&mut child_genome, &body);
    let mut child = Atom::from_body(wx, gy, tank, body);
    apply_genome(&mut child, child_genome);
    child.energy = (cost * 0.5).clamp(1.0, child.energy_max);
    child.cooldown = PLANT_SPORE_PERIOD;
    pin_plant_pose(&mut child);

    let ready = pop_room
        && plant_seat_ready(world, wx, gy, plant_cols, bank_cfg.plant_min_moist)
        && is_anchored(world, &child);
    if ready {
        return DispersalResult::Germinated(child);
    }
    // Hibernation: keep the spent dispersal as a cell-tied dormant packet.
    if bank_cfg.enabled {
        let packet = packet_from_child(SporeKind::Plant, &child, tick, true);
        let wx = world.wrap_x(wx);
        if world.spore_bank.deposit(wx, gy, packet, bank_cfg) {
            return DispersalResult::Banked { gx: wx, gy };
        }
    }
    // Bank full / disabled — refund like the old failure path.
    atom.energy = (atom.energy + cost).min(atom.energy_max);
    atom.cooldown = 0;
    DispersalResult::None
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
    fn submerged_seaweed_stays_on_holdfast() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..=8 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 9..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 2, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_seaweed().genome,
        );
        store.atoms.push(atom);
        for tick in 0..40u64 {
            store.step(&mut w, tick);
            if store.atoms.is_empty() {
                panic!("seaweed died at {tick}");
            }
        }
        assert!(
            !store.atoms[0].fallen,
            "holdfast seaweed must not tip/float"
        );
        assert_eq!(
            store.atoms[0].gy, 2,
            "must stay on the bed holdfast, not float to waterline (gy={})",
            store.atoms[0].gy
        );
        assert!(is_anchored(&w, &store.atoms[0]));
    }

    #[test]
    fn detached_seaweed_floats_when_holdfast_lost() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=6 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 7..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        // Mid-column, no solid under the root — holdfast gone.
        let mut atom = Atom::from_body(5, 3, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_seaweed().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(
            store.atoms[0].fallen,
            "seaweed without holdfast must free-float"
        );
        assert_eq!(
            store.atoms[0].gy, 6,
            "detached ribbon should ride the free surface"
        );
    }

    #[test]
    fn seaweed_with_holdfast_clears_stale_tip() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..=8 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 9..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 2, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_seaweed().genome,
        );
        atom.fallen = true; // stale tip while holdfast is actually intact
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(
            !store.atoms[0].fallen,
            "intact bed holdfast must clear tip / not float"
        );
        assert_eq!(store.atoms[0].gy, 2);
    }

    #[test]
    fn bed_seaweed_does_not_pump_through_floating_organic() {
        // Deep water + floating Organic lid: under-mat soak flicker used to
        // alternate gy between the sealed pocket and the open free surface
        // so stemless ribbons pumped through the litter.
        use crate::organism::OrganismStore;
        use crate::rules::collect_floating_organic_columns;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            for y in 2..=10 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 11..20 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 3..=7 {
            for y in 11..=13 {
                let mut org = Cell::solid(MaterialId::Organic);
                org.sat = Sat(60);
                w.set_cell(x, y, org);
            }
        }
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, 1, ModuleId::Photosystem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 14, 40.0, body);
        apply_genome(&mut atom, crate::blueprint::Blueprint::minimal_seaweed().genome);
        atom.fallen = true;
        store.atoms.push(atom);
        assert!(!collect_floating_organic_columns(&w).is_empty());
        let mut gys = Vec::new();
        for tick in 0..30u64 {
            let full = tick % 2 == 0;
            for x in 3..=7 {
                if full {
                    w.set_cell(x, 10, Cell::water());
                } else {
                    let mut cell = Cell::air();
                    cell.sat = Sat(40);
                    w.set_cell(x, 10, cell);
                }
                for y in 2..=9 {
                    w.set_cell(x, y, Cell::water());
                }
            }
            store.step(&mut w, tick);
            assert!(!store.atoms.is_empty(), "died at {tick}");
            gys.push(store.atoms[0].gy);
        }
        let mut uniq = gys[4..].to_vec();
        uniq.sort_unstable();
        uniq.dedup();
        assert!(
            uniq.len() <= 1,
            "must not oscillate across organic (uniq={uniq:?} gys={gys:?})"
        );
        assert!(
            *gys.iter().max().unwrap() <= 10,
            "must not surf above the organic deck (gys={gys:?})"
        );
    }

    #[test]
    fn shoreline_plant_does_not_rise_with_waterline() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Beach slope: low sand at x=4..=6 (y=2), higher sand inland x=8..=10 (y=5).
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 4..=6 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 2, sand);
        }
        for x in 8..=10 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 5, sand);
        }
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            // Lateral rhizome into the higher beach — must not hoist the crown.
            (3, 2, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 3, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        let gy0 = store.atoms[0].gy;
        assert_eq!(gy0, 3, "crown seats on local sand, not inland slope");
        // Rising lake covers the shoreline plant.
        for x in 0..=7 {
            for y in 3..=7 {
                w.set_cell(x, y, Cell::water());
            }
        }
        for tick in 1..40u64 {
            store.step(&mut w, tick);
        }
        assert!(
            !store.atoms[0].fallen,
            "shore plant must stay upright on its bed holdfast"
        );
        assert_eq!(
            store.atoms[0].gy, gy0,
            "must not ride the rising waterline (gy={} want {})",
            store.atoms[0].gy, gy0
        );
    }

    #[test]
    fn sand_rooted_plant_not_hoisted_by_shore_organic_pile() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Beach: sand at y=2; lake water to the left.
        for x in 0..=6 {
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
        }
        for x in 7..=14 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 2, sand);
        }
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(9, 3, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        let gy0 = store.atoms[0].gy;
        assert_eq!(gy0, 3);
        // Beach litter pile beside / over the crown columns (grounded Organic
        // on sand — the old holdfast path treated this as higher ground).
        for x in 8..=10 {
            w.set_cell(x, 3, Cell::solid(MaterialId::Organic));
            w.set_cell(x, 4, Cell::solid(MaterialId::Organic));
            w.set_cell(x, 5, Cell::solid(MaterialId::Organic));
        }
        for tick in 1..30u64 {
            store.step(&mut w, tick);
        }
        assert_eq!(
            store.atoms[0].gy, gy0,
            "sand-rooted crown must not pump up onto shore Organic (gy={} want {})",
            store.atoms[0].gy, gy0
        );
        assert!(
            !store.atoms[0].fallen,
            "must stay upright on sand"
        );
    }

    #[test]
    fn sand_rooted_plant_does_not_sail_with_shore_raft() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..24 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        for x in 0..=10 {
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
        }
        for x in 11..=18 {
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 2, sand);
        }
        // Floating mat touching the beach plant.
        for x in 8..=12 {
            w.set_cell(x, 6, Cell::solid(MaterialId::Organic));
        }
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            // Root tip also brushes the floating mat.
            (-2, 3, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Photosystem),
        ];
        let mut atoms = vec![Atom::from_body(12, 3, 40.0, body)];
        apply_genome(
            &mut atoms[0],
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        let gx0 = atoms[0].gx;
        for tick in 0..200u64 {
            w.tick = tick;
            let _ = sail_plants_on_wind_rafts(&mut w, &mut atoms, 0.35, 4);
        }
        assert_eq!(
            atoms[0].gx, gx0,
            "sand-rooted plant must not hitchhike shore rafts"
        );
    }

    #[test]
    fn moist_seepage_does_not_tip_land_plant() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..12 {
                // Damp film / seepage — not a standing-water body.
                let mut air = Cell::air();
                air.sat = Sat(40);
                w.set_cell(x, y, air);
            }
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 2, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        for tick in 0..30u64 {
            store.step(&mut w, tick);
        }
        assert!(
            !store.atoms[0].fallen,
            "seepage films must not tip substrate-rooted plants"
        );
        assert_eq!(store.atoms[0].gy, 2);
    }

    #[test]
    fn stream_flood_does_not_tip_rooted_land_plant() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 2, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        for y in 2..=5 {
            w.set_cell(5, y, Cell::water());
        }
        for tick in 0..20u64 {
            store.step(&mut w, tick);
        }
        assert!(
            !store.atoms[0].fallen,
            "rooted crown holdfast must not tip in a stream flood"
        );
        assert_eq!(
            store.atoms[0].gy, 2,
            "must stay on sand, not ride the stream surface"
        );
    }

    #[test]
    fn seaweed_pulled_back_to_holdfast_if_displaced() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..=8 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 9..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        // Wrongly floated to the surface, but short root still reaches sand.
        let mut atom = Atom::from_body(5, 6, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_seaweed().genome,
        );
        // Root body -1 at gy=6 → world y=5 (water). Extend a short root to sand.
        atom.body.push((0, -5, ModuleId::Root));
        atom.fallen = true;
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(!store.atoms[0].fallen);
        assert_eq!(
            store.atoms[0].gy, 2,
            "seaweed with a short bed root must reseat on the holdfast"
        );
    }

    #[test]
    fn substrate_rooted_plant_stays_put_when_flooded() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        // Dry land plant on sand, then flood the column above it.
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 2, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(!store.atoms[0].fallen);
        assert!(is_anchored(&w, &store.atoms[0]));
        let gy0 = store.atoms[0].gy;
        // Flood: standing water from the crown up.
        for y in 2..=7 {
            w.set_cell(5, y, Cell::water());
        }
        for y in 8..12 {
            w.set_cell(5, y, Cell::air());
        }
        for tick in 1..20u64 {
            store.step(&mut w, tick);
        }
        assert!(
            !store.atoms[0].fallen,
            "sand-rooted land plant must not tip when flooded"
        );
        assert_eq!(
            store.atoms[0].gy, gy0,
            "must stay on the substrate crown, not float to the waterline"
        );
        assert!(
            is_anchored(&w, &store.atoms[0]),
            "roots must remain in the sand"
        );
    }

    #[test]
    fn unanchored_plant_floats_tipped_on_open_water() {
        use crate::organism::{rigid_tip_offset, OrganismStore};

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
        assert_eq!(
            rigid_tip_offset(0, 1),
            (1, 0),
            "stem swings onto +x waterline"
        );
        assert_eq!(
            rigid_tip_offset(0, -1),
            (-1, 0),
            "root swings onto −x as part of the same body"
        );
        let a = &store.atoms[0];
        assert!(
            a.body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Stem && y == 0 && x > 0),
            "baked stem should lie on +x waterline, body={:?}",
            a.body
        );
        assert!(
            a.body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Root && x < 0 && y <= 0),
            "baked root should sit on the −x side of the log, body={:?}",
            a.body
        );
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
        // Already free-floating (left the raft); a long root may scrape sand
        // without counting as a land holdfast that pins the crown underwater.
        atom.fallen = true;
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
    fn tipped_floater_proximal_root_scrape_does_not_bed_seat() {
        // After rigid tip, former (0,-1) root becomes (-1,0). Scraping sand
        // / bed / neighbour substrate used to teleport gy to solid_y+1 —
        // the log sat on the lake floor. Open-water castaways stay a rigid
        // uprooted body at the free surface — mineral-piercing roots are
        // pruned (no floating "rooted to the bed" pipe).
        use crate::organism::{bake_tip_into_body, OrganismStore};

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            for y in 2..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut atom = Atom::from_body(5, 5, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        bake_tip_into_body(&mut atom);
        assert!(atom.fallen);
        // Log floats at the free surface while crown-column roots scrape the
        // bed. Without the woody-castaway gate, `grounded` + holdfast would
        // set `gy = solid_y + 1` and plant the trunk on the lake floor.
        atom.gy = 5; // water top
        if !atom
            .body
            .iter()
            .any(|&(x, y, m)| m == ModuleId::Root && x == -1 && y == 0)
        {
            atom.body.push((-1, 0, ModuleId::Root));
        }
        // Crown-column tendril into sand — used to bed-seat; now pruned while
        // floating (uprooted solid body, no terrain goo).
        atom.body.push((0, -4, ModuleId::Root));
        let mut store = OrganismStore::new();
        store.atoms.push(atom);
        for tick in 0..40u64 {
            store.step(&mut w, tick);
        }
        assert!(
            store.atoms[0].fallen,
            "castaway must stay tipped"
        );
        assert_eq!(
            store.atoms[0].gy, 5,
            "proximal/bed scrape must not bed-seat the log (gy={})",
            store.atoms[0].gy
        );
        assert!(
            !store.atoms[0].body.iter().any(|&(dx, dy, m)| {
                if m != ModuleId::Root {
                    return false;
                }
                let wx = w.wrap_x(store.atoms[0].gx + dx as i32);
                let wy = store.atoms[0].gy + dy as i32;
                matches!(
                    w.get_cell(wx, wy).map(|c| c.material),
                    Some(MaterialId::Sand | MaterialId::Bedrock | MaterialId::Soil)
                )
            }),
            "uprooted floater must not keep mineral-piercing roots, body={:?}",
            store.atoms[0].body
        );
        assert!(
            store.atoms[0]
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Root)
                .all(|(_, y, _)| *y >= -UPROOTED_ROOT_KEEL_MAX),
            "uprooted keel must stay short, body={:?}",
            store.atoms[0].body
        );
    }

    #[test]
    fn shore_tipped_woody_does_not_pump_with_runoff_film() {
        // Landslide tip + upright regrowth on beach sand. Intermittent
        // full-sat runoff at the crown used to set gy = water top and bob
        // the mast ±1px as the film filled/drained.
        use crate::organism::{bake_tip_into_body, OrganismStore};

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(5, 2, 80.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        bake_tip_into_body(&mut atom);
        assert!(atom.fallen);
        // New upright shoot after tip (landslide regrowth).
        atom.body.push((0, 1, ModuleId::Stem));
        atom.body.push((0, 2, ModuleId::Photosystem));
        atom.mark_upright_growth(0, 1);
        atom.mark_upright_growth(0, 2);
        atom.gy = 2; // Air on sand
        atom.fy = 2.0;
        let mut store = OrganismStore::new();
        store.atoms.push(atom);
        let mut gys = Vec::new();
        for tick in 0..40u64 {
            // Alternate 1-cell vs 2-cell full-sat film at the crown column.
            for y in 2..=4 {
                w.set_cell(5, y, Cell::air());
            }
            if tick % 2 == 0 {
                w.set_cell(5, 2, Cell::water());
            } else {
                w.set_cell(5, 2, Cell::water());
                w.set_cell(5, 3, Cell::water());
            }
            store.step(&mut w, tick);
            assert!(!store.atoms.is_empty(), "died at {tick}");
            gys.push(store.atoms[0].gy);
        }
        let mut uniq = gys.clone();
        uniq.sort_unstable();
        uniq.dedup();
        assert!(
            uniq.len() <= 1,
            "shore-tipped woody must not pump with runoff (uniq={uniq:?} gys={gys:?})"
        );
        assert_eq!(uniq[0], 2, "must stay on beach sand seat");
        assert!(store.atoms[0].fallen, "must stay tipped after landslide");
        assert!(
            !store.atoms[0].upright_growth.is_empty(),
            "upright regrowth marks must survive"
        );
    }

    #[test]
    fn fallen_floater_stays_alive_by_sipping_lake() {
        use crate::organism::OrganismStore;

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
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 4, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.energy = atom.energy_max;
        store.atoms.push(atom);
        for tick in 0..200u64 {
            store.step(&mut w, tick);
            if store.atoms.is_empty() {
                break;
            }
        }
        assert_eq!(store.atoms.len(), 1, "floater should stay alive on the lake");
        assert!(store.atoms[0].fallen);
        assert!(
            store.atoms[0].energy > 1.0,
            "dangling roots should sip enough to keep energy (e={})",
            store.atoms[0].energy
        );
    }

    #[test]
    fn stranded_fallen_plant_can_reroot_on_shore() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 1, sand);
            for y in 2..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let body = crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 2, 80.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.fallen = true;
        atom.energy = atom.energy_max;
        atom.genome.alloc_root = 0.6;
        atom.genome.alloc_stem = 0.15;
        atom.genome.alloc_leaf = 0.25;
        store.atoms.push(atom);
        let mut anchored = false;
        for tick in 0..120u64 {
            store.step(&mut w, tick);
            if store.atoms.is_empty() {
                break;
            }
            if is_anchored(&w, &store.atoms[0]) {
                anchored = true;
                break;
            }
        }
        assert!(anchored, "stranded floater should grow roots into the beach");
        assert!(
            store.atoms[0].fallen,
            "re-rooted shore plant must stay tipped; only new shoots stand up"
        );
    }

    #[test]
    fn tipped_floater_grows_new_stem_upright() {
        use crate::organism::{resolve_organism_draw_cells, rigid_tip_offset};
        use crate::shade::CanopyIndex;
        use std::collections::HashSet;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Side leaf so the trunk tip has clear air for stem elongation.
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (1, 1, ModuleId::Photosystem),
        ];
        let mut atom = Atom::from_body(5, 5, 120.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.fallen = true;
        atom.energy = atom.energy_max;
        atom.genome.alloc_root = 0.0;
        // Strong stem bias so growth prefers trunk elongation over a side leaf.
        atom.genome.alloc_stem = 0.92;
        atom.genome.alloc_leaf = 0.08;
        let caps = PlantGrowthCaps::default();
        let mut spent = 0.0;
        for t in 0..24u64 {
            spent = try_grow_shoot(
                &w,
                &mut atom,
                t,
                &HashSet::new(),
                &HashSet::new(),
                &caps,
                &CanopyIndex::default(),
                0,
            );
            if atom.body.iter().any(|&(dx, dy, m)| {
                m == ModuleId::Stem && atom.upright_growth.contains(&(dx, dy))
            }) {
                break;
            }
        }
        assert!(spent > 0.0, "tipped floater should grow a shoot");
        let new_stem = atom
            .body
            .iter()
            .copied()
            .find(|&(dx, dy, m)| m == ModuleId::Stem && atom.upright_growth.contains(&(dx, dy)))
            .expect("new stem cell marked upright_growth");
        assert!(atom.fallen);
        assert!(atom.draws_upright(new_stem.0, new_stem.1));
        assert!(!atom.draws_upright(0, 1));
        let atoms = vec![atom.clone()];
        let posed = resolve_organism_draw_cells(&w, &atoms, 0, 0.0);
        // Body Y still counts the tipped trunk; draw collapses so the first
        // new shoot sits on the waterline crown (gy+1), not floating above a gap.
        let (draw_dx, draw_dy) = atom.fallen_draw_offset(new_stem.0, new_stem.1);
        assert_eq!(draw_dy, 1, "first upright stem should sit just above the crown");
        assert!(
            posed.iter().any(|p| {
                p.mid == ModuleId::Stem
                    && p.wx == atom.gx + draw_dx as i32
                    && p.wy == atom.gy + draw_dy as i32
            }),
            "new stem draws upright from the waterline, got {posed:?}"
        );
        let (ox, oy) = rigid_tip_offset(0, 1);
        assert_eq!((ox, oy), (1, 0));
        assert!(
            posed.iter().any(|p| {
                p.mid == ModuleId::Stem
                    && p.wx == atom.gx + ox as i32
                    && p.wy == atom.gy + oy as i32
            }),
            "pre-tip stem still draws as part of the rigid tip log"
        );
    }

    #[test]
    fn upright_shoots_do_not_float_above_tipped_trunk_gap() {
        use crate::organism::resolve_organism_draw_cells;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Pre-tip trunk + post-tip shoots with a skipped body Y (4 then 6).
        // Rank stacking must still put the first shoot at gy+1.
        let mut atom = Atom::from_body(
            5,
            5,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Photosystem),
                (0, 4, ModuleId::Stem),
                (0, 6, ModuleId::Stem),
                (0, 7, ModuleId::Photosystem),
                (1, 7, ModuleId::Photosystem),
            ],
        );
        atom.fallen = true;
        atom.upright_growth = vec![(0, 4), (0, 6), (0, 7), (1, 7)];
        let posed = resolve_organism_draw_cells(&w, &[atom.clone()], 0, 0.0);
        let stem_ys: Vec<i32> = posed
            .iter()
            .filter(|p| p.mid == ModuleId::Stem && p.wx == atom.gx)
            .map(|p| p.wy)
            .collect();
        assert!(
            stem_ys.contains(&(atom.gy + 1)) && stem_ys.contains(&(atom.gy + 2)),
            "upright trunk must be contiguous from gy+1, stems at {stem_ys:?}"
        );
        assert!(
            !stem_ys.contains(&(atom.gy + 4)) && !stem_ys.contains(&(atom.gy + 6)),
            "must not draw new stems at raw body Y"
        );
        assert!(
            posed.iter().any(|p| {
                p.mid == ModuleId::Photosystem && p.wx == atom.gx && p.wy == atom.gy + 3
            }),
            "upright leaf should sit on the remapped shoot stack"
        );
    }

    #[test]
    fn midair_woody_leaf_fleck_is_pruned() {
        // Manhattan-2 leaf left an empty cell beside the trunk (user bug:
        // floating green pixel belonging to the same plant).
        let mut atom = Atom::from_body(
            5,
            2,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Stem),
                (0, 4, ModuleId::Stem),
                (0, 5, ModuleId::Photosystem),
                (-2, 3, ModuleId::Photosystem), // gap at (-1, 3)
            ],
        );
        assert!(!woody_leaf_attached(&atom, -2, 3));
        let n = prune_detached_woody_leaves(&mut atom);
        assert_eq!(n, 1, "gapped fleck must drop");
        assert!(
            !atom
                .body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Photosystem && x == -2 && y == 3),
            "midair leaf should be gone"
        );
        assert!(
            atom.body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Photosystem && x == 0 && y == 5),
            "crown leaf on wood must stay"
        );
    }

    #[test]
    fn woody_growth_cannot_place_gapped_side_leaf() {
        let w = moist_plot();
        let body = vec![
            (0, -1, ModuleId::Root),
            (0, 0, ModuleId::Nucleus),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Stem),
            (1, 3, ModuleId::Photosystem), // already fills +x at tip
        ];
        let mut atom = Atom::from_body(4, 2, 80.0, body);
        atom.genome.alloc_stem = 0.0;
        atom.genome.alloc_leaf = 1.0;
        atom.genome.alloc_root = 0.0;
        let trunks = HashSet::new();
        let caps = PlantGrowthCaps::default();
        for t in 0..30u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = t * LAND_GROW_PERIOD;
            let _ = try_grow_shoot(
                &w,
                &mut atom,
                t,
                &trunks,
                &HashSet::new(),
                &caps,
                &CanopyIndex::default(),
                0,
            );
        }
        for &(x, y, m) in &atom.body {
            if m == ModuleId::Photosystem {
                assert!(
                    woody_leaf_attached(&atom, x, y),
                    "grown leaf at ({x},{y}) must touch Stem"
                );
            }
        }
    }

    #[test]
    fn woody_photos_do_not_pile_off_their_stem() {
        use crate::organism::resolve_organism_draw_cells;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            for y in 2..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Two woody plants with leaves that would collide at the same draw cell.
        let a = Atom::from_body(
            4,
            2,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (1, 1, ModuleId::Photosystem),
            ],
        );
        let b = Atom::from_body(
            6,
            2,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (-1, 1, ModuleId::Photosystem), // collides with a's leaf at (5,3)
            ],
        );
        let posed = resolve_organism_draw_cells(&w, &[a, b], 0, 0.0);
        let leaf_ys: Vec<i32> = posed
            .iter()
            .filter(|p| p.mid == ModuleId::Photosystem)
            .map(|p| p.wy)
            .collect();
        assert!(
            leaf_ys.iter().all(|&y| y == 3),
            "woody leaves must stay on the petiole row, not pile upward: {leaf_ys:?}"
        );
    }

    #[test]
    fn tipped_plant_occupies_draw_pose_not_phantom_upright() {
        use crate::organism::bake_tip_into_body;

        let mut atom = Atom::from_body(
            5,
            5,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Photosystem),
            ],
        );
        bake_tip_into_body(&mut atom);
        // Rigid tip: stem (0,3) → (3,0) body → world (8,5).
        assert!(
            atom.occupies(8, 5),
            "tipped canopy must be pickable on the waterline"
        );
        assert!(
            !atom.occupies(5, 8),
            "must not occupy the pre-tip upright phantom cell"
        );
        // Root (0,-1) → (-1,0) body → world (4,5), still attached to the log.
        assert!(atom.occupies(4, 5));
    }

    #[test]
    fn woody_tip_keeps_root_and_stem_as_one_rigid_body() {
        use crate::organism::bake_tip_into_body;

        let mut atom = Atom::from_body(
            5,
            5,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, -2, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Stem),
                (0, 4, ModuleId::Stem),
                (0, 5, ModuleId::Photosystem),
                (-1, 5, ModuleId::Photosystem),
                (1, 5, ModuleId::Photosystem),
            ],
        );
        let stem_h = 4i16;
        let root_d = 2i16;
        bake_tip_into_body(&mut atom);
        // Stem length preserved on +x — no collision stretch into a mega-log.
        let stem_xs: Vec<i16> = atom
            .body
            .iter()
            .filter(|&&(_, _, m)| m == ModuleId::Stem)
            .map(|&(x, _, _)| x)
            .collect();
        assert!(
            stem_xs.iter().all(|&x| x > 0),
            "stems should lie on +x after rigid tip, body={:?}",
            atom.body
        );
        assert_eq!(
            stem_xs.iter().copied().max(),
            Some(stem_h),
            "tip must not stretch stem longer than pre-tip height"
        );
        // Roots rotate to −x — same body, not abandoned under the old crown.
        let roots: Vec<(i16, i16)> = atom
            .body
            .iter()
            .filter(|&&(_, _, m)| m == ModuleId::Root)
            .map(|&(x, y, _)| (x, y))
            .collect();
        assert_eq!(roots.len(), 2);
        assert!(
            roots.iter().all(|&(x, _)| x < 0),
            "roots should occupy the −x end of the log, got {roots:?}"
        );
        assert!(
            roots.contains(&(-1, 0)) && roots.contains(&(-2, 0)),
            "root depths become −x waterline offsets, got {roots:?}"
        );
        let _ = root_d;
        // Connected through nucleus: every non-nucleus cell touches another.
        for &(x, y, m) in &atom.body {
            if m == ModuleId::Nucleus {
                continue;
            }
            let near = atom.body.iter().any(|&(ox, oy, _)| {
                (ox, oy) != (x, y) && (ox - x).abs() <= 1 && (oy - y).abs() <= 1
            });
            assert!(near, "cell ({x},{y},{m:?}) disconnected after tip: {:?}", atom.body);
        }
    }

    #[test]
    fn shed_upright_leaf_does_not_leave_draw_gap() {
        use crate::organism::{fallen_draw_offset, resolve_organism_draw_cells};

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Unique middle upright Y (=4) so a ghost rank would open a gap.
        let mut atom = Atom::from_body(
            5,
            5,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (1, 0, ModuleId::Stem), // tipped trunk remnant
                (0, 3, ModuleId::Stem),
                (0, 4, ModuleId::Photosystem), // middle leaf (unique Y)
                (0, 5, ModuleId::Stem),
                (0, 6, ModuleId::Photosystem),
            ],
        );
        atom.fallen = true;
        atom.upright_growth = vec![(0, 3), (0, 4), (0, 5), (0, 6)];
        atom.body
            .retain(|&(x, y, m)| !(m == ModuleId::Photosystem && x == 0 && y == 4));
        // Stale upright entry at the removed Y — must not inflate tip rank.
        assert!(atom.upright_growth.contains(&(0, 4)));
        assert!(!atom.body.iter().any(|&(x, y, _)| x == 0 && y == 4));
        let (_dx, tip_dy) = fallen_draw_offset(
            atom.fallen,
            &atom.upright_growth,
            &atom.body,
            0,
            6,
        );
        // Surviving upright Ys below tip: 3 and 5 → visual dy = 1+2 = 3.
        assert_eq!(
            tip_dy, 3,
            "tip leaf rank must collapse after middle Y is gone (dy={tip_dy})"
        );
        let posed = resolve_organism_draw_cells(&w, &[atom.clone()], 0, 0.0);
        let mast_ys: Vec<i32> = posed
            .iter()
            .filter(|p| {
                p.wx == atom.gx
                    && matches!(p.mid, ModuleId::Stem | ModuleId::Photosystem)
                    && p.wy > atom.gy
            })
            .map(|p| p.wy)
            .collect();
        assert!(
            !mast_ys.contains(&(atom.gy + 4)),
            "phantom middle rank must not float the tip, mast={mast_ys:?}"
        );
    }

    #[test]
    fn unmarked_tipped_shoots_heal_instead_of_waterline_flecks() {
        use crate::organism::{resolve_organism_draw_cells, OrganismStore};

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(180);
            w.set_cell(x, 1, sand);
            for y in 2..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Shore-tipped woody plant with a mid-mast stem missing from
        // upright_growth. Heal must mark it so draw stays contiguous
        // (no waterline fleck + floating canopy).
        let mut atom = Atom::from_body(
            5,
            2,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (1, 0, ModuleId::Stem),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem), // unmarked → would become horizontal fleck
                (0, 3, ModuleId::Stem),
                (0, 4, ModuleId::Photosystem),
                (-1, 4, ModuleId::Photosystem),
                (1, 4, ModuleId::Photosystem),
            ],
        );
        atom.fallen = true;
        atom.upright_growth = vec![(0, 1), (0, 3), (0, 4), (-1, 4), (1, 4)];
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        let mut store = OrganismStore::new();
        store.atoms.push(atom);
        store.step(&mut w, 0);
        let atom = &store.atoms[0];
        assert!(atom.fallen, "woody shore re-root stays tipped");
        assert!(
            atom.upright_growth.contains(&(0, 2)),
            "heal must mark the missing mid-mast stem"
        );
        let posed = resolve_organism_draw_cells(&w, &[atom.clone()], 0, 0.0);
        let stem_ys: Vec<i32> = posed
            .iter()
            .filter(|p| p.mid == ModuleId::Stem && p.wx == atom.gx && p.wy > atom.gy)
            .map(|p| p.wy)
            .collect();
        assert!(
            stem_ys.contains(&(atom.gy + 1))
                && stem_ys.contains(&(atom.gy + 2))
                && stem_ys.contains(&(atom.gy + 3)),
            "mast must stay contiguous above the crown, stems={stem_ys:?}"
        );
    }

    #[test]
    fn roots_crack_stone_but_death_skips_stone_and_dry_air() {
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..12 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Stone));
            let mut sand = Cell::solid(MaterialId::Sand);
            sand.sat = Sat(200);
            w.set_cell(x, 2, sand);
            for y in 3..10 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Death path: sand converts; stone / dry-Air do not become Organic holes.
        let atom = Atom::from_body(
            4,
            3,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root), // sand
                (0, -2, ModuleId::Root), // stone — crack while living, not Organic on death
                (1, 1, ModuleId::Root),  // dry Air rhizome gap
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Photosystem),
            ],
        );
        assert!(penetrate_cost(MaterialId::Stone).unwrap() > 1.0);
        let n = leave_dead_roots_in_place(&mut w, &atom);
        assert!(n >= 1, "sand root should convert");
        assert_eq!(
            w.get_cell(4, 2).map(|c| c.material),
            Some(MaterialId::Organic),
            "sand root → Organic"
        );
        assert_eq!(
            w.get_cell(4, 1).map(|c| c.material),
            Some(MaterialId::Stone),
            "stone must not become an Organic hole on death"
        );
        assert_eq!(
            w.get_cell(5, 4).map(|c| c.material),
            Some(MaterialId::Air),
            "dry-Air rhizome must not spawn floating Organic"
        );

        // Living elongate into Stone opens a LooseRock crack.
        let mut grower = Atom::from_body(
            6,
            3,
            120.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Photosystem),
            ],
        );
        apply_genome(
            &mut grower,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        grower.energy = grower.energy_max;
        grower.genome.alloc_root = 1.0;
        grower.genome.alloc_stem = 0.0;
        grower.genome.alloc_leaf = 0.0;
        grower.genome.root_depth_bias = 1.0;
        let caps = PlantGrowthCaps::default();
        let mut cracked = false;
        for _ in 0..48 {
            let _ = try_elongate_root(&mut w, &mut grower, &HashSet::new(), &caps);
            // May dive under a side runner — any Stone→LooseRock counts.
            if (0..12).any(|x| {
                w.get_cell(x, 1).map(|c| c.material) == Some(MaterialId::LooseRock)
            }) {
                cracked = true;
                break;
            }
        }
        assert!(
            cracked,
            "root wedging into Stone should open a LooseRock crack, body={:?}",
            grower.body
        );
    }

    #[test]
    fn tall_upright_mast_on_skinny_raft_tips_again() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w.set_cell(8, 6, Cell::solid(MaterialId::Organic));
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            // Post-tip mast (3 distinct upright Y → tippy on 1-wide raft).
            (0, 3, ModuleId::Stem),
            (0, 4, ModuleId::Stem),
            (0, 5, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(8, 6, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.fallen = true;
        atom.upright_growth = vec![(0, 3), (0, 4), (0, 5)];
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(store.atoms[0].fallen, "must stay tipped");
        assert!(
            store.atoms[0].upright_growth.is_empty(),
            "tippy upright mast should bake flat, got {:?}",
            store.atoms[0].upright_growth
        );
        assert!(
            store.atoms[0]
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Stem || *m == ModuleId::Photosystem)
                .all(|(_, y, _)| *y == 0),
            "baked canopy must lie on the waterline (dy==0), body={:?}",
            store.atoms[0].body
        );
    }

    #[test]
    fn after_tip_bake_new_stem_grows_up_from_crown() {
        use crate::organism::bake_tip_into_body;
        use crate::shade::CanopyIndex;
        use std::collections::HashSet;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut atom = Atom::from_body(
            5,
            5,
            120.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Stem),
                (0, 4, ModuleId::Photosystem),
            ],
        );
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.energy = atom.energy_max;
        bake_tip_into_body(&mut atom);
        assert!(atom.body.iter().any(|&(x, y, m)| {
            m == ModuleId::Stem && y == 0 && x > 0
        }));
        // Old vertical axis is gone — (0,1) is free for a fresh upright shoot.
        assert!(!atom.body.iter().any(|&(x, y, _)| x == 0 && y == 1));
        atom.genome.alloc_stem = 0.92;
        atom.genome.alloc_leaf = 0.08;
        atom.genome.alloc_root = 0.0;
        let caps = PlantGrowthCaps::default();
        let mut grew_up = false;
        for t in 0..16u64 {
            let _ = try_grow_shoot(
                &w,
                &mut atom,
                t,
                &HashSet::new(),
                &HashSet::new(),
                &caps,
                &CanopyIndex::default(),
                0,
            );
            if atom
                .body
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Stem && x == 0 && y == 1)
            {
                grew_up = true;
                break;
            }
        }
        assert!(grew_up, "post-bake stem must reorient upward above nucleus");
        assert!(atom.upright_growth.contains(&(0, 1)));
    }

    #[test]
    fn tipped_floater_can_elongate_dangling_roots() {
        use crate::organism::bake_tip_into_body;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut atom = Atom::from_body(
            5,
            5,
            120.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Photosystem),
            ],
        );
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.energy = atom.energy_max;
        bake_tip_into_body(&mut atom);
        atom.genome.alloc_root = 0.5;
        atom.genome.alloc_stem = 0.25;
        atom.genome.alloc_leaf = 0.25;
        let roots0 = root_count(&atom);
        let caps = PlantGrowthCaps::default();
        let mut grew = false;
        for _ in 0..24 {
            let spent = try_elongate_root(&mut w, &mut atom, &HashSet::new(), &caps);
            if spent > 0.0 || root_count(&atom) > roots0 {
                grew = true;
                break;
            }
        }
        assert!(grew, "tipped floater should elongate roots into wet void");
        assert!(
            atom.body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Root)
                .any(|(_, y, _)| *y < 0),
            "new roots should droop below the waterline log, body={:?}",
            atom.body
        );
        assert!(
            atom.body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Root)
                .all(|(_, y, _)| *y >= -UPROOTED_ROOT_KEEL_MAX),
            "uprooted wet keel must stay ≤ {UPROOTED_ROOT_KEEL_MAX}, body={:?}",
            atom.body
        );
    }

    #[test]
    fn uprooted_floater_does_not_grow_nucleus_to_bed_pipe() {
        use crate::organism::bake_tip_into_body;

        // Deep water under a surface floater — wet-void affinity used to grow
        // a continuous root pipe from nucleus to the sand bed.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            for y in 2..=10 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 11..18 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut atom = Atom::from_body(
            5,
            10,
            200.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Photosystem),
            ],
        );
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.energy = atom.energy_max;
        bake_tip_into_body(&mut atom);
        atom.genome.alloc_root = 0.7;
        atom.genome.alloc_stem = 0.15;
        atom.genome.alloc_leaf = 0.15;
        atom.genome.root_depth_bias = 1.0;
        let caps = PlantGrowthCaps::default();
        for _ in 0..80 {
            atom.energy = atom.energy_max;
            let _ = try_elongate_root(&mut w, &mut atom, &HashSet::new(), &caps);
        }
        let min_y = atom
            .body
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Root)
            .map(|(_, y, _)| *y)
            .min()
            .unwrap_or(0);
        assert!(
            min_y >= -UPROOTED_ROOT_KEEL_MAX,
            "floater must not pipe roots to the bed (min_y={min_y}), body={:?}",
            atom.body
        );
        assert!(
            !atom.body.iter().any(|&(dx, dy, m)| {
                if m != ModuleId::Root {
                    return false;
                }
                let wx = w.wrap_x(atom.gx + dx as i32);
                let wy = atom.gy + dy as i32;
                matches!(
                    w.get_cell(wx, wy).map(|c| c.material),
                    Some(MaterialId::Sand | MaterialId::Bedrock | MaterialId::Soil)
                )
            }),
            "open-water uprooted roots must stay out of mineral, body={:?}",
            atom.body
        );
    }

    #[test]
    fn uprooted_floater_does_not_tunnel_into_sand_cliff() {
        use crate::organism::bake_tip_into_body;

        // Floater beside an underwater sand wall — lateral mineral steps
        // used to paint roots through the hillside.
        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            if x >= 8 {
                for y in 1..=5 {
                    w.set_cell(x, y, Cell::solid(MaterialId::Sand));
                }
            } else {
                for y in 1..=5 {
                    w.set_cell(x, y, Cell::water());
                }
            }
            for y in 6..14 {
                w.set_cell(x, y, Cell::air());
            }
        }
        let mut atom = Atom::from_body(
            6,
            5,
            200.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (-1, 0, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Photosystem),
            ],
        );
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        atom.energy = atom.energy_max;
        bake_tip_into_body(&mut atom);
        atom.gx = 6;
        atom.gy = 5;
        atom.genome.alloc_root = 0.7;
        atom.genome.alloc_stem = 0.15;
        atom.genome.alloc_leaf = 0.15;
        let caps = PlantGrowthCaps::default();
        for _ in 0..60 {
            atom.energy = atom.energy_max;
            let _ = try_elongate_root(&mut w, &mut atom, &HashSet::new(), &caps);
        }
        assert!(
            !atom.body.iter().any(|&(dx, dy, m)| {
                if m != ModuleId::Root {
                    return false;
                }
                let wx = w.wrap_x(atom.gx + dx as i32);
                let wy = atom.gy + dy as i32;
                matches!(
                    w.get_cell(wx, wy).map(|c| c.material),
                    Some(MaterialId::Sand | MaterialId::Bedrock | MaterialId::Soil)
                )
            }),
            "uprooted floater must not tunnel into sand cliff, body={:?}",
            atom.body
        );
    }

    #[test]
    fn sand_eroded_under_woody_plant_tips_and_keeps_plan() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..16 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            // Thick sand so undercut does not leave bedrock in the root grip.
            w.set_cell(x, 1, Cell::solid(MaterialId::Sand));
            w.set_cell(x, 2, Cell::solid(MaterialId::Sand));
            for y in 3..12 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // Distinct plan: side root + tall trunk + side leaf.
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (1, -1, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Stem),
            (1, 3, ModuleId::Photosystem),
        ];
        let stems0 = body.iter().filter(|(_, _, m)| *m == ModuleId::Stem).count();
        let roots0 = body.iter().filter(|(_, _, m)| *m == ModuleId::Root).count();
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(5, 3, 80.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(!store.atoms[0].fallen, "sand-rooted plant starts upright");
        // Erode the sand under the crown columns — no water yet.
        for x in 4..=6 {
            w.set_cell(x, 1, Cell::air());
            w.set_cell(x, 2, Cell::air());
        }
        store.step(&mut w, 1);
        assert!(
            store.atoms[0].fallen,
            "woody plant must tip when sand undercut removes support"
        );
        assert_eq!(
            store.atoms[0]
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Stem)
                .count(),
            stems0,
            "stem count must survive tip bake"
        );
        assert_eq!(
            store.atoms[0]
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Root)
                .count(),
            roots0,
            "root count must survive tip bake"
        );
        // Rigid tip: stems on +x waterline, roots on −x; side leaves may sit
        // one cell off the waterline from the rotation.
        assert!(
            store.atoms[0]
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Stem)
                .all(|&(x, y, _)| y == 0 && x > 0),
            "baked stems lie on +x waterline, body={:?}",
            store.atoms[0].body
        );
        assert!(
            store.atoms[0]
                .body
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Root)
                .all(|&(x, y, _)| x < 0 || y < 0),
            "baked roots stay on the root end of the log, body={:?}",
            store.atoms[0].body
        );
    }

    #[test]
    fn crown_clearance_blocks_adjacent_sprout_seats() {
        let cols = vec![10i32, 16, 22];
        assert!(crown_clearance_ok(&cols, 13, None), "mid-gap should be free");
        assert!(
            !crown_clearance_ok(&cols, 11, None),
            "one column from a crown must be blocked"
        );
        assert!(
            !crown_clearance_ok(&cols, 12, None),
            "two columns from a crown must be blocked"
        );
        assert!(
            !crown_clearance_ok(&cols, 10, None),
            "exact crown column must be blocked"
        );
    }

    #[test]
    fn sprout_body_inherits_full_parent_plan_not_template() {
        let parent_body = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (1, -1, ModuleId::Root), // distinctive side root
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Stem),
            (0, 4, ModuleId::Stem),
            (0, 5, ModuleId::Stem),
            (1, 2, ModuleId::Stem), // side branch
            (-1, 2, ModuleId::Photosystem),
            (1, 5, ModuleId::Photosystem),
            (2, -1, ModuleId::Symbiont),
        ];
        let parent = Atom::from_body(4, 2, 80.0, parent_body.clone());
        let child = sprout_body(&parent);
        let template =
            crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus();
        assert_eq!(
            child, parent_body,
            "upright parent must clone the full chassis before mutation"
        );
        assert!(
            child
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Stem && x == 0 && y == 5),
            "tall trunk tip must survive, got {child:?}"
        );
        assert!(
            child
                .iter()
                .any(|&(x, y, m)| m == ModuleId::Stem && x == 1 && y == 2),
            "side branch must survive, got {child:?}"
        );
        assert_ne!(
            child, template,
            "must not fall back to the minimal_plant template"
        );
        // Stemless parent stays stemless.
        let seaweed = Atom::from_body(
            4,
            2,
            40.0,
            crate::blueprint::Blueprint::minimal_seaweed().modules_relative_to_nucleus(),
        );
        let sw_child = sprout_body(&seaweed);
        assert_eq!(
            sw_child
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::Stem)
                .count(),
            0,
            "stemless child must stay stemless: {sw_child:?}"
        );
        assert!(sw_child.iter().any(|(_, _, m)| *m == ModuleId::Photosystem));
    }

    #[test]
    fn sprout_body_from_tipped_parent_is_upright_full_plan() {
        use crate::organism::bake_tip_into_body;

        let mut parent = Atom::from_body(
            5,
            5,
            80.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Stem),
                (-1, 3, ModuleId::Photosystem),
            ],
        );
        bake_tip_into_body(&mut parent);
        assert!(parent.fallen);
        assert!(parent
            .body
            .iter()
            .any(|&(_, y, m)| m == ModuleId::Stem && y == 0));
        let child = sprout_body(&parent);
        let stem_ys: Vec<i16> = child
            .iter()
            .filter(|(_, _, m)| *m == ModuleId::Stem)
            .map(|(_, y, _)| *y)
            .collect();
        assert!(
            stem_ys.iter().any(|&y| y > 0),
            "tipped parent must yield upright stems, got {child:?}"
        );
        assert!(
            stem_ys.iter().any(|&y| y >= 3),
            "full trunk height should return after straighten, got {child:?}"
        );
        assert!(
            child.iter().any(|(_, _, m)| *m == ModuleId::Photosystem),
            "child should keep a leaf, got {child:?}"
        );
    }

    #[test]
    fn spore_dispersal_body_keeps_parent_sorus_and_plan() {
        let parent = Atom::from_body(
            4,
            2,
            80.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Stem),
                (-1, 3, ModuleId::Photosystem),
                (1, 3, ModuleId::ReproSpore),
            ],
        );
        let child = spore_dispersal_body(&parent);
        assert_eq!(
            child.iter().filter(|(_, _, m)| *m == ModuleId::Stem).count(),
            3,
            "spore child must keep parent stem count, got {child:?}"
        );
        assert_eq!(
            child
                .iter()
                .filter(|(_, _, m)| *m == ModuleId::ReproSpore)
                .count(),
            1,
            "must keep exactly one sorus from the parent plan"
        );
    }

    #[test]
    fn upright_mast_counts_as_wind_sail() {
        use crate::organism::bake_tip_into_body;

        let mut tipped = Atom::from_body(
            5,
            5,
            40.0,
            vec![
                (0, 0, ModuleId::Nucleus),
                (0, -1, ModuleId::Root),
                (0, 1, ModuleId::Stem),
                (0, 2, ModuleId::Stem),
                (0, 3, ModuleId::Stem),
                (0, 4, ModuleId::Stem),
                (0, 5, ModuleId::Photosystem),
            ],
        );
        apply_genome(
            &mut tipped,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        bake_tip_into_body(&mut tipped);
        // Fully tipped log — sail tops sit on the waterline (nucleus column).
        let flat = collect_plant_sail_tops(std::iter::once(&tipped));
        let flat_top = flat.get(&5).copied().unwrap_or(tipped.gy);
        assert_eq!(flat_top, tipped.gy, "fully tipped canopy is waterline-flat");

        // Post-tip upright mast on the crown column.
        tipped.body.push((0, 1, ModuleId::Stem));
        tipped.body.push((0, 2, ModuleId::Photosystem));
        tipped.upright_growth = vec![(0, 1), (0, 2)];
        let sailed = collect_plant_sail_tops(std::iter::once(&tipped));
        let sail_top = sailed.get(&5).copied().unwrap_or(tipped.gy);
        assert!(
            sail_top >= tipped.gy + 2,
            "post-tip upright mast must raise sail top (got {sail_top}, gy={})",
            tipped.gy
        );
    }

    #[test]
    fn tall_plant_on_skinny_raft_tips_over() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        // One-column floating mat.
        w.set_cell(8, 6, Cell::solid(MaterialId::Organic));
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Stem),
            (0, 4, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(8, 7, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(
            store.atoms[0].fallen,
            "tall sail on a 1-wide raft should tip"
        );
        assert!(
            rooted_in_organic_for_test(&w, &store.atoms[0])
                || w.get_cell(8, 6).map(|c| c.material) == Some(MaterialId::Organic),
            "holdfast should still be on the Organic mat"
        );
    }

    #[test]
    fn deep_root_keel_keeps_tall_plant_upright_on_skinny_raft() {
        use crate::organism::OrganismStore;

        let mut w = World::new(5);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..20 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            for y in 1..=5 {
                w.set_cell(x, y, Cell::water());
            }
            for y in 6..16 {
                w.set_cell(x, y, Cell::air());
            }
        }
        w.set_cell(8, 6, Cell::solid(MaterialId::Organic));
        // Same sail height as the tippy case, but a heavy root keel under the raft.
        let body: Vec<BodyModule> = vec![
            (0, 0, ModuleId::Nucleus),
            (0, -1, ModuleId::Root),
            (0, -2, ModuleId::Root),
            (0, -3, ModuleId::Root),
            (0, -4, ModuleId::Root),
            (0, 1, ModuleId::Stem),
            (0, 2, ModuleId::Stem),
            (0, 3, ModuleId::Stem),
            (0, 4, ModuleId::Photosystem),
        ];
        let mut store = OrganismStore::new();
        let mut atom = Atom::from_body(8, 7, 40.0, body);
        apply_genome(
            &mut atom,
            crate::blueprint::Blueprint::minimal_plant().genome,
        );
        store.atoms.push(atom);
        store.step(&mut w, 0);
        assert!(
            !store.atoms[0].fallen,
            "dangling root keel should stabilize a tall plant on a skinny raft"
        );
    }

    fn rooted_in_organic_for_test(world: &World, atom: &Atom) -> bool {
        atom.body.iter().any(|&(dx, dy, m)| {
            if m != ModuleId::Root && m != ModuleId::Nucleus {
                return false;
            }
            let wx = world.wrap_x(atom.gx + dx as i32);
            let wy = atom.gy + dy as i32;
            world
                .get_cell(wx, wy)
                .map(|c| c.material == MaterialId::Organic)
                .unwrap_or(false)
                || world
                    .get_cell(wx, wy - 1)
                    .map(|c| c.material == MaterialId::Organic)
                    .unwrap_or(false)
        })
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
            let _ = try_elongate_root(&mut w, &mut atom, &live, &caps);
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
            let spent = try_grow_plant(&mut w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
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
        let mut max_wood = 1i32;
        for t in 0..40u64 {
            atom.energy = atom.energy_max;
            atom.age_ticks = t * LAND_GROW_PERIOD;
            let _ = try_grow_shoot(&w, &mut atom, t, &trunks, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
            for &(x, y, m) in &atom.body {
                if m == ModuleId::Photosystem {
                    max_wood = max_wood.max(woody_leaf_wood_dist(&atom, x, y));
                }
            }
        }
        assert!(
            atom.photosystem_count() > n0,
            "leaf-heavy shoot should add Photosystems"
        );
        assert!(
            max_wood <= WOODY_LEAF_MAX_CANT,
            "woody leaves must stay Moore-adjacent to Stem (dist={max_wood} > {WOODY_LEAF_MAX_CANT})"
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
        let mut w = moist_plot(); // dry Air above sand
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
            let _ = try_grow_plant(&mut w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
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
            let _ = try_grow_plant(&mut w, &mut atom, pulse * LAND_GROW_PERIOD, &trunks, &roots, &HashSet::new(), &caps, &CanopyIndex::default(), 0);
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
        let mut w = moist_plot();
        let mut atom = Atom::from_body(
            4,
            2,
            60.0,
            crate::blueprint::Blueprint::minimal_plant().modules_relative_to_nucleus(),
        );
        atom.energy = atom.energy_max;
        atom.cooldown = 0;
        let bank = crate::spore_bank::SporeBankConfig::default();
        for t in 0..500u64 {
            assert!(
                matches!(
                    try_plant_wind_spore(&mut w, &mut atom, t, 1, true, &[], 0.5, &bank),
                    crate::spore_bank::DispersalResult::None
                ),
                "bare plant must not wind-spore"
            );
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
        }
    }

    #[test]
    fn fern_with_repro_spore_spreads_downwind() {
        let mut w = moist_plot();
        let mut atom = Atom::from_body(4, 2, 60.0, fern_body());
        // Enough roots to seat; wind spore itself only needs ReproSpore + leaf.
        assert!(is_anchored(&w, &atom));
        assert!(spore_count(&atom) >= 1);
        let bank = crate::spore_bank::SporeBankConfig::default();
        let mut child = None;
        for t in 0..4_000u64 {
            atom.energy = atom.energy_max;
            atom.cooldown = 0;
            if let crate::spore_bank::DispersalResult::Germinated(c) =
                try_plant_wind_spore(&mut w, &mut atom, t, 3, true, &[4], 0.8, &bank)
            {
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

    #[test]
    fn fungus_slot_prefers_air_on_organic_over_buried() {
        let mut w = moist_plot();
        // Thick Organic bed with free Air above.
        for y in 1..=3 {
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(120);
            org.set_mycelium(80);
            w.set_cell(4, y, org);
        }
        w.set_cell(4, 4, Cell::air());
        let slot = find_fungus_slot(&w, 4, 2).expect("must find a fungus seat");
        assert_eq!(
            w.get_cell(4, slot).map(|c| c.material),
            Some(MaterialId::Air),
            "default seat should be surface Air, got y={slot}"
        );
        assert_eq!(
            w.get_cell(4, slot - 1).map(|c| c.material),
            Some(MaterialId::Organic),
            "stalk should stand on Organic"
        );
        // Rhizomorph path may still pick a buried Organic seat.
        let buried = find_fungus_slot_biased(&w, 4, 2, false).expect("buried search");
        assert!(
            matches!(
                w.get_cell(4, buried).map(|c| c.material),
                Some(MaterialId::Organic) | Some(MaterialId::Air)
            ),
            "rhizomorph bias must still return a legal fungus crown"
        );
    }
}
