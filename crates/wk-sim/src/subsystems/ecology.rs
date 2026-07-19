//! Per-column plant growth / death / nutrient recycle (stage 8).

use wk_material::CHUNK_W;
use wk_world::column::Activity;
use wk_world::world::World;

/// kg of new alive biomass per ecology step under ideal conditions
/// (light=water=temp=nutrient=1) for a column that already has a starter.
const GROWTH_COEFF: f32 = 6.0;

/// Fraction of alive biomass that can die per step under full stress.
const DEATH_COEFF: f32 = 0.02;

/// Fraction of dead biomass mineralised per step.
const DECAY_COEFF: f32 = 0.02;

/// Nutrient consumed per kg grown.
const NUTRIENT_USE: f32 = 0.0008;

/// Nutrient returned per kg decayed.
const NUTRIENT_RECYCLE: f32 = 0.0005;

/// Moisture (kg) incorporated per kg of new biomass.
const WATER_USE_PER_KG: f32 = 0.15;

/// Soft cap on alive biomass per column (kg).
const ALIVE_CAP: f32 = 4_000.0;

fn temp_factor(temp_c: f32) -> f32 {
    // Unimodal comfort centred near 18 °C; frozen / hot → near zero.
    let x = (temp_c - 18.0) / 14.0;
    (-(x * x)).exp().clamp(0.0, 1.0)
}

fn light_factor(col: &wk_world::column::Column, sea_level: f32) -> f32 {
    if col.climate_elevation() < sea_level - 0.5 {
        return 0.05; // submerged
    }
    let mut light = col.cover_light_factor();
    // Deep standing water shades the bed (snow/ice already handled above).
    let water = col.flowable_water().map(|(_, m)| m).unwrap_or(0);
    if water > 2_000 {
        light *= 0.4;
    }
    light.clamp(0.0, 1.0)
}

/// Post-barrier direct mutation: grow / stress plants and recycle litter.
pub fn run_ecology(world: &mut World, tick: u64) {
    let sea = world.sea_level;
    let coords: Vec<i32> = world.chunks.keys().copied().collect();
    let mut grew = 0i64;
    let mut decayed = 0i64;

    for coord in coords {
        let base = world.chunks.get(&coord).map(|c| c.world_x_base()).unwrap_or(0);
        for i in 0..CHUNK_W {
            let wx = base + i as i32;
            let (activity, elev, moisture, cap, eco_snapshot) = {
                let Some(chunk) = world.chunks.get(&coord) else {
                    continue;
                };
                let col = &chunk.columns[i];
                (
                    col.activity,
                    col.climate_elevation(),
                    col.moisture,
                    col.moisture_cap(),
                    col.ecology,
                )
            };
            // Dormant hydrology columns can still host plants — drought
            // stress and slow decay must keep running.
            if activity == Activity::Dormant
                && eco_snapshot.alive_biomass <= 0
                && eco_snapshot.dead_biomass <= 0
                && eco_snapshot.nutrient <= 0.01
            {
                continue;
            }

            let temp = world.temperature_at_point(wx, elev, tick);
            let Some(chunk) = world.chunks.get_mut(&coord) else {
                continue;
            };
            let col = &mut chunk.columns[i];
            let light = light_factor(col, sea);
            let water = if cap > 0 {
                (moisture as f32 / cap as f32).clamp(0.0, 1.0)
            } else {
                0.0
            };
            let tf = temp_factor(temp);
            let eco = &mut col.ecology;

            // Stress: drought or temperature far from comfort.
            let stress = ((1.0 - water) * 0.7 + (1.0 - tf) * 0.3).clamp(0.0, 1.0);
            let death = ((eco.alive_biomass as f32) * DEATH_COEFF * stress) as i64;
            let death = death.min(eco.alive_biomass).max(0);
            if death > 0 {
                eco.alive_biomass -= death;
                eco.dead_biomass += death;
            }

            let room = (ALIVE_CAP - eco.alive_biomass as f32).max(0.0) / ALIVE_CAP;
            let growth_f = light
                * water
                * tf
                * eco.nutrient.clamp(0.0, 1.0)
                * room
                * GROWTH_COEFF
                // Starter: bare columns grow slowly from seed nutrient alone.
                * (0.15 + 0.85 * (eco.alive_biomass as f32 / 100.0).min(1.0));
            let mut growth = growth_f.floor() as i64;
            if growth > 0 {
                // Incorporate a little pore water into tissue.
                let water_need = ((growth as f32) * WATER_USE_PER_KG) as i64;
                if water_need > 0 && col.moisture > 0 {
                    let take = water_need.min(col.moisture);
                    col.moisture -= take;
                    // If we can't afford full water use, scale growth down.
                    if take < water_need {
                        growth = ((growth as f32) * (take as f32 / water_need as f32)) as i64;
                    }
                } else if water_need > 0 {
                    growth = 0;
                }
            }
            if growth > 0 {
                eco.alive_biomass += growth;
                eco.nutrient = (eco.nutrient - growth as f32 * NUTRIENT_USE).clamp(0.0, 1.0);
                grew += growth;
            }

            let decay = ((eco.dead_biomass as f32) * DECAY_COEFF) as i64;
            let decay = decay.min(eco.dead_biomass).max(0);
            if decay > 0 {
                eco.dead_biomass -= decay;
                eco.nutrient = (eco.nutrient + decay as f32 * NUTRIENT_RECYCLE).clamp(0.0, 1.0);
                decayed += decay;
            }

            // Canopy / roots track alive biomass (smooth asymptote).
            let cover = ((eco.alive_biomass as f32) / 800.0).clamp(0.0, 1.0);
            eco.leaf_area += (cover - eco.leaf_area) * 0.05;
            eco.root_density += (cover * 0.9 - eco.root_density) * 0.04;
            eco.leaf_area = eco.leaf_area.clamp(0.0, 1.0);
            eco.root_density = eco.root_density.clamp(0.0, 1.0);

            let _ = eco_snapshot;
        }
    }

    if grew > 0 {
        world.mass_audit.biomass_grow_total += grew;
    }
    if decayed > 0 {
        world.mass_audit.biomass_decay_total += decayed;
    }
}
