//! Land-plant roots (Set D): moisture / nutrient harvest, elongation,
//! and substrate bore into Organic. Spec: `docs/organism/PLANTS.md`.

use wk_material::MaterialId;
use wk_world::column::{Activity, Column};
use wk_world::dig::root_penetrate_cost;
use wk_world::terrain::hash_u64;
use wk_world::world::World;

use crate::blueprint::{Blueprint, PlacedModule};
use crate::module::{LaneId, ModuleId};
use crate::organism::MODULE_CELL_COLS;
use crate::{Energy, Genome};

/// Soft cap on sienna Root pixels per blueprint.
pub const MAX_ROOT_MODULES: usize = 24;
/// Soft cap on olive Stem pixels.
pub const MAX_STEM_MODULES: usize = 12;

/// Max columns a root sucker may emerge from the parent crown.
/// Seeds / wind dispersal come later — vegetative spread stays local.
pub const ROOT_SPROUT_MAX_DIST: i32 = 6;
/// Pore saturation floor for a sprout site (avoid bone-dry rock).
pub const ROOT_SPROUT_MIN_MOIST_FRAC: f32 = 0.02;
/// Neighbour wall height (m) that makes a notch unplantable (embedded look).
pub const PLANTABLE_WALL_DROP_M: f32 = 6.0;
/// Standing water shallower than this is always plantable (wade / mudflat).
pub const SHALLOW_PLANT_WATER_M: f32 = 0.5;
/// Hard cap — ocean trenches stay unplantable even with absurd root paint.
pub const MAX_PLANT_WATER_M: f32 = 8.0;

/// Per-root energy drain / tick. Must stay tiny vs photo (~0.15/tick):
/// the old `0.015` bankrupted any tree with ≥3 roots over a default night
/// (10 h × 0.045 ≈ 1620 energy on a 200 tank) even on wet fertile coast.
/// Sized so a maxed root system (~24) still outlasts a default night on a
/// full tank; night multiplies this by the same factor as basal upkeep.
pub const ROOT_UPKEEP_PER_MODULE: f32 = 0.0001;
/// Extra Root modules allowed per photosystem beyond the sprout minimum.
/// Past this soft budget, hydrated plants invest in shoots instead of
/// boring more sand (avoids root-only death spirals). Thirsty plants may
/// still dig past this; full-tank "luxury" boring is not allowed.
pub const LAND_ROOTS_PER_PHOTOSYSTEM: usize = 3;

/// Fraction of spawn tank size unlocked as storage per Root module.
/// Real-world analogy: roots bank starch / sugars — deeper systems hold more.
pub const ROOT_STORE_FRAC: f32 = 0.04;
/// Cap on capacity multiplier from roots (`base_max × this`).
pub const ROOT_STORE_MAX_MULT: f32 = 2.0;

/// Base energy to place one new Root pixel in soft Organic.
pub const ROOT_ELONGATE_BASE_COST: f32 = 2.4;
/// kg of substrate converted per successful rock/sand bore.
pub const ROOT_BORE_KG: i64 = 40;
/// Energy gained per kg of moisture drunk by roots.
pub const ROOT_WATER_ENERGY: f32 = 0.035;
/// Penetrate multiplier inside an open cavity (easy path, no bore).
pub const ROOT_VOID_PENETRATE: f32 = 0.18;
/// Score bonus for stepping into any cavity.
pub const ROOT_VOID_SCORE_BONUS: f32 = 2.4;
/// Extra score when the cavity already holds free water.
pub const ROOT_WET_VOID_SCORE_BONUS: f32 = 1.6;
/// Extra photo multiplier from rich organic substrate (on top of nutrient).
pub const ORGANIC_SUBSTRATE_BONUS: f32 = 0.35;
/// Soft stress drain while drying (moist below [`DROUGHT_STRESS_FRAC`]).
/// Kept tiny — real survival uses hibernation, not a one-second kill.
pub const ROOT_DROUGHT_STRESS_DRAIN: f32 = 0.003;
/// Moisture fraction below which photo/growth slow and stress starts.
pub const DROUGHT_STRESS_FRAC: f32 = 0.18;
/// Moisture fraction that triggers drought dormancy (hibernate).
pub const DROUGHT_DORMANT_FRAC: f32 = 0.06;
/// Max consecutive dormant ticks before the plant dies (~2.5 min at 60 Hz).
pub const DROUGHT_HIBERNATE_MAX_TICKS: u32 = 9_000;
/// Upkeep multiplier while drought-dormant (respiration only).
pub const DROUGHT_DORMANT_UPKEEP: f32 = 0.18;
/// Fractional kg accumulated per root module per tick. Integer drinks
/// only fire once the organism accumulator reaches 1 kg — the old
/// `.ceil().max(1)` path took ≥1 kg every tick and flash-dried hills.
///
/// ~0.0004 × 5 roots ≈ 1 kg every ~500 ticks (orders of magnitude
/// slower than shore recharge / rain soak).
pub const ROOT_SIP_KG_PER_ROOT: f32 = 0.0004;
/// Hard cap on pore water removed in one drink event.
pub const ROOT_SIP_MAX_KG_PER_TICK: i64 = 1;
/// How often (ticks) a plant may attempt elongation.
pub const ROOT_GROW_PERIOD: u64 = 48;
/// Soft cap on the energy fraction needed to fire a vegetative root sprout.
/// Plankton keep the genome `reproduce_at` default (0.7); land suckers would
/// never bank that high on sand plains while elongation spends every surplus.
pub const LAND_SPROUT_ENERGY_FRAC: f32 = 0.52;
/// Phase period for land vegetative sprouts (shorter than plankton fission).
pub const LAND_SPROUT_PERIOD: u64 = 48;
/// Max column distance for genet moisture/energy sharing along a rhizome.
pub const GENET_SHARE_MAX_DIST: i32 = 8;
/// Fraction of the energy-fraction gap closed toward the genet mean / tick.
pub const GENET_ENERGY_EQUALIZE: f32 = 0.20;
/// Energy fraction above which roots/shoots may elongate. Must stay **below**
/// [`LAND_SPROUT_ENERGY_FRAC`] so plants grow tissue while banking for a
/// sucker — tying both gates to 0.52 left forests as 1-pixel stubs.
pub const LAND_GROW_ENERGY_FRAC: f32 = 0.30;
/// Painted Root modules required before a vegetative sprout may fire.
/// Minimal plants start with 1; they must dig a little first.
pub const LAND_SPROUT_MIN_ROOTS: usize = 3;

/// Soft useful-root budget: sprout minimum + leaf-driven extras.
pub fn useful_root_budget(blueprint: &Blueprint) -> usize {
    LAND_SPROUT_MIN_ROOTS
        .saturating_add(blueprint.photosystem_count().saturating_mul(LAND_ROOTS_PER_PHOTOSYSTEM))
        .min(MAX_ROOT_MODULES)
}

/// Drought-aware soft budget. Stress lifts the cap so plants keep digging
/// for deeper water and starch storage instead of switching to shoots.
pub fn useful_root_budget_for(blueprint: &Blueprint, drought: DroughtBand) -> usize {
    let base = useful_root_budget(blueprint);
    match drought {
        DroughtBand::Hydrated | DroughtBand::Dormant => base,
        // Roughly half the remaining modules — storage + stone diving can pay.
        DroughtBand::Stressed => {
            let lift = (MAX_ROOT_MODULES.saturating_sub(base) + 1) / 2;
            base.saturating_add(lift).min(MAX_ROOT_MODULES)
        }
    }
}

/// True when further root growth is optional (plant is already well rooted).
pub fn roots_past_soft_budget(blueprint: &Blueprint) -> bool {
    blueprint.root_count() >= useful_root_budget(blueprint)
}

/// Soft-budget gate that respects drought-lifted root allowance.
pub fn roots_past_soft_budget_for(blueprint: &Blueprint, drought: DroughtBand) -> bool {
    blueprint.root_count() >= useful_root_budget_for(blueprint, drought)
}

/// Effective energy tank size from painted roots (starch / reserve analogy).
///
/// Photo, basal upkeep, and growth floors stay keyed to `base_max`; only
/// storage clamp uses this larger capacity.
pub fn energy_capacity(base_max: f32, n_roots: usize) -> f32 {
    let base = base_max.max(1.0);
    let mult = (1.0 + ROOT_STORE_FRAC * n_roots as f32).min(ROOT_STORE_MAX_MULT);
    base * mult
}

/// Growth / sprout floor keyed to the spawn tank (not root-inflated max).
pub fn growth_energy_floor(base_max: f32) -> f32 {
    base_max.max(1.0) * LAND_GROW_ENERGY_FRAC
}

/// Energy multiplier to bore through `mat`. Higher = harder.
pub fn penetrate_cost(mat: MaterialId) -> Option<f32> {
    root_penetrate_cost(mat)
}

/// Plant-available nutrient factor in `0..1+` for a column.
///
/// Land: ecology nutrient × organic-layer richness.
/// Water column: dissolved nutrients tracked on ecology (same field),
/// boosted when free water is present.
pub fn column_nutrient_factor(col: &Column) -> f32 {
    let base = col.ecology.nutrient.clamp(0.0, 1.0);
    let mut organic_kg = 0i64;
    let mut solid_kg = 0i64;
    for i in 0..col.layer_count as usize {
        let m = col.layers[i].material;
        if !m.is_solid() {
            continue;
        }
        solid_kg += col.layers[i].thickness;
        if m == MaterialId::Organic {
            organic_kg += col.layers[i].thickness;
        }
    }
    let organic_frac = if solid_kg > 0 {
        (organic_kg as f32 / solid_kg as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let litter = (col.ecology.dead_biomass as f32 / 400.0).clamp(0.0, 0.4);
    let land = (base * (0.35 + 0.65 * (organic_frac + litter).min(1.0))
        + ORGANIC_SUBSTRATE_BONUS * organic_frac)
        .clamp(0.05, 1.6);

    if col.flowable_water().is_some() {
        // Ocean / pool: nutrients are "dissolved" — ecology.nutrient plus
        // a dissolved-CO₂ proxy so algal blooms and plant roots share a pool.
        let dissolved = (base * 0.7 + col.ecology.water_co2 * 0.25).clamp(0.15, 1.4);
        land.max(dissolved)
    } else {
        land
    }
}

/// Moisture availability `0..1` across host + left/right neighbours.
pub fn adjacent_moisture_frac(world: &World, wx: i32) -> f32 {
    let mut sum = 0.0f32;
    let mut n = 0.0f32;
    for dx in [-1, 0, 1] {
        let Some(col) = world.column_at(wx + dx) else {
            continue;
        };
        let cap = col.moisture_cap().max(1) as f32;
        let standing = col.top_water_mass().max(0) as f32;
        let moist = col.moisture.max(0) as f32 + standing * 0.5;
        sum += (moist / cap).clamp(0.0, 1.5);
        n += 1.0;
    }
    if n <= 0.0 {
        0.0
    } else {
        (sum / n).clamp(0.0, 1.5)
    }
}

/// Gentle sip of **pore moisture** from host + adjacent columns.
///
/// Prefers pore water (leaves ~40% capacity so recharge wins). If the
/// budget is still open, takes a tiny standing-water sip — reed / mangrove
/// payoff when roots reach a wet column without flash-draining lakes.
pub fn drink_adjacent(world: &mut World, wx: i32, budget_kg: i64) -> i64 {
    drink_from_hosts(world, &[wx], budget_kg)
}

/// Sip pore moisture (then a trickle of standing water) from host columns.
///
/// Used by genet-sharing ramets so a dry sucker can drink through a
/// wetter sibling's rhizome patch. Same reserve rules as [`drink_adjacent`].
pub fn drink_from_hosts(world: &mut World, hosts: &[i32], budget_kg: i64) -> i64 {
    if budget_kg <= 0 || hosts.is_empty() {
        return 0;
    }
    let mut left = budget_kg.min(ROOT_SIP_MAX_KG_PER_TICK);
    let mut taken = 0i64;
    let mut order: Vec<i32> = Vec::with_capacity(hosts.len() * 3);
    for &h in hosts {
        for dx in [-1, 0, 1] {
            let x = h + dx;
            if !order.contains(&x) {
                order.push(x);
            }
        }
    }
    order.sort_by_key(|&x| world.column_at(x).map(|c| -c.moisture).unwrap_or(0));
    for &x in &order {
        if left <= 0 {
            break;
        }
        let available = match world.column_at(x) {
            Some(col) => {
                let reserve = ((col.moisture_cap() as f32) * 0.40).round() as i64;
                col.moisture.saturating_sub(reserve.max(2))
            }
            None => continue,
        };
        if available <= 0 {
            continue;
        }
        let want = left.min(available).min(1);
        let got = if let Some(col) = world.column_at_mut(x) {
            let got = want.min(col.moisture);
            col.moisture -= got;
            if got > 0 {
                col.activity = Activity::HydrologyActive;
            }
            got
        } else {
            0
        };
        if got > 0 {
            world.mass_audit.evap_out_total += got;
        }
        taken += got;
        left -= got;
    }
    // Standing-water trickle when pore is dry — long-root / reed payoff.
    // Cap 1 kg and require a real free-surface puddle (not a sheen).
    if left > 0 {
        for &x in &order {
            if left <= 0 {
                break;
            }
            let wet = world
                .column_at(x)
                .and_then(|c| c.flowable_water())
                .map(|(_, m)| m)
                .unwrap_or(0);
            if wet < 50 {
                continue;
            }
            let got = world.drink_water(x, left.min(1));
            if got > 0 {
                taken += got;
                left -= got;
            }
        }
    }
    taken
}

/// Drought band for a moisture fraction: hydrated / stressed / dormant.
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

/// World Y of a module cell for a rooted plant (`pose.y` = ground).
pub fn module_world_y(pose_y: f32, module_y: i16) -> f32 {
    pose_y + module_y as f32 * MODULE_CELL_COLS
}

fn elevation_in_void(col: &wk_world::column::Column, y: f32) -> bool {
    col.voids.iter().any(|v| {
        v.height_m > 1e-4 && y <= v.top_y + 1e-3 && y >= v.floor_y() - 1e-3
    })
}

fn solid_purchase_at(world: &World, wx: i32, y: f32) -> bool {
    let Some(col) = world.column_at(wx) else {
        return false;
    };
    // Explicit void cavity — root hanging in open air / collapsed soil.
    if elevation_in_void(col, y) {
        return false;
    }
    world
        .material_at(wx, y)
        .map(|mat| {
            mat.is_solid()
                && !matches!(
                    mat,
                    MaterialId::Ice | MaterialId::Snow | MaterialId::Water
                )
        })
        .unwrap_or(false)
}

/// True when at least one Root tip still has solid purchase.
///
/// Anchored plants are immobile under collision. If the soil around the
/// roots collapses (void / no solid at tip depth), this returns false and
/// the plant can topple / be shoved.
pub fn plant_is_anchored(world: &World, pose_x: f32, pose_y: f32, blueprint: &Blueprint) -> bool {
    if !blueprint.is_rooted() {
        return false;
    }
    let roots: Vec<&PlacedModule> = blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
        .collect();
    if roots.is_empty() {
        // Stem-only habit: weak surface contact counts as anchored.
        let wx = pose_x.floor() as i32;
        return solid_purchase_at(world, wx, pose_y - 0.1);
    }
    let min_x = blueprint.modules.iter().map(|m| m.x).min().unwrap_or(0);
    let max_x = blueprint.modules.iter().map(|m| m.x).max().unwrap_or(0);
    let mid_x = (min_x as f32 + max_x as f32) * 0.5;
    let mut purchase = 0usize;
    for m in &roots {
        let cell_wx = (pose_x + (m.x as f32 - mid_x) * MODULE_CELL_COLS).floor() as i32;
        let tip_y = module_world_y(pose_y, m.y);
        // Sample tip and a hair below — root must bite solid, not hang in a void.
        if solid_purchase_at(world, cell_wx, tip_y)
            || solid_purchase_at(world, cell_wx, tip_y - 0.05)
        {
            purchase += 1;
        }
    }
    // Need a real foothold — a single clinging tip is enough to stay put;
    // zero purchase → free to fall / be pushed.
    purchase >= 1
}

/// World-X of the blueprint's horizontal mid (crown column).
fn blueprint_mid_x(blueprint: &Blueprint) -> f32 {
    let min_x = blueprint.modules.iter().map(|m| m.x).min().unwrap_or(0);
    let max_x = blueprint.modules.iter().map(|m| m.x).max().unwrap_or(0);
    (min_x as f32 + max_x as f32) * 0.5
}

/// How deep below the crown the deepest Root tip reaches (metres).
///
/// Used as the standing-water depth a plant (or its sucker) can colonise —
/// long roots unlock deeper shallows / reed beds.
pub fn root_reach_m(blueprint: &Blueprint) -> f32 {
    if !blueprint.is_rooted() {
        return 0.0;
    }
    let crown = crate::organism::blueprint_land_crown_y(blueprint);
    let deepest = blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
        .map(|m| m.y)
        .min()
        .unwrap_or(crown);
    let cells = (crown as i32 - deepest as i32).max(0) as f32;
    // At least one cell of bite so minimal plants clear the shallow band.
    (cells.max(1.0) * MODULE_CELL_COLS).clamp(SHALLOW_PLANT_WATER_M, MAX_PLANT_WATER_M)
}

/// Solid ledge suitable for a rooted plant (not a cliff notch).
///
/// Standing water deeper than [`SHALLOW_PLANT_WATER_M`] is still allowed
/// when `root_reach_m` can span the water column (reed / mangrove habit).
pub fn column_is_plantable(world: &World, wx: i32) -> bool {
    column_is_plantable_for_reach(world, wx, SHALLOW_PLANT_WATER_M)
}

/// Like [`column_is_plantable`], but allow water up to `root_reach_m`.
pub fn column_is_plantable_for_reach(world: &World, wx: i32, root_reach_m: f32) -> bool {
    let Some(col) = world.column_at(wx) else {
        return false;
    };
    let allow_depth = root_reach_m.clamp(SHALLOW_PLANT_WATER_M, MAX_PLANT_WATER_M);
    if let Some((_, mass)) = col.flowable_water() {
        if mass > 0 {
            let depth = col.mass_to_height_delta(MaterialId::Water, mass);
            if depth > allow_depth {
                return false;
            }
        }
    }
    let bed = col.climate_elevation();
    if !solid_purchase_at(world, wx, bed - 0.15)
        && !solid_purchase_at(world, wx, bed - 0.35)
    {
        return false;
    }
    // Both neighbours much higher → recess in a cliff face (plants look
    // buried in the wall). Peaks / one-sided drops stay allowed.
    let left = world
        .column_at(wx - 1)
        .map(|c| c.climate_elevation());
    let right = world
        .column_at(wx + 1)
        .map(|c| c.climate_elevation());
    if let (Some(l), Some(r)) = (left, right) {
        if l > bed + PLANTABLE_WALL_DROP_M && r > bed + PLANTABLE_WALL_DROP_M {
            return false;
        }
    }
    true
}

/// True when a sucker can emerge here (plantable for `root_reach_m` + moisture).
pub fn column_can_host_sprout(world: &World, wx: i32) -> bool {
    column_can_host_sprout_for_reach(world, wx, SHALLOW_PLANT_WATER_M)
}

/// Like [`column_can_host_sprout`], with an explicit root-reach water budget.
pub fn column_can_host_sprout_for_reach(world: &World, wx: i32, root_reach_m: f32) -> bool {
    if !column_is_plantable_for_reach(world, wx, root_reach_m) {
        return false;
    }
    let Some(col) = world.column_at(wx) else {
        return false;
    };
    let cap = col.moisture_cap();
    if cap <= 0 {
        return false;
    }
    let moist = col.moisture.max(0) as f32 / cap as f32;
    moist >= ROOT_SPROUT_MIN_MOIST_FRAC || col.top_water_mass() > 0
}

/// World column occupied by a blueprint module at `pose_x`.
pub fn root_module_world_x(pose_x: f32, blueprint: &Blueprint, module_x: i16) -> i32 {
    let mid_x = blueprint_mid_x(blueprint);
    (pose_x + (module_x as f32 - mid_x) * MODULE_CELL_COLS).floor() as i32
}

/// True when at least one Root pixel sits in a column other than the crown.
pub fn has_lateral_runner(pose_x: f32, blueprint: &Blueprint) -> bool {
    if !blueprint.is_rooted() {
        return false;
    }
    let parent_wx = pose_x.floor() as i32;
    blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
        .any(|m| root_module_world_x(pose_x, blueprint, m.x) != parent_wx)
}

/// Pick a column for vegetative **root sprout** propagation.
///
/// Only emerges from a **painted lateral runner** — a Root module that
/// already reaches a neighbouring column. No teleport suckers: if the
/// plant hasn't shot a horizontal root yet, returns `None` and the
/// caller should keep elongating (see runner bias in
/// [`try_elongate_root`]).
///
/// Returns `None` when no eligible tip exists (caller skips without
/// charging energy). Seeds / fruiting come later.
pub fn pick_root_sprout_x(
    world: &World,
    pose_x: f32,
    pose_y: f32,
    blueprint: &Blueprint,
    _world_seed: u64,
    _tick: u64,
    _entity_id: u32,
) -> Option<f32> {
    if !blueprint.is_rooted() || blueprint.root_count() == 0 {
        return None;
    }
    let parent_wx = pose_x.floor() as i32;
    let parent_bed = world
        .column_at(parent_wx)
        .map(|c| c.climate_elevation())
        .unwrap_or(pose_y);

    // Painted lateral roots with solid purchase — farthest first.
    let mut lateral: Vec<(i32, i32, f32, i16)> = Vec::new(); // |dx|, wx, moist, module_y
    for m in blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
    {
        let cell_wx = root_module_world_x(pose_x, blueprint, m.x);
        let dx = (cell_wx - parent_wx).abs();
        if dx < 1 || dx > ROOT_SPROUT_MAX_DIST {
            continue;
        }
        let tip_y = module_world_y(pose_y, m.y);
        if !solid_purchase_at(world, cell_wx, tip_y)
            && !solid_purchase_at(world, cell_wx, tip_y - 0.05)
        {
            continue;
        }
        let reach = root_reach_m(blueprint);
        if !column_can_host_sprout_for_reach(world, cell_wx, reach) {
            continue;
        }
        let Some(col) = world.column_at(cell_wx) else {
            continue;
        };
        // Don't sprout up/down a cliff face from a runner tip.
        if (col.climate_elevation() - parent_bed).abs() > 10.0 {
            continue;
        }
        let cap = col.moisture_cap().max(1) as f32;
        let moist = col.moisture.max(0) as f32 / cap;
        // Prefer shallow runners (rhizomes) over deep laterals.
        let shallow = (-m.y).min(6) as f32;
        lateral.push((dx, cell_wx, moist + (6.0 - shallow) * 0.05, m.y));
    }
    lateral.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    lateral.first().map(|&(_, wx, _, _)| wx as f32 + 0.5)
}

/// Pick the deepest Root tip (most negative module y), or Nucleus if none.
pub fn root_tips(blueprint: &Blueprint) -> Vec<(i16, i16)> {
    let roots: Vec<(i16, i16)> = blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
        .map(|m| (m.x, m.y))
        .collect();
    if !roots.is_empty() {
        let min_y = roots.iter().map(|(_, y)| *y).min().unwrap_or(0);
        return roots.into_iter().filter(|(_, y)| *y == min_y).collect();
    }
    // Seed from nucleus / lowest module so elongation can start.
    let seed = blueprint
        .modules
        .iter()
        .min_by_key(|m| m.y)
        .map(|m| (m.x, m.y))
        .unwrap_or((0, 0));
    vec![seed]
}

/// Try to elongate one Root pixel toward moisture / depth bias.
///
/// `base_max` is the spawn tank size — growth floors and sprout banking
/// use it so root storage capacity does not raise the spend gate.
///
/// Returns energy spent (0 if nothing grew). May call [`World::root_bore`].
pub fn try_elongate_root(
    blueprint: &mut Blueprint,
    energy: &mut Energy,
    world: &mut World,
    pose_x: f32,
    pose_y: f32,
    genome: &Genome,
    base_max: f32,
    world_seed: u64,
    tick: u64,
    entity_id: u32,
) -> f32 {
    let n_roots = blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
        .count();
    if n_roots >= MAX_ROOT_MODULES {
        return 0.0;
    }
    let (_, _, w_root) = genome.alloc_weights();
    if w_root < 0.08 {
        return 0.0;
    }
    let tank = base_max.max(1.0);
    let grow_floor = growth_energy_floor(tank);
    // Grow while banking toward the sprout gate (see LAND_GROW_ENERGY_FRAC).
    if energy.current < grow_floor {
        return 0.0;
    }

    let occupied: std::collections::HashSet<(i16, i16)> =
        blueprint.modules.iter().map(|m| (m.x, m.y)).collect();
    let tips = root_tips(blueprint);
    let depth_bias = genome.root_depth_bias.clamp(0.0, 1.0);
    let parent_wx = pose_x.floor() as i32;
    // No painted runner yet → shoot sideways (rhizome) before diving.
    // Vegetative sprouts only fire from lateral tips.
    let need_runner = !has_lateral_runner(pose_x, blueprint)
        && blueprint.root_count() >= LAND_SPROUT_MIN_ROOTS.saturating_sub(1);
    let banking_for_sprout = energy.current >= tank * LAND_SPROUT_ENERGY_FRAC * 0.85;
    // Past the soft root:shoot budget, only grow roots when stressed for
    // water or forcing a rhizome runner.
    let host_moist = world
        .column_at(parent_wx)
        .map(|c| {
            let cap = c.moisture_cap().max(1) as f32;
            (c.moisture.max(0) as f32 / cap).clamp(0.0, 1.5)
        })
        .unwrap_or(0.0);
    let drought = drought_band(host_moist);
    let thirsty = matches!(drought, DroughtBand::Stressed);
    if roots_past_soft_budget_for(blueprint, drought) && !need_runner && !thirsty {
        return 0.0;
    }

    // Candidate steps from each tip: down + diagonals + lateral.
    const DIRS: [(i16, i16); 5] = [(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)];
    let mut best: Option<(f32, i16, i16, i32, f32)> = None; // score, x, y, wx, cost

    for &(tx, ty) in &tips {
        for &(dx, dy) in &DIRS {
            let nx = tx + dx;
            let ny = ty + dy;
            // Wide enough that a shallow rhizome can cross ~ROOT_SPROUT_MAX_DIST columns.
            if ny > 2 || ny < -20 || nx.abs() > 14 {
                continue;
            }
            if occupied.contains(&(nx, ny)) {
                continue;
            }
            let cell_wx = root_module_world_x(pose_x, blueprint, nx);
            let tip_y = module_world_y(pose_y, ny);
            let Some(col) = world.column_at(cell_wx) else {
                continue;
            };
            let in_void = elevation_in_void(col, tip_y);
            let (pen, void_bonus) = if in_void {
                let wet = col
                    .void_index_at(tip_y)
                    .map(|i| col.voids[i].fill_frac())
                    .unwrap_or(0.0);
                (
                    ROOT_VOID_PENETRATE,
                    ROOT_VOID_SCORE_BONUS + ROOT_WET_VOID_SCORE_BONUS * wet,
                )
            } else {
                let mat = world
                    .material_at(cell_wx, tip_y)
                    .or_else(|| world.material_at(cell_wx, col.surface_y - 0.15))
                    .unwrap_or(MaterialId::Sand);
                let Some(pen) = penetrate_cost(mat) else {
                    continue;
                };
                (pen, 0.0)
            };
            let cost = ROOT_ELONGATE_BASE_COST * pen;
            if energy.current < cost + grow_floor {
                continue;
            }
            let cap = col.moisture_cap().max(1) as f32;
            let moist = ((col.moisture.max(0) as f32) / cap).clamp(0.0, 1.5);
            let down = if dy < 0 { 1.0 } else { 0.0 };
            let lateral = if dx != 0 && dy == 0 { 0.35 } else { 0.0 };
            let enters_new_col = cell_wx != parent_wx;
            let mut score = moist
                + void_bonus
                + depth_bias * down
                + (1.0 - depth_bias) * lateral
                - pen * 0.03;
            if need_runner || banking_for_sprout {
                // Rhizome urge: horizontal into a neighbour beats diving.
                if dx != 0 && dy == 0 {
                    score += 2.8;
                } else if dx != 0 && dy < 0 {
                    score += 1.1;
                }
                if enters_new_col {
                    score += 1.6;
                }
                if need_runner && dy < 0 && dx == 0 {
                    score -= 1.2;
                }
            }
            let better = best.map(|(s, ..)| score > s).unwrap_or(true);
            if better {
                best = Some((score, nx, ny, cell_wx, cost));
            }
        }
    }

    let Some((_score, nx, ny, cell_wx, _cost)) = best else {
        return 0.0;
    };

    // Deterministic jitter so siblings diverge left/right.
    let flip = hash_u64(world_seed, tick as i64, entity_id as i64, 0x7007) & 1 == 1;
    let nx = if flip && !occupied.contains(&(nx.saturating_neg(), ny)) && nx != 0 {
        // occasionally mirror for divergence
        let mirrored = -nx;
        if !occupied.contains(&(mirrored, ny)) {
            mirrored
        } else {
            nx
        }
    } else {
        nx
    };

    let tip_y = module_world_y(pose_y, ny);
    let in_void = world
        .column_at(cell_wx)
        .map(|c| elevation_in_void(c, tip_y))
        .unwrap_or(false);
    if in_void {
        // Cavities are free paths — no bore, cheap elongate.
        let cost = ROOT_ELONGATE_BASE_COST * ROOT_VOID_PENETRATE;
        if energy.current < cost + grow_floor {
            return 0.0;
        }
        energy.current -= cost;
        let lane = blueprint
            .modules
            .first()
            .map(|m| m.lane)
            .unwrap_or(LaneId::Mid);
        blueprint.modules.push(PlacedModule {
            x: nx,
            y: ny,
            lane,
            module: ModuleId::Root,
        });
        return cost;
    }
    let mat = world
        .material_at(cell_wx, tip_y)
        .unwrap_or(MaterialId::Sand);
    if let Some(pen) = penetrate_cost(mat) {
        let cost = ROOT_ELONGATE_BASE_COST * pen;
        if energy.current < cost + grow_floor {
            return 0.0;
        }
        energy.current -= cost;
        // Displace rock/sand into Organic when boring hard substrate.
        if mat != MaterialId::Organic {
            let _ = world.root_bore(cell_wx, tip_y, ROOT_BORE_KG, tick);
        }
        let lane = blueprint
            .modules
            .first()
            .map(|m| m.lane)
            .unwrap_or(LaneId::Mid);
        blueprint.modules.push(PlacedModule {
            x: nx,
            y: ny,
            lane,
            module: ModuleId::Root,
        });
        return cost;
    }
    0.0
}

/// Optional stem/leaf growth from surplus allocation (cheap vertical habit).
///
/// `base_max` keys the growth floor (same rule as [`try_elongate_root`]).
pub fn try_grow_shoot(
    blueprint: &mut Blueprint,
    energy: &mut Energy,
    genome: &Genome,
    base_max: f32,
    world_seed: u64,
    tick: u64,
    entity_id: u32,
) -> f32 {
    let (w_stem, w_leaf, _) = genome.alloc_weights();
    if energy.current < base_max.max(1.0) * (LAND_GROW_ENERGY_FRAC + 0.08) {
        return 0.0;
    }
    let occupied: std::collections::HashSet<(i16, i16)> =
        blueprint.modules.iter().map(|m| (m.x, m.y)).collect();
    let n_stem = blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Stem)
        .count();
    let n_photo = blueprint.photosystem_count();

    let roll = hash_u64(world_seed, tick as i64, entity_id as i64, 0x5707) as f32 / u64::MAX as f32;
    let prefer_leaf = roll < w_leaf / (w_stem + w_leaf).max(1e-6);

    let cost = 1.6f32;
    if energy.current < cost + 1.0 {
        return 0.0;
    }

    let lane = blueprint
        .modules
        .first()
        .map(|m| m.lane)
        .unwrap_or(LaneId::Mid);

    if prefer_leaf && n_photo < 12 {
        // Place leaf beside / above the highest module.
        let top = blueprint.modules.iter().max_by_key(|m| m.y).cloned();
        if let Some(t) = top {
            for &(dx, dy) in &[(0i16, 1), (1, 1), (-1, 1), (1, 0), (-1, 0)] {
                let nx = t.x + dx;
                let ny = t.y + dy;
                if ny > 14 || occupied.contains(&(nx, ny)) {
                    continue;
                }
                energy.current -= cost;
                blueprint.modules.push(PlacedModule {
                    x: nx,
                    y: ny,
                    lane,
                    module: ModuleId::Photosystem,
                });
                return cost;
            }
        }
    } else if n_stem < MAX_STEM_MODULES {
        let anchor = blueprint
            .modules
            .iter()
            .filter(|m| matches!(m.module, ModuleId::Stem | ModuleId::Nucleus | ModuleId::Root))
            .max_by_key(|m| m.y)
            .map(|m| (m.x, m.y))
            .or_else(|| blueprint.modules.first().map(|m| (m.x, m.y)));
        if let Some((ax, ay)) = anchor {
            let nx = ax;
            let ny = ay + 1;
            if ny <= 14 && !occupied.contains(&(nx, ny)) {
                energy.current -= cost;
                blueprint.modules.push(PlacedModule {
                    x: nx,
                    y: ny,
                    lane,
                    module: ModuleId::Stem,
                });
                return cost;
            }
        }
    }
    0.0
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::Blueprint;
    use crate::{Energy, Genome};
    use wk_world::terrain::generate_flat_sand;
    use wk_world::world::World;

    fn dry_land() -> World {
        let mut world = World::new(11);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        if let Some(col) = world.column_at_mut(8) {
            col.moisture = col.moisture_cap() / 2;
            col.ecology.nutrient = 0.5;
        }
        world
    }

    /// Minimal plant plus a shallow rhizome into column `parent+1`.
    fn plant_with_runner(pose_x: f32) -> Blueprint {
        let mut bp = Blueprint::minimal_plant(Genome::default());
        // MODULE_CELL_COLS=0.45 → module x≈+3 crosses into the next column.
        let mid = blueprint_mid_x(&bp);
        let target_x = ((pose_x.floor() + 1.0 + 0.5 - pose_x) / MODULE_CELL_COLS + mid).round() as i16;
        bp.modules.push(PlacedModule {
            x: target_x,
            y: -1,
            lane: LaneId::Mid,
            module: ModuleId::Root,
        });
        bp.modules.push(PlacedModule {
            x: target_x + 1,
            y: -1,
            lane: LaneId::Mid,
            module: ModuleId::Root,
        });
        assert!(
            has_lateral_runner(pose_x, &bp),
            "fixture must paint a lateral runner"
        );
        bp
    }

    #[test]
    fn root_sprout_requires_painted_lateral_runner() {
        let mut world = dry_land();
        for x in 6..12 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        let bp = Blueprint::minimal_plant(Genome::default());
        assert!(
            pick_root_sprout_x(&world, 8.5, 8.0, &bp, 1, 0, 7).is_none(),
            "crown-only roots must not teleport a sucker"
        );
    }

    #[test]
    fn root_sprout_emerges_from_runner_tip() {
        let mut world = dry_land();
        for x in 6..12 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        let pose_x = 8.5;
        let pose_y = world.column_at(8).unwrap().surface_y;
        let bp = plant_with_runner(pose_x);
        let site = pick_root_sprout_x(&world, pose_x, pose_y, &bp, 1, 0, 7)
            .expect("sucker from runner tip");
        let dx = (site.floor() as i32 - 8).abs();
        assert!(
            (1..=ROOT_SPROUT_MAX_DIST).contains(&dx),
            "sprout should emerge at runner column (site={site}, dx={dx})"
        );
        assert!(column_can_host_sprout(&world, site.floor() as i32));
    }

    #[test]
    fn root_sprout_skips_unreachable_deep_water() {
        let mut world = dry_land();
        for x in 6..12 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        // Ocean-scale flood — beyond any root reach.
        for x in [6, 7, 9, 10, 11, 12] {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 20_000, 0);
            }
        }
        let bp = plant_with_runner(8.5);
        let site = pick_root_sprout_x(&world, 8.5, 8.0, &bp, 2, 10, 3);
        assert!(
            site.is_none(),
            "no sprout into drowned neighbours (got {site:?})"
        );
    }

    #[test]
    fn long_roots_can_plant_in_shallow_water() {
        let mut world = dry_land();
        // ~1.2 m of water (mass/250) — above the 0.5 m wade band, within
        // a deep-rooted plant's reach.
        if let Some(col) = world.column_at_mut(10) {
            col.moisture = col.moisture_cap();
            col.deposit_to_top(MaterialId::Water, 300, 0);
            let depth = col.mass_to_height_delta(
                MaterialId::Water,
                col.flowable_water().map(|(_, m)| m).unwrap_or(0),
            );
            assert!(depth > SHALLOW_PLANT_WATER_M);
            assert!(depth < 2.0);
        }
        let mut deep = Blueprint::minimal_plant(Genome::default());
        for y in -6i16..=-2 {
            deep.modules.push(PlacedModule {
                x: 0,
                y,
                lane: LaneId::Mid,
                module: ModuleId::Root,
            });
        }
        let reach = root_reach_m(&deep);
        assert!(reach > SHALLOW_PLANT_WATER_M);
        assert!(
            column_is_plantable_for_reach(&world, 10, reach),
            "deep roots should colonise shallow water (reach={reach})"
        );
        assert!(
            !column_is_plantable(&world, 10),
            "default shallow gate still rejects without reach"
        );
    }

    #[test]
    fn roots_sip_standing_water_when_pore_is_dry() {
        let mut world = dry_land();
        if let Some(col) = world.column_at_mut(8) {
            col.moisture = 0;
            col.deposit_to_top(MaterialId::Water, 800, 0);
        }
        let water_before = world
            .column_at(8)
            .and_then(|c| c.flowable_water())
            .map(|(_, m)| m)
            .unwrap_or(0);
        let drunk = drink_from_hosts(&mut world, &[8], 1);
        assert_eq!(drunk, 1, "should sip standing water");
        let water_after = world
            .column_at(8)
            .and_then(|c| c.flowable_water())
            .map(|(_, m)| m)
            .unwrap_or(0);
        assert_eq!(water_before - water_after, 1);
    }

    #[test]
    fn elongate_shoots_runner_when_banking_for_sprout() {
        let mut world = dry_land();
        for x in 6..12 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
                col.ecology.nutrient = 0.7;
                col.deposit_to_top(MaterialId::Organic, 500, 0);
            }
        }
        let mut bp = Blueprint::minimal_plant(Genome {
            alloc_root: 0.9,
            alloc_stem: 0.05,
            alloc_leaf: 0.05,
            root_depth_bias: 0.2, // sprawl
            ..Genome::default()
        });
        // Extra vertical roots so need_runner engages.
        bp.modules.push(PlacedModule {
            x: 0,
            y: -2,
            lane: LaneId::Mid,
            module: ModuleId::Root,
        });
        let mut energy = Energy {
            current: 120.0,
            max: 120.0,
        };
        let genome = bp.genome;
        let pose_x = 8.5;
        let pose_y = land_plant_pose_y_for_test(world.column_at(8).unwrap().surface_y, &bp);
        assert!(!has_lateral_runner(pose_x, &bp));
        let base_max = energy.max;
        for t in 0..24u64 {
            let _ = try_elongate_root(
                &mut bp,
                &mut energy,
                &mut world,
                pose_x,
                pose_y,
                &genome,
                base_max,
                1,
                t,
                3,
            );
            energy.current = energy.max * 0.9;
            if has_lateral_runner(pose_x, &bp) {
                break;
            }
        }
        assert!(
            has_lateral_runner(pose_x, &bp),
            "banking plant should shoot a horizontal runner before sprouting"
        );
    }

    #[test]
    fn roots_prefer_elongating_into_cavities() {
        use wk_world::column::VoidOrigin;
        let mut world = dry_land();
        let surface = world.column_at(8).unwrap().surface_y;
        // Cavity just below the crown so the first down-step enters it.
        if let Some(col) = world.column_at_mut(8) {
            col.moisture = col.moisture_cap();
            col.grow_void_at(surface - 0.7, 1.2, MaterialId::Sand, VoidOrigin::Karst);
            col.voids[0].water_mass = col.voids[0].capacity_kg() / 2;
        }
        let mut bp = Blueprint::minimal_plant(Genome {
            alloc_root: 0.9,
            alloc_stem: 0.05,
            alloc_leaf: 0.05,
            root_depth_bias: 0.9,
            ..Genome::default()
        });
        let mut energy = Energy {
            current: 80.0,
            max: 80.0,
        };
        let genome = bp.genome;
        let pose_y = land_plant_pose_y_for_test(surface, &bp);
        let base_max = energy.max;
        let spent = try_elongate_root(
            &mut bp,
            &mut energy,
            &mut world,
            8.5,
            pose_y,
            &genome,
            base_max,
            1,
            0,
            1,
        );
        assert!(spent > 0.0, "should elongate");
        let new_root = bp
            .modules
            .iter()
            .filter(|m| m.module == ModuleId::Root)
            .min_by_key(|m| m.y)
            .expect("root tip");
        let tip_y = module_world_y(pose_y, new_root.y);
        let in_void = world
            .column_at(8)
            .map(|c| elevation_in_void(c, tip_y))
            .unwrap_or(false);
        assert!(
            in_void,
            "new tip should enter the cavity (tip_y={tip_y}, surface={surface})"
        );
    }

    fn land_plant_pose_y_for_test(surface: f32, bp: &Blueprint) -> f32 {
        crate::organism::land_plant_pose_y(surface, bp)
    }

    #[test]
    fn drink_leaves_pore_reserve() {
        let mut world = dry_land();
        let cap = world.column_at(8).unwrap().moisture_cap();
        if let Some(col) = world.column_at_mut(8) {
            col.moisture = cap; // fully saturated
        }
        // Even with a huge budget, only 1 kg leaves and ≥40% stays.
        let taken = drink_adjacent(&mut world, 8, 10_000);
        assert_eq!(taken, 1);
        let moist = world.column_at(8).unwrap().moisture;
        let floor = ((cap as f32) * 0.40).round() as i64;
        assert!(moist >= floor, "reserve violated: moist={moist} floor={floor}");
        assert_eq!(moist, cap - 1);
    }

    #[test]
    fn organic_substrate_beats_bare_sand_nutrients() {
        let mut world = dry_land();
        let sand_n = {
            let col = world.column_at(8).unwrap();
            column_nutrient_factor(col)
        };
        if let Some(col) = world.column_at_mut(8) {
            col.deposit_to_top(MaterialId::Organic, 800, 1);
        }
        let organic_n = {
            let col = world.column_at(8).unwrap();
            column_nutrient_factor(col)
        };
        assert!(
            organic_n > sand_n + 0.02,
            "organic={organic_n} sand={sand_n}"
        );
    }

    #[test]
    fn rooted_plant_elongates_and_bores_organic() {
        let mut world = dry_land();
        let mut bp = Blueprint::minimal_plant(Genome {
            alloc_root: 0.8,
            alloc_stem: 0.1,
            alloc_leaf: 0.1,
            root_depth_bias: 0.9,
            ..Genome::default()
        });
        let roots_before = bp.root_count();
        let genome = bp.genome;
        let mut energy = Energy {
            current: 80.0,
            max: 100.0,
        };
        let pose_y = world.column_at(8).unwrap().surface_y;
        let base_max = energy.max;
        let spent = try_elongate_root(
            &mut bp,
            &mut energy,
            &mut world,
            8.5,
            pose_y,
            &genome,
            base_max,
            11,
            100,
            3,
        );
        assert!(spent > 0.0, "should spend energy driving a root");
        assert!(bp.root_count() > roots_before, "root pixel added");
        assert!(energy.current < 80.0);
    }

    #[test]
    fn root_modules_expand_energy_capacity() {
        let base = 200.0;
        let with_one = energy_capacity(base, 1);
        let with_many = energy_capacity(base, 20);
        assert!(
            (with_one - base * (1.0 + ROOT_STORE_FRAC)).abs() < 1e-4,
            "one root unlocks ROOT_STORE_FRAC of base"
        );
        assert!(with_many > with_one);
        assert!(
            (energy_capacity(base, 100) - base * ROOT_STORE_MAX_MULT).abs() < 1e-4,
            "capacity caps at ROOT_STORE_MAX_MULT × base"
        );
        // Photo/upkeep stay keyed to base — capacity alone is the root payoff.
        assert_eq!(growth_energy_floor(base), base * LAND_GROW_ENERGY_FRAC);
    }

    #[test]
    fn drought_lifts_soft_root_budget() {
        let bp = Blueprint::minimal_plant(Genome::default());
        let hydrated = useful_root_budget_for(&bp, DroughtBand::Hydrated);
        let stressed = useful_root_budget_for(&bp, DroughtBand::Stressed);
        assert!(
            stressed > hydrated,
            "stress should allow deeper boring (hydrated={hydrated} stressed={stressed})"
        );
        assert!(stressed <= MAX_ROOT_MODULES);
    }

    #[test]
    fn stone_harder_than_sand_penetrate_cost() {
        assert!(
            penetrate_cost(MaterialId::Stone).unwrap()
                > penetrate_cost(MaterialId::Sand).unwrap()
        );
        assert!(
            penetrate_cost(MaterialId::Sand).unwrap()
                > penetrate_cost(MaterialId::Organic).unwrap()
        );
    }
}
