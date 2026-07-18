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
/// Does not touch standing lake/sea water (`drink_water` would vacuum a
/// free surface). Leaves ~40% of pore capacity so recharge wins.
pub fn drink_adjacent(world: &mut World, wx: i32, budget_kg: i64) -> i64 {
    if budget_kg <= 0 {
        return 0;
    }
    let mut left = budget_kg.min(ROOT_SIP_MAX_KG_PER_TICK);
    let mut taken = 0i64;
    // Prefer host, then wetter neighbour (pore water only).
    let mut order = [wx, wx - 1, wx + 1];
    order[1..].sort_by_key(|&x| {
        world
            .column_at(x)
            .map(|c| -c.moisture)
            .unwrap_or(0)
    });
    for x in order {
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
            // Water left into the plant — same audit bucket as evap/drink.
            world.mass_audit.evap_out_total += got;
        }
        taken += got;
        left -= got;
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

/// Solid ledge suitable for a land plant (not deep water, not a cliff notch).
pub fn column_is_plantable(world: &World, wx: i32) -> bool {
    let Some(col) = world.column_at(wx) else {
        return false;
    };
    // Drown risk: more than ~0.5 m of standing water.
    if let Some((_, mass)) = col.flowable_water() {
        if mass > 0 {
            let depth = col.mass_to_height_delta(MaterialId::Water, mass);
            if depth > 0.5 {
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

/// True when a land sprout can emerge here (plantable + some pore water).
pub fn column_can_host_sprout(world: &World, wx: i32) -> bool {
    if !column_is_plantable(world, wx) {
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

/// Pick a column for vegetative **root sprout** propagation.
///
/// Prefers painted Root tips that already sit in a neighbouring column
/// (runners). If roots are still under the crown only, allows a near
/// sucker within [`ROOT_SPROUT_MAX_DIST`], biased by moisture.
///
/// Returns `None` when no eligible site exists (caller should skip the
/// attempt without charging energy). Seeds / fruiting come later.
pub fn pick_root_sprout_x(
    world: &World,
    pose_x: f32,
    pose_y: f32,
    blueprint: &Blueprint,
    world_seed: u64,
    tick: u64,
    entity_id: u32,
) -> Option<f32> {
    if !blueprint.is_rooted() || blueprint.root_count() == 0 {
        return None;
    }
    let parent_wx = pose_x.floor() as i32;
    let mid_x = blueprint_mid_x(blueprint);

    // 1) Painted lateral roots with solid purchase — farthest first.
    let mut lateral: Vec<(i32, i32, f32)> = Vec::new(); // |dx|, wx, moist
    for m in blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Root)
    {
        let cell_wx = (pose_x + (m.x as f32 - mid_x) * MODULE_CELL_COLS).floor() as i32;
        let dx = (cell_wx - parent_wx).abs();
        if dx < 1 {
            continue;
        }
        let tip_y = module_world_y(pose_y, m.y);
        if !solid_purchase_at(world, cell_wx, tip_y)
            && !solid_purchase_at(world, cell_wx, tip_y - 0.05)
        {
            continue;
        }
        if !column_can_host_sprout(world, cell_wx) {
            continue;
        }
        let moist = world
            .column_at(cell_wx)
            .map(|c| {
                let cap = c.moisture_cap().max(1) as f32;
                c.moisture.max(0) as f32 / cap
            })
            .unwrap_or(0.0);
        lateral.push((dx, cell_wx, moist));
    }
    lateral.sort_by(|a, b| {
        b.0.cmp(&a.0)
            .then(b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal))
    });
    if let Some(&(_, wx, _)) = lateral.first() {
        return Some(wx as f32 + 0.5);
    }

    // 2) Near-crown sucker — underground runner not yet painted as modules.
    // Prefer gentler elevation steps so sprouts don't leap off cliff faces.
    let parent_bed = world
        .column_at(parent_wx)
        .map(|c| c.climate_elevation())
        .unwrap_or(pose_y);
    let prefer_right = hash_u64(world_seed, tick as i64, entity_id as i64, 0x5352_4F54) & 1 == 0;
    let mut best: Option<(f32, i32)> = None; // score, wx
    for dist in 1..=ROOT_SPROUT_MAX_DIST {
        for sign in [1i32, -1] {
            let signed = if prefer_right { sign } else { -sign };
            let wx = parent_wx + signed * dist;
            if !column_can_host_sprout(world, wx) {
                continue;
            }
            let Some(col) = world.column_at(wx) else {
                continue;
            };
            let bed = col.climate_elevation();
            let step = (bed - parent_bed).abs();
            // Hard reject: sprout would sit on a pillar/cliff lip far above/below.
            if step > 10.0 {
                continue;
            }
            let cap = col.moisture_cap().max(1) as f32;
            let moist = col.moisture.max(0) as f32 / cap;
            // Wetter + nearer grade + slightly farther out.
            let score = moist * 2.0 + dist as f32 * 0.15 - step * 0.35;
            match best {
                Some((s, _)) if s >= score => {}
                _ => best = Some((score, wx)),
            }
        }
    }
    best.map(|(_, wx)| wx as f32 + 0.5)
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
/// Returns energy spent (0 if nothing grew). May call [`World::root_bore`].
pub fn try_elongate_root(
    blueprint: &mut Blueprint,
    energy: &mut Energy,
    world: &mut World,
    pose_x: f32,
    pose_y: f32,
    genome: &Genome,
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
    // Need surplus above the vegetative sprout reserve — elongating at
    // 0.45 kept coastal plains permanently below the sprout gate.
    if energy.current < energy.max * LAND_SPROUT_ENERGY_FRAC {
        return 0.0;
    }

    let occupied: std::collections::HashSet<(i16, i16)> =
        blueprint.modules.iter().map(|m| (m.x, m.y)).collect();
    let tips = root_tips(blueprint);
    let depth_bias = genome.root_depth_bias.clamp(0.0, 1.0);

    // Candidate steps from each tip: down + diagonals + lateral.
    const DIRS: [(i16, i16); 5] = [(0, -1), (-1, -1), (1, -1), (-1, 0), (1, 0)];
    let mut best: Option<(f32, i16, i16, i32, f32)> = None; // score, x, y, wx, cost

    for &(tx, ty) in &tips {
        for &(dx, dy) in &DIRS {
            let nx = tx + dx;
            let ny = ty + dy;
            if ny > 2 || ny < -20 || nx.abs() > 10 {
                continue;
            }
            if occupied.contains(&(nx, ny)) {
                continue;
            }
            // World column for this tip cell.
            let mid_x = {
                let min_x = blueprint.modules.iter().map(|m| m.x).min().unwrap_or(0);
                let max_x = blueprint.modules.iter().map(|m| m.x).max().unwrap_or(0);
                (min_x as f32 + max_x as f32) * 0.5
            };
            let cell_wx = (pose_x + (nx as f32 - mid_x) * MODULE_CELL_COLS).floor() as i32;
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
            let floor = energy.max * LAND_SPROUT_ENERGY_FRAC;
            if energy.current < cost + floor {
                continue;
            }
            let cap = col.moisture_cap().max(1) as f32;
            let moist = ((col.moisture.max(0) as f32) / cap).clamp(0.0, 1.5);
            let down = if dy < 0 { 1.0 } else { 0.0 };
            let lateral = if dx != 0 && dy == 0 { 0.35 } else { 0.0 };
            let score = moist
                + void_bonus
                + depth_bias * down
                + (1.0 - depth_bias) * lateral
                - pen * 0.03;
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
        let floor = energy.max * LAND_SPROUT_ENERGY_FRAC;
        if energy.current < cost + floor {
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
        let floor = energy.max * LAND_SPROUT_ENERGY_FRAC;
        if energy.current < cost + floor {
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
pub fn try_grow_shoot(
    blueprint: &mut Blueprint,
    energy: &mut Energy,
    genome: &Genome,
    world_seed: u64,
    tick: u64,
    entity_id: u32,
) -> f32 {
    let (w_stem, w_leaf, _) = genome.alloc_weights();
    if energy.current < energy.max * 0.55 {
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

    #[test]
    fn root_sprout_picks_nearby_moist_column() {
        let mut world = dry_land();
        // Saturate neighbours so suckers have somewhere to go.
        for x in 6..12 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        let bp = Blueprint::minimal_plant(Genome::default());
        let pose_x = 8.5;
        let pose_y = world.column_at(8).unwrap().surface_y;
        let site = pick_root_sprout_x(&world, pose_x, pose_y, &bp, 1, 0, 7)
            .expect("sucker site");
        let dx = (site.floor() as i32 - 8).abs();
        assert!(
            (1..=ROOT_SPROUT_MAX_DIST).contains(&dx),
            "sprout should emerge near parent (site={site}, dx={dx})"
        );
        assert!(column_can_host_sprout(&world, site.floor() as i32));
    }

    #[test]
    fn root_sprout_skips_deep_water() {
        let mut world = dry_land();
        for x in 6..12 {
            if let Some(col) = world.column_at_mut(x) {
                col.moisture = col.moisture_cap();
            }
        }
        // Flood every neighbour — only parent column dry-ish.
        for x in [6, 7, 9, 10, 11, 12] {
            if let Some(col) = world.column_at_mut(x) {
                col.deposit_to_top(MaterialId::Water, 20_000, 0);
            }
        }
        let bp = Blueprint::minimal_plant(Genome::default());
        let site = pick_root_sprout_x(&world, 8.5, 8.0, &bp, 2, 10, 3);
        assert!(
            site.is_none(),
            "no sprout into drowned neighbours (got {site:?})"
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
        let spent = try_elongate_root(
            &mut bp,
            &mut energy,
            &mut world,
            8.5,
            pose_y,
            &genome,
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
        let spent = try_elongate_root(
            &mut bp,
            &mut energy,
            &mut world,
            8.5,
            pose_y,
            &genome,
            11,
            100,
            3,
        );
        assert!(spent > 0.0, "should spend energy driving a root");
        assert!(bp.root_count() > roots_before, "root pixel added");
        assert!(energy.current < 80.0);
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
