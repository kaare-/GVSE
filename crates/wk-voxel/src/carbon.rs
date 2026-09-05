//! wk-voxel is an isolated greenfield sim. It MUST NOT import from
//! wk-world / wk-field / wk-agents / wk-sim / wk-io / wk-app. See
//! docs/VOXEL_MIGRATION.md § "Isolation Guardrails".
//!
//! Crude global carbon buckets (atmosphere + dissolved in water).
//!
//! Not a per-cell chemistry field — two scalars stepped on a slow cadence:
//! - Surface Organic slowly oxidizes → Soil (pore water conserved) and
//!   credits atmospheric C. Never invents humidity mass.
//! - Atmosphere ↔ dissolved exchange toward a Henry-ish ratio when lakes
//!   / standing water are present (sampled, not every cell every tick).
//! - Set A algae / blooms draw dissolved C (harvest throttles as the pool
//!   empties). Land plants lightly pull atmosphere on photo growth so
//!   litter→air oxidation is not a one-way leak.
//!
//! Oxygen creatures get an O₂ bucket beside this one later.

use serde::{Deserialize, Serialize};
use wk_material::MaterialId;

use crate::chunk::{CHUNK_CELLS_H, CHUNK_CELLS_W};
use crate::fungi::compost_organic_to_soil;
use crate::grid::World;
use crate::rules::{hash_prob, is_standing_water};

/// Ambient starting atmosphere units for a fresh scene.
pub const AMBIENT_ATM_C: f32 = 1_000.0;
/// Ambient starting dissolved units (lakes / pore films share one pool).
pub const AMBIENT_DISSOLVED_C: f32 = 200.0;
/// Carbon credited when one surface Organic cell oxidizes to Soil.
pub const OXIDIZE_C_PER_CELL: f32 = 1.0;

/// Live Tab knobs for the crude C budget.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CarbonConfig {
    /// Master enable (Tab → Carbon).
    pub enabled: bool,
    /// Ticks between surface-Organic oxidation pulses.
    pub oxidize_period: u64,
    /// Chance per scanned surface Organic cell to oxidize (0..1).
    pub oxidize_rate: f32,
    /// Max oxidation events per pulse (perf cap).
    pub oxidize_max_events: u32,
    /// Ticks between atm ↔ dissolved exchange pulses.
    pub exchange_period: u64,
    /// Fraction of the Henry gap closed per exchange pulse.
    pub exchange_rate: f32,
    /// Target dissolved / atmosphere ratio at equilibrium.
    pub henry_ratio: f32,
    /// Atmospheric C added per oxidized Organic cell.
    pub oxidize_c_per_cell: f32,
    /// Dissolved C drawn per unit of algae photo harvest.
    pub algae_c_per_energy: f32,
    /// Half-saturation for algae dissolved-C throttle (Michaelis–Menten).
    pub algae_half_sat: f32,
    /// Atmospheric C drawn per unit of land-plant photo harvest (light).
    pub plant_c_per_energy: f32,
    /// Half-saturation for land-plant atm-C throttle (softer than algae).
    pub plant_half_sat: f32,
}

impl Default for CarbonConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            oxidize_period: 64,
            // Slow: thick blankets shed C over long soaks, not every tick.
            oxidize_rate: 0.004,
            oxidize_max_events: 24,
            exchange_period: 32,
            exchange_rate: 0.08,
            // Dissolved sits below air at equilibrium (Henry-ish).
            henry_ratio: 0.25,
            oxidize_c_per_cell: OXIDIZE_C_PER_CELL,
            // Blooms can starve lakes; land plants only nibble air.
            algae_c_per_energy: 0.18,
            algae_half_sat: 30.0,
            // Light draw — meadows used to empty atm in ~5 days at 0.04.
            plant_c_per_energy: 0.015,
            // Softer MM curve than the old 120 half-sat.
            plant_half_sat: 60.0,
        }
    }
}

/// Minimum land-plant photo factor even when atmosphere is empty.
///
/// Matches the "without hard-starving forests" contract — algae may hard
/// throttle on dissolved C; land plants always keep a limp-along harvest.
pub const PLANT_PHOTO_C_FLOOR: f32 = 0.50;

/// World-level atmosphere + dissolved carbon pools.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CarbonBudget {
    /// Atmospheric CO₂-ish units (global).
    pub atmosphere: f32,
    /// Dissolved CO₂ in standing water / lakes (global pool for now).
    pub dissolved: f32,
}

impl Default for CarbonBudget {
    fn default() -> Self {
        Self {
            atmosphere: AMBIENT_ATM_C,
            dissolved: AMBIENT_DISSOLVED_C,
        }
    }
}

impl CarbonBudget {
    pub fn total(self) -> f32 {
        self.atmosphere + self.dissolved
    }

    /// Algae / bloom consumer. Returns units actually taken.
    pub fn take_dissolved(&mut self, want: f32) -> f32 {
        let take = want.max(0.0).min(self.dissolved);
        self.dissolved -= take;
        take
    }

    /// Land photosynthesis consumer (draw from atmosphere).
    pub fn take_atmosphere(&mut self, want: f32) -> f32 {
        let take = want.max(0.0).min(self.atmosphere);
        self.atmosphere -= take;
        take
    }
}

/// Gate Set A algae photo harvest on dissolved C and debit the pool.
///
/// When the pool empties, harvest → 0 (bloom throttles). No per-cell chemistry.
pub fn gate_algae_photo(
    budget: &mut CarbonBudget,
    cfg: &CarbonConfig,
    raw_harvest: f32,
) -> f32 {
    if !cfg.enabled || raw_harvest <= 0.0 {
        return raw_harvest.max(0.0);
    }
    let half = cfg.algae_half_sat.max(1e-3);
    let factor = budget.dissolved / (budget.dissolved + half);
    let harvest = raw_harvest.max(0.0) * factor;
    let want = harvest * cfg.algae_c_per_energy.max(0.0);
    let _ = budget.take_dissolved(want);
    harvest
}

/// Soft-gate land-plant photo harvest on atmosphere C and debit the pool.
///
/// Closes the litter→atm oxidation loop without hard-starving forests:
/// factor never drops below [`PLANT_PHOTO_C_FLOOR`]. C is only debited for
/// the share actually available in the pool (floor harvest can limp with
/// empty air).
pub fn gate_plant_photo(
    budget: &mut CarbonBudget,
    cfg: &CarbonConfig,
    raw_harvest: f32,
) -> f32 {
    if !cfg.enabled || raw_harvest <= 0.0 {
        return raw_harvest.max(0.0);
    }
    let half = cfg.plant_half_sat.max(1e-3);
    let raw_factor = budget.atmosphere / (budget.atmosphere + half);
    let factor = raw_factor.max(PLANT_PHOTO_C_FLOOR).min(1.0);
    let harvest = raw_harvest.max(0.0) * factor;
    // Charge only the C-limited share, not the floor limp-along.
    let billed = raw_harvest.max(0.0) * raw_factor * cfg.plant_c_per_energy.max(0.0);
    let _ = budget.take_atmosphere(billed);
    harvest
}

/// One crude carbon step: optional surface oxidation + atm↔dissolved exchange.
///
/// Water mass is untouched except via [`compost_organic_to_soil`] pore push
/// (same path as mycelium compost). Carbon only moves Organic ↔ buckets.
pub fn step_carbon_budget(budget: &mut CarbonBudget, world: &mut World, cfg: &CarbonConfig) {
    if !cfg.enabled {
        return;
    }
    let tick = world.tick;
    if cfg.oxidize_period > 0 && tick % cfg.oxidize_period == 0 {
        oxidize_surface_organic(budget, world, cfg);
    }
    if cfg.exchange_period > 0 && tick % cfg.exchange_period == 0 {
        exchange_atm_dissolved(budget, world, cfg);
    }
    budget.atmosphere = budget.atmosphere.max(0.0);
    budget.dissolved = budget.dissolved.max(0.0);
}

fn oxidize_surface_organic(budget: &mut CarbonBudget, world: &mut World, cfg: &CarbonConfig) {
    let rate = cfg.oxidize_rate.clamp(0.0, 1.0);
    if rate <= 0.0 || cfg.oxidize_max_events == 0 {
        return;
    }
    let credit = cfg.oxidize_c_per_cell.max(0.0);
    let mut events = 0u32;
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        // Surface litter. Mid-ocean / empty sky never held Organic —
        // leftover 64×64 every oxidize pulse. Occupancy is the source
        // of truth.
        if !chunk.has_organic {
            continue;
        }
        for ly in 0..CHUNK_CELLS_H {
            for lx in 0..CHUNK_CELLS_W {
                if events >= cfg.oxidize_max_events {
                    return;
                }
                let gx = coord.cx * CHUNK_CELLS_W as i32 + lx as i32;
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                let Some(c) = world.get_cell(gx, gy) else {
                    continue;
                };
                if c.material != MaterialId::Organic {
                    continue;
                }
                // Surface only — Air (or empty) above the litter.
                let open = match world.get_cell(gx, gy + 1) {
                    None => true,
                    Some(a) => a.material == MaterialId::Air,
                };
                if !open {
                    continue;
                }
                if hash_prob(
                    world.seed.0,
                    gx,
                    world.tick.wrapping_add(gy as u64),
                    0xC020_01D1_u64,
                ) >= rate
                {
                    continue;
                }
                if compost_organic_to_soil(world, gx, gy) {
                    budget.atmosphere += credit;
                    events += 1;
                }
            }
        }
    }
}

fn exchange_atm_dissolved(budget: &mut CarbonBudget, world: &World, cfg: &CarbonConfig) {
    let rate = cfg.exchange_rate.clamp(0.0, 1.0);
    if rate <= 0.0 {
        return;
    }
    // Crude wetness proxy: count a capped sample of standing-water Air cells.
    // No exchange when the world is bone-dry.
    let wet = sample_standing_water_cells(world);
    if wet == 0 {
        return;
    }
    let henry = cfg.henry_ratio.clamp(0.01, 4.0);
    // Scale exchange slightly with wetness (capped) so tiny ponds lag lakes.
    let wet_scale = (wet as f32 / 64.0).clamp(0.15, 1.0);
    let target_dissolved = budget.atmosphere * henry;
    let gap = target_dissolved - budget.dissolved;
    let delta = gap * rate * wet_scale;
    budget.dissolved += delta;
    budget.atmosphere -= delta;
}

fn sample_standing_water_cells(world: &World) -> u32 {
    let mut n = 0u32;
    let coords: Vec<_> = world.chunks.keys().copied().collect();
    for coord in coords {
        let Some(chunk) = world.chunks.get(&coord) else {
            continue;
        };
        // Wetness dial is standing water. Rain-film sky and dry rock
        // never raise the flag — leftover on the exchange pulse.
        // An ocean free surface still samples. Occupancy is the
        // source of truth.
        if !chunk.has_standing_air {
            continue;
        }
        // Sparse sample — every 2nd cell — enough for a wetness dial.
        for ly in (0..CHUNK_CELLS_H).step_by(2) {
            for lx in (0..CHUNK_CELLS_W).step_by(2) {
                let gx = coord.cx * CHUNK_CELLS_W as i32 + lx as i32;
                let gy = coord.cy * CHUNK_CELLS_H as i32 + ly as i32;
                if is_standing_water(world, gx, gy) {
                    n += 1;
                    if n >= 256 {
                        return n;
                    }
                }
            }
        }
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cell::{Cell, Sat};
    use crate::chunk::ChunkCoord;

    fn litter_world() -> World {
        let mut w = World::new(9);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        for x in 0..8 {
            w.set_cell(x, 0, Cell::solid(MaterialId::Bedrock));
            let mut org = Cell::solid(MaterialId::Organic);
            org.sat = Sat(80);
            w.set_cell(x, 1, org);
            w.set_cell(x, 2, Cell::air());
        }
        w.tick = 64;
        w
    }

    #[test]
    fn oxidation_credits_atmosphere_and_makes_soil() {
        let mut w = litter_world();
        let mut budget = CarbonBudget {
            atmosphere: 0.0,
            dissolved: 0.0,
        };
        let cfg = CarbonConfig {
            oxidize_period: 1,
            oxidize_rate: 1.0,
            oxidize_max_events: 8,
            exchange_period: 0,
            ..CarbonConfig::default()
        };
        // Align tick so oxidize runs.
        w.tick = 0;
        step_carbon_budget(&mut budget, &mut w, &cfg);
        assert!(
            budget.atmosphere > 0.0,
            "oxidation must credit atmosphere (got {})",
            budget.atmosphere
        );
        let soils = (0..8)
            .filter(|&x| {
                w.get_cell(x, 1)
                    .map(|c| c.material == MaterialId::Soil)
                    .unwrap_or(false)
            })
            .count();
        assert!(soils > 0, "surface Organic should become Soil");
        // Virgin litter must not paint fake mycelium on the new Soil.
        for x in 0..8 {
            if let Some(c) = w.get_cell(x, 1) {
                if c.material == MaterialId::Soil {
                    assert_eq!(
                        c.mycelium(),
                        0,
                        "oxidation Soil must not invent cream (x={x})"
                    );
                }
            }
        }
        // Pore water conserved into Soil (or pushed) — no humidity invent.
        let sat_total: u32 = (0..8)
            .flat_map(|x| (0..4).map(move |y| (x, y)))
            .filter_map(|(x, y)| w.get_cell(x, y).map(|c| c.sat.0 as u32))
            .sum();
        assert!(sat_total >= 80, "pore sat must not vanish (total={sat_total})");
    }

    #[test]
    fn exchange_moves_toward_henry_when_wet() {
        let mut w = litter_world();
        // Pond on the Organic bed (standing water needs solid/wet under).
        for x in 0..4 {
            w.set_cell(x, 2, Cell::water());
        }
        let mut budget = CarbonBudget {
            atmosphere: 1_000.0,
            dissolved: 0.0,
        };
        let cfg = CarbonConfig {
            oxidize_period: 0,
            exchange_period: 1,
            exchange_rate: 0.5,
            henry_ratio: 0.25,
            ..CarbonConfig::default()
        };
        w.tick = 0;
        step_carbon_budget(&mut budget, &mut w, &cfg);
        assert!(
            budget.dissolved > 10.0,
            "wet exchange should fill dissolved (got {})",
            budget.dissolved
        );
        assert!(
            (budget.total() - 1_000.0).abs() < 0.01,
            "exchange conserves C buckets"
        );
    }

    #[test]
    fn take_dissolved_throttles_to_pool() {
        let mut budget = CarbonBudget {
            atmosphere: 100.0,
            dissolved: 10.0,
        };
        assert_eq!(budget.take_dissolved(40.0), 10.0);
        assert_eq!(budget.dissolved, 0.0);
        assert_eq!(budget.take_dissolved(1.0), 0.0);
    }

    #[test]
    fn algae_photo_throttles_when_dissolved_empty() {
        let cfg = CarbonConfig::default();
        let mut rich = CarbonBudget {
            atmosphere: 1_000.0,
            dissolved: 200.0,
        };
        let mut dry = CarbonBudget {
            atmosphere: 1_000.0,
            dissolved: 0.0,
        };
        let rich_h = gate_algae_photo(&mut rich, &cfg, 1.0);
        let dry_h = gate_algae_photo(&mut dry, &cfg, 1.0);
        assert!(rich_h > 0.5, "rich lake should feed algae (got {rich_h})");
        assert_eq!(dry_h, 0.0, "empty dissolved pool must zero algae harvest");
        assert!(rich.dissolved < 200.0, "algae must debit dissolved C");
    }

    #[test]
    fn plant_photo_lightly_draws_atmosphere() {
        let cfg = CarbonConfig::default();
        let mut budget = CarbonBudget {
            atmosphere: 1_000.0,
            dissolved: 200.0,
        };
        let h = gate_plant_photo(&mut budget, &cfg, 1.0);
        assert!(h > 0.9, "ambient atm should barely throttle plants (got {h})");
        assert!(
            budget.atmosphere < 1_000.0,
            "land photo must debit atmosphere"
        );
        assert!(
            1_000.0 - budget.atmosphere < 0.05,
            "plant draw should be light (delta={})",
            1_000.0 - budget.atmosphere
        );
    }

    #[test]
    fn plant_photo_keeps_floor_when_atmosphere_empty() {
        let cfg = CarbonConfig::default();
        let mut budget = CarbonBudget {
            atmosphere: 0.0,
            dissolved: 0.0,
        };
        let h = gate_plant_photo(&mut budget, &cfg, 1.0);
        assert!(
            (h - PLANT_PHOTO_C_FLOOR).abs() < 1e-4,
            "empty atm must limp at photo floor (got {h})"
        );
        assert_eq!(budget.atmosphere, 0.0, "floor harvest must not mint atm C");
    }

    #[test]
    fn oxidize_skips_water_only_chunks() {
        let mut w = World::new(128);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(1, 0));
        for x in 0..8 {
            w.set_cell(x, 2, Cell::water());
        }
        w.set_cell(68, 0, Cell::solid(MaterialId::Bedrock));
        let mut org = Cell::solid(MaterialId::Organic);
        org.sat = Sat(80);
        w.set_cell(68, 1, org);
        w.set_cell(68, 2, Cell::air());
        assert!(
            !w.chunks[&ChunkCoord::new(0, 0)].has_organic,
            "precondition: mid-ocean did not raise has_organic"
        );
        assert!(
            w.chunks[&ChunkCoord::new(1, 0)].has_organic,
            "precondition: litter raised has_organic"
        );
        let mut budget = CarbonBudget {
            atmosphere: 0.0,
            dissolved: 0.0,
        };
        let cfg = CarbonConfig {
            oxidize_period: 1,
            oxidize_rate: 1.0,
            oxidize_max_events: 8,
            exchange_period: 0,
            ..CarbonConfig::default()
        };
        w.tick = 0;
        step_carbon_budget(&mut budget, &mut w, &cfg);
        assert!(
            budget.atmosphere > 0.0,
            "litter beside ocean must still oxidize"
        );
        assert_eq!(
            w.get_cell(68, 1).unwrap().material,
            MaterialId::Soil
        );
    }

    #[test]
    fn standing_sample_skips_rain_film_sky() {
        let mut w = World::new(8);
        w.ensure_chunk(ChunkCoord::new(0, 0));
        w.ensure_chunk(ChunkCoord::new(0, 1));
        for x in 0..8 {
            w.set_cell(
                x,
                70,
                Cell {
                    material: MaterialId::Air,
                    sat: Sat(33),
                    ..Cell::default()
                },
            );
        }
        let sky = &w.chunks[&ChunkCoord::new(0, 1)];
        assert!(
            sky.has_wet_air && !sky.has_standing_air,
            "precondition: drizzle is rain-film, not standing"
        );
        assert_eq!(
            sample_standing_water_cells(&w),
            0,
            "rain-film sky must not count as lake wetness"
        );
        for x in 0..8 {
            w.set_cell(x, 1, Cell::water());
            w.set_cell(x, 2, Cell::water());
        }
        assert!(
            sample_standing_water_cells(&w) > 0,
            "ocean / pond standing water still samples"
        );
    }
}
