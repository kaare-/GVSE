//! Land-plant roots (Set D): moisture / nutrient harvest, elongation,
//! and substrate bore into Organic. Spec: `docs/organism/PLANTS.md`.

use wk_material::MaterialId;
use wk_world::column::Column;
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

/// Base energy to place one new Root pixel in soft Organic.
pub const ROOT_ELONGATE_BASE_COST: f32 = 2.4;
/// kg of substrate converted per successful rock/sand bore.
pub const ROOT_BORE_KG: i64 = 40;
/// Energy gained per kg of moisture drunk by roots.
pub const ROOT_WATER_ENERGY: f32 = 0.035;
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
/// kg sipped per root module per tick when soil still has water.
/// Deliberately small so a patch of plants can't empty a hill in seconds.
pub const ROOT_SIP_KG_PER_ROOT: f32 = 0.05;
/// How often (ticks) a plant may attempt elongation.
pub const ROOT_GROW_PERIOD: u64 = 48;

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

/// Gentle sip from host + adjacent columns; returns kg taken.
///
/// Caps per-column take so dense plant patches leave pore water in place
/// (rain / infiltration can refill; plants should not flash-dry a hill).
pub fn drink_adjacent(world: &mut World, wx: i32, budget_kg: i64) -> i64 {
    if budget_kg <= 0 {
        return 0;
    }
    let mut left = budget_kg;
    let mut taken = 0i64;
    // Prefer host, then wetter neighbour.
    let mut order = [wx, wx - 1, wx + 1];
    order[1..].sort_by_key(|&x| {
        world
            .column_at(x)
            .map(|c| -(c.moisture + c.top_water_mass()))
            .unwrap_or(0)
    });
    for x in order {
        if left <= 0 {
            break;
        }
        // Never drink a column below a small reserve — leaves drought buffer.
        let reserve = world
            .column_at(x)
            .map(|c| (c.moisture_cap() / 12).max(2))
            .unwrap_or(2);
        let available = world
            .column_at(x)
            .map(|c| (c.moisture + c.top_water_mass()).saturating_sub(reserve))
            .unwrap_or(0);
        if available <= 0 {
            continue;
        }
        let want = left.min(available).min(2); // ≤2 kg from any one column / tick
        let got = world.drink_water(x, want);
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
    // Need surplus — don't elongate while hungry.
    if energy.current < energy.max * 0.45 {
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
            let mat = world
                .material_at(cell_wx, tip_y)
                .or_else(|| {
                    // Just below surface when tip is near ground.
                    world.material_at(cell_wx, col.surface_y - 0.15)
                })
                .unwrap_or(MaterialId::Sand);
            let Some(pen) = penetrate_cost(mat) else {
                continue;
            };
            let cost = ROOT_ELONGATE_BASE_COST * pen;
            if energy.current < cost + 1.0 {
                continue;
            }
            let cap = col.moisture_cap().max(1) as f32;
            let moist = ((col.moisture.max(0) as f32) / cap).clamp(0.0, 1.5);
            let down = if dy < 0 { 1.0 } else { 0.0 };
            let lateral = if dx != 0 && dy == 0 { 0.35 } else { 0.0 };
            let score = moist + depth_bias * down + (1.0 - depth_bias) * lateral - pen * 0.03;
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
    let mat = world
        .material_at(cell_wx, tip_y)
        .unwrap_or(MaterialId::Sand);
    if let Some(pen) = penetrate_cost(mat) {
        let cost = ROOT_ELONGATE_BASE_COST * pen;
        if energy.current < cost {
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
