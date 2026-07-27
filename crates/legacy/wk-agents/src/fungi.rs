//! Litter fungi (Set E, thin slice): Digest + Hypha on dead organic.
//!
//! Spec: `docs/organism/FUNGI.md`. v1 goals:
//! - Only a **labile fraction** of litter/Organic is digestible (humus remains).
//! - Digest → fungal energy + soil `Ecology.nutrient` kick.
//! - Dormancy when food/moisture is gone (mirror plant drought hibernate).
//! - Spore-style fission reuses the shared clone pipeline.

use wk_material::MaterialId;
use wk_world::column::Activity;
use wk_world::world::World;

use crate::blueprint::Blueprint;
use crate::module::ModuleId;
use crate::Genome;

/// Max Hypha pixels on a blueprint (upkeep + reach soft cap).
pub const MAX_HYPHA_MODULES: usize = 16;
/// Fraction of stratigraphic Organic mass treated as labile food.
pub const LABILE_ORGANIC_FRAC: f32 = 0.20;
/// Per digest event: fraction of the labile pool that may be taken.
pub const DIGEST_TICK_FRAC: f32 = 0.10;
/// Soft kg cap removed from one column per digest event.
pub const DIGEST_MAX_KG: i64 = 4;
/// Energy gained per kg digested (before gene / hypha scaling).
pub const DIGEST_ENERGY_PER_KG: f32 = 0.55;
/// Soil nutrient added per kg digested.
pub const DIGEST_NUTRIENT_PER_KG: f32 = 0.0025;
/// Upkeep per Hypha module / tick while active.
pub const HYPHA_UPKEEP: f32 = 0.0012;
/// Upkeep per Digest module / tick while active.
pub const DIGEST_UPKEEP: f32 = 0.0020;
/// Labile food (kg) below which fungi hibernate.
pub const FUNGUS_STARVE_KG: f32 = 0.5;
/// Moisture fraction below which fungi hibernate (drier than plant stress).
pub const FUNGUS_DROUGHT_FRAC: f32 = 0.05;
/// Max consecutive dormant ticks before death (~2 min at 60 Hz).
pub const FUNGUS_HIBERNATE_MAX_TICKS: u32 = 7_200;
/// Upkeep multiplier while dormant.
pub const FUNGUS_DORMANT_UPKEEP: f32 = 0.15;
/// Energy fraction to attempt a spore burst.
pub const FUNGUS_SPORE_ENERGY_FRAC: f32 = 0.55;
/// Spore attempt period (ticks).
pub const FUNGUS_SPORE_PERIOD: u64 = 64;
/// Max columns a spore may land from the parent.
pub const FUNGUS_SPORE_MAX_DIST: i32 = 5;

/// How many Digest modules are painted.
pub fn digest_count(blueprint: &Blueprint) -> usize {
    blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Digest)
        .count()
}

/// How many Hypha modules are painted.
pub fn hypha_count(blueprint: &Blueprint) -> usize {
    blueprint
        .modules
        .iter()
        .filter(|m| m.module == ModuleId::Hypha)
        .count()
}

/// Labile food (kg) visible to fungi in this column.
///
/// Soft litter (`dead_biomass`) is fully labile; only
/// [`LABILE_ORGANIC_FRAC`] of stratigraphic Organic counts — the rest is
/// humus / structure fungi must not vacuum.
pub fn labile_food_kg(world: &World, wx: i32) -> f32 {
    let Some(col) = world.column_at(wx) else {
        return 0.0;
    };
    let litter = col.ecology.dead_biomass.max(0) as f32;
    let mut organic = 0i64;
    for i in 0..col.layer_count as usize {
        if col.layers[i].material == MaterialId::Organic {
            organic += col.layers[i].thickness.max(0);
        }
    }
    litter + organic as f32 * LABILE_ORGANIC_FRAC
}

/// True when fungi should hibernate (no food and/or bone-dry).
pub fn fungus_should_hibernate(world: &World, wx: i32) -> bool {
    let food = labile_food_kg(world, wx);
    if food < FUNGUS_STARVE_KG {
        return true;
    }
    let moist = crate::root::adjacent_moisture_frac(world, wx);
    moist < FUNGUS_DROUGHT_FRAC
}

/// kg budget for one digest event from gene + modules.
pub fn digest_budget_kg(genome: &Genome, blueprint: &Blueprint) -> i64 {
    let n_d = digest_count(blueprint).max(1) as f32;
    let n_h = hypha_count(blueprint) as f32;
    let rate = genome.digest_rate.clamp(0.05, 2.0);
    // Hyphae extend reach: each adds ~15% budget.
    let scale = n_d * (1.0 + 0.15 * n_h) * rate;
    let kg = (scale * DIGEST_MAX_KG as f32).round() as i64;
    kg.clamp(1, DIGEST_MAX_KG.saturating_mul(2))
}

/// Remove up to `want_kg` of labile food; returns (kg_taken, energy, nutrient).
///
/// Prefers `dead_biomass`, then peels stratigraphic Organic (capped so humus
/// remains). Books mass into `biomass_decay_total`.
pub fn digest_labile(world: &mut World, wx: i32, want_kg: i64) -> (i64, f32, f32) {
    if want_kg <= 0 {
        return (0, 0.0, 0.0);
    }
    let labile = labile_food_kg(world, wx);
    if labile < FUNGUS_STARVE_KG {
        return (0, 0.0, 0.0);
    }
    let tick_cap = (labile * DIGEST_TICK_FRAC).floor().max(0.0) as i64;
    let mut left = want_kg.min(tick_cap).min(DIGEST_MAX_KG);
    if left <= 0 {
        return (0, 0.0, 0.0);
    }

    let mut taken = 0i64;
    // 1) Soft litter first.
    if let Some(col) = world.column_at_mut(wx) {
        let lit = col.ecology.dead_biomass.max(0);
        let take = left.min(lit);
        if take > 0 {
            col.ecology.dead_biomass -= take;
            col.activity = Activity::HydrologyActive;
            taken += take;
            left -= take;
        }
    }
    // 2) Stratigraphic Organic — only up to labile slice of current mass.
    if left > 0 {
        if let Some(col) = world.column_at_mut(wx) {
            let mut organic_kg = 0i64;
            for i in 0..col.layer_count as usize {
                if col.layers[i].material == MaterialId::Organic {
                    organic_kg += col.layers[i].thickness.max(0);
                }
            }
            let labile_org = ((organic_kg as f32) * LABILE_ORGANIC_FRAC).floor() as i64;
            let mut org_left = left.min(labile_org).max(0);
            if org_left > 0 {
                // Peel from topmost Organic layer(s) (index 0 = top).
                let mut i = 0usize;
                while i < col.layer_count as usize && org_left > 0 {
                    if col.layers[i].material != MaterialId::Organic {
                        i += 1;
                        continue;
                    }
                    let peel = org_left.min(col.layers[i].thickness);
                    if peel <= 0 {
                        i += 1;
                        continue;
                    }
                    let dh = col.mass_to_height_delta(MaterialId::Organic, peel);
                    col.layers[i].thickness -= peel;
                    col.surface_y -= dh;
                    taken += peel;
                    left -= peel;
                    org_left -= peel;
                    col.activity = Activity::HydrologyActive;
                    if col.layers[i].thickness <= 0 {
                        for j in i..(col.layer_count as usize).saturating_sub(1) {
                            col.layers[j] = col.layers[j + 1];
                        }
                        if col.layer_count > 0 {
                            col.layer_count -= 1;
                        }
                        // same index now holds the next layer
                    } else {
                        i += 1;
                    }
                }
            }
        }
    }

    if taken > 0 {
        world.mass_audit.biomass_decay_total = world
            .mass_audit
            .biomass_decay_total
            .saturating_add(taken);
        let energy = taken as f32 * DIGEST_ENERGY_PER_KG;
        let nutrient = taken as f32 * DIGEST_NUTRIENT_PER_KG;
        if let Some(col) = world.column_at_mut(wx) {
            col.ecology.nutrient = (col.ecology.nutrient + nutrient).clamp(0.0, 1.0);
        }
        (taken, energy, nutrient)
    } else {
        (0, 0.0, 0.0)
    }
}

/// Active upkeep for Digest + Hypha tissue.
pub fn fungus_upkeep(blueprint: &Blueprint, dormant: bool) -> f32 {
    let base = DIGEST_UPKEEP * digest_count(blueprint) as f32
        + HYPHA_UPKEEP * hypha_count(blueprint) as f32;
    if dormant {
        base * FUNGUS_DORMANT_UPKEEP
    } else {
        base
    }
}

/// Pick a nearby column with labile food for a spore landing.
pub fn pick_spore_site(world: &World, pose_x: f32, world_seed: u64, tick: u64, id: u32) -> Option<f32> {
    let wx0 = pose_x.floor() as i32;
    let mut best: Option<(f32, i32)> = None; // score, wx
    for dist in 1..=FUNGUS_SPORE_MAX_DIST {
        for sign in [1i32, -1] {
            let wx = wx0 + sign * dist;
            let Some(col) = world.column_at(wx) else {
                continue;
            };
            // Need solid purchase; shallow water OK.
            if !crate::root::column_is_plantable_for_reach(
                world,
                wx,
                crate::root::SHALLOW_PLANT_WATER_M,
            ) {
                continue;
            }
            let food = labile_food_kg(world, wx);
            if food < FUNGUS_STARVE_KG {
                continue;
            }
            let score = food - dist as f32 * 0.3 + col.ecology.nutrient;
            let better = best.map(|(s, _)| score > s).unwrap_or(true);
            if better {
                best = Some((score, wx));
            }
        }
    }
    // Deterministic tie-break jitter toward left/right.
    if best.is_none() {
        // Fallback: any plantable neighbour even if food-poor (spore bank).
        let flip = wk_world::terrain::hash_u64(world_seed, tick as i64, id as i64, 0xF5C0) & 1;
        let dir = if flip == 0 { 1 } else { -1 };
        for dist in 1..=FUNGUS_SPORE_MAX_DIST {
            let wx = wx0 + dir * dist;
            if crate::root::column_is_plantable_for_reach(
                world,
                wx,
                crate::root::SHALLOW_PLANT_WATER_M,
            ) {
                return Some(wx as f32 + 0.5);
            }
        }
        return None;
    }
    best.map(|(_, wx)| wx as f32 + 0.5)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blueprint::{Blueprint, PlacedModule};
    use crate::module::{LaneId, ModuleId};
    use crate::Genome;
    use wk_world::terrain::generate_flat_sand;

    fn fungus_world() -> World {
        let mut world = World::new(7);
        world.sea_level = 0.0;
        world.insert_chunk(generate_flat_sand(0, 0.0, 8.0));
        world
    }

    #[test]
    fn digest_prefers_litter_and_leaves_humus() {
        let mut world = fungus_world();
        if let Some(col) = world.column_at_mut(8) {
            col.ecology.dead_biomass = 40;
            col.deposit_to_top(MaterialId::Organic, 1_000, 0);
        }
        let organic_before: i64 = {
            let col = world.column_at(8).unwrap();
            (0..col.layer_count as usize)
                .filter(|&i| col.layers[i].material == MaterialId::Organic)
                .map(|i| col.layers[i].thickness)
                .sum()
        };
        let (taken, energy, _) = digest_labile(&mut world, 8, 4);
        assert!(taken > 0 && taken <= 4);
        assert!(energy > 0.0);
        let litter = world.column_at(8).unwrap().ecology.dead_biomass;
        assert!(litter < 40, "should eat soft litter first");
        let organic_after: i64 = {
            let col = world.column_at(8).unwrap();
            (0..col.layer_count as usize)
                .filter(|&i| col.layers[i].material == MaterialId::Organic)
                .map(|i| col.layers[i].thickness)
                .sum()
        };
        // Humus: most of the 1000 kg Organic must remain.
        assert!(
            organic_after >= organic_before - 200,
            "must not vacuum Organic (before={organic_before} after={organic_after})"
        );
        assert!(world.column_at(8).unwrap().ecology.nutrient > 0.0);
    }

    #[test]
    fn starve_column_hibernates() {
        let world = fungus_world();
        assert!(fungus_should_hibernate(&world, 8));
    }

    #[test]
    fn rich_litter_does_not_hibernate() {
        let mut world = fungus_world();
        if let Some(col) = world.column_at_mut(8) {
            col.moisture = col.moisture_cap();
            col.ecology.dead_biomass = 80;
        }
        assert!(!fungus_should_hibernate(&world, 8));
    }

    #[test]
    fn minimal_fungus_blueprint_is_fungus() {
        let bp = Blueprint::minimal_fungus(Genome::default());
        assert!(bp.is_fungus());
        assert!(!bp.is_plankton());
        assert!(!bp.is_rooted());
        assert!(digest_count(&bp) >= 1);
    }

    #[test]
    fn hypha_raises_digest_budget() {
        let g = Genome {
            digest_rate: 1.0,
            ..Genome::default()
        };
        let mut bp = Blueprint::minimal_fungus(g);
        let base = digest_budget_kg(&g, &bp);
        bp.modules.push(PlacedModule {
            x: 1,
            y: 0,
            lane: LaneId::Mid,
            module: ModuleId::Hypha,
        });
        bp.modules.push(PlacedModule {
            x: 2,
            y: 0,
            lane: LaneId::Mid,
            module: ModuleId::Hypha,
        });
        let boosted = digest_budget_kg(&g, &bp);
        assert!(boosted >= base);
    }
}
